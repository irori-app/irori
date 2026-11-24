//! VNDB, the visual novel database: descriptions, release dates,
//! developers, covers and screenshots for visual novels the stores say
//! little about.
//!
//! VNDB's HTTPS JSON API ("Kana", `api.vndb.org/kana`) answers public
//! data without an account. Queries are `POST`ed to `/vn` as JSON
//! (`filters`, `fields`, `results`). VNDB allows 200 requests in five
//! minutes per address, so requests are spaced 1.5 s apart
//! ([`REQUEST_SPACING`]) and every answer is cached on disk under
//! `<cache>/vndb` ([`novel_for`]): a month for a novel, a week for "not on
//! VNDB".
//!
//! A library game's novel is the one the user chose (`iroriVndbId`) or the
//! search result one of whose names folds like the game's title, the
//! release year deciding between several. Images keep VNDB's own flags:
//! one its users rated sexual at all, or violent beyond mild, is never
//! kept ([`VndbImage`]).

mod model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::Runner;

pub use model::{
    SITE, SOURCE_NAME, VndbImage, VndbNovel, best_match, fill_extra, novel_id, plain_text,
};

use crate::igdb::RateLimiter;
use crate::util;

/// The per-game choice (`iroriVndbId`) that says VNDB doesn't have the game.
pub const VNDB_NONE: &str = "none";
/// 200 requests in five minutes, with room for the user's own searches.
pub const REQUEST_SPACING: Duration = Duration::from_millis(1500);
/// Lifetime of a cached novel.
pub const FOUND_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Lifetime of a cached "VNDB has no such novel".
pub const MISSING_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// VNDB aborts queries that run three seconds; the rest is the network.
const TIMEOUT: Duration = Duration::from_secs(20);
/// VNDB's API.
pub const API: &str = "https://api.vndb.org/kana";

/// What VNDB refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VndbError {
    #[error("VNDB is busy; try again in a few minutes")]
    TooManyRequests,
    #[error("VNDB answered with HTTP status {0}")]
    Status(u16),
}

/// A VNDB client. Share one so requests stay spaced.
#[derive(Debug)]
pub struct Client {
    api: String,
    limiter: RateLimiter,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_api(API, REQUEST_SPACING)
    }

    /// A client for another server (a test server), without a trailing
    /// slash.
    pub fn with_api(api: &str, spacing: Duration) -> Self {
        Self {
            api: api.trim_end_matches('/').to_string(),
            limiter: RateLimiter::new(spacing),
        }
    }

    async fn query(&self, body: &serde_json::Value) -> Result<Vec<VndbNovel>> {
        self.limiter.wait().await;
        let resp = irori_core::http::client()
            .post(format!("{}/vn", self.api))
            .header("Accept", "application/json")
            .json(body)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach VNDB")?;
        let status = resp.status().as_u16();
        tracing::debug!(status, "vndb call");
        match status {
            200..=299 => {}
            429 => return Err(VndbError::TooManyRequests.into()),
            s => return Err(VndbError::Status(s).into()),
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("VNDB's answer was cut off")?;
        model::parse_answer(&bytes).context("VNDB sent an unexpected answer")
    }

    /// Novels named like `title`, VNDB's best matches first.
    pub async fn search(&self, title: &str) -> Result<Vec<VndbNovel>> {
        if title.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.query(&model::search_query(title)).await
    }

    /// Novel `id` (`v17`), if VNDB has it.
    pub async fn novel(&self, id: &str) -> Result<Option<VndbNovel>> {
        let Some(id) = novel_id(id) else {
            return Ok(None);
        };
        let found = self.query(&model::novel_query(&id)).await?;
        Ok(found.into_iter().find(|n| n.id == id))
    }
}

/// Which library game to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub runner: Runner,
    pub app_name: String,
    pub title: String,
    pub release_year: Option<u32>,
    /// The user's choice (`iroriVndbId`): a VNDB id or [`VNDB_NONE`];
    /// `None` finds the novel by title.
    pub chosen: Option<String>,
}

/// Where answers are cached.
pub fn cache_dir() -> PathBuf {
    util::cache_dir("vndb")
}

