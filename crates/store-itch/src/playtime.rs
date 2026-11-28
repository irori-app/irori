//! Bidirectional playtime and last-played sync with itch.io.
//!
//! Finished sessions are persisted in `<config>/playtime-queue.json` before
//! upload so offline play is never lost, and flushed to
//! `POST /profile/game-sessions` on exit and library refresh. Historical
//! totals and last-played timestamps are fetched politely per game from
//! `GET /profile/game-sessions/summaries/{game_id}`, cached on disk in
//! `<cache>/playtime-summaries.json`, and refreshed at most every six hours
//! with a bounded per-refresh budget for uninstalled bundle libraries.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Result;
use irori_core::{InstallPlatform, json_store, util};
use irori_itch_api::{ApiError, CreateGameSessionRequest, GameSessionSummary};
use serde::{Deserialize, Serialize};

use crate::ItchStore;

/// Minimum session duration uploaded to itch.io (matches Epic and GOG).
const MIN_SESSION_SECS: u64 = 60;

/// itch.io exposes playtime summaries per game; throttle full passes to at
/// most once every six hours (matching GOG).
const TOTALS_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Concurrent summary lookups in flight at once.
const TOTALS_CONCURRENCY: usize = 2;

/// Cap on uninstalled owned games queried per 6-hour refresh pass so large
/// charity bundles stay polite to itch.io's rate limiter while gradually
/// populating the persistent cache.
const MAX_UNINSTALLED_PER_REFRESH: usize = 60;

/// One finished session waiting for upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedSession {
    pub account_id: u64,
    pub app_name: String,
    pub game_id: u64,
    pub upload_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_key_id: Option<u64>,
    pub seconds_run: u64,
    /// RFC 3339 UTC timestamp when the session ended.
    pub last_run_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
}

/// Cached playtime summary for one itch.io game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedSummary {
    pub seconds_run: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// Unix seconds when this entry was checked.
    pub fetched: u64,
}

/// Persisted `<cache>/playtime-summaries.json` cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaytimeCache {
    pub account_id: u64,
    pub games: HashMap<String, CachedSummary>,
}

/// Map an [`InstallPlatform`] to itch.io's session platform identifier.
fn session_platform(platform: InstallPlatform) -> Option<String> {
    match platform {
        InstallPlatform::Linux => Some("linux".into()),
        InstallPlatform::Mac => Some("macos".into()),
        InstallPlatform::Windows => Some("windows".into()),
        InstallPlatform::Browser => None,
    }
}

/// Map the host CPU architecture to itch.io's `SessionArchitecture` string.
fn session_arch() -> Option<String> {
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64".into()),
        "aarch64" => Some("arm64".into()),
        "x86" => Some("386".into()),
        "arm" => Some("arm".into()),
        _ => None,
    }
}

/// Build a queued session ending at `now` (Unix seconds); returns `None` for
/// sessions shorter than a minute or installs without an itch.io upload id.
#[allow(clippy::too_many_arguments)] // Direct fields of the itch.io game-session payload.
pub fn session_for(
    account_id: u64,
    app_name: &str,
    game_id: u64,
    upload_id: u64,
    build_id: Option<u64>,
    download_key_id: Option<u64>,
    platform: InstallPlatform,
    played_secs: u64,
    now: i64,
) -> Option<QueuedSession> {
    if played_secs < MIN_SESSION_SECS || game_id == 0 || upload_id == 0 {
        return None;
    }
    Some(QueuedSession {
        account_id,
        app_name: app_name.to_string(),
        game_id,
        upload_id,
        build_id,
        download_key_id,
        seconds_run: played_secs,
        last_run_at: util::rfc3339_from_unix(now)?,
        platform: session_platform(platform),
        architecture: session_arch(),
    })
}

fn totals_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= TOTALS_EVERY)
}

fn is_stale(entry: Option<&CachedSummary>, now: u64) -> bool {
    entry.is_none_or(|c| now.saturating_sub(c.fetched) >= TOTALS_EVERY.as_secs())
}

enum UploadOutcome {
    Accepted(Option<GameSessionSummary>),
    Rejected,
    ScopeDenied,
    Retry,
}

fn classify_error(err: &anyhow::Error) -> UploadOutcome {
    if let Some(api) = err.downcast_ref::<ApiError>() {
        if api.scope_denied {
            return UploadOutcome::ScopeDenied;
        }
        if api.is_permanent_client_error() {
            return UploadOutcome::Rejected;
        }
    }
    UploadOutcome::Retry
}

