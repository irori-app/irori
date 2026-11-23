//! The Steam account connected to Irori, which is optional: installed games
//! need none. Connecting adds owned games, playtime, friends, and achievements
//! using either:
//!
//! * a browser web session obtained from `store.steampowered.com/login/`
//!   (supports private profiles and Steam Family), or
//! * a personal Steam Web API key (no login session, never expires, requires a
//!   public Game Details privacy setting, and does not include Steam Family).
//!
//! Credentials are kept in `session.json` in Steam's config folder (through
//! `irori_secrets`); the games in `library.json` in its cache, so the library
//! shows offline and before the first refresh.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::{Runner, SessionExpired, json_store};
use irori_steam_api::{Client, OwnedGame, SharedApp, SteamAuth, SteamError, TokenClaims};
use serde::{Deserialize, Serialize};

/// Renew the access token when it has less than this left.
const RENEW_MARGIN: Duration = Duration::from_secs(10 * 60);

/// Apps that are part of Steam rather than games.
pub(crate) const NOT_GAMES: [u32; 3] = [
    221410,  // Steam for Linux
    228980,  // Steamworks Common Redistributables
    1070560, // Steam Linux Runtime
];

/// How a connected Steam account authenticates with Steam's Web API.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum SessionAuth {
    /// Web session cookies captured from Steam's official web sign-in page.
    WebSession {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(default, rename = "refreshCookie")]
        refresh_cookie: Option<String>,
    },
    /// Personal Steam Web API key from `steamcommunity.com/dev/apikey`.
    ApiKey {
        #[serde(rename = "apiKey")]
        api_key: String,
    },
    /// Saved by an older Irori build that used `IAuthenticationService`
    /// tokens. Kept only so `init()` still shows the cached library until the
    /// next refresh asks the user to reconnect cleanly.
    Legacy,
}

impl SessionAuth {
    fn legacy() -> Self {
        Self::Legacy
    }
}

impl fmt::Debug for SessionAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSession { refresh_cookie, .. } => f
                .debug_struct("WebSession")
                .field("has_refresh_cookie", &refresh_cookie.is_some())
                .finish_non_exhaustive(),
            Self::ApiKey { .. } => f.debug_struct("ApiKey").finish_non_exhaustive(),
            Self::Legacy => f.write_str("Legacy"),
        }
    }
}

/// A connected Steam account.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Session {
    #[serde(default)]
    pub account_name: String,
    /// SteamID64, as text so JSON readers keep every digit.
    pub steamid: String,
    #[serde(default = "SessionAuth::legacy")]
    pub auth: SessionAuth,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("account_name", &self.account_name)
            .field("steamid", &self.steamid)
            .field("auth", &self.auth)
            .finish()
    }
}

impl Session {
    pub fn steamid(&self) -> Result<u64> {
        self.steamid
            .parse()
            .context("The saved Steam account is damaged. Sign in again.")
    }

