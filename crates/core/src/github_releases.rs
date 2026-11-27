//! Shared GitHub (and Forgejo) release-check rate-limit protection.
//!
//! GitHub allows 60 unauthenticated `api.github.com` requests per hour per IP,
//! shared with every other process on the host and devices behind the same NAT.
//! Callers across Irori (Wine/Proton manager, DXVK/VKD3D-Proton/DXVK-NVAPI,
//! emulator engine releases, OptiScaler/RenoDX, and app update checks) route
//! through this module so they:
//!
//! 1. Pre-check the uncounted Atom release feed (`releases.atom`) when cached
//!    data exists, skipping `api.github.com` entirely when nothing changed.
//! 2. Send `If-None-Match` with the saved `ETag` so `304 Not Modified` answers
//!    do not spend hourly API quota.
//! 3. Share one persisted `x-ratelimit-*` budget (`state/github-release-checks.json`,
//!    migrating from legacy `state/wine-release-checks.json`), keeping [`RESERVE`]
//!    calls free for manual user-triggered refreshes and installs.
//! 4. Surface a clean [`RateLimited`] error with the reset timestamp instead of
//!    raw `reqwest` 403/429 error URLs.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::{Paths, json_store, util};

/// API calls automatic checks leave for the ones the user asks for.
pub const RESERVE: u32 = 10;

/// Feed entries the fingerprint covers: enough to see a new or edited
/// release, few enough that a long history doesn't matter.
const FINGERPRINT_ENTRIES: usize = 10;

/// Serializes read-modify-write updates to the shared rate-limit state file so
/// concurrent release checks never overwrite each other's budget or feed map.
static STATE_LOCK: Mutex<()> = Mutex::new(());

/// Who asked for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// The user (Refresh, an install): may use the whole budget.
    Manual,
    /// Irori on its own (a page opening, the daily update check): keeps
    /// [`RESERVE`] calls.
    Automatic,
}

/// GitHub's API budget as its last answer gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub limit: u32,
    pub remaining: u32,
    /// Unix time the budget refills.
    pub reset: u64,
}

/// What the checks remember between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckState {
    pub github: Option<Budget>,
    /// The release feed's fingerprint per source key when its list was last
    /// fetched.
    pub feeds: BTreeMap<String, String>,
    /// The last HTTP `ETag` per source key for conditional `If-None-Match`
    /// requests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub etags: BTreeMap<String, String>,
}

/// GitHub refused a check, or would: its API budget is used up until
/// `reset` (unix time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    pub reset: u64,
}

impl RateLimited {
    /// Whole minutes until the budget refills, at least one.
    pub fn minutes_left(&self, now: u64) -> u64 {
        self.reset.saturating_sub(now).div_ceil(60).max(1)
    }
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = self.minutes_left(util::unix_now());
        write!(
            f,
            "GitHub's limit for release checks is used up; it resets in {minutes} min"
        )
    }
}

impl std::error::Error for RateLimited {}

/// Outcome of a guarded GitHub/Forgejo release API request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardedFetch {
    /// The upstream release feed fingerprint or HTTP `ETag` (`304 Not Modified`)
    /// confirmed nothing changed since the cached payload was stored.
    Unchanged,
    /// A fresh response body was fetched from the API.
    Updated(String),
    /// The API returned HTTP 404 (for example, a repository with no published
    /// releases yet).
    NotFound,
}

/// The rate limit in `err`'s chain, if that is why it failed.
pub fn rate_limited(err: &anyhow::Error) -> Option<RateLimited> {
    err.chain()
        .find_map(|e| e.downcast_ref::<RateLimited>().copied())
}

/// Path to the shared persisted release-check state file.
pub fn state_file() -> PathBuf {
    Paths::get().state_dir().join("github-release-checks.json")
}

fn legacy_state_file() -> PathBuf {
    Paths::get().state_dir().join("wine-release-checks.json")
}