impl ItchStore {
    fn sync_enabled(&self) -> bool {
        self.enabled() && !(self.settings)().disable_playtime_sync
    }

    fn note_scope_denied(&self, err: &anyhow::Error) {
        if !self.sessions_unsupported.swap(true, Ordering::AcqRel) {
            tracing::warn!(
                "itch.io playtime sync is unavailable for this API key (keeping local playtime): {err:#}"
            );
        }
    }

    /// Merge one game's server summary into memory and disk cache.
    async fn cache_summary(
        &self,
        account_id: u64,
        app_name: &str,
        summary: Option<GameSessionSummary>,
        now: u64,
    ) {
        let snapshot = {
            let mut cache = self.summaries.write();
            if cache.account_id != account_id {
                cache.account_id = account_id;
                cache.games.clear();
            }
            let entry = cache.games.entry(app_name.to_string()).or_default();
            if let Some(s) = summary {
                entry.seconds_run = entry.seconds_run.max(s.seconds_run);
                if let Some(at) = s.last_run_at.filter(|t| !t.is_empty()) {
                    entry.last_run_at = Some(at);
                }
            }
            entry.fetched = now;
            cache.clone()
        };
        let path = self.layout.playtime_cache_file();
        if let Err(e) = Self::blocking(move || json_store::write(&path, &snapshot))
            .await
            .and_then(|r| r)
        {
            tracing::warn!("saving itch.io playtime cache: {e:#}");
        }
    }

    /// Queue a finished session on disk and try to upload all queued sessions.
    pub(crate) async fn record_playtime(&self, app_name: &str, played_secs: u64) -> Result<()> {
        if !self.sync_enabled() {
            return Ok(());
        }
        let (account_id, download_key_id, game_id, record) = {
            let state = self.state.read();
            let Some(auth) = state.auth.as_ref() else {
                return Ok(());
            };
            let Some(record) = state.installed.get(app_name).cloned() else {
                return Ok(());
            };
            let key = state.keys.get(app_name);
            let game_id = key
                .map(|k| k.game.as_ref().map_or(k.game_id, |g| g.id))
                .or_else(|| app_name.parse::<u64>().ok())
                .unwrap_or(0);
            (auth.user.id, key.map(|k| k.id), game_id, record)
        };
        let Some(session) = session_for(
            account_id,
            app_name,
            game_id,
            record.upload_id,
            record.build_id(),
            download_key_id,
            record.platform,
            played_secs,
            util::unix_now() as i64,
        ) else {
            return Ok(());
        };
        {
            let _g = self.playtime_lock.lock().await;
            let path = self.layout.playtime_queue_file();
            Self::blocking(move || -> Result<()> {
                let mut queue: Vec<QueuedSession> = json_store::read_or_default(&path);
                queue.push(session);
                json_store::write(&path, &queue)
            })
            .await??;
        }
        self.flush_playtime().await;
        Ok(())
    }