    pub fn auth(&self) -> Option<SteamAuth<'_>> {
        match &self.auth {
            SessionAuth::WebSession { access_token, .. } => {
                Some(SteamAuth::WebSession(access_token.as_str()))
            }
            SessionAuth::ApiKey { api_key } => Some(SteamAuth::ApiKey(api_key.as_str())),
            SessionAuth::Legacy => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebViewPayload {
    steamid: String,
    access_token: String,
    #[serde(default)]
    refresh_cookie: Option<String>,
}

/// Convert the JSON emitted by `irori-sign-in --steam-account` into the input
/// accepted by [`irori_core::Store::login`].
pub fn web_login_input(webview_payload_json: &str) -> Result<String> {
    let payload: WebViewPayload = serde_json::from_str(webview_payload_json.trim())
        .context("Steam didn't finish signing in. Try again.")?;
    let steamid: u64 = payload
        .steamid
        .trim()
        .parse()
        .ok()
        .filter(|&id| id > 0)
        .context("Steam didn't return a valid account ID. Try again.")?;
    let access_token = payload.access_token.trim().to_string();
    anyhow::ensure!(
        !access_token.is_empty(),
        "Steam didn't finish signing in. Try again."
    );
    let refresh_cookie = payload
        .refresh_cookie
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    let session = Session {
        account_name: String::new(),
        steamid: steamid.to_string(),
        auth: SessionAuth::WebSession {
            access_token,
            refresh_cookie,
        },
    };
    Ok(serde_json::to_string(&session).unwrap_or_default())
}

/// Build the input accepted by [`irori_core::Store::login`] for a personal
/// Steam Web API key connection.
pub fn api_key_login_input(steamid: u64, api_key: &str) -> String {
    let session = Session {
        account_name: String::new(),
        steamid: steamid.to_string(),
        auth: SessionAuth::ApiKey {
            api_key: api_key.trim().to_string(),
        },
    };
    serde_json::to_string(&session).unwrap_or_default()
}

/// Parse what [`web_login_input`] or [`api_key_login_input`] produced.
pub(crate) fn parse_login_input(input: &str) -> Result<Session> {
    let session: Session = serde_json::from_str(input.trim())
        .context("Connect to Steam using the Steam dialog in Irori")?;
    anyhow::ensure!(
        session.steamid().is_ok(),
        "Steam didn't complete the sign-in. Try again."
    );
    match &session.auth {
        SessionAuth::WebSession { access_token, .. } => {
            anyhow::ensure!(
                !access_token.trim().is_empty(),
                "Steam didn't complete the sign-in. Try again."
            );
        }
        SessionAuth::ApiKey { api_key } => {
            anyhow::ensure!(
                !api_key.trim().is_empty(),
                "Enter your Steam Web API key and try again."
            );
        }
        SessionAuth::Legacy => {
            anyhow::bail!("Connect to Steam using the Steam dialog in Irori");
        }
    }
    Ok(session)
}

/// One game of the account's library, as cached.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LibraryGame {
    pub app_id: String,
    pub name: String,
    pub playtime_minutes: u64,
    /// Unix time.
    pub last_played: u64,
    /// The family member sharing it (SteamID64), for games the account
    /// doesn't own itself.
    pub shared_by: Option<String>,
    /// The user hid it in Steam, as the Steam client said
    /// ([`crate::client_library`]); the Web API doesn't say.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Store-page art of one app, from Steam's store (newer apps keep their art
/// under addresses only the store knows).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct RemoteArt {
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub logo: Option<String>,
    pub header: Option<String>,
}

/// `library.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LibraryCache {
    /// Whose library this is.
    pub steamid: String,
    pub games: Vec<LibraryGame>,
    /// Art by app ID, for apps Steam has no art cached for. Also kept for
    /// installed games without an account.
    pub art: HashMap<String, RemoteArt>,
}

/// Where the account's files live.
#[derive(Debug, Clone)]
pub(crate) struct AccountFiles {
    pub session: PathBuf,
    pub library: PathBuf,
}

impl AccountFiles {
    pub fn new(config: &Path, cache: &Path) -> Self {
        Self {
            session: config.join("session.json"),
            library: cache.join("library.json"),
        }
    }

    pub fn read_session(&self) -> Option<Session> {
        irori_secrets::load(&self.session)
    }

    pub fn write_session(&self, session: &Session) -> Result<()> {
        irori_secrets::write_json(&self.session, session)
    }

    pub fn remove_session(&self) -> Result<()> {
        match std::fs::remove_file(&self.session) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).context("Couldn't remove the saved Steam sign-in")
            }
            _ => Ok(()),
        }
    }

    pub fn read_library(&self) -> LibraryCache {
        json_store::read_or_default(&self.library)
    }

    pub fn write_library(&self, cache: &LibraryCache) -> Result<()> {
        json_store::write(&self.library, cache)
    }
}

fn expired_error(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(SessionExpired::new(Runner::Steam, message.into()))
}

/// Ensure the session has a usable credential.
///
/// * `ApiKey`: never expires by time, so returns `Ok(None)`.
/// * `WebSession`: renews the access token via Steam's browser refresh
///   redirect chain when `TokenClaims::is_expiring` is true.
/// * `Legacy`: asks the user to sign in again cleanly via [`SessionExpired`].
pub(crate) async fn fresh(client: &Client, session: &Session) -> Result<Option<Session>> {
    match &session.auth {
        SessionAuth::ApiKey { .. } => Ok(None),
        SessionAuth::Legacy => Err(expired_error(
            "Steam sign-in changed. Sign in to Steam again.",
        )),
        SessionAuth::WebSession { access_token, .. } => {
            let now = irori_core::util::unix_now();
            let valid = TokenClaims::parse(access_token)
                .is_ok_and(|c| !c.is_expiring(now, RENEW_MARGIN.as_secs()));
            if valid {
                return Ok(None);
            }
            renew(client, session).await.map(Some)
        }
    }
}