fn read_state_at(path: &Path, fallback: Option<&Path>) -> CheckState {
    if path.is_file() {
        return json_store::read_or_default(path);
    }
    if let Some(legacy) = fallback
        && legacy.is_file()
    {
        return json_store::read_or_default(legacy);
    }
    CheckState::default()
}

fn write_state_at(path: &Path, state: &CheckState) {
    if let Err(e) = json_store::write(path, state) {
        tracing::warn!("saving the release check state: {e:#}");
    }
}

/// Read the shared release-check state (migrating from the legacy Wine file if
/// the new file has not been written yet).
pub fn read_state() -> CheckState {
    let _guard = STATE_LOCK.lock();
    read_state_at(&state_file(), Some(&legacy_state_file()))
}

/// Persist the shared release-check state.
pub fn write_state(state: &CheckState) {
    let _guard = STATE_LOCK.lock();
    write_state_at(&state_file(), state);
}

/// Atomically read, update, and persist the shared release-check state.
pub fn update_state<R>(f: impl FnOnce(&mut CheckState) -> R) -> R {
    let _guard = STATE_LOCK.lock();
    let path = state_file();
    let legacy = legacy_state_file();
    let mut state = read_state_at(&path, Some(&legacy));
    let out = f(&mut state);
    write_state_at(&path, &state);
    out
}

fn update_state_at<R>(
    path: &Path,
    fallback: Option<&Path>,
    f: impl FnOnce(&mut CheckState) -> R,
) -> R {
    let _guard = STATE_LOCK.lock();
    let mut state = read_state_at(path, fallback);
    let out = f(&mut state);
    write_state_at(path, &state);
    out
}

/// When GitHub's budget is used up, the time it refills. No network.
pub fn exhausted_until(now: u64) -> Option<u64> {
    read_state()
        .github
        .filter(|b| b.remaining == 0 && b.reset > now)
        .map(|b| b.reset)
}

/// Check the persisted GitHub budget for `check` without hitting the network.
pub fn check_budget(check: CheckKind) -> Result<(), RateLimited> {
    let state = read_state();
    may_call_api(state.github, util::unix_now(), check)
}

/// Record `x-ratelimit-*` headers from a GitHub API response and return
/// [`RateLimited`] if `status` was a rate-limit refusal (403/429).
pub fn record_response(headers: &HeaderMap, status: u16, now: u64) -> Option<RateLimited> {
    record_response_at(
        &state_file(),
        Some(&legacy_state_file()),
        headers,
        status,
        now,
    )
}

fn record_response_at(
    path: &Path,
    fallback: Option<&Path>,
    headers: &HeaderMap,
    status: u16,
    now: u64,
) -> Option<RateLimited> {
    let budget = budget_from_headers(headers);
    let refused = refusal(status, headers, now);
    if budget.is_some() || refused.is_some() {
        update_state_at(path, fallback, |state| {
            if let Some(b) = budget {
                state.github = Some(b);
            } else if let Some(limited) = refused {
                let limit = state.github.map_or(60, |b| b.limit);
                state.github = Some(Budget {
                    limit,
                    remaining: 0,
                    reset: limited.reset,
                });
            }
        });
    }
    refused
}

/// Whether `api_url` is GitHub's API, whose calls are counted.
pub fn is_github(api_url: &str) -> bool {
    api_url.starts_with("https://api.github.com/")
}

/// The Atom feed of a releases API address: GitHub's
/// `https://github.com/<owner>/<repo>/releases.atom`, or a Forgejo's
/// `https://<host>/<owner>/<repo>/releases.atom`.
///
/// Accepts `/releases`, `/releases/latest`, and `/releases/tags/<tag>` URLs.
pub fn feed_url(api_url: &str) -> Option<String> {
    static API: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"^https://([^/]+)/(?:api/v1/)?repos/([^/?#]+)/([^/?#]+)/releases(?:/(?:latest|tags/[^/?#]+))?/?(?:\?.*)?$",
        )
        .expect("valid regex")
    });
    let c = API.captures(api_url)?;
    let host = match &c[1] {
        "api.github.com" => "github.com",
        other => other,
    };
    Some(format!("https://{host}/{}/{}/releases.atom", &c[2], &c[3]))
}

