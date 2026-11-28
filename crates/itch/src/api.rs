//! The itch.io API client (`https://api.itch.io`, version 2), authenticated
//! with the user's API key in the `Authorization` header.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::model::{
    CreateGameSessionRequest, GameSessionSummary, OwnedKey, Upload, User, lenient_summary,
};

/// itch.io's API server.
pub const API_BASE: &str = "https://api.itch.io";

/// Maximum JSON response body read from the itch.io API (8 MiB).
const MAX_API_BYTES: usize = 8 * 1024 * 1024;

/// Owned keys asked for per page (the API allows up to 500).
const PER_PAGE: usize = 100;

/// A bound on owned-key pages (500,000 keys), against a server that keeps
/// answering.
const MAX_PAGES: usize = 5_000;

/// itch.io refused a request.
///
/// `unauthorized` is set when the API key itself was rejected (revoked or
/// mistyped), which only a new key fixes. `scope_denied` marks a key that is
/// valid for the account but lacks permission for a specific endpoint (such as
/// game sessions), so callers can degrade gracefully without logging the user
/// out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("itch.io answered {status}: {message}")]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub unauthorized: bool,
    pub scope_denied: bool,
    pub rate_limited: bool,
}

impl ApiError {
    /// Whether the server rejected the payload permanently (not auth, scope,
    /// timeout, or rate-limit).
    pub fn is_permanent_client_error(&self) -> bool {
        (400..500).contains(&self.status)
            && !matches!(self.status, 401 | 403 | 408 | 429)
            && !self.unauthorized
            && !self.scope_denied
            && !self.rate_limited
    }
}

/// An authenticated API client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// For download links, which answer with a redirect to the file.
    no_redirect: reqwest::Client,
    base: String,
    key: String,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("base", &self.base).finish()
    }
}

impl Client {
    pub fn new(key: &str) -> Result<Self> {
        Self::with_base(API_BASE, key)
    }

    /// A client for another server (a test server).
    pub fn with_base(base: &str, key: &str) -> Result<Self> {
        let build = |redirects: reqwest::redirect::Policy| {
            reqwest::Client::builder()
                .user_agent(concat!("Irori/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .redirect(redirects)
                .build()
        };
        Ok(Self {
            http: build(reqwest::redirect::Policy::default())?,
            no_redirect: build(reqwest::redirect::Policy::none())?,
            base: base.trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
        })
    }

    fn get(&self, client: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
        client
            .get(format!("{}{path}", self.base))
            .header(reqwest::header::AUTHORIZATION, &self.key)
            .header(reqwest::header::ACCEPT, "application/vnd.itch.v2")
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.base))
            .header(reqwest::header::AUTHORIZATION, &self.key)
            .header(reqwest::header::ACCEPT, "application/vnd.itch.v2")
    }

    async fn read_json_value(&self, mut resp: reqwest::Response) -> Result<(u16, Value)> {
        let status = resp.status().as_u16();
        let mut buf = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)
            .context("itch.io's answer was cut off")?
        {
            if buf.len().saturating_add(chunk.len()) > MAX_API_BYTES {
                bail!("itch.io sent a response larger than {MAX_API_BYTES} bytes");
            }
            buf.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
        Ok((status, value))
    }

