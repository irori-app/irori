//! Direct HTTP/HTTPS link and opt-in BitTorrent download manager.
//!
//! Direct HTTP/HTTPS links run on Irori's native `irori-depot` engine
//! (`stream_segmented` + `SharedRateLimiter` + `finalize_part_async` +
//! `allow_unknown_size`). Only `.torrent` and `magnet:` downloads use an
//! on-demand external `aria2c` process over local loopback WebSocket JSON-RPC
//! with zero post-completion seeding.

pub mod aria2;
pub mod aria2_download;
mod aria2_rpc;
mod http_probe;
pub mod model;
pub mod post_process;
mod setup_cleanup;
pub mod torrent_meta;
mod worker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{Event, Paths, json_store, util};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use http_probe::{
    fetch_remote_torrent_bytes, journal_path, prepare_enqueued_source, torrent_copy_path,
};
pub use model::{
    Aria2Status, DownloadSource, PostAction, SideloadBridge, SideloadCandidate, TorrentFileEntry,
    TorrentPreview, WebDownload, WebDownloadId, WebDownloadKind, WebDownloadRequest,
    WebDownloadSnapshot, WebDownloadState,
};

use crate::Backend;

const FINISHED_LIMIT: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    Pause { user_initiated: bool },
    Cancel { delete_files: bool },
    Remove,
}

pub(super) struct RunningWebTask {
    pub(super) cancel: CancellationToken,
    pub(super) stop: Option<StopReason>,
    pub(super) finished_tx: watch::Sender<bool>,
}

impl RunningWebTask {
    pub(super) fn new(cancel: CancellationToken) -> Self {
        let (finished_tx, _) = watch::channel(false);
        Self {
            cancel,
            stop: None,
            finished_tx,
        }
    }
}

