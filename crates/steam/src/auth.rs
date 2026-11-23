//! Steam authentication helpers:
//!
//! * [`SteamAuth`]: either a web session `access_token` obtained from Valve's
//!   official login page (`store.steampowered.com/login/`), or a personal
//!   Steam Web API key from `steamcommunity.com/dev/apikey`.
//! * [`TokenClaims`]: JWT payload inspection (expiration and SteamID64).
//! * [`Client::refresh_web_session`]: renews an expiring browser
//!   `steamLoginSecure` access token using the `steamRefresh_steam` cookie
//!   through `GET https://login.steampowered.com/jwt/refresh?redir=...`,
//!   matching the redirect chain a real browser follows instead of calling
//!   `IAuthenticationService` as `PLATFORM_MOBILE_APP`.
//!
//! Tokens, cookies, and Web API keys never appear in logs or `Debug` output.

use std::fmt;

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use reqwest::header::{COOKIE, LOCATION, REFERER, SET_COOKIE, USER_AGENT};
use serde::Deserialize;

use crate::api::Client;
use crate::error::SteamError;

/// Maximum redirects followed during a web JWT refresh (`jwt/refresh -> settoken`).
const MAX_REFRESH_HOPS: usize = 5;

/// Standard desktop browser user agent used for the web cookie refresh flow.
const WEB_REFRESH_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// Trusted Steam hosts allowed while following web refresh redirects.
const TRUSTED_REFRESH_HOSTS: &[&str] = &[
    "login.steampowered.com",
    "store.steampowered.com",
    "steamcommunity.com",
];

/// How an authenticated Steam Web API call proves identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SteamAuth<'a> {
    /// Browser `access_token` JWT from `steamLoginSecure`.
    WebSession(&'a str),
    /// User-provided Steam Web API key (`steamcommunity.com/dev/apikey`).
    ApiKey(&'a str),
}

impl fmt::Debug for SteamAuth<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSession(_) => f.write_str("SteamAuth::WebSession(<redacted>)"),
            Self::ApiKey(_) => f.write_str("SteamAuth::ApiKey(<redacted>)"),
        }
    }
}

impl<'a> SteamAuth<'a> {
    /// Query parameter name and value for endpoints that accept both
    /// `access_token` and `key` (such as `IPlayerService/GetOwnedGames`).
    pub(crate) fn query_param(self) -> (&'static str, String) {
        match self {
            Self::WebSession(token) => ("access_token", token.to_string()),
            Self::ApiKey(key) => ("key", key.to_string()),
        }
    }
}

/// Result of renewing a browser session via `/jwt/refresh`.
#[derive(Clone, PartialEq, Eq)]
pub struct Renewed {
    pub access_token: String,
    /// Updated `steamRefresh_steam` cookie value when Steam rotated it.
    pub refresh_cookie: Option<String>,
}

impl fmt::Debug for Renewed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renewed")
            .field("new_refresh_cookie", &self.refresh_cookie.is_some())
            .finish_non_exhaustive()
    }
}

/// What a Steam JWT says about itself. Read without checking the signature:
/// only Steam judges the token; this only tells when to renew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenClaims {
    pub steamid: u64,
    /// Unix time the token expires.
    pub expires_at: u64,
    /// Where the token is valid (`web`, `client`, `renew`, `derive`...).
    pub audience: Vec<String>,
}

impl TokenClaims {
    /// Read the claims of a JWT.
    pub fn parse(jwt: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        #[derive(Deserialize)]
        struct Raw {
            sub: serde_json::Value,
            #[serde(default)]
            exp: u64,
            #[serde(default)]
            aud: Option<OneOrMany>,
        }
        let unreadable = || anyhow!("Steam sent a sign-in token Irori could not read.");
        let payload = jwt.trim().split('.').nth(1).ok_or_else(unreadable)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .map_err(|_| unreadable())?;
        let raw: Raw = serde_json::from_slice(&bytes).map_err(|_| unreadable())?;
        let steamid = match &raw.sub {
            serde_json::Value::String(s) => s.parse().ok(),
            v => v.as_u64(),
        }
        .ok_or_else(unreadable)?;
        Ok(Self {
            steamid,
            expires_at: raw.exp,
            audience: match raw.aud {
                Some(OneOrMany::One(a)) => vec![a],
                Some(OneOrMany::Many(a)) => a,
                None => Vec::new(),
            },
        })
    }

    /// Whether the token expires within `margin_secs` of `now` (Unix seconds).
    pub fn is_expiring(&self, now: u64, margin_secs: u64) -> bool {
        now.saturating_add(margin_secs) >= self.expires_at
    }
}

