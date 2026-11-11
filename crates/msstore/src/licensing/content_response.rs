//! Bounded content-license response parsing. No payload or provider text in errors.
use crate::models::{
    devicecredential::License,
    licensing::{LicenseContent, LicenseContentResponse},
};
use base64_23::prelude::*;

const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LicenseContentError {
    #[error("Microsoft content licensing timed out. Check your connection and try again.")]
    Timeout,
    #[error(
        "Could not connect to Microsoft content licensing. Check your connection and try again."
    )]
    Request,
    #[error("Microsoft content licensing rejected sign-in (HTTP 401). Sign in again in Settings.")]
    Unauthorized,
    #[error(
        "Microsoft denied this content-license request (HTTP 403). Check the account, purchased edition and Store region."
    )]
    Forbidden,
    #[error(
        "Microsoft content licensing is rate-limiting requests (HTTP 429). Wait before trying again."
    )]
    RateLimited,
    #[error("Microsoft content licensing returned a server error (HTTP {0}). Try again later.")]
    Server(u16),
    #[error(
        "Microsoft content licensing returned HTTP {0}. Share the backend diagnostic log if this persists."
    )]
    Http(u16),
    #[error(
        "The content-license response exceeded the safety limit. Share the backend diagnostic log."
    )]
    Size,
    #[error(
        "The content-license response could not be read completely. Check your connection and try again."
    )]
    Read,
    #[error("Microsoft returned invalid content-license JSON. Share the backend diagnostic log.")]
    Json,
    #[error(
        "Microsoft returned an unrecognized content-license response. Share the backend diagnostic log."
    )]
    Schema,
    #[error(
        "Microsoft did not grant a license for this account and package. Check the purchased edition, subscription and Store region."
    )]
    NotEntitled,
    #[error(
        "Microsoft Store device limit reached. Open https://account.microsoft.com/devices/content, unlink an old device you no longer use, then try again. You do not need to sign out or reset the wallet."
    )]
    DeviceLimit,
    #[error(
        "Microsoft returned a content license without a key. Share the backend diagnostic log."
    )]
    EmptyKeys,
    #[error(
        "Microsoft returned an invalid base64 content-license key. Share the backend diagnostic log."
    )]
    Base64,
    #[error(
        "Microsoft returned a content-license key with invalid text encoding. Share the backend diagnostic log."
    )]
    Utf8,
    #[error(
        "Microsoft returned an invalid content-license document. Share the backend diagnostic log."
    )]
    Xml,
}

impl LicenseContentError {
    fn stage(&self) -> &'static str {
        match self {
            Self::Timeout => "content-timeout",
            Self::Request => "content-request",
            Self::Unauthorized
            | Self::Forbidden
            | Self::RateLimited
            | Self::Server(_)
            | Self::Http(_) => "content-http",
            Self::Size => "content-size",
            Self::Read => "content-read",
            Self::Json => "content-json",
            Self::Schema => "content-schema",
            Self::NotEntitled => "content-not-entitled",
            Self::DeviceLimit => "content-device-limit",
            Self::EmptyKeys => "content-empty-keys",
            Self::Base64 => "content-base64",
            Self::Utf8 => "content-utf8",
            Self::Xml => "content-xml",
        }
    }
    fn http(status: u16) -> Self {
        match status {
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            429 => Self::RateLimited,
            500..=599 => Self::Server(status),
            _ => Self::Http(status),
        }
    }
}

fn diagnostic(stage: &'static str, status: u16) {
    eprintln!("NEXUS_CONTENT_LICENSE_DIAGNOSTIC_V1 {stage} {status}");
}

pub(super) async fn fetch(
    request: reqwest::RequestBuilder,
) -> Result<(LicenseContent, License), LicenseContentError> {
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let error = if error.is_timeout() {
                LicenseContentError::Timeout
            } else {
                LicenseContentError::Request
            };
            diagnostic(error.stage(), 0);
            return Err(error);
        }
    };
    receive(response).await
}

