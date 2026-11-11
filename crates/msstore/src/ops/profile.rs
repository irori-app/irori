//! Optional, bounded Xbox profile read. Never provisions a device or opens login.
use crate::{
    models::{live::ExchangeUserTokenOutcome, secrets::Token, soap},
    tokens::TokenManager,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::{Value, json};
use std::time::Duration;

const PROFILE: &str =
    "https://profile.xboxlive.com/users/me/profile/settings?settings=Gamertag,PublicGamerpic";
const MAX_IMAGE: usize = 2 * 1024 * 1024;

pub(super) async fn bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, &'static str> {
    if !response.status().is_success() {
        return Err("http-status");
    }
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("response-size");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "response-read")? {
        if bytes.len() + chunk.len() > limit {
            return Err("response-size");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) fn picture_url(text: &str) -> Option<reqwest::Url> {
    if text.len() > 8192 {
        return None;
    }
    let url = reqwest::Url::parse(text).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || ![
            "gamerpic.xboxlive.com",
            "images-eds-ssl.xboxlive.com",
            "avatar-ssl.xboxlive.com",
            "xboxavatargamerpic.azureedge.net",
        ]
        .contains(&host)
    {
        return None;
    }
    Some(url)
}

fn fields(value: &Value) -> Result<(String, Option<reqwest::Url>), &'static str> {
    let users = value["profileUsers"].as_array().ok_or("profile-shape")?;
    if users.len() != 1 {
        return Err("profile-count");
    }
    let settings = users[0]["settings"].as_array().ok_or("profile-settings")?;
    let setting = |name| {
        settings
            .iter()
            .find(|item| item["id"] == name)
            .and_then(|item| item["value"].as_str())
    };
    let gamertag = setting("Gamertag")
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control))
        .ok_or("gamertag")?;
    Ok((
        gamertag.to_owned(),
        setting("PublicGamerpic").and_then(picture_url),
    ))
}

fn image_data(bytes: &[u8]) -> Option<String> {
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        return None;
    };
    Some(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

pub(super) async fn xbox_session(
    tokens: &TokenManager,
) -> Result<(reqwest::Client, String, String), &'static str> {
    let (client, authorization, account, _) = xbox_session_identity(tokens).await?;
    Ok((client, authorization, account))
}

pub(super) async fn xbox_session_identity(
    tokens: &TokenManager,
) -> Result<(reqwest::Client, String, String, Option<String>), &'static str> {
    xbox_session_for(tokens, "http://xboxlive.com").await
}

fn auth_failure(stage: &'static str, error: reqwest::Error) -> &'static str {
    if let Some(status) = error.status() {
        tracing::debug!(
            "NEXUS_SOCIAL_AUTH_DIAGNOSTIC_V1 {stage} status {}",
            status.as_u16()
        );
    } else {
        let class = if error.is_timeout() {
            "timeout"
        } else if error.is_connect() {
            "connect"
        } else if error.is_decode() {
            "decode"
        } else {
            "request"
        };
        tracing::debug!("NEXUS_SOCIAL_AUTH_DIAGNOSTIC_V1 {stage} {class}");
    }
    stage
}

fn social_cache_key(
    account: &str,
    user: &crate::models::secrets::LegacyToken,
    audience: &str,
) -> Result<String, &'static str> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    account.hash(&mut hash);
    audience.hash(&mut hash);
    serde_json::to_vec(user)
        .map_err(|_| "session-cache-key")?
        .hash(&mut hash);
    Ok(format!("nexus-social-bearer:{:016x}", hash.finish()))
}
pub(super) fn invalidate_social_session(tokens: &TokenManager) {
    let Ok(snapshot) = tokens.user_token_snapshot() else {
        return;
    };
    let Some(account) = snapshot.stored_user() else {
        return;
    };
    let Token::Legacy(user) = snapshot.token() else {
        return;
    };
    for audience in ["http://xboxlive.com", "http://gssv.xboxlive.com/"] {
        if let Ok(key) = social_cache_key(
            &format!("{}:{}", account.username, snapshot.cache_binding()),
            &user,
            audience,
        ) {
            tokens.remove_cached_xsts(&key);
        }
    }
}

pub(super) async fn xbox_session_for(
    tokens: &TokenManager,
    audience: &str,
) -> Result<(reqwest::Client, String, String, Option<String>), &'static str> {
    xbox_session_role(tokens, audience, false).await
}