/// Split a `steamLoginSecure` or `steamRefresh_steam` cookie value
/// (`<steamid64>%7C%7C<jwt>` or `<steamid64>||<jwt>`) into `(steamid, jwt)`.
pub fn parse_steam_cookie(raw: &str) -> Option<(u64, String)> {
    let trimmed = raw.trim();
    let (id_str, token) = trimmed
        .split_once("%7C%7C")
        .or_else(|| trimmed.split_once("%7c%7c"))
        .or_else(|| trimmed.split_once("||"))?;
    let steamid: u64 = id_str.parse().ok().filter(|&id| id > 0)?;
    if token.split('.').count() != 3 || token.split('.').any(str::is_empty) {
        return None;
    }
    Some((steamid, token.to_string()))
}

fn extract_set_cookie<'a>(header_value: &'a str, name: &str) -> Option<&'a str> {
    let pair = header_value.split(';').next()?.trim();
    let (cookie_name, cookie_value) = pair.split_once('=')?;
    (cookie_name.trim() == name).then_some(cookie_value.trim())
}

fn host_port(url: &Url) -> Option<(String, Option<u16>)> {
    Some((
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default(),
    ))
}

fn is_allowed_redirect(target: &Url, allowed_test_origins: &[(String, Option<u16>)]) -> bool {
    let Some(target_hp) = host_port(target) else {
        return false;
    };
    if target.scheme() == "https" && TRUSTED_REFRESH_HOSTS.contains(&target_hp.0.as_str()) {
        return true;
    }
    allowed_test_origins.iter().any(|hp| hp == &target_hp)
}

impl Client {
    /// Renew an expiring browser `access_token` using the `steamRefresh_steam`
    /// cookie via `GET {login}/jwt/refresh?redir={store}/`.
    ///
    /// Follows only trusted Steam redirects (or the configured test endpoints)
    /// and extracts the fresh `steamLoginSecure` (and any rotated
    /// `steamRefresh_steam`) from `Set-Cookie` headers.
    pub async fn refresh_web_session(&self, refresh_cookie: &str) -> Result<Renewed> {
        let (expected_steamid, _) =
            parse_steam_cookie(refresh_cookie).ok_or(SteamError::SessionRejected)?;

        let store_redir = format!("{}/", self.endpoints().store);
        let mut current = Url::parse(&format!("{}/jwt/refresh", self.endpoints().login))
            .context("invalid Steam login endpoint")?;
        current.query_pairs_mut().append_pair("redir", &store_redir);

        let mut allowed_test_origins = Vec::new();
        for base in [&self.endpoints().login, &self.endpoints().store] {
            if let Ok(u) = Url::parse(base)
                && let Some(hp) = host_port(&u)
            {
                allowed_test_origins.push(hp);
            }
        }
        let login_origin = Url::parse(&self.endpoints().login)
            .ok()
            .and_then(|u| host_port(&u));

        let mut new_access_token: Option<String> = None;
        let mut rotated_refresh: Option<String> = None;

        for hop in 0..=MAX_REFRESH_HOPS {
            let mut req = self
                .no_redirect_http()
                .get(current.clone())
                .header(USER_AGENT, WEB_REFRESH_USER_AGENT)
                .header(REFERER, &store_redir);
            // Only send `steamRefresh_steam` to the login host.
            if host_port(&current) == login_origin {
                req = req.header(COOKIE, format!("steamRefresh_steam={refresh_cookie}"));
            }
            let resp = req
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("Could not reach Steam to renew the session")?;
            let status = resp.status();
            tracing::debug!(hop, status = status.as_u16(), "steam web refresh hop");

            for val in resp.headers().get_all(SET_COOKIE) {
                let Ok(text) = val.to_str() else { continue };
                if let Some(raw) = extract_set_cookie(text, "steamLoginSecure")
                    && let Some((id, jwt)) = parse_steam_cookie(raw)
                    && id == expected_steamid
                    && TokenClaims::parse(&jwt)
                        .is_ok_and(|claims| claims.steamid == expected_steamid)
                {
                    new_access_token = Some(jwt);
                }
                if let Some(raw) = extract_set_cookie(text, "steamRefresh_steam")
                    && parse_steam_cookie(raw).is_some_and(|(id, _)| id == expected_steamid)
                {
                    rotated_refresh = Some(raw.to_string());
                }
            }

            if let Some(access_token) = new_access_token {
                return Ok(Renewed {
                    access_token,
                    refresh_cookie: rotated_refresh,
                });
            }

            if status.is_redirection() {
                let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()) else {
                    break;
                };
                let next = current.join(loc).map_err(|_| SteamError::SessionRejected)?;
                if !is_allowed_redirect(&next, &allowed_test_origins) {
                    return Err(SteamError::SessionRejected.into());
                }
                // Redirecting back to Steam's login form means the refresh cookie expired.
                if next.path().starts_with("/login") && !next.path().contains("settoken") {
                    return Err(SteamError::SessionRejected.into());
                }
                current = next;
                continue;
            }

            if let Some(e) = SteamError::from_http_status(status.as_u16()) {
                return Err(e.into());
            }
            break;
        }