/// Renew a web session using its `steamRefresh_steam` cookie. When there is no
/// refresh cookie or Steam refuses it, returns [`SessionExpired`].
pub(crate) async fn renew(client: &Client, session: &Session) -> Result<Session> {
    let SessionAuth::WebSession { refresh_cookie, .. } = &session.auth else {
        return Err(expired_error(
            "Steam no longer accepts the saved sign-in. Sign in to Steam again.",
        ));
    };
    let Some(refresh_cookie) = refresh_cookie.as_deref() else {
        return Err(expired_error(
            "Your Steam web session expired. Sign in to Steam again.",
        ));
    };
    match client.refresh_web_session(refresh_cookie).await {
        Ok(renewed) => Ok(Session {
            auth: SessionAuth::WebSession {
                access_token: renewed.access_token,
                refresh_cookie: Some(
                    renewed
                        .refresh_cookie
                        .unwrap_or_else(|| refresh_cookie.to_string()),
                ),
            },
            ..session.clone()
        }),
        Err(e) => Err(map_rejection(e)),
    }
}

/// Steam refusing the saved sign-in becomes [`SessionExpired`], which the
/// app turns into one "sign in again" prompt.
pub(crate) fn map_rejection(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<SteamError>() {
        Some(SteamError::SessionRejected) => {
            anyhow::Error::new(SessionExpired::new(Runner::Steam, format!("{e:#}")))
        }
        _ => e,
    }
}

/// [`fetch_library`] with recovery when a web access token is refused early:
///
/// * `WebSession`: if the Web API returns 401/403 (even when JWT claims still
///   look valid, issue #165), renews the session once with
///   `refresh_web_session` and retries. Only a refused refresh ends the
///   session.
/// * `ApiKey`: a 401/403 means the API key is invalid or revoked, so maps
///   directly to [`SessionExpired`].
pub(crate) async fn fetch_library_renewing(
    client: &Client,
    session: &Session,
) -> Result<(Vec<LibraryGame>, Option<Session>)> {
    match fetch_library(client, session).await {
        Err(e) if is_refused(&e) => match &session.auth {
            SessionAuth::WebSession { .. } => {
                tracing::info!("Steam refused the web access token; renewing it");
                let renewed = renew(client, session).await?;
                let games = fetch_library(client, &renewed).await?;
                Ok((games, Some(renewed)))
            }
            SessionAuth::ApiKey { .. } => Err(expired_error(
                "Steam rejected your Web API key. Check the key or reconnect.",
            )),
            SessionAuth::Legacy => Err(expired_error(
                "Steam sign-in changed. Sign in to Steam again.",
            )),
        },
        other => other.map(|games| (games, None)),
    }
}

fn is_refused(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<SteamError>(),
        Some(SteamError::SessionRejected)
    )
}

/// Fetch the games the account owns and, for web sessions, those its Steam
/// Family shares.
pub(crate) async fn fetch_library(client: &Client, session: &Session) -> Result<Vec<LibraryGame>> {
    let steamid = session.steamid()?;
    let auth = session
        .auth()
        .ok_or_else(|| expired_error("Steam sign-in changed. Sign in to Steam again."))?;
    let owned = client
        .owned_games(auth, steamid)
        .await
        .context("Couldn't read your Steam library")?;
    // Steam Family APIs require an OAuth/web access token; API keys cannot
    // read family groups.
    let shared = match auth {
        SteamAuth::WebSession(token) => match client.family_group(token, steamid).await {
            Ok(Some(group)) => client
                .family_shared_apps(token, group, steamid)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("reading the Steam Family library: {e:#}");
                    Vec::new()
                }),
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!("reading the Steam Family: {e:#}");
                Vec::new()
            }
        },
        SteamAuth::ApiKey(_) => Vec::new(),
    };
    Ok(merge(owned, shared, steamid))
}

