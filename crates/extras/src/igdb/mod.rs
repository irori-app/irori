//! IGDB, Twitch's games database: descriptions, genres, release dates,
//! companies, covers, artworks and screenshots for games the stores say
//! little about (added games, ROMs, launchers' games).
//!
//! IGDB has no keyless access: the user registers an application of their
//! own with Twitch and enters its client ID and secret ([`Credentials`],
//! kept by services through `irori_secrets`, never logged). The client
//! credentials grant (`id.twitch.tv/oauth2/token`) gives an app token, held
//! in memory only. Requests are Apicalypse queries `POST`ed to
//! `api.igdb.com/v4/games`, spaced to IGDB's limit of four a second
//! ([`RateLimiter`]); a 401 asks for a new token once.
//!
//! A library game's record is found by title (an exact folded match, the
//! release year deciding between several) or is the one the user chose,
//! and is cached on disk under `<cache>/igdb` ([`game_for`]).

mod model;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::Runner;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

pub use model::{IgdbGame, ImageSize, SOURCE_NAME, best_match, fill_extra, image_url};

use crate::util;

/// Where the user registers an application and finds its client ID and
/// secret (IGDB's own account guide).
pub const ACCOUNT_GUIDE: &str = "https://api-docs.igdb.com/#account-creation";
/// The per-game choice (`iroriIgdbId`) that says IGDB doesn't have the game.
pub const IGDB_NONE: &str = "none";
/// IGDB answers at most four requests a second per client.
pub const REQUEST_SPACING: Duration = Duration::from_millis(250);
/// Lifetime of a cached record.
pub const FOUND_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Lifetime of a cached "IGDB has no such game".
pub const MISSING_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const TIMEOUT: Duration = Duration::from_secs(20);

/// Server addresses (overridable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// The OAuth token endpoint.
    pub token: String,
    /// The API base, without a trailing slash.
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            token: "https://id.twitch.tv/oauth2/token".into(),
            api: "https://api.igdb.com/v4".into(),
        }
    }
}

/// A Twitch application's client ID and secret. `Debug` leaves the secret
/// out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Trimmed, both present.
    pub fn new(client_id: &str, client_secret: &str) -> Option<Self> {
        let (id, secret) = (client_id.trim(), client_secret.trim());
        (!id.is_empty() && !secret.is_empty()).then(|| Self {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
        })
    }
}

/// What IGDB or Twitch refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IgdbError {
    #[error("Twitch refused the IGDB client ID or secret; check them in Settings")]
    BadCredentials,
    #[error("IGDB is busy; try again in a moment")]
    TooManyRequests,
    #[error("IGDB answered with HTTP status {0}")]
    Status(u16),
}

/// Spaces requests at least `spacing` apart, in the order they ask.
#[derive(Debug)]
pub struct RateLimiter {
    spacing: Duration,
    next: tokio::sync::Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(spacing: Duration) -> Self {
        Self {
            spacing,
            next: tokio::sync::Mutex::new(None),
        }
    }