async fn receive(
    mut response: reqwest::Response,
) -> Result<(LicenseContent, License), LicenseContentError> {
    let status = response.status().as_u16();
    let result = async {
        // A valid-looking license inside a refused HTTP response is not permission.
        if !response.status().is_success() {
            return Err(LicenseContentError::http(status));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY as u64)
        {
            return Err(LicenseContentError::Size);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                LicenseContentError::Timeout
            } else {
                LicenseContentError::Read
            }
        })? {
            if chunk.len() > MAX_BODY - bytes.len() {
                return Err(LicenseContentError::Size);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse(&bytes)
    }
    .await;
    diagnostic(
        match &result {
            Ok(_) => "content-parsed",
            Err(error) => error.stage(),
        },
        status,
    );
    result
}

fn parse(bytes: &[u8]) -> Result<(LicenseContent, License), LicenseContentError> {
    if bytes.len() > MAX_BODY {
        return Err(LicenseContentError::Size);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| LicenseContentError::Json)?;
    // Never select a success from an ambiguous success-plus-denial envelope.
    if value.get("license").is_some_and(|value| !value.is_null())
        && value
            .get("satisfactionFailure")
            .is_some_and(|value| !value.is_null())
    {
        return Err(LicenseContentError::Schema);
    }
    let response: LicenseContentResponse =
        serde_json::from_value(value).map_err(|_| LicenseContentError::Schema)?;
    let content = match response {
        LicenseContentResponse::Success { license } => license,
        LicenseContentResponse::SatisfactionFailure {
            satisfaction_failure,
        } => {
            // This is a licensing reason code, not an HTTP status. Never use
            // provider text, remediation URLs or alternate content as guidance.
            return Err(if satisfaction_failure.code == 501 {
                LicenseContentError::DeviceLimit
            } else {
                LicenseContentError::NotEntitled
            });
        }
    };
    // Preserve the first-key selection used by successful downloads.
    let key = content
        .keys
        .first()
        .filter(|key| !key.value.is_empty())
        .ok_or(LicenseContentError::EmptyKeys)?;
    let xml = BASE64_STANDARD
        .decode(&key.value)
        .map_err(|_| LicenseContentError::Base64)?;
    let xml = std::str::from_utf8(&xml).map_err(|_| LicenseContentError::Utf8)?;
    let license: License = quick_xml::de::from_str(xml).map_err(|_| LicenseContentError::Xml)?;
    if license.splicense_block.is_empty() {
        return Err(LicenseContentError::Xml);
    }
    Ok((content, license))
}

/// Parses one lease document from the content response (base64 XML), with
/// the same bounds and fixed errors as the key license.
pub fn parse_lease(value: &str) -> Result<License, LicenseContentError> {
    if value.is_empty() || value.len() > MAX_BODY {
        return Err(LicenseContentError::EmptyKeys);
    }
    let xml = BASE64_STANDARD
        .decode(value)
        .map_err(|_| LicenseContentError::Base64)?;
    let xml = std::str::from_utf8(&xml).map_err(|_| LicenseContentError::Utf8)?;
    quick_xml::de::from_str(xml).map_err(|_| LicenseContentError::Xml)
}

/// The policy Microsoft attaches to a lease (`CustomPolicies`, base64 JSON).
/// Only the fields Nexus acts on are read; unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasePolicy {
    /// Microsoft marks leases issued to the account's designated offline device.
    pub is_offline: bool,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub poll_at: Option<chrono::DateTime<chrono::Utc>>,
    pub lease_enforcement: Option<String>,
}

pub fn lease_policy(lease: &License) -> Option<LeasePolicy> {
    let encoded = lease.custom_policies.as_deref()?;
    if encoded.len() > 64 * 1024 {
        return None;
    }
    let json = BASE64_STANDARD.decode(encoded).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let time = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|time| time.with_timezone(&chrono::Utc))
    };
    Some(LeasePolicy {
        is_offline: value.get("isOffline").and_then(serde_json::Value::as_bool) == Some(true),
        expires_at: time("expiresAt"),
        poll_at: time("pollAt"),
        lease_enforcement: value
            .get("leaseEnforcement")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
#[path = "content_response_tests.rs"]
mod tests;