    async fn json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .get(&self.http, path)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach itch.io")?;
        let (status, value) = self.read_json_value(resp).await?;
        if let Some(e) = api_error(status, &value) {
            return Err(e.into());
        }
        serde_json::from_value(value)
            .with_context(|| format!("unexpected answer from itch.io for {path}"))
    }

    async fn post_form_json<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(&str, String)],
    ) -> Result<T> {
        let resp = self
            .post(path)
            .form(form)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach itch.io")?;
        let (status, value) = self.read_json_value(resp).await?;
        if let Some(e) = api_error(status, &value) {
            return Err(e.into());
        }
        serde_json::from_value(value)
            .with_context(|| format!("unexpected answer from itch.io for {path}"))
    }

    /// The account the key belongs to.
    pub async fn profile(&self) -> Result<User> {
        #[derive(serde::Deserialize)]
        struct Answer {
            user: User,
        }
        Ok(self.json::<Answer>("/profile").await?.user)
    }

    /// Every download key the account owns, page by page.
    ///
    /// Pages end at an empty page or a short one, measured against the
    /// page size the answer names (the server may hold fewer keys a page
    /// than asked for), else the size asked for. A page that adds no new
    /// key ends them too, so a server that ignores `page` can't loop
    /// forever.
    pub async fn owned_keys(&self) -> Result<Vec<OwnedKey>> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Page {
            #[serde(deserialize_with = "crate::model::lenient_vec")]
            owned_keys: Vec<OwnedKey>,
            per_page: Option<u64>,
        }
        let mut all: Vec<OwnedKey> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for page in 1..=MAX_PAGES {
            let got: Page = self
                .json(&format!(
                    "/profile/owned-keys?page={page}&per_page={PER_PAGE}"
                ))
                .await?;
            let n = got.owned_keys.len();
            let before = all.len();
            for key in got.owned_keys {
                if seen.insert(key.id) {
                    all.push(key);
                }
            }
            let page_size = got.per_page.filter(|p| *p > 0).unwrap_or(PER_PAGE as u64);
            let short = (n as u64) < page_size;
            if n == 0 || short || all.len() == before {
                break;
            }
        }
        Ok(all)
    }

    /// The uploads of a game the key (`download_key_id`) gives access to.
    pub async fn uploads(&self, game_id: u64, key_id: u64) -> Result<Vec<Upload>> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            #[serde(deserialize_with = "crate::model::lenient_vec")]
            uploads: Vec<Upload>,
        }
        Ok(self
            .json::<Answer>(&format!(
                "/games/{game_id}/uploads?download_key_id={key_id}"
            ))
            .await?
            .uploads)
    }

    /// Where to download an upload from right now: itch.io answers with a
    /// redirect to a short-lived address on its file servers.
    pub async fn download_location(&self, upload_id: u64, key_id: u64) -> Result<String> {
        let path = format!("/uploads/{upload_id}/download?download_key_id={key_id}");
        let resp = self
            .get(&self.no_redirect, &path)
            .send()
            .await
            .context("Could not reach itch.io")?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .context("itch.io sent a download redirect without an address")?;
            return Ok(match reqwest::Url::parse(location) {
                Ok(url) => url.to_string(),
                Err(_) => format!("{}/{}", self.base, location.trim_start_matches('/')),
            });
        }
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        if let Some(e) = api_error(status.as_u16(), &value) {
            return Err(e.into());
        }
        value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("itch.io did not say where to download the game from")
    }

    /// Record a finished game session on the user's itch.io profile
    /// (`POST /profile/game-sessions`), returning the updated aggregate summary
    /// when itch.io includes one.
    pub async fn create_game_session(
        &self,
        req: &CreateGameSessionRequest,
    ) -> Result<Option<GameSessionSummary>> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            #[serde(deserialize_with = "lenient_summary")]
            summary: Option<GameSessionSummary>,
        }
        let mut form = vec![
            ("game_id", req.game_id.to_string()),
            ("upload_id", req.upload_id.to_string()),
            ("seconds_run", req.seconds_run.to_string()),
            ("last_run_at", req.last_run_at.clone()),
        ];
        if let Some(build_id) = req.build_id.filter(|id| *id > 0) {
            form.push(("build_id", build_id.to_string()));
        }
        if let Some(key_id) = req.download_key_id.filter(|id| *id > 0) {
            form.push(("download_key_id", key_id.to_string()));
        }
        if let Some(platform) = req.platform.as_deref().filter(|s| !s.is_empty()) {
            form.push(("platform", platform.to_string()));
        }
        if let Some(arch) = req.architecture.as_deref().filter(|s| !s.is_empty()) {
            form.push(("architecture", arch.to_string()));
        }
        Ok(self
            .post_form_json::<Answer>("/profile/game-sessions", &form)
            .await?
            .summary)
    }

    /// Fetch the recorded playtime summary for one game
    /// (`GET /profile/game-sessions/summaries/{game_id}`). Returns `Ok(None)`
    /// when no sessions have been recorded yet.
    pub async fn game_sessions_summary(&self, game_id: u64) -> Result<Option<GameSessionSummary>> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            #[serde(deserialize_with = "lenient_summary")]
            summary: Option<GameSessionSummary>,
        }
        let path = format!("/profile/game-sessions/summaries/{game_id}");
        let resp = self
            .get(&self.http, &path)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach itch.io")?;
        let (status, value) = self.read_json_value(resp).await?;
        if status == 404 {
            return Ok(None);
        }
        if let Some(e) = api_error(status, &value) {
            return Err(e.into());
        }
        let answer: Answer = serde_json::from_value(value)
            .with_context(|| format!("unexpected answer from itch.io for {path}"))?;
        Ok(answer.summary)
    }
}