    /// Wait for this request's turn. Holding the lock while waiting keeps
    /// later callers behind.
    pub async fn wait(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        let start = next.map_or(now, |n| n.max(now));
        tokio::time::sleep_until(start).await;
        *next = Some(start + self.spacing);
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

struct AppToken {
    value: String,
    expires: Instant,
}

/// An IGDB client for one Twitch application. Holds its app token in
/// memory; share one per application so the rate limit holds.
pub struct Client {
    endpoints: Endpoints,
    credentials: Credentials,
    token: tokio::sync::Mutex<Option<AppToken>>,
    limiter: RateLimiter,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(credentials: Credentials) -> Self {
        Self::with_endpoints(credentials, Endpoints::default(), REQUEST_SPACING)
    }

    /// A client for other servers (a test server).
    pub fn with_endpoints(
        credentials: Credentials,
        endpoints: Endpoints,
        spacing: Duration,
    ) -> Self {
        Self {
            endpoints,
            credentials,
            token: tokio::sync::Mutex::new(None),
            limiter: RateLimiter::new(spacing),
        }
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// Ask Twitch for an app token: proves the credentials.
    pub async fn verify(&self) -> Result<()> {
        self.app_token(true).await.map(|_| ())
    }

    async fn app_token(&self, renew: bool) -> Result<String> {
        let mut held = self.token.lock().await;
        if !renew && let Some(t) = held.as_ref().filter(|t| t.expires > Instant::now()) {
            return Ok(t.value.clone());
        }
        let resp = irori_core::http::client()
            .post(&self.endpoints.token)
            .form(&[
                ("client_id", self.credentials.client_id.as_str()),
                ("client_secret", self.credentials.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach Twitch to sign in to IGDB")?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {}
            400 | 401 | 403 => return Err(IgdbError::BadCredentials.into()),
            429 => return Err(IgdbError::TooManyRequests.into()),
            s => return Err(IgdbError::Status(s).into()),
        }
        let answer: TokenAnswer = resp
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Twitch sent an unexpected answer to IGDB's sign-in")?;
        // A minute's margin, so a token never runs out mid-request.
        let life = Duration::from_secs(answer.expires_in.max(120) - 60);
        *held = Some(AppToken {
            value: answer.access_token.clone(),
            expires: Instant::now() + life,
        });
        Ok(answer.access_token)
    }

    async fn post(&self, endpoint: &str, body: &str, token: &str) -> Result<reqwest::Response> {
        self.limiter.wait().await;
        irori_core::http::client()
            .post(format!("{}/{endpoint}", self.endpoints.api))
            .header("Client-ID", &self.credentials.client_id)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .body(body.to_string())
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach IGDB")
    }

    /// Run an Apicalypse query against `endpoint`, with a new token once
    /// when IGDB says the old one ran out.
    async fn query<T>(
        &self,
        endpoint: &str,
        body: &str,
        parse: fn(&[u8]) -> serde_json::Result<T>,
    ) -> Result<T> {
        let mut resp = self
            .post(endpoint, body, &self.app_token(false).await?)
            .await?;
        if resp.status().as_u16() == 401 {
            resp = self
                .post(endpoint, body, &self.app_token(true).await?)
                .await?;
        }
        let status = resp.status().as_u16();
        tracing::debug!(status, endpoint, "igdb call");
        match status {
            200..=299 => {}
            401 | 403 => return Err(IgdbError::BadCredentials.into()),
            429 => return Err(IgdbError::TooManyRequests.into()),
            s => return Err(IgdbError::Status(s).into()),
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("IGDB's answer was cut off")?;
        parse(&bytes).context("IGDB sent an unexpected answer")
    }

    /// Games named like `title`, IGDB's best matches first.
    pub async fn search(&self, title: &str) -> Result<Vec<IgdbGame>> {
        if model::escape(title).is_empty() {
            return Ok(Vec::new());
        }
        self.query("games", &model::search_query(title), model::parse_games)
            .await
    }

    /// Game `id`, if IGDB has it.
    pub async fn game(&self, id: u64) -> Result<Option<IgdbGame>> {
        let games = self
            .query("games", &model::game_query(id), model::parse_games)
            .await?;
        Ok(games.into_iter().find(|g| g.id == id))
    }
}

/// Which library game to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub runner: Runner,
    pub app_name: String,
    pub title: String,
    pub release_year: Option<u32>,
    /// The user's choice (`iroriIgdbId`): an IGDB id or [`IGDB_NONE`];
    /// `None` finds the game by title.
    pub chosen: Option<String>,
}

/// Where records are cached.
pub fn cache_dir() -> PathBuf {
    util::cache_dir("igdb")
}

fn cache_file(dir: &Path, lookup: &Lookup) -> PathBuf {
    // The choice is part of the name, so changing it reads the game again.
    let choice = lookup
        .chosen
        .as_deref()
        .map(|c| format!("-id{}", irori_core::paths::sanitize(c)))
        .unwrap_or_default();
    dir.join(format!(
        "{}-{}{choice}.json",
        lookup.runner.id(),
        irori_core::paths::sanitize(&lookup.app_name)
    ))
}

/// The cached answer for `lookup`, without asking IGDB.
pub async fn cached(dir: &Path, lookup: &Lookup) -> Option<Option<IgdbGame>> {
    let (found, age) = util::cache_read_entry::<Option<IgdbGame>>(&cache_file(dir, lookup)).await?;
    let ttl = if found.is_some() {
        FOUND_TTL
    } else {
        MISSING_TTL
    };
    (age <= ttl).then_some(found)
}

/// The IGDB record of a library game: the cached one, else the user's
/// choice or the search result that is the game. `Ok(None)` when IGDB
/// has no such game (or the user said so).
pub async fn game_for(client: &Client, dir: &Path, lookup: &Lookup) -> Result<Option<IgdbGame>> {
    if lookup.chosen.as_deref() == Some(IGDB_NONE) {
        return Ok(None);
    }
    if let Some(found) = cached(dir, lookup).await {
        return Ok(found);
    }
    let found = match lookup.chosen.as_deref().map(str::parse::<u64>) {
        Some(Ok(id)) => client.game(id).await?,
        Some(Err(_)) => None,
        None => {
            let results = client.search(&lookup.title).await?;
            best_match(&results, &lookup.title, lookup.release_year).cloned()
        }
    };
    if let Err(e) = util::cache_write(&cache_file(dir, lookup), &found).await {
        tracing::warn!("caching IGDB's answer: {e:#}");
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use irori_depot::test_server::{Req, Resp, TestServer};

    use super::*;

    fn creds() -> Credentials {
        Credentials::new(" id-1 ", "secret-1").unwrap()
    }

    fn form(req: &Req) -> Vec<(String, String)> {
        url::form_urlencoded::parse(&req.body)
            .into_owned()
            .collect()
    }

    /// Twitch's token endpoint and IGDB's games endpoint. The first token
    /// is refused by IGDB once, as an expired one would be.
    fn server(tokens: Arc<AtomicUsize>) -> TestServer {
        TestServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/token") => {
                let f = form(req);
                let has = |k: &str, v: &str| f.iter().any(|(a, b)| a == k && b == v);
                if !(has("client_id", "id-1")
                    && has("client_secret", "secret-1")
                    && has("grant_type", "client_credentials"))
                {
                    return Resp::status(400);
                }
                let n = tokens.fetch_add(1, Ordering::SeqCst) + 1;
                Resp::json(serde_json::json!({
                    "access_token": format!("token-{n}"), "expires_in": 5_000_000, "token_type": "bearer"
                }))
            }
            ("POST", "/v4/games") => {
                assert_eq!(req.header("Client-ID"), Some("id-1"));
                if req.header("Authorization") == Some("Bearer token-1") {
                    return Resp::status(401);
                }
                assert_eq!(req.header("Authorization"), Some("Bearer token-2"));
                let body = String::from_utf8_lossy(&req.body).to_string();
                if body.starts_with("search \"Hades\"") {
                    Resp::json(serde_json::json!([
                        {"id": 2, "name": "Hades II", "first_release_date": 1715212800},
                        {"id": 1, "name": "Hades", "summary": "Defy the god of the dead.",
                         "first_release_date": 1600300800, "cover": {"id": 1, "image_id": "co1"}}
                    ]))
                } else if body.contains("where id = 2;") {
                    Resp::json(serde_json::json!([{"id": 2, "name": "Hades II"}]))
                } else {
                    Resp::json(serde_json::json!([]))
                }
            }
            _ => Resp::status(404),
        })
    }

    fn client(s: &TestServer) -> Client {
        Client::with_endpoints(
            creds(),
            Endpoints {
                token: s.url("/token"),
                api: s.url("/v4"),
            },
            Duration::from_millis(1),
        )
    }

    #[tokio::test]
    async fn finds_caches_and_honours_the_users_choice() {
        let tokens = Arc::new(AtomicUsize::new(0));
        let s = server(tokens.clone());
        let c = client(&s);
        let dir = tempfile::tempdir().unwrap();
        let mut lookup = Lookup {
            runner: Runner::Sideload,
            app_name: "abc".into(),
            title: "Hades".into(),
            release_year: None,
            chosen: None,
        };
        let found = game_for(&c, dir.path(), &lookup).await.unwrap().unwrap();
        assert_eq!(found.id, 1);
        assert_eq!(found.summary.as_deref(), Some("Defy the god of the dead."));
        // The refused token was replaced once.
        assert_eq!(tokens.load(Ordering::SeqCst), 2);
        let asked = s.requests().len();
        // Cached: IGDB isn't asked again.
        assert_eq!(
            game_for(&c, dir.path(), &lookup).await.unwrap(),
            Some(found)
        );
        assert_eq!(s.requests().len(), asked);

        lookup.chosen = Some("2".into());
        let chosen = game_for(&c, dir.path(), &lookup).await.unwrap().unwrap();
        assert_eq!(chosen.name, "Hades II");
        lookup.chosen = Some(IGDB_NONE.into());
        assert_eq!(game_for(&c, dir.path(), &lookup).await.unwrap(), None);

        lookup.chosen = None;
        lookup.app_name = "other".into();
        lookup.title = "Nothing Like It".into();
        assert_eq!(game_for(&c, dir.path(), &lookup).await.unwrap(), None);
        assert_eq!(cached(dir.path(), &lookup).await, Some(None));
    }

    #[tokio::test]
    async fn refused_credentials_say_so_without_the_secret() {
        let s = server(Arc::new(AtomicUsize::new(0)));
        let c = Client::with_endpoints(
            Credentials::new("id-1", "wrong").unwrap(),
            Endpoints {
                token: s.url("/token"),
                api: s.url("/v4"),
            },
            Duration::from_millis(1),
        );
        let e = c.verify().await.unwrap_err();
        assert_eq!(
            e.downcast_ref::<IgdbError>(),
            Some(&IgdbError::BadCredentials)
        );
        assert!(!format!("{e:#}").contains("wrong"));
        assert!(!format!("{c:?}").contains("wrong"));
        assert!(Credentials::new("id", " ").is_none());
    }

    #[tokio::test]
    async fn spaces_requests_to_the_limit() {
        let limiter = RateLimiter::new(Duration::from_millis(40));
        let start = Instant::now();
        for _ in 0..4 {
            limiter.wait().await;
        }
        // The first goes at once; each later one waits its turn.
        assert!(start.elapsed() >= Duration::from_millis(120));
    }
}