fn cache_file(dir: &Path, lookup: &Lookup) -> PathBuf {
    // The choice is part of the name, so changing it reads the novel again.
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

/// The cached answer for `lookup`, without asking VNDB.
pub async fn cached(dir: &Path, lookup: &Lookup) -> Option<Option<VndbNovel>> {
    let (found, age) =
        util::cache_read_entry::<Option<VndbNovel>>(&cache_file(dir, lookup)).await?;
    let ttl = if found.is_some() {
        FOUND_TTL
    } else {
        MISSING_TTL
    };
    (age <= ttl).then_some(found)
}

/// The VNDB novel of a library game: the cached one, else the user's
/// choice or the search result that is the game. `Ok(None)` when VNDB has
/// no such novel (or the user said so).
pub async fn novel_for(client: &Client, dir: &Path, lookup: &Lookup) -> Result<Option<VndbNovel>> {
    if lookup.chosen.as_deref() == Some(VNDB_NONE) {
        return Ok(None);
    }
    if let Some(found) = cached(dir, lookup).await {
        return Ok(found);
    }
    let found = match lookup.chosen.as_deref() {
        Some(id) => client.novel(id).await?,
        None => {
            let results = client.search(&lookup.title).await?;
            best_match(&results, &lookup.title, lookup.release_year).cloned()
        }
    };
    if let Err(e) = util::cache_write(&cache_file(dir, lookup), &found).await {
        tracing::warn!("caching VNDB's answer: {e:#}");
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use irori_depot::test_server::{Resp, TestServer};

    use super::*;

    /// VNDB's `/vn` endpoint, answering searches for "Ever17" and the id
    /// `v2`; anything else is empty. A search for "busy" is throttled.
    fn server() -> TestServer {
        TestServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/kana/vn") => {
                assert_eq!(req.header("Content-Type"), Some("application/json"));
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                assert!(body["fields"].as_str().unwrap().contains("image{"));
                let filter = &body["filters"];
                if filter == &serde_json::json!(["search", "=", "busy"]) {
                    return Resp::status(429);
                }
                if filter == &serde_json::json!(["search", "=", "Ever17"]) {
                    Resp::json(serde_json::json!({"more": false, "results": [
                        {"id": "v19373", "title": "Ever17 -the out of infinity- Premium Edition",
                         "released": "2011-06-30"},
                        {"id": "v17", "title": "Ever17 -the out of infinity-",
                         "aliases": ["Ever17"], "released": "2002-08-29",
                         "description": "Under the sea. [spoiler]Twist.[/spoiler]"}
                    ]}))
                } else if filter == &serde_json::json!(["id", "=", "v2"]) {
                    Resp::json(serde_json::json!({"more": false, "results": [
                        {"id": "v2", "title": "Kana: Little Sister"}
                    ]}))
                } else {
                    Resp::json(serde_json::json!({"more": false, "results": []}))
                }
            }
            _ => Resp::status(404),
        })
    }

    fn client(s: &TestServer) -> Client {
        Client::with_api(&s.url("/kana/"), Duration::from_millis(1))
    }

    #[tokio::test]
    async fn finds_caches_and_honours_the_users_choice() {
        let s = server();
        let c = client(&s);
        let dir = tempfile::tempdir().unwrap();
        let mut lookup = Lookup {
            runner: Runner::Sideload,
            app_name: "abc".into(),
            title: "Ever17".into(),
            release_year: None,
            chosen: None,
        };
        let found = novel_for(&c, dir.path(), &lookup).await.unwrap().unwrap();
        assert_eq!(found.id, "v17");
        assert_eq!(found.description.as_deref(), Some("Under the sea."));
        let asked = s.requests().len();
        // Cached: VNDB isn't asked again.
        assert_eq!(
            novel_for(&c, dir.path(), &lookup).await.unwrap(),
            Some(found)
        );
        assert_eq!(s.requests().len(), asked);

        lookup.chosen = Some("v2".into());
        let chosen = novel_for(&c, dir.path(), &lookup).await.unwrap().unwrap();
        assert_eq!(chosen.title, "Kana: Little Sister");
        lookup.chosen = Some(VNDB_NONE.into());
        let asked = s.requests().len();
        assert_eq!(novel_for(&c, dir.path(), &lookup).await.unwrap(), None);
        assert_eq!(s.requests().len(), asked);

        lookup.chosen = None;
        lookup.app_name = "other".into();
        lookup.title = "Nothing Like It".into();
        assert_eq!(novel_for(&c, dir.path(), &lookup).await.unwrap(), None);
        assert_eq!(cached(dir.path(), &lookup).await, Some(None));
    }

    #[tokio::test]
    async fn says_when_vndb_is_busy() {
        let s = server();
        let e = client(&s).search("busy").await.unwrap_err();
        assert_eq!(
            e.downcast_ref::<VndbError>(),
            Some(&VndbError::TooManyRequests)
        );
        // An id that isn't one is never asked.
        let asked = s.requests().len();
        assert_eq!(client(&s).novel("../x").await.unwrap(), None);
        assert_eq!(s.requests().len(), asked);
    }
}
