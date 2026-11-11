use reqwest::Client;

use crate::models::live::ExchangeUserTokenOutcome;
use crate::models::secrets::Token;
use crate::models::soap;
use crate::sisu::{Signer, SisuRpsResponse, XToken};
use crate::tokens::TokenManager;

type RefreshedUserToken = (String, Token);

/// Validated token types and expiry fields, including an optional refresh credential. Selection is
/// shared by the CLI and service so response ordering cannot change token roles.
pub struct IssuedUserTokens {
    pub token: String,
    pub expiry: i64,
    pub refreshed: Option<(String, Token)>,
}

pub fn issued_user_tokens(
    outcome: ExchangeUserTokenOutcome,
) -> Result<IssuedUserTokens, crate::models::secrets::InvalidTokenResponse> {
    use crate::models::secrets::InvalidTokenResponse;
    let responses = match outcome {
        ExchangeUserTokenOutcome::Issued(
            soap::BodyContent::RequestSecurityTokenResponseCollection(collection),
        ) => collection.security_tokens,
        ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
            response,
        )) => vec![*response],
        _ => return Err(InvalidTokenResponse),
    };
    let mut compact = None;
    let mut refreshed = None;
    for response in responses {
        let address = response.applies_to.endpoint_reference.address.clone();
        let expiry = chrono::DateTime::parse_from_rfc3339(&response.lifetime.expires)
            .map_err(|_| InvalidTokenResponse)?
            .timestamp();
        match Token::try_from(response)? {
            Token::Compact(value)
                if compact.is_none()
                    && !value.is_empty()
                    && !matches!(value.as_str(), "d=" | "t=") =>
            {
                compact = Some((value, expiry))
            }
            Token::Legacy(value) if refreshed.is_none() => {
                let address = value.key_name.clone().unwrap_or(address);
                if address.trim().is_empty() {
                    return Err(InvalidTokenResponse);
                }
                refreshed = Some((address, Token::Legacy(value)));
            }
            _ => return Err(InvalidTokenResponse),
        }
    }
    let (token, expiry) = compact.ok_or(InvalidTokenResponse)?;
    Ok(IssuedUserTokens {
        token,
        expiry,
        refreshed,
    })
}

fn sisu_user_tokens(
    outcome: ExchangeUserTokenOutcome,
) -> Result<(String, Option<RefreshedUserToken>), crate::models::secrets::InvalidTokenResponse> {
    let selected = issued_user_tokens(outcome)?;
    Ok((selected.token, selected.refreshed))
}

#[cfg(test)]
mod sisu_response_tests;

/// Device-bound SISU sign-in of the account in `role` for an app (`client_id`
/// and its Xbox title id): the device's RPS ticket becomes a device token, the
/// account's ticket for `xboxlive.signin` is exchanged silently, and SISU
/// answers with title, user and XSTS tokens. Use the returned signer and
/// device token for further XSTS requests.
pub async fn do_sisu_role(
    client: &Client,
    manager: &TokenManager,
    client_id: &str,
    role: crate::tokens::AccountRole,
) -> Result<(Signer, SisuRpsResponse, XToken), Box<dyn std::error::Error + Send + Sync>> {
    let mut snapshot = manager.user_token_snapshot_role(role)?;
    let Token::Legacy(token) = snapshot.token() else {
        return Err(Box::new(crate::models::secrets::InvalidTokenResponse));
    };
    let scope = "xboxlive.signin";
    let Token::Legacy(device_token) = manager.get_device_sts_token()? else {
        return Err(Box::new(crate::models::secrets::InvalidTokenResponse));
    };
    let device_token_resp: soap::RequestSecurityTokenResponse =
        crate::api::live::exchange_device_token(
            client,
            device_token.clone(),
            "{28C08266-F973-4AE6-FFE4-409B249F138F}".to_string(),
            "scope=service::user.auth.xboxlive.com::MBI_SSL&api-version=2.0".to_owned(),
            Some(soap::PolicyReference::token_broker()),
        )
        .await?;

    let Token::Compact(ms_device_token) = Token::try_from(device_token_resp)? else {
        return Err(Box::new(crate::models::secrets::InvalidTokenResponse));
    };

    let user_token = crate::api::live::exchange_user_token(
        client,
        token,
        "USERNAME".to_string(),
        device_token,
        None,
        Some("Silent".to_string()),
        client_id.to_string(),
        &[
            (
                format!("scope={scope}&api-version=2.0&clientid={client_id}"),
                Some(soap::PolicyReference::token_broker()),
            ),
            ("http://Passport.NET/tb".to_string(), None),
        ],
    )
    .await?;

    let (user_token, refreshed) = sisu_user_tokens(user_token)?;
    manager.accept_user_token_refresh(&mut snapshot, refreshed)?;

    let mut signer = Signer::new();
    let device = signer
        .device_token_rps(client, &ms_device_token, "10.0.22621")
        .await?;
    let resp = signer
        .authorize_rps(client, &user_token, client_id, &device.token)
        .await?;
    manager.validate_user_token_snapshot(&snapshot)?;
    Ok((signer, resp, device))
}