    /// Upload queued sessions for the signed-in account in FIFO order.
    pub(crate) async fn flush_playtime(&self) {
        if !self.sync_enabled()
            || irori_core::net::working_offline()
            || self.sessions_unsupported.load(Ordering::Acquire)
        {
            return;
        }
        let Ok(_g) = self.playtime_lock.try_lock() else {
            return;
        };
        let Some(account_id) = self.state.read().auth.as_ref().map(|a| a.user.id) else {
            return;
        };
        let path = self.layout.playtime_queue_file();
        let read_path = path.clone();
        let Ok(queue) =
            Self::blocking(move || json_store::read_or_default::<Vec<QueuedSession>>(&read_path))
                .await
        else {
            return;
        };
        if queue.is_empty() {
            return;
        }
        let Ok(api) = self.api() else {
            return;
        };
        let mut keep = Vec::new();
        let mut stop = false;
        let mut updated_summaries: Vec<(String, GameSessionSummary)> = Vec::new();
        for s in queue {
            if stop || s.account_id != account_id {
                keep.push(s);
                continue;
            }
            let req = CreateGameSessionRequest {
                game_id: s.game_id,
                upload_id: s.upload_id,
                build_id: s.build_id,
                download_key_id: s.download_key_id,
                seconds_run: s.seconds_run,
                last_run_at: s.last_run_at.clone(),
                platform: s.platform.clone(),
                architecture: s.architecture.clone(),
            };
            let outcome = match api.create_game_session(&req).await {
                Ok(summary) => UploadOutcome::Accepted(summary),
                Err(e) => {
                    let classified = classify_error(&e);
                    if matches!(classified, UploadOutcome::ScopeDenied) {
                        self.note_scope_denied(&e);
                    } else if matches!(classified, UploadOutcome::Rejected) {
                        tracing::warn!(
                            "itch.io rejected a playtime session for {}: {e:#}",
                            s.app_name
                        );
                    } else {
                        tracing::debug!("itch.io playtime upload for {}: {e:#}", s.app_name);
                    }
                    classified
                }
            };
            match outcome {
                UploadOutcome::Accepted(Some(summary)) => {
                    updated_summaries.push((s.app_name, summary));
                }
                UploadOutcome::Accepted(None) | UploadOutcome::Rejected => {}
                UploadOutcome::ScopeDenied | UploadOutcome::Retry => {
                    stop = true;
                    keep.push(s);
                }
            }
        }
        let write_path = path;
        if let Err(e) = Self::blocking(move || json_store::write(&write_path, &keep))
            .await
            .and_then(|r| r)
        {
            tracing::warn!("saving the itch.io playtime queue: {e:#}");
        }
        if !updated_summaries.is_empty() {
            let now = util::unix_now();
            for (app, summary) in updated_summaries {
                self.cache_summary(account_id, &app, Some(summary), now)
                    .await;
            }
        }
    }

    /// Refresh stale per-game playtime summaries from itch.io at most every
    /// six hours, prioritizing installed games and bounding uninstalled lookups.
    async fn refresh_summaries(&self) {
        if !self.sync_enabled()
            || irori_core::net::working_offline()
            || self.sessions_unsupported.load(Ordering::Acquire)
        {
            return;
        }
        let Some(account_id) = self.state.read().auth.as_ref().map(|a| a.user.id) else {
            return;
        };
        {
            let mut last = self.totals_read.lock();
            let now = Instant::now();
            if !totals_due(*last, now) {
                return;
            }
            *last = Some(now);
        }
        let Ok(api) = self.api() else {
            *self.totals_read.lock() = None;
            return;
        };
        let now = util::unix_now();
        let candidates = self.summary_candidates(account_id, now);
        if candidates.is_empty() {
            return;
        }
        let mut updates: Vec<(String, Option<GameSessionSummary>)> = Vec::new();
        let mut hit_error = false;
        for chunk in candidates.chunks(TOTALS_CONCURRENCY) {
            let results = futures::future::join_all(chunk.iter().map(|(app, game_id)| {
                let api = api.clone();
                let app = app.clone();
                let game_id = *game_id;
                async move { (app, api.game_sessions_summary(game_id).await) }
            }))
            .await;
            let mut stop = false;
            for (app, result) in results {
                match result {
                    Ok(summary) => updates.push((app, summary)),
                    Err(e) => {
                        if let Some(api_err) = e.downcast_ref::<ApiError>() {
                            if api_err.scope_denied {
                                self.note_scope_denied(&e);
                                stop = true;
                            } else if api_err.rate_limited || api_err.unauthorized {
                                tracing::debug!("itch.io playtime summary stopped: {e:#}");
                                hit_error = true;
                                stop = true;
                            } else {
                                tracing::debug!("itch.io playtime of {app}: {e:#}");
                            }
                        } else {
                            tracing::debug!("itch.io playtime of {app}: {e:#}");
                            hit_error = true;
                            stop = true;
                        }
                    }
                }
            }
            if stop {
                break;
            }
        }
        if hit_error && updates.is_empty() {
            *self.totals_read.lock() = None;
        }
        if updates.is_empty() {
            return;
        }
        let snapshot = {
            let mut cache = self.summaries.write();
            if cache.account_id != account_id {
                cache.account_id = account_id;
                cache.games.clear();
            }
            for (app, summary) in updates {
                let entry = cache.games.entry(app).or_default();
                if let Some(s) = summary {
                    entry.seconds_run = entry.seconds_run.max(s.seconds_run);
                    if let Some(at) = s.last_run_at.filter(|t| !t.is_empty()) {
                        entry.last_run_at = Some(at);
                    }
                }
                entry.fetched = now;
            }
            cache.clone()
        };
        let path = self.layout.playtime_cache_file();
        if let Err(e) = Self::blocking(move || json_store::write(&path, &snapshot))
            .await
            .and_then(|r| r)
        {
            tracing::warn!("saving itch.io playtime cache: {e:#}");
        }
    }

