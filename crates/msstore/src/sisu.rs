//! Xbox Live device sign-in through SISU, as Windows' Gaming Services does it:
//! a device token from an RPS device ticket, SISU authorization of the user
//! for a title, and XSTS tokens for relying parties. Every request is signed
//! with a P-256 proof key (`Signature` header, policy version 1).
//!
//! The parts of xal-rs (MIT, Team OpenXbox) that Xodus uses, written against
//! this crate's dependencies.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use p256::ecdsa::signature::hazmat::PrehashSigner;
use p256::ecdsa::{Signature, SigningKey};
use serde::{Deserialize, Serialize};
use sha2_11::{Digest, Sha256};

pub const SISU_AUTHORIZE_URL: &str = "https://sisu.xboxlive.com/authorize";
pub const DEVICE_AUTH_URL: &str = "https://device.auth.xboxlive.com/device/authenticate";
pub const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const RELYING_PARTY_AUTH: &str = "http://auth.xboxlive.com";
/// The signing policy the title endpoints list for the three auth hosts.
const POLICY_VERSION: i32 = 1;
const MAX_BODY_BYTES: usize = 8192;
/// Seconds between 1601-01-01 and 1970-01-01.
const FILETIME_EPOCH_OFFSET: u64 = 11_644_473_600;

#[derive(Debug, thiserror::Error)]
pub enum SisuError {
    #[error("Xbox sign-in request failed")]
    Request(#[source] reqwest::Error),
    #[error("Xbox sign-in was refused ({0})")]
    Refused(u16),
    #[error("Xbox sign-in returned an unexpected answer")]
    Invalid,
}

impl From<reqwest::Error> for SisuError {
    fn from(error: reqwest::Error) -> Self {
        SisuError::Request(error.without_url())
    }
}

/// The proof key requests are signed with; one per sign-in.
pub struct Signer {
    key: SigningKey,
    cv: CorrelationVector,
}

impl Default for Signer {
    fn default() -> Self {
        Self::new()
    }
}

impl Signer {
    pub fn new() -> Self {
        Self {
            key: SigningKey::random(&mut rand_core::OsRng),
            cv: CorrelationVector::new(),
        }
    }

    /// The public key as a JWK, as the requests carry it.
    pub fn proof_key(&self) -> ProofKey {
        let point = self.key.verifying_key().to_encoded_point(false);
        let coordinate = |c: Option<&p256::FieldBytes>| {
            URL_SAFE_NO_PAD.encode(c.map(|c| c.as_slice()).unwrap_or_default())
        };
        ProofKey {
            alg: "ES256",
            crv: "P-256",
            kty: "EC",
            u: "sig",
            x: coordinate(point.x()),
            y: coordinate(point.y()),
        }
    }

    /// The `Signature` header value of a request.
    pub fn signature(
        &self,
        unix_seconds: u64,
        method: &str,
        path_and_query: &str,
        authorization: &str,
        body: &[u8],
    ) -> String {
        let filetime = (unix_seconds + FILETIME_EPOCH_OFFSET) * 10_000_000;
        let version = POLICY_VERSION.to_be_bytes();
        let time = filetime.to_be_bytes();
        let mut hasher = Sha256::new();
        for part in [
            &version[..],
            &time[..],
            method.to_ascii_uppercase().as_bytes(),
            path_and_query.as_bytes(),
            authorization.as_bytes(),
            &body[..body.len().min(MAX_BODY_BYTES)],
        ] {
            hasher.update(part);
            hasher.update([0u8]);
        }
        let prehash = hasher.finalize();
        // Signing a 32-byte prehash with a valid key can't fail.
        let signature: Signature = self
            .key
            .sign_prehash(&prehash)
            .expect("P-256 signs any 32-byte prehash");
        let mut bytes = Vec::with_capacity(76);
        bytes.extend_from_slice(&version);
        bytes.extend_from_slice(&time);
        bytes.extend_from_slice(&signature.to_bytes());
        STANDARD.encode(bytes)
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &mut self,
        client: &reqwest::Client,
        url: &str,
        body: &impl Serialize,
        contract: bool,
    ) -> Result<T, SisuError> {
        let body = serde_json::to_vec(body).map_err(|_| SisuError::Invalid)?;
        let parsed = url::Url::parse(url).map_err(|_| SisuError::Invalid)?;
        let path = match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_string(),
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let mut request = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("MS-CV", self.cv.next_value())
            .header("Signature", self.signature(now, "POST", &path, "", &body));
        if contract {
            request = request.header("x-xbl-contract-version", "1");
        }
        let response = request.body(body).send().await?;
        let status = response.status();
        if status.is_client_error() {
            return Err(SisuError::Refused(status.as_u16()));
        }
        let bytes = response.error_for_status()?.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|_| SisuError::Invalid)
    }

