use base64_23::prelude::*;
use zerocopy::transmute;

use crate::licensing::splicense::ClepHmacState;
use crate::models::devicecredential::{DeviceAddRequest, DeviceAddResponse};
use crate::models::live::ExchangeUserTokenOutcome;
use crate::models::secrets::{LegacyToken, Token};
use crate::models::soap;

mod rst;
mod utils;

/// Fixed diagnostic categories, never error text, XML, keys or account data.
pub fn exchange_failure_stage(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    use rst::RSTError;
    Some(match error.downcast_ref::<RSTError>()? {
        RSTError::Request(_) => return None,
        RSTError::Serialization(_) | RSTError::Builder(_) => "ticket-request-build",
        RSTError::Deserialization(_) => "ticket-response-parse",
        RSTError::InvalidResponseSignature(_) => "ticket-response-signature",
        _ => "ticket-response-crypto",
    })
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn ticket_diagnostic_classifies_without_disclosing_error_payloads() {
        let xml = quick_xml::de::from_str::<soap::Envelope>("<private-email-secret>").unwrap_err();
        assert_eq!(
            exchange_failure_stage(&rst::RSTError::Deserialization(xml)),
            Some("ticket-response-parse")
        );
        assert_eq!(
            exchange_failure_stage(&rst::RSTError::InvalidResponseSignature(
                "private-email-secret".into()
            )),
            Some("ticket-response-signature")
        );
        assert_eq!(
            exchange_failure_stage(&rst::RSTError::MissingNonce),
            Some("ticket-response-crypto")
        );
        assert_eq!(
            exchange_failure_stage(&std::io::Error::other("private-email-secret")),
            None
        );
    }
}

pub const XML_HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;