/// `license` selects the Store/Game Pass account instead of the player account.
async fn xbox_session_role(
    tokens: &TokenManager,
    audience: &str,
    license: bool,
) -> Result<(reqwest::Client, String, String, Option<String>), &'static str> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(super::session::REQUEST_TIMEOUT)
        .user_agent("xodus-nexus-profile/1")
        .build()
        .map_err(|_| "http-client")?;
    let prepared = if license {
        super::session::prepare_license(tokens, &client).await
    } else {
        super::session::prepare(tokens, &client).await
    };
    let (account, device, user, snapshot) = prepared.inspect_err(|&stage| {
        super::session::diagnostic(stage);
    })?;
    let cache_key = social_cache_key(
        &format!("{}:{}", account, snapshot.cache_binding()),
        &user,
        audience,
    )?;
    let cached = tokens
        .get_cached_xsts(&cache_key)
        .filter(|t| t.not_after > chrono::Utc::now() + chrono::Duration::seconds(60));
    if std::env::var_os("NEXUS_SOCIAL_TRACE_CACHE").is_some() {
        tracing::debug!(
            "NEXUS_SOCIAL_CACHE_DIAGNOSTIC_V1 {}",
            if cached.is_some() { "hit" } else { "miss" }
        );
    }
    let xsts = if let Some(token) = cached {
        token
    } else {
        // Reuse the upstream RPS -> Xbox user -> XSTS exchange. This is a bearer
        // user token (no ProofKey); it must not use the runtime's PoP-bound token.
        let outcome = super::session::exchange(|| async {
            crate::api::live::exchange_user_token(
                &client,
                user.clone(),
                account.clone(),
                device.clone(),
                None,
                Some("Silent".into()),
                "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".into(),
                &[(
                    "user.auth.xboxlive.com".into(),
                    Some(soap::PolicyReference::mbi_ssl()),
                )],
            )
            .await
            .map_err(|error| super::session::classify(&error))
        })
        .await?;
        if let Some(stage) = super::session::rejection(&outcome) {
            return Err(stage);
        }
        let token: Token = match outcome {
            ExchangeUserTokenOutcome::Issued(
                soap::BodyContent::RequestSecurityTokenResponseCollection(mut collection),
            ) if !collection.security_tokens.is_empty() => {
                Token::try_from(collection.security_tokens.remove(0))
                    .map_err(|_| "ticket-response-parse")?
            }
            ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
                response,
            )) => Token::try_from(*response).map_err(|_| "ticket-response-parse")?,
            _ => return Err("msa-response"),
        };
        let Token::Compact(ticket) = token else {
            return Err("msa-ticket");
        };
        let user = crate::api::xbox::authenticate_xbox_user(&client, ticket)
            .await
            .map_err(|e| auth_failure("xbox-user", e))?;
        let xsts = crate::api::xbox::request_xsts_token(&client, user.token, audience)
            .await
            .map_err(|e| auth_failure("xsts", e))?;
        if xsts.user_hash().is_none_or(str::is_empty) {
            return Err("xsts-claim");
        }
        tokens
            .validate_user_token_snapshot(&snapshot)
            .map_err(|_| "account-changed")?;
        tokens.cache_xsts(&cache_key, &xsts);
        xsts
    };
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "account-changed")?;
    let uhs = xsts.user_hash().ok_or("xsts-claim")?;
    let identity = xsts.user_id().map(str::to_owned);
    let authorization = format!("XBL3.0 x={uhs};{}", xsts.token);
    Ok((client, authorization, account, identity))
}

async fn profile(tokens: &TokenManager, license: bool) -> Result<Value, &'static str> {
    let snapshot = tokens
        .user_token_snapshot_role(if license {
            crate::tokens::AccountRole::License
        } else {
            crate::tokens::AccountRole::Xbox
        })
        .map_err(|_| "stored-account")?;
    let (client, authorization, account, _) =
        xbox_session_role(tokens, "http://xboxlive.com", license).await?;
    let response = client
        .get(PROFILE)
        .header("Authorization", authorization)
        .header("x-xbl-contract-version", "2")
        .send()
        .await
        .map_err(|_| "profile-request")?;
    if response.status().as_u16() == 401 && !license {
        invalidate_social_session(tokens);
    }
    let body = bounded(response, 128 * 1024).await?;
    let (gamertag, picture) = fields(&serde_json::from_slice(&body).map_err(|_| "profile-json")?)?;
    // Fresh request, no Authorization/Signature or redirects to the image CDN.
    let image = if let Some(url) = picture {
        match client.get(url).send().await {
            Ok(response) => bounded(response, MAX_IMAGE)
                .await
                .ok()
                .and_then(|bytes| image_data(&bytes)),
            Err(_) => None,
        }
    } else {
        None
    };
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "account-changed")?;
    Ok(json!({ "schema": 1, "account": account, "gamertag": gamertag, "gamerpicDataUrl": image }))
}