    /// A device token from an RPS device ticket (`exchange_device_token` with
    /// the `user.auth.xboxlive.com` scope).
    pub async fn device_token_rps(
        &mut self,
        client: &reqwest::Client,
        rps_ticket: &str,
        os_version: &str,
    ) -> Result<XToken, SisuError> {
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Properties<'a> {
            auth_method: &'a str,
            rps_ticket: &'a str,
            site_name: &'a str,
            version: &'a str,
            proof_key: ProofKey,
        }
        let body = TokenRequest {
            relying_party: RELYING_PARTY_AUTH,
            token_type: "JWT",
            properties: Properties {
                auth_method: "RPS",
                rps_ticket,
                site_name: "user.auth.xboxlive.com",
                version: os_version,
                proof_key: self.proof_key(),
            },
        };
        self.post(client, DEVICE_AUTH_URL, &body, true).await
    }

    /// SISU authorization of a user (RPS access token) on a device for an app.
    pub async fn authorize_rps(
        &mut self,
        client: &reqwest::Client,
        access_token: &str,
        app_id: &str,
        device_token: &str,
    ) -> Result<SisuRpsResponse, SisuError> {
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Request<'a> {
            access_token: &'a str,
            app_id: &'a str,
            device_token: &'a str,
            sandbox: &'a str,
            site_name: &'a str,
            proof_key: ProofKey,
        }
        let body = Request {
            access_token,
            app_id,
            device_token,
            sandbox: "RETAIL",
            site_name: "user.auth.xboxlive.com",
            proof_key: self.proof_key(),
        };
        self.post(client, SISU_AUTHORIZE_URL, &body, false).await
    }

    /// An XSTS token for `relying_party` from any of the three tokens.
    pub async fn xsts(
        &mut self,
        client: &reqwest::Client,
        device: Option<&str>,
        title: Option<&str>,
        user: Option<&str>,
        relying_party: &str,
    ) -> Result<XToken, SisuError> {
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Properties<'a> {
            sandbox_id: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            device_token: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            title_token: Option<&'a str>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            user_tokens: Vec<&'a str>,
        }
        let body = TokenRequest {
            relying_party,
            token_type: "JWT",
            properties: Properties {
                sandbox_id: "RETAIL",
                device_token: device,
                title_token: title,
                user_tokens: user.into_iter().collect(),
            },
        };
        self.post(client, XSTS_AUTH_URL, &body, true).await
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TokenRequest<'a, T> {
    relying_party: &'a str,
    token_type: &'a str,
    properties: T,
}

/// A proof key as a JWK.
#[derive(Debug, Clone, Serialize)]
pub struct ProofKey {
    alg: &'static str,
    crv: &'static str,
    kty: &'static str,
    #[serde(rename = "use")]
    u: &'static str,
    x: String,
    y: String,
}

/// An Xbox token (device, title, user or XSTS) with its claims.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct XToken {
    pub issue_instant: String,
    pub not_after: String,
    pub token: String,
    #[serde(default)]
    pub display_claims: Option<serde_json::Value>,
}

impl std::fmt::Debug for XToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XToken")
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

impl XToken {
    /// The user hash (`uhs`) of an XSTS token's first user.
    pub fn user_hash(&self) -> Option<&str> {
        self.display_claims
            .as_ref()?
            .pointer("/xui/0/uhs")?
            .as_str()
            .filter(|s| !s.is_empty())
    }