#[derive(Debug, thiserror::Error)]
pub enum DeviceCredentialError {
    #[error("Microsoft device registration request failed.")]
    Transport(#[from] reqwest::Error),
    #[error("Microsoft returned an invalid device registration response.")]
    InvalidResponse,
}

const DEVICE_RESPONSE_LIMIT: usize = 1024 * 1024;

async fn device_credential_response(
    response: reqwest::Response,
) -> Result<DeviceAddResponse, DeviceCredentialError> {
    let mut response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > DEVICE_RESPONSE_LIMIT as u64)
    {
        return Err(DeviceCredentialError::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > DEVICE_RESPONSE_LIMIT {
            return Err(DeviceCredentialError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    let text = std::str::from_utf8(&body).map_err(|_| DeviceCredentialError::InvalidResponse)?;
    let parsed: DeviceAddResponse =
        quick_xml::de::from_str(text).map_err(|_| DeviceCredentialError::InvalidResponse)?;
    if !parsed.success || parsed.puid.is_empty() || parsed.license.splicense_block.is_empty() {
        return Err(DeviceCredentialError::InvalidResponse);
    }
    Ok(parsed)
}

#[cfg(test)]
mod device_response_tests;

pub async fn login_device_credential(
    client: &reqwest::Client,
    data: DeviceAddRequest,
) -> Result<DeviceAddResponse, DeviceCredentialError> {
    let data =
        quick_xml::se::to_string(&data).map_err(|_| DeviceCredentialError::InvalidResponse)?;

    let response = client
        .post("https://login.live.com/ppsecure/deviceaddcredential.srf")
        .header("User-Agent", "MSAWindows/55 (OS 10.0.26100.0.0 ge_release; IDK 10.0.26100.5074 ge_release; Cfg 16.000.29325.00; Test 0)")
        .header("Content-Type", "application/soap+xml")
        .header("Host", "login.live.com")
        .body(data)
        .send()
        .await?;
    device_credential_response(response).await
}

pub async fn authenticate_device(
    client: &reqwest::Client,
    username: String,
    private_key: rsa::RsaPrivateKey,
) -> Result<soap::Envelope, rst::RSTError> {
    let request = rst::RSTRequestBuilder::new()
        .username(soap::UsernameToken::devicetoken(username))
        .signature(rst::RSTSignature::Rsa(Box::new(private_key)))
        .scope_policy("http://Passport.NET/tb", None)
        .build()?;

    request.request(client).await
}

pub async fn exchange_device_token(
    client: &reqwest::Client,
    token: LegacyToken,
    hosting_app: String,
    scope: String,
    policy: Option<soap::PolicyReference>,
) -> Result<soap::RequestSecurityTokenResponse, rst::RSTError> {
    let secret = BASE64_STANDARD.decode(
        token
            .binary_secret
            .as_ref()
            .ok_or(rst::RSTError::InvalidEncryptedPayload)?,
    )?;
    let secret: [u8; 4096] = secret
        .try_into()
        .map_err(|_| rst::RSTError::InvalidEncryptedPayload)?;
    let secret: ClepHmacState = transmute!(secret);
    let hmac_secret = secret.get_hmac_state();

    let request = rst::RSTRequestBuilder::new()
        .sso_flags("SsoRestr")
        .hosting_app(&hosting_app)
        .device_token(token)
        .signature(rst::RSTSignature::Hmac {
            clep_secret: &*hmac_secret,
            tpm_secret: &[],
        })
        .scope_policy(&scope, policy)
        .build()?;

    let envelope = request.request(client).await?;

    match envelope.body.body {
        soap::BodyContent::RequestSecurityTokenResponse(res) => Ok(*res),
        soap::BodyContent::RequestSecurityTokenResponseCollection(mut collection) => {
            let token = collection
                .security_tokens
                .drain(..)
                .next()
                .ok_or(rst::RSTError::InvalidEncryptedPayload)?;
            Ok(token)
        }
        _ => Err(rst::RSTError::InvalidEncryptedPayload),
    }
}

// Each parameter maps directly to a distinct SOAP request field; grouping them
// into a params struct would just move the sprawl rather than reduce it.
#[allow(clippy::too_many_arguments)]
pub async fn exchange_user_token(
    client: &reqwest::Client,
    user_token: LegacyToken,
    username: String,
    device_token: LegacyToken,
    inline_token: Option<String>,
    inline_ux: Option<String>,
    hosting_app: String,
    scope_policies: &[(String, Option<soap::PolicyReference>)],
) -> Result<ExchangeUserTokenOutcome, rst::RSTError> {
    let secret = BASE64_STANDARD.decode(
        device_token
            .binary_secret
            .as_ref()
            .ok_or(rst::RSTError::InvalidEncryptedPayload)?,
    )?;
    let secret: [u8; 4096] = secret
        .try_into()
        .map_err(|_| rst::RSTError::InvalidEncryptedPayload)?;
    let secret: ClepHmacState = transmute!(secret);
    let hmac_secret = secret.get_hmac_state();

    let mut builder = rst::RSTRequestBuilder::new()
        .username(soap::UsernameToken::user_hint(username))
        .device_token(device_token)
        .user_token(Token::Legacy(user_token))
        .hosting_app(&hosting_app)
        .sso_flags("SsoRestr")
        .license_signature_key_version(None)
        .signature(rst::RSTSignature::Hmac {
            clep_secret: &*hmac_secret,
            tpm_secret: &[],
        });

    if let Some(ux) = inline_ux.as_deref() {
        builder = builder.inline_ux(ux);
    }
    if let Some(ft) = inline_token.as_deref() {
        builder = builder.inline_ft(ft);
    }
    for (scope, policy) in scope_policies {
        builder = builder.scope_policy(scope, policy.clone());
    }

    let request = builder.build()?;
    let envelope = request.request(client).await?;

    Ok(match envelope.body.body {
        soap::BodyContent::Fault(_) => ExchangeUserTokenOutcome::Fault(envelope.header.pp),
        body => ExchangeUserTokenOutcome::Issued(body),
    })
}

#[cfg(test)]
mod test {
    use crate::api::live::exchange_device_token;
    use crate::models::secrets::Token;
    use crate::models::soap;
    use crate::tokens::TokenManager;
    use crate::tokens::device::ensure_device_credentials;

    #[tokio::test]
    async fn test_get_xbox_live_dev_token() {
        let client = reqwest::Client::new();

        let mgr = TokenManager::with_memory();
        ensure_device_credentials(&client, &mgr).await.unwrap();

        let token: Token = mgr.get_device_sts_token().unwrap();
        let Token::Legacy(token) = token else {
            todo!("no a LegacyToken");
        };
        let resp = exchange_device_token(
            &client,
            token,
            "{28C08266-F973-4AE6-FFE4-409B249F138F}".to_string(),
            "scope=service::user.auth.xboxlive.com::MBI_SSL&api-version=2.0".to_owned(),
            Some(soap::PolicyReference::token_broker()),
        )
        .await
        .unwrap();

        let ms_device_token = Token::try_from(resp).unwrap();
        let Token::Compact(ms_device_token) = ms_device_token else {
            todo!("Unsupported token");
        };

        println!("{}", ms_device_token);
    }
}