        Err(SteamError::SessionRejected.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};

    const STEAMID: u64 = 76_561_197_960_265_730;

    fn fake_jwt(steamid: u64, exp: u64, aud: &[&str]) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"typ":"JWT","alg":"EdDSA"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "sub": steamid.to_string(),
                "exp": exp,
                "aud": aud,
            })
            .to_string(),
        );
        format!("{header}.{payload}.sig")
    }

    fn client(server: &TestServer) -> Client {
        Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            login: server.base(),
            assets: server.base(),
        })
        .unwrap()
    }

    #[test]
    fn parses_jwt_claims_and_expiry() {
        let jwt = fake_jwt(STEAMID, 2_000_000_000, &["web", "renew"]);
        let claims = TokenClaims::parse(&jwt).unwrap();
        assert_eq!(claims.steamid, STEAMID);
        assert_eq!(claims.expires_at, 2_000_000_000);
        assert_eq!(claims.audience, vec!["web", "renew"]);
        assert!(!claims.is_expiring(1_999_999_000, 600));
        assert!(claims.is_expiring(1_999_999_500, 600));
    }

    #[test]
    fn parses_steam_cookie_formats_and_redacts_debug() {
        let jwt = fake_jwt(STEAMID, 2_000_000_000, &["web"]);
        assert_eq!(
            parse_steam_cookie(&format!("{STEAMID}%7C%7C{jwt}")),
            Some((STEAMID, jwt.clone()))
        );
        assert_eq!(
            parse_steam_cookie(&format!("{STEAMID}||{jwt}")),
            Some((STEAMID, jwt.clone()))
        );
        assert_eq!(parse_steam_cookie("bad"), None);

        let web = SteamAuth::WebSession("super-secret-jwt");
        let key = SteamAuth::ApiKey("super-secret-key");
        assert!(!format!("{web:?}").contains("super-secret"));
        assert!(!format!("{key:?}").contains("super-secret"));
    }

    #[tokio::test]
    async fn refresh_web_session_follows_settoken_and_captures_cookies() {
        let old_refresh_jwt = fake_jwt(STEAMID, 2_100_000_000, &["web", "renew"]);
        let new_refresh_jwt = fake_jwt(STEAMID, 2_200_000_000, &["web", "renew"]);
        let new_access_jwt = fake_jwt(STEAMID, 2_000_000_000, &["web"]);
        let old_cookie = format!("{STEAMID}%7C%7C{old_refresh_jwt}");
        let rotated_cookie = format!("{STEAMID}%7C%7C{new_refresh_jwt}");
        let secure_cookie = format!("{STEAMID}%7C%7C{new_access_jwt}");

        let rotated_for_srv = rotated_cookie.clone();
        let secure_for_srv = secure_cookie.clone();
        let server = TestServer::start(move |req| {
            if req.path.starts_with("/jwt/refresh?") {
                assert!(
                    req.header("cookie")
                        .is_some_and(|c| c.contains("steamRefresh_steam="))
                );
                assert!(
                    req.header("user-agent")
                        .is_some_and(|ua| !ua.contains("Irori/"))
                );
                Resp {
                    status: 302,
                    headers: vec![
                        ("Location".into(), "/login/settoken?nonce=abc".into()),
                        (
                            "Set-Cookie".into(),
                            format!(
                                "steamRefresh_steam={rotated_for_srv}; Path=/; HttpOnly; Secure"
                            ),
                        ),
                    ],
                    ..Default::default()
                }
            } else if req.path.starts_with("/login/settoken?") {
                Resp {
                    status: 302,
                    headers: vec![
                        ("Location".into(), "/".into()),
                        (
                            "Set-Cookie".into(),
                            format!("steamLoginSecure={secure_for_srv}; Path=/; HttpOnly; Secure"),
                        ),
                    ],
                    ..Default::default()
                }
            } else {
                Resp::status(200)
            }
        });

        let renewed = client(&server)
            .refresh_web_session(&old_cookie)
            .await
            .unwrap();
        assert_eq!(renewed.access_token, new_access_jwt);
        assert_eq!(renewed.refresh_cookie, Some(rotated_cookie));
        assert!(!format!("{renewed:?}").contains(&new_access_jwt));
    }

    #[tokio::test]
    async fn refresh_web_session_rejects_expired_cookie_or_login_redirect() {
        let old_refresh_jwt = fake_jwt(STEAMID, 2_100_000_000, &["web", "renew"]);
        let old_cookie = format!("{STEAMID}%7C%7C{old_refresh_jwt}");

        let server = TestServer::start(|_| Resp {
            status: 302,
            headers: vec![("Location".into(), "/login/?redir=/".into())],
            ..Default::default()
        });
        let err = client(&server)
            .refresh_web_session(&old_cookie)
            .await
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<SteamError>(),
            Some(&SteamError::SessionRejected)
        );
    }
}
