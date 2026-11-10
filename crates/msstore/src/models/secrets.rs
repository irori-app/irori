use serde::{Deserialize, Serialize};

use crate::models::soap::{self, Timestamp};

#[derive(Debug, Serialize, Deserialize)]
pub struct Device {
    pub puid: String,
    pub hwid: String,
    pub device_id: String,
    pub splicense: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LegacyToken {
    pub key_name: Option<String>,
    pub token: String,
    pub binary_secret: Option<String>,
    #[serde(default)]
    pub tpm_key: Option<String>,
    pub lifetime: Timestamp,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum Token {
    Legacy(LegacyToken),
    Compact(String),
}

#[derive(Debug, thiserror::Error)]
#[error("Microsoft returned an incomplete or unsupported authentication token.")]
pub struct InvalidTokenResponse;

impl TryFrom<soap::RequestSecurityTokenResponse> for Token {
    type Error = InvalidTokenResponse;
    fn try_from(value: soap::RequestSecurityTokenResponse) -> Result<Self, Self::Error> {
        Ok(match value.token_type.as_str() {
            "urn:passport:legacy" => {
                let encrypted_data = value
                    .requested_security_token
                    .encrypted_data
                    .ok_or(InvalidTokenResponse)?;
                let key_name = encrypted_data.key_info.key_name.clone();
                let token =
                    quick_xml::se::to_string(&encrypted_data).map_err(|_| InvalidTokenResponse)?;
                let binary_secret = value
                    .requested_proof_token
                    .as_ref()
                    .map(|t| t.binary_secret.clone());
                let tpm_key = value
                    .requested_proof_token
                    .and_then(|t| t.encrypted_key.map(|k| k.cipher_data.cipher_value));
                Self::Legacy(LegacyToken {
                    key_name,
                    token,
                    binary_secret,
                    tpm_key,
                    lifetime: value.lifetime,
                })
            }
            "urn:passport:compact" => Self::Compact(
                value
                    .requested_security_token
                    .binary_security_token
                    .ok_or(InvalidTokenResponse)?
                    .value,
            ),
            "urn:passport:delegationcompact" => Self::Compact(delegation_ticket(
                value
                    .requested_security_token
                    .binary_security_token
                    .ok_or(InvalidTokenResponse)?
                    .value,
            )),
            _ => return Err(InvalidTokenResponse),
        })
    }
}

/// Xbox Live sign-in needs opaque delegation tickets as "d=", but rejects
/// compact JWE delegation tickets in that form and accepts them as "t=".
/// Opaque tickets keep their exact previous form.
fn delegation_ticket(value: String) -> String {
    if is_compact_jwe(&value) {
        format!("t={value}")
    } else {
        format!("d={value}")
    }
}

/// Store and licensing services reject delegation-compact tickets in the "d="
/// form that Xbox Live sign-in needs (InvalidMsaTicketFormat) and accept the
/// same value as "t=". Convert only where a ticket is sent to those services.
pub fn store_ticket(ticket: String) -> String {
    match ticket.strip_prefix("d=") {
        Some(value) => format!("t={value}"),
        None => ticket,
    }
}

/// Format a user ticket only when it is sent to content licensing. Compact JWE
/// tickets use their bare representation, unlike opaque delegation tickets.
/// Keep store_ticket (Store) unchanged.
pub fn licensing_ticket(ticket: String) -> String {
    let bare = ticket
        .strip_prefix("d=")
        .or_else(|| ticket.strip_prefix("t="))
        .unwrap_or(&ticket);
    if is_compact_jwe(bare) {
        bare.to_string()
    } else {
        store_ticket(ticket)
    }
}

// This recognizes wire encoding, not authenticity or entitlement. Microsoft
// still validates the entire ticket. Never decrypt, log or rewrite its payload.
fn is_compact_jwe(value: &str) -> bool {
    use base64_23::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut segments = value.split('.');
    let Some(header) = segments.next() else {
        return false;
    };
    if header.is_empty() || header.len() > 8192 {
        return false;
    }
    let Some(key) = segments.next() else {
        return false;
    };
    let Some(iv) = segments.next() else {
        return false;
    };
    let Some(ciphertext) = segments.next() else {
        return false;
    };
    let Some(tag) = segments.next() else {
        return false;
    };
    // The encrypted key may be empty with direct key agreement/encryption.
    if segments.next().is_some() || iv.is_empty() || ciphertext.is_empty() || tag.is_empty() {
        return false;
    }
    if ![header, key, iv, ciphertext, tag].iter().all(|part| {
        part.len() % 4 != 1
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return false;
    }
    let Ok(header) = URL_SAFE_NO_PAD.decode(header) else {
        return false;
    };
    let Ok(header) = serde_json::from_slice::<serde_json::Value>(&header) else {
        return false;
    };
    ["alg", "enc"].iter().all(|key| {
        header
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.is_empty())
    })
}

#[cfg(test)]
#[path = "licensing_ticket_tests.rs"]
mod licensing_ticket_tests;

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenStore {
    #[serde(flatten)]
    pub tokens: std::collections::HashMap<String, Token>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub puid: String,
    pub username: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::soap::*;
    fn response(kind: &str) -> RequestSecurityTokenResponse {
        RequestSecurityTokenResponse {
            token_type: kind.into(),
            applies_to: AppliesTo {
                endpoint_reference: EndpointReference {
                    address: "synthetic.invalid".into(),
                },
            },
            lifetime: Timestamp {
                id: None,
                created: "2000-01-01T00:00:00Z".into(),
                expires: "2099-01-01T00:00:00Z".into(),
            },
            requested_security_token: RequestedSecurityToken {
                encrypted_data: None,
                binary_security_token: None,
            },
            requested_proof_token: None,
        }
    }
    #[test]
    fn credential_token_response_missing_payloads_and_unknown_types_are_errors_not_panics() {
        for kind in [
            "urn:passport:legacy",
            "urn:passport:compact",
            "urn:passport:delegationcompact",
            "unexpected-private-payload",
        ] {
            let xml = format!(
                "<Response><TokenType>{kind}</TokenType><AppliesTo><EndpointReference><Address>synthetic.invalid</Address></EndpointReference></AppliesTo><Lifetime><Created>2000-01-01T00:00:00Z</Created><Expires>2099-01-01T00:00:00Z</Expires></Lifetime><RequestedSecurityToken/></Response>"
            );
            let parsed: RequestSecurityTokenResponse = quick_xml::de::from_str(&xml).unwrap();
            let error = Token::try_from(parsed).unwrap_err();
            assert!(!error.to_string().contains("private-payload"));
        }
    }
    #[test]
    fn credential_token_response_valid_compact_delegation_and_legacy_remain_supported() {
        for (kind, expected) in [
            ("urn:passport:compact", "synthetic"),
            ("urn:passport:delegationcompact", "d=synthetic"),
        ] {
            let mut value = response(kind);
            value.requested_security_token.binary_security_token = Some(BinarySecurityTokenRes {
                id: "fixture".into(),
                value: "synthetic".into(),
                value_type: None,
            });
            assert!(
                matches!(Token::try_from(value).unwrap(), Token::Compact(token) if token == expected)
            );
        }
        let mut value = response("urn:passport:legacy");
        value.requested_security_token.encrypted_data =
            Some(EncryptedData::devicesoftware("synthetic-ciphertext".into()));
        assert!(
            matches!(Token::try_from(value).unwrap(), Token::Legacy(token) if token.key_name.as_deref() == Some("http://Passport.NET/STS"))
        );
    }
    #[test]
    fn credential_xsts_missing_claim_or_token_returns_safe_error() {
        for (claims, token, valid) in [
            (serde_json::json!({}), "synthetic", false),
            (serde_json::json!({"xui":[{"uhs":""}]}), "synthetic", false),
            (serde_json::json!({"xui":[{"uhs":"hash"}]}), "", false),
            (
                serde_json::json!({"xui":[{"uhs":"hash"}]}),
                "synthetic",
                true,
            ),
        ] {
            let xsts = serde_json::from_value(serde_json::json!({"NotAfter":"2099-01-01T00:00:00Z","Token":token,"DisplayClaims":claims})).unwrap();
            let result = crate::api::xbox::get_xsts_auth_header(xsts);
            assert_eq!(result.is_ok(), valid);
            if valid {
                assert_eq!(result.unwrap(), "XBL3.0 x=hash;synthetic");
            }
        }
    }
}