impl Drop for RunningWebTask {
    fn drop(&mut self) {
        let _ = self.finished_tx.send(true);
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WebDownloadsState {
    pub(crate) active: Vec<WebDownload>,
    pub(crate) finished: Vec<WebDownload>,
    pub(crate) paused: bool,
    #[serde(skip)]
    pub(super) running: HashMap<WebDownloadId, RunningWebTask>,
    #[serde(skip)]
    preview_cancel: Option<CancellationToken>,
    #[cfg(test)]
    #[serde(skip)]
    pub(super) state_path: Option<PathBuf>,
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("web-downloads.json")
}

impl WebDownloadsState {
    pub(crate) fn load(allow_auto_start: bool) -> Self {
        let mut state: Self = json_store::read_or_default(&state_file());
        for item in &mut state.active {
            item.download_speed = 0;
            item.eta_secs = None;
            item.peers = None;
            item.seeders = None;
            item.aria2_gid = None;
            if item.state.is_busy() {
                item.state = if allow_auto_start {
                    WebDownloadState::Queued
                } else {
                    WebDownloadState::Paused
                };
            } else if !allow_auto_start && item.state == WebDownloadState::Queued {
                item.state = WebDownloadState::Paused;
            }
        }
        state
    }

    pub(super) fn save(&self) {
        #[cfg(test)]
        let path = self.state_path.clone().unwrap_or_else(state_file);
        #[cfg(not(test))]
        let path = state_file();
        if let Err(err) = json_store::write(&path, self) {
            tracing::error!("saving web download queue: {err:#}");
        }
    }

    pub(crate) fn snapshot(&self) -> WebDownloadSnapshot {
        let total_speed = self
            .active
            .iter()
            .filter(|w| w.state.is_network_active())
            .fold(0u64, |acc, w| acc.saturating_add(w.download_speed));
        WebDownloadSnapshot {
            active: self.active.clone(),
            finished: self.finished.clone(),
            paused: self.paused,
            total_speed,
        }
    }

    pub(crate) fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    pub(crate) fn has_busy_or_unpaused_work(&self) -> bool {
        self.active.iter().any(|w| {
            w.state.is_busy()
                || (!self.paused && !w.user_paused && w.state == WebDownloadState::Queued)
        })
    }

    pub(crate) fn has_paused_items(&self) -> bool {
        self.paused
            || self
                .active
                .iter()
                .any(|w| w.state == WebDownloadState::Paused)
    }

    pub(crate) fn has_running_direct(&self) -> bool {
        self.active
            .iter()
            .any(|w| w.kind == WebDownloadKind::DirectUrl && w.state.is_network_active())
    }

    pub(crate) fn has_running_bt(&self) -> bool {
        self.active
            .iter()
            .any(|w| w.kind.is_bittorrent() && w.state.is_network_active())
    }

    pub(super) fn move_to_finished(
        &mut self,
        id: &WebDownloadId,
        state: WebDownloadState,
        post_action: PostAction,
        status_note: Option<String>,
        error: Option<String>,
    ) -> Option<WebDownload> {
        let pos = self.active.iter().position(|w| &w.id == id)?;
        let mut item = self.active.remove(pos);
        item.state = state;
        item.post_action = post_action;
        item.status_note = status_note;
        item.error = error;
        item.finished_at = Some(util::unix_now());
        item.download_speed = 0;
        item.eta_secs = None;
        item.peers = None;
        item.seeders = None;
        item.aria2_gid = None;
        let cloned = item.clone();
        self.finished.insert(0, item);
        self.finished.truncate(FINISHED_LIMIT);
        Some(cloned)
    }
}

impl Backend {
    /// Current snapshot of direct-link and BitTorrent downloads.
    pub fn web_downloads(&self) -> WebDownloadSnapshot {
        self.web_downloads.lock().snapshot()
    }

    /// Check whether `aria2c` is available for opt-in BitTorrent downloads.
    pub async fn aria2_status(&self) -> Aria2Status {
        let custom = self.settings.app().web_downloads.aria2_path.clone();
        aria2::discover_aria2(&custom).await
    }

    /// Download and verify Windows `aria2c.exe` into `<data>/tools/aria2`.
    pub async fn install_aria2(&self, cancel: CancellationToken) -> Result<PathBuf> {
        self.ensure_online()?;
        aria2_download::install(&cancel).await
    }

    /// Parse a local `.torrent` file, fetch a remote `.torrent` URL, or resolve a
    /// `magnet:` URI via BEP-9 to preview files before starting a download.
    pub async fn preview_torrent_source(&self, source: &str) -> Result<TorrentPreview> {
        match torrent_meta::classify_source(source)? {
            DownloadSource::LocalTorrent(path) => {
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading {}", path.display()))?;
                let mut preview = torrent_meta::parse_torrent(&bytes)?;
                preview.torrent_bytes = Some(bytes);
                Ok(preview)
            }
            DownloadSource::RemoteTorrentUrl(url) => {
                self.ensure_online()?;
                let cancel = CancellationToken::new();
                {
                    let mut state = self.web_downloads.lock();
                    if let Some(prev) = state.preview_cancel.replace(cancel.clone()) {
                        prev.cancel();
                    }
                }
                let result = fetch_remote_torrent_bytes(url.as_str(), &cancel).await;
                self.web_downloads.lock().preview_cancel = None;
                let bytes = result?;
                let mut preview = torrent_meta::parse_torrent(&bytes)?;
                preview.torrent_bytes = Some(bytes);
                Ok(preview)
            }
            DownloadSource::Magnet { uri, .. } => {
                self.ensure_online()?;
                let app = self.settings.app();
                if !app.web_downloads.bittorrent {
                    bail!("BitTorrent support is disabled in Settings -> Downloads");
                }
                let cancel = CancellationToken::new();
                {
                    let mut state = self.web_downloads.lock();
                    if let Some(prev) = state.preview_cancel.replace(cancel.clone()) {
                        prev.cancel();
                    }
                }
                let lease = self
                    .aria2
                    .acquire(&app.web_downloads.aria2_path, self.current_aria2_cap())
                    .await?;
                let scratch = Paths::get()
                    .cache
                    .join("aria2")
                    .join(format!("meta-{}", uuid::Uuid::new_v4().simple()));
                let result = aria2::fetch_magnet_metadata_with_client(
                    &lease.client,
                    &uri,
                    &scratch,
                    aria2::MAGNET_PREVIEW_TIMEOUT,
                    &cancel,
                )
                .await;
                self.web_downloads.lock().preview_cancel = None;
                result
            }
            DownloadSource::DirectUrl(_) => {
                bail!("Direct HTTP/HTTPS links do not carry torrent metadata")
            }
        }
    }

    /// Cancel an in-flight BEP-9 magnet metadata preview.
    pub fn cancel_torrent_preview(&self) {
        if let Some(cancel) = self.web_downloads.lock().preview_cancel.take() {
            cancel.cancel();
        }
    }

    /// Enqueue a new direct-link or BitTorrent download.
    pub fn enqueue_web_download(&self, req: WebDownloadRequest) -> Result<WebDownloadId> {
        let classified = torrent_meta::classify_source(&req.source)?;
        let app = self.settings.app();
        let prefs = &app.web_downloads;

        let is_bt = matches!(
            classified,
            DownloadSource::LocalTorrent(_)
                | DownloadSource::RemoteTorrentUrl(_)
                | DownloadSource::Magnet { .. }
        );
        if is_bt && !prefs.bittorrent {
            bail!("BitTorrent support is turned off. Enable it in Settings -> Downloads.");
        }

        let id = WebDownloadId::new();
        let destination = req
            .destination
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| irori_core::paths::expand_home(&p.to_string_lossy()))
            .unwrap_or_else(|| app.web_download_folder());

        let prepared = prepare_enqueued_source(&id, classified, &destination, &req)?;

        let explicit_title = req
            .title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let custom_title = explicit_title.is_some();
        let title = explicit_title.unwrap_or(prepared.inferred_title);

        self.downloads.lock().when_done.stop_countdown();

        let item = WebDownload {
            id: id.clone(),
            title,
            kind: prepared.kind,
            source: prepared.source,
            stored_torrent: prepared.stored_torrent,
            selected_files: req.selected_files,
            destination,
            output_path: prepared.inferred_output,
            installed_dir: None,
            auto_extract: req.auto_extract.unwrap_or(prefs.auto_extract),
            delete_archive_after_extract: req
                .delete_archive_after_extract
                .unwrap_or(prefs.delete_archive_after_extract),
            auto_add_to_library: req.auto_add_to_library.unwrap_or(prefs.auto_add_to_library),
            state: WebDownloadState::Queued,
            post_action: PostAction::None,
            status_note: None,
            error: None,
            added_at: util::unix_now(),
            started_at: None,
            finished_at: None,
            bytes_total: req.total_bytes.max(prepared.inferred_total),
            bytes_done: 0,
            download_speed: 0,
            eta_secs: None,
            peers: None,
            seeders: None,
            user_paused: false,
            custom_title,
            aria2_gid: None,
        };

        {
            let mut state = self.web_downloads.lock();
            state.active.push(item);
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
        self.web_downloads_wake.notify_one();
        Ok(id)
    }

    /// Pause one web download (marks it `user_paused` so reconnecting never resumes it).
    /// Local post-processing (`Extracting`/`Installing`) is left running.
    pub fn pause_web_download(&self, id: &WebDownloadId) {
        let mut state = self.web_downloads.lock();
        let should_cancel = state
            .active
            .iter_mut()
            .find(|w| &w.id == id)
            .is_some_and(|item| {
                if !matches!(
                    item.state,
                    WebDownloadState::Queued
                        | WebDownloadState::FetchingMetadata
                        | WebDownloadState::Running
                ) {
                    return false;
                }
                item.user_paused = true;
                if item.state == WebDownloadState::Queued {
                    item.state = WebDownloadState::Paused;
                }
                true
            });
        if should_cancel && let Some(running) = state.running.get_mut(id) {
            running.stop = Some(StopReason::Pause {
                user_initiated: true,
            });
            running.cancel.cancel();
        }
        state.save();
        drop(state);
        self.bus.emit(Event::DownloadQueueChanged);
    }

    /// Resume one paused web download.
    pub fn resume_web_download(&self, id: &WebDownloadId) {
        {
            let mut state = self.web_downloads.lock();
            if let Some(item) = state.active.iter_mut().find(|w| &w.id == id) {
                item.user_paused = false;
                if item.state == WebDownloadState::Paused {
                    item.state = WebDownloadState::Queued;
                }
            }
            state.paused = false;
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
        self.web_downloads_wake.notify_one();
    }

    /// Pause the entire web download queue (user-initiated).
    pub fn pause_web_downloads(&self) {
        self.pause_web_downloads_inner(true, false);
    }

    pub(crate) fn pause_web_downloads_for_offline(&self) {
        self.pause_web_downloads_inner(false, false);
        let aria2 = self.aria2.clone();
        self.spawn(async move { aria2.shutdown().await });
    }

    pub(crate) fn pause_web_downloads_for_shutdown(&self) {
        self.pause_web_downloads_inner(false, true);
        let aria2 = self.aria2.clone();
        self.spawn(async move { aria2.shutdown().await });
    }

    fn pause_web_downloads_inner(&self, user_initiated: bool, cancel_local: bool) {
        let mut state = self.web_downloads.lock();
        state.paused = true;
        {
            let WebDownloadsState {
                active, running, ..
            } = &mut *state;
            for item in active.iter_mut() {
                let is_net = matches!(
                    item.state,
                    WebDownloadState::Queued
                        | WebDownloadState::Running
                        | WebDownloadState::FetchingMetadata
                );
                let is_local = matches!(
                    item.state,
                    WebDownloadState::Extracting | WebDownloadState::Installing
                );
                if is_net {
                    if user_initiated {
                        item.user_paused = true;
                    }
                    if item.state == WebDownloadState::Queued {
                        item.state = WebDownloadState::Paused;
                    }
                }
                if (is_net || (cancel_local && is_local))
                    && let Some(task) = running.get_mut(&item.id)
                {
                    task.stop = Some(StopReason::Pause { user_initiated });
                    task.cancel.cancel();
                }
            }
        }
        state.save();
        drop(state);
        self.bus.emit(Event::DownloadQueueChanged);
    }

    /// Resume the web download queue (user-initiated: clears `user_paused`).
    pub fn resume_web_downloads(&self) {
        self.resume_web_downloads_inner(true);
    }

    /// Resume only items that were paused automatically when going offline.
    pub(crate) fn resume_web_downloads_from_offline(&self) {
        self.resume_web_downloads_inner(false);
    }

    fn resume_web_downloads_inner(&self, clear_user_pause: bool) {
        {
            let mut state = self.web_downloads.lock();
            state.paused = false;
            for item in &mut state.active {
                if clear_user_pause {
                    item.user_paused = false;
                }
                if item.state == WebDownloadState::Paused && !item.user_paused {
                    item.state = WebDownloadState::Queued;
                }
            }
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
        self.web_downloads_wake.notify_one();
    }

    /// Cancel an active or paused web download; optionally remove partial files.
    pub fn cancel_web_download(&self, id: &WebDownloadId, delete_files: bool) {
        let mut removed_for_cleanup: Option<WebDownload> = None;
        {
            let mut state = self.web_downloads.lock();
            if let Some(running) = state.running.get_mut(id) {
                running.stop = Some(StopReason::Cancel { delete_files });
                running.cancel.cancel();
                return;
            }
            if let Some(finished) =
                state.move_to_finished(id, WebDownloadState::Canceled, PostAction::None, None, None)
            {
                state.save();
                if delete_files {
                    removed_for_cleanup = Some(finished);
                }
            }
        }
        if let Some(item) = removed_for_cleanup {
            self.spawn_cleanup_files(item);
        }
        self.bus.emit(Event::DownloadQueueChanged);
        self.queue_ran();
    }

    /// Retry a failed or canceled web download.
    pub fn retry_web_download(&self, id: &WebDownloadId) -> Result<()> {
        {
            let mut state = self.web_downloads.lock();
            let pos = state
                .finished
                .iter()
                .position(|w| &w.id == id)
                .context("Web download not found")?;
            let mut item = state.finished.remove(pos);
            item.state = WebDownloadState::Queued;
            item.error = None;
            item.status_note = None;
            item.finished_at = None;
            item.user_paused = false;
            item.post_action = PostAction::None;
            state.active.push(item);
            state.save();
        }
        self.downloads.lock().when_done.stop_countdown();
        self.bus.emit(Event::DownloadQueueChanged);
        self.web_downloads_wake.notify_one();
        Ok(())
    }

    /// Remove a web download from the list (`delete_files == false` touches no files;
    /// `delete_files == true` waits for any active transfer to close its file handles,
    /// then trashes downloaded files and deletes partials after checking every path
    /// with `fs_guard::ensure_deletable`).
    pub async fn remove_web_download(&self, id: &WebDownloadId, delete_files: bool) -> Result<()> {
        let done_rx = {
            let mut state = self.web_downloads.lock();
            state.running.get_mut(id).map(|running| {
                running.stop = Some(StopReason::Remove);
                let rx = running.finished_tx.subscribe();
                running.cancel.cancel();
                rx
            })
        };
        if let Some(mut rx) = done_rx {
            let _ = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|&done| done)).await;
        }

        let removed = {
            let mut state = self.web_downloads.lock();
            let active_item = state
                .active
                .iter()
                .position(|w| &w.id == id)
                .map(|p| state.active.remove(p));
            let finished_item = active_item.or_else(|| {
                state
                    .finished
                    .iter()
                    .position(|w| &w.id == id)
                    .map(|p| state.finished.remove(p))
            });
            if finished_item.is_some() {
                state.save();
            }
            finished_item
        };
        self.bus.emit(Event::DownloadQueueChanged);
        self.queue_ran();

        let Some(item) = removed else {
            return Ok(());
        };
        let _ = tokio::fs::remove_file(torrent_copy_path(&item.id)).await;
        let _ = tokio::fs::remove_file(journal_path(&item.id)).await;

        if delete_files {
            let app = self.settings.app();
            tokio::task::spawn_blocking(move || post_process::delete_download_files(&item, &app))
                .await
                .context("delete task failed")??;
        }
        Ok(())
    }

    /// Clear all finished web downloads from history (keeps files on disk).
    pub fn clear_finished_web_downloads(&self) {
        let ids: Vec<WebDownloadId> = {
            let mut state = self.web_downloads.lock();
            let ids = state.finished.iter().map(|w| w.id.clone()).collect();
            state.finished.clear();
            state.save();
            ids
        };
        for id in ids {
            let _ = std::fs::remove_file(torrent_copy_path(&id));
            let _ = std::fs::remove_file(journal_path(&id));
        }
        self.bus.emit(Event::DownloadQueueChanged);
    }

    /// Disable BitTorrent immediately when the user turns off the setting.
    pub(crate) fn on_bittorrent_disabled(&self) {
        {
            let mut state = self.web_downloads.lock();
            {
                let WebDownloadsState {
                    active, running, ..
                } = &mut *state;
                for item in active.iter_mut() {
                    if item.kind.is_bittorrent()
                        && matches!(
                            item.state,
                            WebDownloadState::Queued
                                | WebDownloadState::FetchingMetadata
                                | WebDownloadState::Running
                        )
                    {
                        if let Some(task) = running.get_mut(&item.id) {
                            task.stop = Some(StopReason::Pause {
                                user_initiated: true,
                            });
                            task.cancel.cancel();
                        }
                        item.user_paused = true;
                        if item.state == WebDownloadState::Queued {
                            item.state = WebDownloadState::Paused;
                        }
                    }
                }
            }
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
        let aria2 = self.aria2.clone();
        self.spawn(async move { aria2.shutdown().await });
    }

    pub(crate) fn set_web_item_state(&self, id: &WebDownloadId, new_state: WebDownloadState) {
        {
            let mut state = self.web_downloads.lock();
            if let Some(item) = state.active.iter_mut().find(|w| &w.id == id) {
                item.state = new_state;
                if new_state == WebDownloadState::Extracting {
                    item.download_speed = 0;
                    item.eta_secs = None;
                }
            }
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
    }

    pub(super) fn spawn_cleanup_files(&self, item: WebDownload) {
        let app = self.settings.app();
        let _ = std::fs::remove_file(torrent_copy_path(&item.id));
        let _ = std::fs::remove_file(journal_path(&item.id));
        self.spawn_blocking(move || {
            if let Err(err) = post_process::delete_download_files(&item, &app) {
                tracing::warn!("cleaning up web download files: {err:#}");
            }
        });
    }
}

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod tests;