    /// `XBL3.0 x=<uhs>;<token>` for an XSTS token.
    pub fn authorization(&self) -> Option<String> {
        let uhs = self.user_hash()?;
        (!self.token.is_empty()).then(|| format!("XBL3.0 x={uhs};{}", self.token))
    }
}

/// What SISU authorization answers.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SisuRpsResponse {
    pub title_token: XToken,
    pub user_token: XToken,
    pub authorization_token: XToken,
    #[serde(default)]
    pub web_page: String,
}

/// Microsoft's correlation vector (`MS-CV`, v2 format: a 22-character
/// base64 base and an extension counter).
pub struct CorrelationVector {
    base: String,
    counter: u32,
}

impl Default for CorrelationVector {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationVector {
    pub fn new() -> Self {
        let mut bytes = *uuid::Uuid::new_v4().as_bytes();
        // The last base64 character carries only 2 bits in Microsoft's form.
        bytes[15] &= 0xF0;
        let base = STANDARD.encode(bytes);
        Self {
            base: base[..22].to_string(),
            counter: 0,
        }
    }

    /// The next value: `<base>.<n>`.
    pub fn next_value(&mut self) -> String {
        self.counter += 1;
        format!("{}.{}", self.base, self.counter)
    }

    /// A fresh vector's first value, for one-off requests.
    pub fn one() -> String {
        Self::new().next_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::hazmat::PrehashVerifier;

    #[test]
    fn signature_covers_policy_time_and_request_and_verifies() {
        let signer = Signer::new();
        let header = signer.signature(1_700_000_000, "post", "/authorize", "", b"{}");
        let bytes = STANDARD.decode(header).unwrap();
        assert_eq!(bytes.len(), 4 + 8 + 64);
        assert_eq!(&bytes[..4], &1i32.to_be_bytes());
        let filetime = u64::from_be_bytes(bytes[4..12].try_into().unwrap());
        assert_eq!(
            filetime,
            (1_700_000_000 + FILETIME_EPOCH_OFFSET) * 10_000_000
        );
        let mut hasher = Sha256::new();
        for part in [
            &bytes[..4],
            &bytes[4..12],
            b"POST",
            b"/authorize",
            b"",
            b"{}",
        ] {
            hasher.update(part);
            hasher.update([0u8]);
        }
        let signature = Signature::from_slice(&bytes[12..]).unwrap();
        signer
            .key
            .verifying_key()
            .verify_prehash(&hasher.finalize(), &signature)
            .unwrap();
    }

    #[test]
    fn proof_key_is_a_p256_jwk() {
        let key = serde_json::to_value(Signer::new().proof_key()).unwrap();
        assert_eq!(key["crv"], "P-256");
        assert_eq!(key["use"], "sig");
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(key["x"].as_str().unwrap())
                .unwrap()
                .len(),
            32
        );
    }

    #[test]
    fn xsts_authorization_needs_a_user_hash() {
        let token: XToken = serde_json::from_value(serde_json::json!({
            "IssueInstant": "2026-01-01T00:00:00Z", "NotAfter": "2026-01-02T00:00:00Z",
            "Token": "tok", "DisplayClaims": {"xui": [{"uhs": "123"}]}
        }))
        .unwrap();
        assert_eq!(token.authorization().unwrap(), "XBL3.0 x=123;tok");
        assert!(!format!("{token:?}").contains("tok\""));
        let bare: XToken = serde_json::from_value(serde_json::json!({
            "IssueInstant": "a", "NotAfter": "b", "Token": "tok"
        }))
        .unwrap();
        assert!(bare.authorization().is_none());
    }

    #[test]
    fn correlation_vector_counts_up_on_one_base() {
        let mut cv = CorrelationVector::new();
        let (a, b) = (cv.next_value(), cv.next_value());
        assert!(a.ends_with(".1") && b.ends_with(".2"));
        assert_eq!(a.split('.').next(), b.split('.').next());
        assert_eq!(a.split('.').next().unwrap().len(), 22);
    }
}