/// What identifies a feed's newest releases: the id and update time of its
/// first entries. A new release, or an edited one, changes it.
pub fn feed_fingerprint(xml: &str) -> Option<String> {
    static ENTRY: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<entry\b.*?</entry>").expect("valid regex"));
    static ID: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<id>\s*(.*?)\s*</id>").expect("valid regex"));
    static UPDATED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<updated>\s*(.*?)\s*</updated>").expect("valid regex"));
    if !xml.contains("<feed") {
        return None;
    }
    let parts: Vec<String> = ENTRY
        .find_iter(xml)
        .take(FINGERPRINT_ENTRIES)
        .map(|entry| {
            let entry = entry.as_str();
            let field = |re: &Regex| {
                re.captures(entry)
                    .map(|c| c[1].to_string())
                    .unwrap_or_default()
            };
            format!("{}@{}", field(&ID), field(&UPDATED))
        })
        .collect();
    // A feed without releases says nothing a list couldn't; ask the API.
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// GitHub's budget from an API answer's headers.
pub fn budget_from_headers(headers: &HeaderMap) -> Option<Budget> {
    let number =
        |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    Some(Budget {
        limit: u32::try_from(number("x-ratelimit-limit")?).ok()?,
        remaining: u32::try_from(number("x-ratelimit-remaining")?).ok()?,
        reset: number("x-ratelimit-reset")?,
    })
}

/// Whether a check may call GitHub's API with `budget` left at `now`. A
/// budget whose reset time has passed is full again.
pub fn may_call_api(budget: Option<Budget>, now: u64, check: CheckKind) -> Result<(), RateLimited> {
    let Some(b) = budget.filter(|b| b.reset > now) else {
        return Ok(());
    };
    let floor = match check {
        CheckKind::Manual => 1,
        CheckKind::Automatic => RESERVE,
    };
    if b.remaining < floor {
        return Err(RateLimited { reset: b.reset });
    }
    Ok(())
}

/// Whether a refused API call (`status`) was GitHub's rate limit: a 403 or
/// 429 with no calls left, or with a `retry-after` (its secondary limit).
pub fn refusal(status: u16, headers: &HeaderMap, now: u64) -> Option<RateLimited> {
    if status != 403 && status != 429 {
        return None;
    }
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if let Some(secs) = retry_after {
        return Some(RateLimited { reset: now + secs });
    }
    budget_from_headers(headers)
        .filter(|b| b.remaining == 0)
        .map(|b| RateLimited {
            reset: b.reset.max(now),
        })
}

/// Check the Atom feed (when `has_cached` is true), enforce the shared GitHub
/// rate-limit budget, send conditional `If-None-Match`, and fetch JSON text
/// from `api_url`.
pub async fn fetch_guarded_json(
    api_url: &str,
    cache_key: &str,
    has_cached: bool,
    check: CheckKind,
) -> Result<GuardedFetch> {
    fetch_guarded_json_with(
        api_url,
        feed_url(api_url).as_deref(),
        cache_key,
        has_cached,
        check,
        &state_file(),
        Some(&legacy_state_file()),
        is_github(api_url),
    )
    .await
}

/// Like [`fetch_guarded_json`], with explicit feed URL, state path, and GitHub
/// flag so offline unit tests can exercise the full flow against a local server.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_guarded_json_with(
    api_url: &str,
    feed_override: Option<&str>,
    cache_key: &str,
    has_cached: bool,
    check: CheckKind,
    state_path: &Path,
    legacy_path: Option<&Path>,
    treat_as_github: bool,
) -> Result<GuardedFetch> {
    let fingerprint = match feed_override {
        Some(feed) => match crate::http::client()
            .get(feed)
            .header("Accept", "application/atom+xml, application/xml, text/xml")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp
                .text()
                .await
                .ok()
                .and_then(|xml| feed_fingerprint(&xml)),
            Ok(resp) => {
                tracing::debug!("{cache_key} release feed returned {}", resp.status());
                None
            }
            Err(e) => {
                tracing::debug!("{cache_key} release feed: {e:#}");
                None
            }
        },
        None => None,
    };

    let (saved_budget, saved_feed, saved_etag) = {
        let _guard = STATE_LOCK.lock();
        let state = read_state_at(state_path, legacy_path);
        (
            state.github,
            state.feeds.get(cache_key).cloned(),
            state.etags.get(cache_key).cloned(),
        )
    };

    if has_cached && fingerprint.is_some() && saved_feed == fingerprint {
        tracing::debug!("{cache_key}: release feed unchanged, no API call");
        return Ok(GuardedFetch::Unchanged);
    }

    let now = util::unix_now();
    if treat_as_github {
        may_call_api(saved_budget, now, check)?;
    }

    let mut req = crate::http::client()
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if has_cached && let Some(etag) = saved_etag.as_deref() {
        req = req.header("If-None-Match", etag);
    }

    let response = req
        .send()
        .await
        .with_context(|| format!("could not reach release API for {cache_key}"))?;
    let status = response.status();
    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let refused = if treat_as_github {
        record_response_at(
            state_path,
            legacy_path,
            response.headers(),
            status.as_u16(),
            now,
        )
    } else {
        None
    };

    if status == reqwest::StatusCode::NOT_MODIFIED && has_cached {
        update_state_at(state_path, legacy_path, |state| {
            if let Some(f) = fingerprint {
                state.feeds.insert(cache_key.to_string(), f);
            }
            if let Some(etag) = new_etag {
                state.etags.insert(cache_key.to_string(), etag);
            }
        });
        return Ok(GuardedFetch::Unchanged);
    }

    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(GuardedFetch::NotFound);
    }

    if !status.is_success() {
        if let Some(limited) = refused {
            return Err(anyhow::Error::new(limited));
        }
        bail!("the server answered {status}");
    }

    let body = response
        .text()
        .await
        .with_context(|| format!("could not read release API response for {cache_key}"))?;

    update_state_at(state_path, legacy_path, |state| {
        match fingerprint {
            Some(f) => {
                state.feeds.insert(cache_key.to_string(), f);
            }
            None => {
                state.feeds.remove(cache_key);
            }
        }
        if let Some(etag) = new_etag {
            state.etags.insert(cache_key.to_string(), etag);
        }
    });

    Ok(GuardedFetch::Updated(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    struct TestResp {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    async fn spawn_server(
        handler: impl Fn(String, String) -> TestResp + Send + Sync + 'static,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let resp = handler(path, req);
                    let reason = match resp.status {
                        200 => "OK",
                        304 => "Not Modified",
                        403 => "Forbidden",
                        404 => "Not Found",
                        429 => "Too Many Requests",
                        _ => "Error",
                    };
                    let mut out = format!("HTTP/1.1 {} {reason}\r\n", resp.status);
                    for (k, v) in &resp.headers {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp.body.len(),
                        resp.body
                    ));
                    let _ = stream.write_all(out.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), handle)
    }

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:https://github.com/o/r/releases</id>
  <updated>2026-11-13T15:14:59Z</updated>
  <entry>
    <id>tag:github.com,2008:Repository/1/v2.6</id>
    <updated>2026-11-13T15:14:59Z</updated>
    <title>v2.6</title>
  </entry>
</feed>"#;

    #[test]
    fn feeds_of_release_endpoints() {
        assert_eq!(
            feed_url("https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases")
                .as_deref(),
            Some("https://github.com/GloriousEggroll/proton-ge-custom/releases.atom")
        );
        assert_eq!(
            feed_url("https://api.github.com/repos/doitsujin/dxvk/releases/latest").as_deref(),
            Some("https://github.com/doitsujin/dxvk/releases.atom")
        );
        assert_eq!(
            feed_url("https://api.github.com/repos/clshortfuse/renodx/releases/tags/snapshot")
                .as_deref(),
            Some("https://github.com/clshortfuse/renodx/releases.atom")
        );
        assert_eq!(
            feed_url("https://dawn.wine/api/v1/repos/dawn-winery/dwproton/releases?limit=50")
                .as_deref(),
            Some("https://dawn.wine/dawn-winery/dwproton/releases.atom")
        );
        assert_eq!(feed_url("https://example.com/releases.json"), None);
        assert!(is_github("https://api.github.com/repos/a/b/releases"));
        assert!(!is_github("https://dawn.wine/api/v1/repos/a/b/releases"));
    }

    #[test]
    fn a_new_or_edited_release_changes_the_fingerprint() {
        let base = feed_fingerprint(FEED).unwrap();
        assert!(base.starts_with("tag:github.com,2008:Repository/1/v2.6@2026-11-13"));
        let touched = FEED.replacen(
            "<updated>2026-11-13T15:14:59Z</updated>\n  <entry>",
            "<updated>2026-11-18T00:00:00Z</updated>\n  <entry>",
            1,
        );
        assert_eq!(feed_fingerprint(&touched).as_deref(), Some(base.as_str()));
        let newer = FEED.replacen(
            "<entry>",
            "<entry><id>x/v2.7</id><updated>2026-11-18T00:00:00Z</updated></entry>\n  <entry>",
            1,
        );
        assert_ne!(feed_fingerprint(&newer).unwrap(), base);
        assert_eq!(feed_fingerprint("<html>rate limited</html>"), None);
        assert_eq!(feed_fingerprint("<feed></feed>"), None);
    }

    #[test]
    fn automatic_checks_keep_a_reserve() {
        let now = 1_000;
        let budget = |remaining| {
            Some(Budget {
                limit: 60,
                remaining,
                reset: now + 600,
            })
        };
        assert!(may_call_api(None, now, CheckKind::Automatic).is_ok());
        assert!(may_call_api(budget(RESERVE), now, CheckKind::Automatic).is_ok());
        assert_eq!(
            may_call_api(budget(RESERVE - 1), now, CheckKind::Automatic),
            Err(RateLimited { reset: now + 600 })
        );
        assert!(may_call_api(budget(1), now, CheckKind::Manual).is_ok());
        assert!(may_call_api(budget(0), now, CheckKind::Manual).is_err());
    }

    #[test]
    fn refusal_recognises_primary_and_secondary_limits() {
        let now = 1_000;
        let primary = headers(&[
            ("x-ratelimit-limit", "60"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "4600"),
        ]);
        assert_eq!(
            refusal(403, &primary, now),
            Some(RateLimited { reset: 4_600 })
        );
        let secondary = headers(&[("retry-after", "120")]);
        assert_eq!(
            refusal(429, &secondary, now),
            Some(RateLimited { reset: 1_120 })
        );
        assert_eq!(refusal(404, &primary, now), None);
    }

    #[tokio::test]
    async fn unchanged_atom_feed_consumes_zero_api_calls() {
        let api_hits = Arc::new(AtomicUsize::new(0));
        let feed_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_c = api_hits.clone();
        let feed_hits_c = feed_hits.clone();
        let reset = util::unix_now() + 1800;

        let (base, server) = spawn_server(move |path, _req| {
            if path == "/releases.atom" {
                feed_hits_c.fetch_add(1, Ordering::SeqCst);
                TestResp {
                    status: 200,
                    headers: vec![],
                    body: FEED.to_string(),
                }
            } else {
                api_hits_c.fetch_add(1, Ordering::SeqCst);
                TestResp {
                    status: 200,
                    headers: vec![
                        ("ETag".into(), "\"v2.6\"".into()),
                        ("x-ratelimit-limit".into(), "60".into()),
                        ("x-ratelimit-remaining".into(), "59".into()),
                        ("x-ratelimit-reset".into(), reset.to_string()),
                    ],
                    body: r#"{"tag_name":"v2.6"}"#.into(),
                }
            }
        })
        .await;

        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");
        let api_url = format!("{base}/releases/latest");
        let feed = format!("{base}/releases.atom");

        // First call (no cache): fetches API and stores feed fingerprint + budget.
        let first = fetch_guarded_json_with(
            &api_url,
            Some(&feed),
            "dxvk",
            false,
            CheckKind::Automatic,
            &state,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            first,
            GuardedFetch::Updated(r#"{"tag_name":"v2.6"}"#.into())
        );
        assert_eq!(api_hits.load(Ordering::SeqCst), 1);

        // Second call (with cache, unchanged feed): 0 API calls!
        let second = fetch_guarded_json_with(
            &api_url,
            Some(&feed),
            "dxvk",
            true,
            CheckKind::Automatic,
            &state,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(second, GuardedFetch::Unchanged);
        assert_eq!(api_hits.load(Ordering::SeqCst), 1);
        assert_eq!(feed_hits.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn etag_304_and_rate_limit_refusal_are_handled_cleanly() {
        let reset = util::unix_now() + 900;
        let (base, server) = spawn_server(move |path, req| {
            if path == "/etag" {
                if req.to_ascii_lowercase().contains("if-none-match: \"abc\"") {
                    TestResp {
                        status: 304,
                        headers: vec![("ETag".into(), "\"abc\"".into())],
                        body: String::new(),
                    }
                } else {
                    TestResp {
                        status: 200,
                        headers: vec![
                            ("ETag".into(), "\"abc\"".into()),
                            ("x-ratelimit-limit".into(), "60".into()),
                            ("x-ratelimit-remaining".into(), "20".into()),
                            ("x-ratelimit-reset".into(), reset.to_string()),
                        ],
                        body: r#"{"ok":true}"#.into(),
                    }
                }
            } else {
                TestResp {
                    status: 403,
                    headers: vec![
                        ("x-ratelimit-limit".into(), "60".into()),
                        ("x-ratelimit-remaining".into(), "0".into()),
                        ("x-ratelimit-reset".into(), reset.to_string()),
                    ],
                    body: r#"{"message":"API rate limit exceeded"}"#.into(),
                }
            }
        })
        .await;

        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");

        let first = fetch_guarded_json_with(
            &format!("{base}/etag"),
            None,
            "test-etag",
            false,
            CheckKind::Manual,
            &state,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(first, GuardedFetch::Updated(r#"{"ok":true}"#.into()));

        let second = fetch_guarded_json_with(
            &format!("{base}/etag"),
            None,
            "test-etag",
            true,
            CheckKind::Manual,
            &state,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(second, GuardedFetch::Unchanged);

        // Now hit a 403 rate limit endpoint: returns RateLimited and persists remaining=0.
        let err = fetch_guarded_json_with(
            &format!("{base}/limited"),
            None,
            "test-limited",
            false,
            CheckKind::Manual,
            &state,
            None,
            true,
        )
        .await
        .unwrap_err();
        let rl = rate_limited(&err).expect("should carry RateLimited");
        assert_eq!(rl.reset, reset);

        // Subsequent automatic check is rejected immediately without hitting the server.
        let err2 = fetch_guarded_json_with(
            &format!("{base}/etag"),
            None,
            "test-etag",
            false,
            CheckKind::Automatic,
            &state,
            None,
            true,
        )
        .await
        .unwrap_err();
        assert!(rate_limited(&err2).is_some());
        server.abort();
    }
}
