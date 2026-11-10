use crate::models::live::ExchangeUserTokenOutcome;
use crate::models::secrets::{LegacyToken, Token};
use crate::models::soap;
use crate::models::xbox::XstsResponse;

pub mod auth;
pub mod title;
pub use auth::{authenticate_xbox_user, get_xsts_auth_header, request_xsts_token};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Xbox authentication request failed. Check your connection and sign-in.")]
    Request,
    #[error("Xbox authentication timed out. Check your connection and retry.")]
    Timeout,
    #[error("Microsoft refused the stored sign-in. Please sign in again.")]
    Refused,
    #[error(transparent)]
    Token(#[from] crate::models::secrets::InvalidTokenResponse),
}

pub async fn run(
    client: &reqwest::Client,
    dev_token: LegacyToken,
    legacy: LegacyToken,
    relying_party: &str,
) -> Result<XstsResponse, SessionError> {
    use crate::models::secrets::InvalidTokenResponse;
    let exchange = async {
        let user_token = crate::api::live::exchange_user_token(
            client,
            legacy,
            "USERNAME".to_string(),
            dev_token,
            None,
            Some("Silent".to_string()),
            "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".to_string(),
            &[(
                "user.auth.xboxlive.com".to_owned(),
                Some(soap::PolicyReference::mbi_ssl()),
            )],
        )
        .await
        .map_err(|_| SessionError::Request)?;

        let user_token: Token = match user_token {
            ExchangeUserTokenOutcome::Fault(_) => return Err(SessionError::Refused),
            ExchangeUserTokenOutcome::Issued(
                soap::BodyContent::RequestSecurityTokenResponseCollection(mut collection),
            ) => {
                let token = collection
                    .security_tokens
                    .drain(..)
                    .next()
                    .ok_or(InvalidTokenResponse)?;
                token.try_into()?
            }
            ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
                token,
            )) => (*token).try_into()?,
            _ => return Err(InvalidTokenResponse.into()),
        };
        let Token::Compact(user_token) = user_token else {
            return Err(InvalidTokenResponse.into());
        };
        let resp = authenticate_xbox_user(client, user_token)
            .await
            .map_err(|_| SessionError::Request)?;

        request_xsts_token(client, resp.token, relying_party)
            .await
            .map_err(|_| SessionError::Request)
    };
    tokio::time::timeout(std::time::Duration::from_secs(60), exchange)
        .await
        .map_err(|_| SessionError::Timeout)?
}