/// The error in an answer: a failed status, or `{"errors": [...]}` (which
/// itch.io also sends with status 200 for some refusals).
fn api_error(status: u16, value: &Value) -> Option<ApiError> {
    let messages: Vec<&str> = value
        .get("errors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if (200..300).contains(&status) && messages.is_empty() {
        return None;
    }
    let message = if messages.is_empty() {
        format!("HTTP {status}")
    } else {
        messages.join(", ")
    };
    let lower = message.to_ascii_lowercase();
    let key_refused = lower.contains("invalid key") || lower.contains("invalid api key");
    let scope_denied = !key_refused
        && (status == 403
            || lower.contains("invalid scope")
            || lower.contains("missing scope")
            || lower.contains("insufficient scope")
            || lower.contains("not allowed"));
    let rate_limited = matches!(status, 429 | 503);
    Some(ApiError {
        status,
        unauthorized: (status == 401 || key_refused) && !scope_denied,
        scope_denied,
        rate_limited,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    #[tokio::test]
    async fn sends_the_key_and_pages_through_owned_keys() {
        let server = TestServer::start(|req| {
            if req.header("authorization") != Some("KEY") {
                return Resp::json(json!({"errors": ["invalid key"]}));
            }
            let path = req.path.as_str();
            if path == "/profile" {
                Resp::json(json!({"user": {"id": 9, "username": "u", "display_name": "U"}}))
            } else if path.starts_with("/profile/owned-keys?page=1&") {
                let keys: Vec<Value> = (0..PER_PAGE)
                    .map(|i| json!({"id": i, "game_id": 1000 + i}))
                    .collect();
                Resp::json(json!({"owned_keys": keys, "page": 1}))
            } else if path.starts_with("/profile/owned-keys?page=2&") {
                Resp::json(json!({"owned_keys": [{"id": 500, "game_id": 7}], "page": 2}))
            } else if path == "/games/7/uploads?download_key_id=500" {
                Resp::json(json!({"uploads": [{"id": 3, "filename": "a.zip", "type": "default"}]}))
            } else if path == "/uploads/3/download?download_key_id=500" {
                Resp {
                    status: 302,
                    headers: vec![(
                        "Location".into(),
                        "https://files.example/a.zip?sig=1".into(),
                    )],
                    ..Default::default()
                }
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_base(&server.base(), "KEY").unwrap();
        assert_eq!(api.profile().await.unwrap().name(), "U");
        let keys = api.owned_keys().await.unwrap();
        assert_eq!(keys.len(), PER_PAGE + 1);
        assert_eq!(api.uploads(7, 500).await.unwrap()[0].id, 3);
        assert_eq!(
            api.download_location(3, 500).await.unwrap(),
            "https://files.example/a.zip?sig=1"
        );

        let wrong = Client::with_base(&server.base(), "nope").unwrap();
        let e = wrong.profile().await.unwrap_err();
        let api_error = e.downcast_ref::<ApiError>().unwrap();
        assert!(api_error.unauthorized, "{api_error}");
        let missing = api.uploads(8, 1).await.unwrap_err();
        assert!(!missing.downcast_ref::<ApiError>().unwrap().unauthorized);
    }

    /// itch.io (PHP) writes an empty list as `{}`: an account without
    /// games, a game without uploads (issue #159).
    #[tokio::test]
    async fn reads_empty_lists_written_as_objects() {
        let server = TestServer::start(|req| match req.path.as_str() {
            p if p.starts_with("/profile/owned-keys?page=1&") => {
                Resp::json(json!({"owned_keys": {}, "page": 1, "per_page": 50}))
            }
            "/games/7/uploads?download_key_id=500" => Resp::json(json!({"uploads": {}})),
            "/games/8/uploads?download_key_id=501" => Resp::json(json!({"uploads": null})),
            _ => Resp::status(404),
        });
        let api = Client::with_base(&server.base(), "KEY").unwrap();
        assert!(api.owned_keys().await.unwrap().is_empty());
        assert!(api.uploads(7, 500).await.unwrap().is_empty());
        assert!(api.uploads(8, 501).await.unwrap().is_empty());
    }

    /// Lists keyed by position, and pages smaller than asked for: the page
    /// size the server names decides where the keys end.
    #[tokio::test]
    async fn reads_keyed_lists_and_pages_by_the_size_the_server_names() {
        let server = TestServer::start(|req| {
            let path = req.path.as_str();
            if path.starts_with("/profile/owned-keys?page=1&") {
                let keys: serde_json::Map<String, Value> = (0..50u64)
                    .map(|i| (i.to_string(), json!({"id": i, "game_id": 1000 + i})))
                    .collect();
                Resp::json(json!({"owned_keys": keys, "page": 1, "per_page": 50}))
            } else if path.starts_with("/profile/owned-keys?page=2&") {
                Resp::json(json!({
                    "owned_keys": {"1": {"id": 61, "game_id": 2}, "0": {"id": 60, "game_id": 1}},
                    "page": 2,
                    "per_page": 50
                }))
            } else if path == "/games/1/uploads?download_key_id=60" {
                Resp::json(json!({"uploads": {"0": {
                    "id": 3, "filename": "a.zip", "type": "default", "traits": {}
                }}}))
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_base(&server.base(), "KEY").unwrap();
        let keys = api.owned_keys().await.unwrap();
        assert_eq!(keys.len(), 52);
        assert_eq!(keys[50].id, 60);
        assert_eq!(keys[51].id, 61);
        assert_eq!(api.uploads(1, 60).await.unwrap()[0].id, 3);
    }

    /// A server that ignores `page` answers the first page again: the keys
    /// end there instead of being asked for forever.
    #[tokio::test]
    async fn stops_when_a_page_repeats() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/profile/owned-keys?") {
                let keys: Vec<Value> = (0..PER_PAGE)
                    .map(|i| json!({"id": i, "game_id": i}))
                    .collect();
                Resp::json(json!({"owned_keys": keys}))
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_base(&server.base(), "KEY").unwrap();
        assert_eq!(api.owned_keys().await.unwrap().len(), PER_PAGE);
    }

    #[tokio::test]
    async fn posts_game_session_and_reads_summaries() {
        let server = TestServer::start(|req| {
            if req.header("authorization") != Some("KEY") {
                return Resp {
                    status: 401,
                    ..Resp::json(json!({"errors": ["invalid key"]}))
                };
            }
            if req.header("accept") != Some("application/vnd.itch.v2") {
                return Resp::status(406);
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/profile/game-sessions") => {
                    let body = String::from_utf8_lossy(&req.body);
                    assert!(body.contains("game_id=7"), "{body}");
                    assert!(body.contains("upload_id=3"), "{body}");
                    assert!(body.contains("build_id=99"), "{body}");
                    assert!(body.contains("download_key_id=500"), "{body}");
                    assert!(body.contains("seconds_run=420"), "{body}");
                    assert!(body.contains("platform=linux"), "{body}");
                    assert!(body.contains("architecture=amd64"), "{body}");
                    Resp::json(json!({
                        "summary": {
                            "seconds_run": 1680,
                            "last_run_at": "2026-11-27T18:00:00Z"
                        },
                        "user_game_session": {
                            "id": 11,
                            "seconds_run": 420,
                            "last_run_at": "2026-11-27T18:00:00Z"
                        }
                    }))
                }
                ("GET", "/profile/game-sessions/summaries/7") => Resp::json(json!({
                    "summary": {
                        "seconds_run": 1680,
                        "last_run_at": "2026-11-27T18:00:00Z"
                    }
                })),
                ("GET", "/profile/game-sessions/summaries/8") => Resp::json(json!({"summary": {}})),
                ("GET", "/profile/game-sessions/summaries/9") => Resp::json(json!({"summary": []})),
                ("GET", "/profile/game-sessions/summaries/10") => Resp::status(404),
                ("GET", "/profile/game-sessions/summaries/403") => Resp {
                    status: 403,
                    ..Resp::json(json!({"errors": ["invalid scope"]}))
                },
                ("GET", "/profile/game-sessions/summaries/503") => Resp::status(503),
                _ => Resp::status(404),
            }
        });
        let api = Client::with_base(&server.base(), "KEY").unwrap();
        let created = api
            .create_game_session(&CreateGameSessionRequest {
                game_id: 7,
                upload_id: 3,
                build_id: Some(99),
                download_key_id: Some(500),
                seconds_run: 420,
                last_run_at: "2026-11-27T18:00:00Z".into(),
                platform: Some("linux".into()),
                architecture: Some("amd64".into()),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(created.seconds_run, 1680);
        assert_eq!(created.last_run_at.as_deref(), Some("2026-11-27T18:00:00Z"));

        assert_eq!(api.game_sessions_summary(7).await.unwrap(), Some(created));
        assert_eq!(api.game_sessions_summary(8).await.unwrap(), None);
        assert_eq!(api.game_sessions_summary(9).await.unwrap(), None);
        assert_eq!(api.game_sessions_summary(10).await.unwrap(), None);

        let denied = api.game_sessions_summary(403).await.unwrap_err();
        let denied = denied.downcast_ref::<ApiError>().unwrap();
        assert!(denied.scope_denied && !denied.unauthorized);

        let limited = api.game_sessions_summary(503).await.unwrap_err();
        assert!(limited.downcast_ref::<ApiError>().unwrap().rate_limited);
    }
}