    /// Stale games to query for playtime summaries: all installed owned games
    /// first, then up to [`MAX_UNINSTALLED_PER_REFRESH`] uninstalled games.
    fn summary_candidates(&self, account_id: u64, now: u64) -> Vec<(String, u64)> {
        let state = self.state.read();
        let cache = self.summaries.read();
        let matches_account = cache.account_id == account_id;
        let mut installed = Vec::new();
        let mut uninstalled = Vec::new();
        let mut keys: Vec<_> = state.keys.iter().collect();
        keys.sort_by_key(|(a, _)| *a);
        for (app, key) in keys {
            let game_id = key.game.as_ref().map_or(key.game_id, |g| g.id);
            if game_id == 0 {
                continue;
            }
            let cached = matches_account.then(|| cache.games.get(app)).flatten();
            if !is_stale(cached, now) {
                continue;
            }
            if state.installed.contains_key(app) {
                installed.push((app.clone(), game_id));
            } else if uninstalled.len() < MAX_UNINSTALLED_PER_REFRESH {
                uninstalled.push((app.clone(), game_id));
            }
        }
        installed.extend(uninstalled);
        installed
    }

    /// Whole minutes itch.io recorded per owned game.
    pub(crate) async fn itch_playtime(&self) -> HashMap<String, u64> {
        if !self.sync_enabled() {
            return HashMap::new();
        }
        let Some(account_id) = self.state.read().auth.as_ref().map(|a| a.user.id) else {
            return HashMap::new();
        };
        self.refresh_summaries().await;
        let cache = self.summaries.read();
        if cache.account_id != account_id {
            return HashMap::new();
        }
        cache
            .games
            .iter()
            .filter_map(|(app, entry)| {
                let minutes = entry.seconds_run / 60;
                (minutes > 0).then(|| (app.clone(), minutes))
            })
            .collect()
    }

    /// Last-played RFC 3339 timestamps from itch.io's cached summaries.
    pub(crate) async fn itch_last_played(&self) -> HashMap<String, String> {
        if !self.sync_enabled() {
            return HashMap::new();
        }
        let Some(account_id) = self.state.read().auth.as_ref().map(|a| a.user.id) else {
            return HashMap::new();
        };
        let cache = self.summaries.read();
        if cache.account_id != account_id {
            return HashMap::new();
        }
        cache
            .games
            .iter()
            .filter_map(|(app, entry)| {
                let at = entry.last_run_at.as_ref().filter(|s| !s.is_empty())?;
                Some((app.clone(), at.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_under_a_minute_or_without_upload_are_skipped() {
        assert!(
            session_for(
                42,
                "7",
                7,
                3,
                None,
                Some(500),
                InstallPlatform::Linux,
                59,
                1_700_000_000
            )
            .is_none()
        );
        assert!(
            session_for(
                42,
                "7",
                7,
                0,
                None,
                Some(500),
                InstallPlatform::Linux,
                180,
                1_700_000_000
            )
            .is_none()
        );
        let s = session_for(
            42,
            "7",
            7,
            3,
            Some(99),
            Some(500),
            InstallPlatform::Windows,
            185,
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(s.seconds_run, 185);
        assert_eq!(s.build_id, Some(99));
        assert_eq!(s.download_key_id, Some(500));
        assert_eq!(s.platform.as_deref(), Some("windows"));
        assert!(!s.last_run_at.is_empty());
    }

    #[test]
    fn totals_throttle_and_staleness() {
        let now = Instant::now();
        assert!(totals_due(None, now));
        assert!(!totals_due(Some(now), now + Duration::from_secs(60)));
        assert!(totals_due(Some(now), now + TOTALS_EVERY));

        let ts = 1_700_000_000;
        let fresh = CachedSummary {
            seconds_run: 600,
            last_run_at: None,
            fetched: ts,
        };
        assert!(!is_stale(Some(&fresh), ts + 60));
        assert!(is_stale(Some(&fresh), ts + TOTALS_EVERY.as_secs()));
        assert!(is_stale(None, ts));
    }
}