/// The profile of the player account (or, with `license`, of the Store
/// account): schema 1 with the account, gamertag, gamerpic as a data URL and
/// the Store account's Game Pass membership (`subscription`).
pub async fn profile_with_membership(
    tokens: &TokenManager,
    license: bool,
) -> Result<Value, &'static str> {
    let role = if license {
        crate::tokens::AccountRole::License
    } else {
        crate::tokens::AccountRole::Xbox
    };
    let output_snapshot = tokens.user_token_snapshot_role(role).ok();
    let account = if license {
        tokens.get_license_user()
    } else {
        tokens.get_user()
    }
    .ok()
    .map(|user| user.username);
    // Game Pass membership belongs to the license account, which is the
    // player account unless a separate one is stored.
    let license_snapshot = tokens
        .user_token_snapshot_role(crate::tokens::AccountRole::License)
        .ok();
    let license_account = license_snapshot
        .as_ref()
        .and_then(|s| s.stored_user())
        .map(|u| u.username.clone());
    let membership = async {
        match license_account.as_deref() {
            Some(account) => super::subscription::lookup(tokens, account).await,
            None => json!({"status":"unknown"}),
        }
    };
    let (result, subscription) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(45), profile(tokens, license)),
        membership
    );
    let mut value = result.map_err(|_| "profile-timeout")??;
    if output_snapshot
        .as_ref()
        .is_none_or(|s| tokens.validate_user_token_snapshot(s).is_err())
    {
        return Err("account-changed");
    }
    if value["account"].as_str() == account.as_deref() {
        value["subscription"] = if license_snapshot
            .as_ref()
            .is_some_and(|s| tokens.validate_user_token_snapshot(s).is_ok())
        {
            subscription
        } else {
            json!({"status":"unknown"})
        };
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn social_memory_cache_is_separate_for_accounts_sessions_and_audiences_and_can_be_invalidated()
    {
        let mut user = crate::models::secrets::LegacyToken {
            key_name: None,
            token: "synthetic-credential".into(),
            binary_secret: None,
            tpm_key: None,
            lifetime: soap::Timestamp {
                id: None,
                created: "2026-01-01T00:00:00Z".into(),
                expires: "2099-01-01T00:00:00Z".into(),
            },
        };
        let first = social_cache_key("first-account", &user, "http://xboxlive.com").unwrap();
        assert_ne!(
            first,
            social_cache_key("second-account", &user, "http://xboxlive.com").unwrap()
        );
        assert_ne!(
            first,
            social_cache_key("first-account", &user, "http://gssv.xboxlive.com/").unwrap()
        );
        user.token = "new-synthetic-session".into();
        assert_ne!(
            first,
            social_cache_key("first-account", &user, "http://xboxlive.com").unwrap()
        );
        assert!(!first.contains("credential"));
        let tokens = TokenManager::with_memory();
        let value:crate::models::xbox::XstsResponse=serde_json::from_value(json!({"NotAfter":"2099-01-01T00:00:00Z","Token":"synthetic-token","DisplayClaims":{"xui":[{"uhs":"hash","xid":"1"}]}})).unwrap();
        tokens.cache_xsts(&first, &value);
        assert!(tokens.get_cached_xsts(&first).is_some());
        tokens.remove_cached_xsts(&first);
        assert!(tokens.get_cached_xsts(&first).is_none());
    }
    #[test]
    fn profile_settings_are_validated_without_exposing_other_fields() {
        let value = json!({"profileUsers":[{"id":"private-id", "settings":[
            {"id":"Gamertag","value":"Example Gamer"},
            {"id":"PublicGamerpic","value":"https://gamerpic.xboxlive.com/avatar.png"}]}]});
        let (name, picture) = fields(&value).unwrap();
        assert_eq!(name, "Example Gamer");
        assert!(picture.is_some());
        assert!(fields(&json!({"profileUsers":[]})).is_err());
        assert!(fields(&json!({"profileUsers":[{},{}]})).is_err());
        assert!(picture_url("http://gamerpic.xboxlive.com/a").is_none());
        assert!(picture_url("https://evil.test/a").is_none());
        assert!(picture_url("https://user:secret@gamerpic.xboxlive.com/a").is_none());
        assert!(image_data(b"<svg/>").is_none());
    }
}
