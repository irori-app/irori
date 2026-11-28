//! Parallel web download worker pool, 500ms coalesced progress emitter, and
//! direct-URL / BitTorrent item runners.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{Event, GameId, Notice, NoticeLevel, util};
use irori_depot::{FileEntry, Finished, Job, Options};

use super::aria2;
use super::http_probe::{
    DirectProbe, fetch_remote_torrent_bytes, journal_path, persist_torrent_bytes, probe_direct_url,
    selected_bytes, unique_file_name,
};
use super::model::{PostAction, WebDownload, WebDownloadId, WebDownloadKind, WebDownloadState};
use super::post_process;
use super::torrent_meta;
use super::{RunningWebTask, StopReason, WebDownloadsState};
use crate::Backend;

const PROGRESS_COALESCE_INTERVAL: Duration = Duration::from_millis(500);

impl Backend {
    /// Run a detected setup installer (`PostAction::InstallSetup`) for a completed
    /// web download and register the installed game in the library.
    pub async fn install_web_download(&self, id: &WebDownloadId) -> Result<Option<GameId>> {
        let (item, installer) = {
            let mut state = self.web_downloads.lock();
            let entry = state
                .finished
                .iter()
                .find(|w| &w.id == id)
                .cloned()
                .context("Completed web download not found")?;
            let installer = match &entry.post_action {
                PostAction::InstallSetup { installer, .. } => installer.clone(),
                _ => bail!("This download has no installer to run"),
            };
            let mut moving = entry;
            moving.state = WebDownloadState::Installing;
            moving.error = None;
            state.finished.retain(|w| &w.id != id);
            state.active.push(moving.clone());
            state.save();
            (moving, installer)
        };
        self.downloads.lock().when_done.stop_countdown();
        self.bus.emit(Event::DownloadQueueChanged);

        let cancel = CancellationToken::new();
        self.web_downloads
            .lock()
            .running
            .insert(id.clone(), RunningWebTask::new(cancel.clone()));

        let outcome = post_process::run_setup_installer(self, &item, &installer, &cancel).await;
        {
            let mut state = self.web_downloads.lock();
            let final_stop = state.running.remove(id).and_then(|r| r.stop);
            match (final_stop, &outcome) {
                (Some(StopReason::Remove), _) => {
                    if let Some(active) = state.active.iter_mut().find(|w| &w.id == id) {
                        active.state = WebDownloadState::Canceled;
                    }
                }
                (Some(StopReason::Pause { .. }), Err(_)) => {
                    state.move_to_finished(
                        id,
                        WebDownloadState::Done,
                        item.post_action.clone(),
                        None,
                        None,
                    );
                }
                (_, Ok((install_dir, post_action))) => {
                    if let Some(active) = state.active.iter_mut().find(|w| &w.id == id) {
                        active.installed_dir = Some(install_dir.clone());
                    }
                    state.move_to_finished(
                        id,
                        WebDownloadState::Done,
                        post_action.clone(),
                        None,
                        None,
                    );
                }
                (_, Err(err)) => {
                    state.move_to_finished(
                        id,
                        WebDownloadState::Done,
                        item.post_action.clone(),
                        Some(format!("Installer failed: {err:#}")),
                        None,
                    );
                }
            }
            state.save();
        }
        self.bus.emit(Event::DownloadQueueChanged);
        self.queue_ran();

        match outcome {
            Ok((_, PostAction::Play(game_id))) => Ok(Some(game_id)),
            Ok((_, _)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Background parallel worker pool (`1..=5` concurrent web downloads).
    pub(crate) async fn web_download_worker(&self) {
        loop {
            let max_concurrent = self.settings.app().web_downloads.clamped_max_concurrent();
            let bt_enabled = self.settings.app().web_downloads.bittorrent;
            let to_start: Vec<(WebDownload, CancellationToken)> = {
                let mut state = self.web_downloads.lock();
                if state.paused || self.working_offline() {
                    Vec::new()
                } else {
                    let active_net = state
                        .active
                        .iter()
                        .filter(|w| w.state.is_network_active())
                        .count();
                    let open_slots = max_concurrent.saturating_sub(active_net);
                    let mut batch = Vec::new();
                    if open_slots > 0 {
                        let WebDownloadsState {
                            active, running, ..
                        } = &mut *state;
                        for item in active.iter_mut() {
                            if batch.len() >= open_slots {
                                break;
                            }
                            if item.state != WebDownloadState::Queued
                                || item.user_paused
                                || (item.kind.is_bittorrent() && !bt_enabled)
                                || running.contains_key(&item.id)
                            {
                                continue;
                            }
                            item.state = if item.kind == WebDownloadKind::Magnet
                                && item.stored_torrent.is_none()
                            {
                                WebDownloadState::FetchingMetadata
                            } else {
                                WebDownloadState::Running
                            };
                            item.started_at.get_or_insert(util::unix_now());
                            item.error = None;
                            let cancel = CancellationToken::new();
                            running.insert(item.id.clone(), RunningWebTask::new(cancel.clone()));
                            batch.push((item.clone(), cancel));
                        }
                        if !batch.is_empty() {
                            state.save();
                        }
                    }
                    batch
                }
            };

            if to_start.is_empty() {
                self.web_downloads_wake.notified().await;
                continue;
            }

            self.bus.emit(Event::DownloadQueueChanged);
            self.web_downloads_progress_wake.notify_one();
            self.reconcile_bandwidth_limits();

            for (item, cancel) in to_start {
                let b = self.clone();
                self.spawn(async move {
                    b.execute_web_item(item, cancel).await;
                });
            }
        }
    }

    /// Coalesces progress `Event::DownloadQueueChanged` notifications to at most
    /// once every 500ms while >= 1 web download is transferring (0% idle CPU).
    pub(crate) async fn web_progress_emitter(&self) {
        loop {
            let has_running = self
                .web_downloads
                .lock()
                .active
                .iter()
                .any(|w| w.state.is_network_active());
            if !has_running {
                self.web_downloads_progress_wake.notified().await;
                continue;
            }
            tokio::time::sleep(PROGRESS_COALESCE_INTERVAL).await;
            if self
                .web_downloads
                .lock()
                .active
                .iter()
                .any(|w| w.state.is_network_active())
            {
                self.bus.emit(Event::DownloadQueueChanged);
            }
        }
    }

    async fn execute_web_item(&self, item: WebDownload, cancel: CancellationToken) {
        let id = item.id.clone();
        let transfer_result = match item.kind {
            WebDownloadKind::DirectUrl => self.run_direct_item(&item, &cancel).await,
            WebDownloadKind::TorrentFile | WebDownloadKind::Magnet => {
                self.run_bt_item(&item, &cancel).await
            }
        };

        enum TransferOutcome {
            Completed {
                output_path: PathBuf,
                title: Option<String>,
            },
            Paused {
                user_initiated: bool,
            },
            Canceled {
                delete_files: bool,
            },
            Removed,
            Failed(String),
        }

        let stop = self
            .web_downloads
            .lock()
            .running
            .get(&id)
            .and_then(|r| r.stop);

        let outcome = match (stop, transfer_result) {
            (Some(StopReason::Remove), _) => TransferOutcome::Removed,
            (Some(StopReason::Pause { user_initiated }), _) => {
                TransferOutcome::Paused { user_initiated }
            }
            (Some(StopReason::Cancel { delete_files }), _) => {
                TransferOutcome::Canceled { delete_files }
            }
            (None, Ok(Some((output_path, title)))) => {
                TransferOutcome::Completed { output_path, title }
            }
            (None, Ok(None)) => TransferOutcome::Canceled {
                delete_files: false,
            },
            (None, Err(err)) => {
                tracing::warn!(
                    source = %torrent_meta::redact_source(&item.source),
                    "web download failed: {err:#}"
                );
                TransferOutcome::Failed(format!("{err:#}"))
            }
        };

        match outcome {
            TransferOutcome::Completed { output_path, title } => {
                let updated_item = {
                    let mut state = self.web_downloads.lock();
                    let cloned = if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                        entry.output_path = Some(output_path.clone());
                        if !entry.custom_title
                            && let Some(t) = title
                            && !t.trim().is_empty()
                        {
                            entry.title = t;
                        }
                        entry.bytes_done = entry.bytes_done.max(entry.bytes_total);
                        entry.download_speed = 0;
                        entry.eta_secs = None;
                        Some(entry.clone())
                    } else {
                        None
                    };
                    if cloned.is_some() {
                        state.save();
                    }
                    cloned
                };
                self.reconcile_bandwidth_limits();

                let Some(updated_item) = updated_item else {
                    self.web_downloads.lock().running.remove(&id);
                    self.web_downloads_wake.notify_one();
                    return;
                };

                let post =
                    post_process::run_post_process(self, &updated_item, output_path, &cancel).await;
                let mut cleanup_item = None;
                let mut notice_done = None;
                {
                    let mut state = self.web_downloads.lock();
                    let final_stop = state.running.remove(&id).and_then(|r| r.stop);
                    match (final_stop, post) {
                        (Some(StopReason::Remove), _) => {
                            if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                                entry.state = WebDownloadState::Canceled;
                                entry.download_speed = 0;
                                entry.eta_secs = None;
                            }
                        }
                        (Some(StopReason::Pause { user_initiated }), Err(_)) => {
                            if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                                entry.state = WebDownloadState::Paused;
                                entry.user_paused |= user_initiated;
                                entry.download_speed = 0;
                                entry.eta_secs = None;
                            }
                        }
                        (Some(StopReason::Cancel { delete_files }), _) => {
                            if let Some(f) = state.move_to_finished(
                                &id,
                                WebDownloadState::Canceled,
                                PostAction::None,
                                None,
                                None,
                            ) && delete_files
                            {
                                cleanup_item = Some(f);
                            }
                        }
                        (_, Ok(pp)) => {
                            if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                                entry.output_path = Some(pp.output_path);
                                entry.installed_dir = pp.installed_dir;
                            }
                            if let Some(done) = state.move_to_finished(
                                &id,
                                WebDownloadState::Done,
                                pp.post_action,
                                pp.status_note,
                                None,
                            ) {
                                notice_done = Some(done.title);
                            }
                        }
                        (_, Err(err)) => {
                            state.move_to_finished(
                                &id,
                                WebDownloadState::Failed,
                                PostAction::ShowInFolder,
                                None,
                                Some(format!("{err:#}")),
                            );
                        }
                    }
                    state.save();
                }
                if let Some(item) = cleanup_item {
                    self.spawn_cleanup_files(item);
                }
                if let Some(title) = notice_done {
                    self.bus.notice(Notice::new(
                        NoticeLevel::Success,
                        format!("{title} downloaded"),
                    ));
                }
            }
            TransferOutcome::Removed => {
                let mut state = self.web_downloads.lock();
                if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                    entry.state = WebDownloadState::Canceled;
                    entry.download_speed = 0;
                    entry.eta_secs = None;
                    entry.aria2_gid = None;
                }
                state.running.remove(&id);
                state.save();
            }
            TransferOutcome::Paused { user_initiated } => {
                let mut state = self.web_downloads.lock();
                state.running.remove(&id);
                if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                    entry.state = WebDownloadState::Paused;
                    entry.user_paused |= user_initiated;
                    entry.download_speed = 0;
                    entry.eta_secs = None;
                    entry.aria2_gid = None;
                }
                state.save();
            }
            TransferOutcome::Canceled { delete_files } => {
                let removed = {
                    let mut state = self.web_downloads.lock();
                    state.running.remove(&id);
                    let f = state.move_to_finished(
                        &id,
                        WebDownloadState::Canceled,
                        PostAction::None,
                        None,
                        None,
                    );
                    state.save();
                    f
                };
                if delete_files && let Some(item) = removed {
                    self.spawn_cleanup_files(item);
                }
            }
            TransferOutcome::Failed(err_msg) => {
                let title = {
                    let mut state = self.web_downloads.lock();
                    state.running.remove(&id);
                    let t = state
                        .move_to_finished(
                            &id,
                            WebDownloadState::Failed,
                            PostAction::None,
                            None,
                            Some(err_msg.clone()),
                        )
                        .map(|w| w.title);
                    state.save();
                    t
                };
                if let Some(title) = title {
                    self.bus.notice(
                        Notice::new(NoticeLevel::Error, format!("{title} failed to download"))
                            .message(err_msg),
                    );
                }
            }
        }

        self.reconcile_bandwidth_limits();
        self.bus.emit(Event::DownloadQueueChanged);
        self.web_downloads_wake.notify_one();
        self.queue_ran();
    }

    async fn run_direct_item(
        &self,
        item: &WebDownload,
        cancel: &CancellationToken,
    ) -> Result<Option<(PathBuf, Option<String>)>> {
        let probe = probe_direct_url(&item.source, cancel).await?;
        let (final_url, filename, size) = match probe {
            DirectProbe::RemoteTorrent { bytes } => {
                if !self.settings.app().web_downloads.bittorrent {
                    bail!(
                        "This link points to a .torrent file, and BitTorrent is turned off in Settings -> Downloads"
                    );
                }
                let preview = torrent_meta::parse_torrent(&bytes)?;
                let stored = persist_torrent_bytes(&item.id, &bytes)?;
                let bt_output = item.destination.join(&preview.name);
                let mut bt_item = item.clone();
                bt_item.kind = WebDownloadKind::TorrentFile;
                bt_item.stored_torrent = Some(stored.clone());
                bt_item.output_path = Some(bt_output.clone());
                if !bt_item.custom_title {
                    bt_item.title = preview.name.clone();
                }
                bt_item.bytes_total = preview.total_bytes;
                {
                    let mut state = self.web_downloads.lock();
                    if let Some(entry) = state.active.iter_mut().find(|w| w.id == item.id) {
                        entry.kind = WebDownloadKind::TorrentFile;
                        entry.stored_torrent = Some(stored);
                        entry.output_path = Some(bt_output);
                        if !entry.custom_title {
                            entry.title = preview.name;
                        }
                        entry.bytes_total = preview.total_bytes;
                    }
                    state.save();
                }
                self.reconcile_bandwidth_limits();
                return self.run_bt_item(&bt_item, cancel).await;
            }
            DirectProbe::File {
                final_url,
                filename,
                size,
            } => (final_url, filename, size),
        };

        tokio::fs::create_dir_all(&item.destination)
            .await
            .with_context(|| format!("creating {}", item.destination.display()))?;

        let final_name = match &item.output_path {
            Some(existing) => existing
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(filename),
            None => unique_file_name(&item.destination, &filename),
        };
        let output_path = item.destination.join(&final_name);
        {
            let mut state = self.web_downloads.lock();
            if let Some(entry) = state.active.iter_mut().find(|w| w.id == item.id) {
                entry.output_path = Some(output_path.clone());
                if size > 0 {
                    entry.bytes_total = size;
                }
                if !entry.custom_title {
                    entry.title = final_name.clone();
                }
            }
            state.save();
        }

        let normalized = irori_depot::path::normalize(&final_name)?;
        let entry = FileEntry::whole(normalized, size, None, vec![final_url]);
        let opts = Options {
            workers: self.settings.app().max_workers as usize,
            rate_limiter: Some(self.rate_limiter.clone()),
            journal: Some(journal_path(&item.id)),
            allow_unknown_size: true,
            ..Options::default()
        };

        let (b, id) = (self.clone(), item.id.clone());
        let progress = Arc::new(move |p: irori_depot::Progress| {
            let mut state = b.web_downloads.lock();
            if let Some(entry) = state.active.iter_mut().find(|w| w.id == id) {
                entry.bytes_done = p.bytes_done;
                if p.bytes_total > 0 {
                    entry.bytes_total = p.bytes_total;
                }
                entry.download_speed = p.download_speed;
                entry.eta_secs = p.eta_secs;
            }
        });

        let finished = irori_depot::download(
            irori_core::http::download_client(),
            Job {
                root: &item.destination,
                files: &[entry],
                remove: &[],
            },
            &opts,
            progress,
            cancel,
        )
        .await?;

        match finished {
            Finished::Done => Ok(Some((
                output_path,
                (!item.custom_title).then_some(final_name),
            ))),
            Finished::Cancelled => Ok(None),
        }
    }

    async fn run_bt_item(
        &self,
        item: &WebDownload,
        cancel: &CancellationToken,
    ) -> Result<Option<(PathBuf, Option<String>)>> {
        let app = self.settings.app();
        if !app.web_downloads.bittorrent {
            bail!("BitTorrent support is turned off in Settings -> Downloads");
        }

        let torrent_bytes = match &item.stored_torrent {
            Some(path) if path.is_file() => Some(
                tokio::fs::read(path)
                    .await
                    .with_context(|| format!("reading {}", path.display()))?,
            ),
            _ if item.kind == WebDownloadKind::TorrentFile
                && (item.source.starts_with("http://") || item.source.starts_with("https://")) =>
            {
                let bytes = fetch_remote_torrent_bytes(&item.source, cancel).await?;
                let preview = torrent_meta::parse_torrent(&bytes)?;
                let stored = persist_torrent_bytes(&item.id, &bytes)?;
                let total = selected_bytes(&preview, &item.selected_files);
                let bt_output = item.destination.join(&preview.name);
                let mut state = self.web_downloads.lock();
                if let Some(entry) = state.active.iter_mut().find(|w| w.id == item.id) {
                    entry.stored_torrent = Some(stored);
                    entry.output_path = Some(bt_output);
                    if !entry.custom_title {
                        entry.title = preview.name;
                    }
                    if entry.bytes_total == 0 && total > 0 {
                        entry.bytes_total = total;
                    }
                }
                state.save();
                Some(bytes)
            }
            _ => None,
        };

        let magnet_uri = (torrent_bytes.is_none() && item.kind == WebDownloadKind::Magnet)
            .then_some(item.source.as_str());

        let lease = self
            .aria2
            .acquire(&app.web_downloads.aria2_path, self.current_aria2_cap())
            .await?;

        let (b_pause, id_pause) = (self.clone(), item.id.clone());
        let is_pause = move || {
            matches!(
                b_pause
                    .web_downloads
                    .lock()
                    .running
                    .get(&id_pause)
                    .and_then(|r| r.stop),
                Some(StopReason::Pause { .. })
            )
        };

        let (b_prog, id_prog) = (self.clone(), item.id.clone());
        let on_progress = move |p: aria2::BtProgress| {
            let mut state = b_prog.web_downloads.lock();
            if let Some(entry) = state.active.iter_mut().find(|w| w.id == id_prog) {
                entry.aria2_gid = Some(p.gid);
                entry.state = if p.fetching_metadata {
                    WebDownloadState::FetchingMetadata
                } else {
                    WebDownloadState::Running
                };
                if !entry.custom_title
                    && let Some(t) = p.title
                    && !t.trim().is_empty()
                {
                    entry.title = t;
                }
                if let Some(out) = p.output_path {
                    entry.output_path = Some(out);
                }
                entry.bytes_done = p.bytes_done;
                if p.bytes_total > 0 {
                    entry.bytes_total = p.bytes_total;
                }
                entry.download_speed = p.download_speed;
                entry.eta_secs = (p.download_speed > 0 && entry.bytes_total > p.bytes_done)
                    .then(|| (entry.bytes_total - p.bytes_done) / p.download_speed);
                entry.peers = Some(p.peers);
                entry.seeders = p.seeders;
            }
        };

        let outcome = aria2::run_bt_download_with_client(
            &lease.client,
            torrent_bytes.as_deref(),
            magnet_uri,
            &item.destination,
            &item.selected_files,
            cancel,
            &is_pause,
            &on_progress,
        )
        .await?;

        match outcome {
            aria2::BtRunOutcome::Done {
                output_path,
                total_bytes,
                title,
            } => {
                if total_bytes > 0 {
                    let mut state = self.web_downloads.lock();
                    if let Some(entry) = state.active.iter_mut().find(|w| w.id == item.id) {
                        entry.bytes_total = total_bytes;
                        entry.bytes_done = total_bytes;
                    }
                }
                Ok(Some((
                    output_path,
                    if item.custom_title { None } else { title },
                )))
            }
            aria2::BtRunOutcome::Paused | aria2::BtRunOutcome::Canceled => Ok(None),
        }
    }
}