/// The account's own games, then the family's games it doesn't own, without
/// Steam's own apps.
fn merge(owned: Vec<OwnedGame>, shared: Vec<SharedApp>, steamid: u64) -> Vec<LibraryGame> {
    let me = steamid.to_string();
    let mut games: Vec<LibraryGame> = owned
        .into_iter()
        .filter(|g| !NOT_GAMES.contains(&g.appid) && !g.name.trim().is_empty())
        .map(|g| LibraryGame {
            app_id: g.appid.to_string(),
            name: g.name,
            playtime_minutes: g.playtime_forever,
            last_played: g.rtime_last_played,
            shared_by: None,
            hidden: false,
        })
        .collect();
    let known: std::collections::HashSet<String> = games.iter().map(|g| g.app_id.clone()).collect();
    games.extend(
        shared
            .into_iter()
            .filter(|a| !NOT_GAMES.contains(&a.appid) && !a.name.trim().is_empty())
            .filter(|a| !known.contains(&a.appid.to_string()))
            .filter(|a| !a.owner_steamids.contains(&me))
            .map(|a| LibraryGame {
                app_id: a.appid.to_string(),
                name: a.name,
                // The family response's time fields are the owner's or
                // timestamps; the account's own playtime comes from Steam's
                // local files.
                playtime_minutes: 0,
                last_played: 0,
                shared_by: a.owner_steamids.into_iter().next(),
                hidden: false,
            }),
    );
    games.sort_by(|a, b| a.app_id.cmp(&b.app_id));
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(appid: u32, name: &str, minutes: u64) -> OwnedGame {
        serde_json::from_value(serde_json::json!({
            "appid": appid, "name": name, "playtime_forever": minutes
        }))
        .unwrap()
    }

    fn shared(appid: u32, name: &str, owners: &[&str]) -> SharedApp {
        serde_json::from_value(serde_json::json!({
            "appid": appid, "name": name, "owner_steamids": owners, "rt_playtime": 7200
        }))
        .unwrap()
    }

    #[test]
    fn merges_owned_and_family_games() {
        let games = merge(
            vec![owned(620, "Portal 2", 95), owned(228980, "Redist", 0)],
            vec![
                shared(620, "Portal 2", &["2"]),
                shared(70, "Half-Life", &["2"]),
                shared(400, "Portal", &["1"]),
            ],
            1,
        );
        let ids: Vec<&str> = games.iter().map(|g| g.app_id.as_str()).collect();
        assert_eq!(ids, ["620", "70"]);
        assert_eq!(games[0].playtime_minutes, 95);
        assert_eq!(games[0].shared_by, None);
        assert_eq!(games[1].shared_by.as_deref(), Some("2"));
        assert_eq!(games[1].playtime_minutes, 0);
    }

    #[test]
    fn web_and_api_key_inputs_round_trip_without_leaking_in_debug() {
        let web_raw = r#"{"steamid":"76561197960265729","accessToken":"access-secret","refreshCookie":"76561197960265729%7C%7Crefresh-secret"}"#;
        let web_input = web_login_input(web_raw).unwrap();
        let web_session = parse_login_input(&web_input).unwrap();
        assert_eq!(web_session.steamid().unwrap(), 76561197960265729);
        assert_eq!(
            web_session.auth(),
            Some(SteamAuth::WebSession("access-secret"))
        );
        let web_debug = format!("{web_session:?}");
        assert!(!web_debug.contains("secret"), "{web_debug}");

        let key_input = api_key_login_input(76561197960265729, "0123456789ABCDEF0123456789ABCDEF");
        let key_session = parse_login_input(&key_input).unwrap();
        assert_eq!(key_session.steamid().unwrap(), 76561197960265729);
        assert_eq!(
            key_session.auth(),
            Some(SteamAuth::ApiKey("0123456789ABCDEF0123456789ABCDEF"))
        );
        let key_debug = format!("{key_session:?}");
        assert!(!key_debug.contains("0123456789"), "{key_debug}");

        assert!(parse_login_input("some pasted code").is_err());
    }

    #[test]
    fn legacy_session_loads_as_legacy_and_cannot_be_used_as_login_input() {
        let dir = tempfile::tempdir().unwrap();
        let files = AccountFiles::new(dir.path(), dir.path());
        let legacy_json = r#"{"accountName":"olduser","steamid":"76561197960265729","refreshToken":"r","accessToken":"x"}"#;
        std::fs::write(&files.session, legacy_json).unwrap();
        let loaded = files.read_session().unwrap();
        assert_eq!(loaded.account_name, "olduser");
        assert!(loaded.auth().is_none());
        assert!(parse_login_input(legacy_json).is_err());
    }

    #[test]
    fn session_file_is_private_and_removable() {
        let dir = tempfile::tempdir().unwrap();
        let files = AccountFiles::new(dir.path(), dir.path());
        assert!(files.read_session().is_none());
        let input = api_key_login_input(1, "0123456789abcdef0123456789abcdef");
        let mut session = parse_login_input(&input).unwrap();
        session.account_name = "a".into();
        files.write_session(&session).unwrap();
        assert_eq!(files.read_session().unwrap().account_name, "a");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&files.session)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        files.remove_session().unwrap();
        files.remove_session().unwrap();
        assert!(files.read_session().is_none());
    }
}
