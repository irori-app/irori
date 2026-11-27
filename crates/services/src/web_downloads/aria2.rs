//! On-demand `aria2c` supervisor, discovery, config generation, and BitTorrent
//! session orchestration (BEP-9 metadata preview, `followedBy` GID transitions,
//! and strict zero-seeding removal on `aria2.onBtDownloadComplete`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::process::{self, CancellationToken, CommandSpec, ConsoleWindow};
use irori_core::{Paths, host};
use serde_json::{Map, Value};
use tokio::net::TcpListener;
use tokio::process::Child;
use tokio::sync::{Mutex, broadcast};

pub use super::aria2_download::discover_aria2;
pub(crate) use super::aria2_rpc::{Aria2Client, Aria2Event, Aria2TaskStatus};
use super::model::{Aria2Status, TorrentPreview};
use super::torrent_meta;

/// Maximum ephemeral port bind/launch retries against TOCTOU races.
const STARTUP_PORT_RETRIES: usize = 4;
/// Maximum handshake polling iterations per launch attempt (30 * 100ms = 3s).
const HANDSHAKE_ATTEMPTS: usize = 30;
const HANDSHAKE_INTERVAL: Duration = Duration::from_millis(100);
/// Idle duration before shutting down an unused `aria2c` daemon.
const IDLE_SHUTDOWN: Duration = Duration::from_secs(30);
/// Default timeout for BEP-9 magnet metadata preview.
pub const MAGNET_PREVIEW_TIMEOUT: Duration = Duration::from_secs(45);
/// Polling interval for BT progress updates.
const PROGRESS_POLL: Duration = Duration::from_millis(500);

/// Outcome of a BitTorrent download run through `aria2c`.
#[derive(Debug, Clone)]
pub(crate) enum BtRunOutcome {
    Done {
        output_path: PathBuf,
        total_bytes: u64,
        title: Option<String>,
    },
    Paused,
    Canceled,
}

/// Progress update emitted while a BitTorrent download runs.
#[derive(Debug, Clone, Default)]
pub(crate) struct BtProgress {
    pub gid: String,
    pub fetching_metadata: bool,
    pub title: Option<String>,
    pub output_path: Option<PathBuf>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub download_speed: u64,
    pub peers: u32,
    pub seeders: Option<u32>,
}

/// Resolve the saved file or common top-level directory from `tellStatus` when
/// concrete payload metadata is known (never returning the bare download root).
pub(crate) fn resolve_bt_output_path_opt(status: &Aria2TaskStatus, dest: &Path) -> Option<PathBuf> {
    let selected: Vec<&PathBuf> = status
        .files
        .iter()
        .filter(|f| f.selected && !f.path.as_os_str().is_empty())
        .map(|f| &f.path)
        .collect();
    if let [single] = selected.as_slice() {
        return Some((*single).clone());
    }
    let base = if status.dir.as_os_str().is_empty() {
        dest
    } else {
        status.dir.as_path()
    };
    if let Some(name) = &status.bittorrent_name
        && !name.trim().is_empty()
    {
        let mut comps = Path::new(name).components();
        if matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none() {
            let candidate = base.join(name);
            if candidate.exists() || selected.is_empty() {
                return Some(candidate);
            }
        }
    }
    if let Some(first) = selected.first()
        && let Ok(rel) = first.strip_prefix(base)
        && let Some(first_comp) = rel.components().next()
    {
        return Some(base.join(first_comp.as_os_str()));
    }
    None
}

/// Resolve the saved file or common top-level directory from `tellStatus`.
pub(crate) fn resolve_bt_output_path(status: &Aria2TaskStatus, dest: &Path) -> PathBuf {
    resolve_bt_output_path_opt(status, dest).unwrap_or_else(|| {
        if status.dir.as_os_str().is_empty() {
            dest.to_path_buf()
        } else {
            status.dir.clone()
        }
    })
}

/// Fetch a `magnet:` link's `.torrent` metadata via BEP-9 without downloading
/// the payload, so the UI can display file checkboxes before enqueuing.
pub(crate) async fn fetch_magnet_metadata_with_client(
    client: &Aria2Client,
    magnet_uri: &str,
    scratch_dir: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<TorrentPreview> {
    tokio::fs::create_dir_all(scratch_dir)
        .await
        .with_context(|| format!("creating {}", scratch_dir.display()))?;
    let mut events = client.subscribe();
    let mut opts = Map::new();
    opts.insert(
        "dir".into(),
        Value::String(scratch_dir.to_string_lossy().into_owned()),
    );
    opts.insert("bt-metadata-only".into(), Value::String("true".into()));
    opts.insert("bt-save-metadata".into(), Value::String("true".into()));
    opts.insert("follow-torrent".into(), Value::String("false".into()));

    let gid = client.add_uri(&[magnet_uri], opts).await?;
    let wait_result = tokio::time::timeout(timeout, async {
        let mut poll = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                () = cancel.cancelled() => bail!("Metadata fetch cancelled"),
                ev = events.recv() => match ev {
                    Ok(Aria2Event::DownloadComplete { gid: g }) if g == gid => return Ok(()),
                    Ok(Aria2Event::DownloadError { gid: g }) if g == gid => {
                        let st = client.tell_status(&gid).await.ok();
                        let msg = st.and_then(|s| s.error_message)
                            .unwrap_or_else(|| "Failed to fetch magnet metadata".into());
                        bail!("{msg}");
                    }
                    Ok(Aria2Event::DownloadStop { gid: g }) if g == gid => {
                        bail!("Metadata fetch stopped");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        bail!("aria2 connection closed during metadata fetch");
                    }
                    _ => {}
                },
                _ = poll.tick() => {
                    if let Ok(st) = client.tell_status(&gid).await {
                        match st.status.as_str() {
                            "complete" => return Ok(()),
                            "error" => {
                                let msg = st.error_message
                                    .unwrap_or_else(|| "Failed to fetch magnet metadata".into());
                                bail!("{msg}");
                            }
                            "removed" => bail!("Metadata fetch cancelled"),
                            _ => {}
                        }
                    }
                }
            }
        }
    })
    .await;

    client.stop_and_purge(&gid).await;

    let outcome = match wait_result {
        Ok(inner) => inner,
        Err(_) => Err(anyhow::anyhow!(
            "Timed out fetching magnet metadata (no peers responded)"
        )),
    };

    let preview = match outcome {
        Ok(()) => read_saved_torrent_from_dir(scratch_dir).await,
        Err(err) => Err(err),
    };
    let _ = tokio::fs::remove_dir_all(scratch_dir).await;
    preview
}

async fn read_saved_torrent_from_dir(dir: &Path) -> Result<TorrentPreview> {
    let mut rd = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("reading {}", dir.display()))?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
        {
            let bytes = tokio::fs::read(&path)
                .await
                .with_context(|| format!("reading {}", path.display()))?;
            let mut preview = torrent_meta::parse_torrent(&bytes)?;
            preview.torrent_bytes = Some(bytes);
            return Ok(preview);
        }
    }
    bail!("aria2 finished metadata fetch but saved no .torrent file")
}

/// Run a BitTorrent download (`.torrent` bytes or `magnet:` URI) to completion,
/// handling `followedBy` GID transitions and immediate zero-seeding removal on
/// `aria2.onBtDownloadComplete` / `complete`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_bt_download_with_client(
    client: &Aria2Client,
    torrent_bytes: Option<&[u8]>,
    magnet_uri: Option<&str>,
    dest: &Path,
    selected_files: &[usize],
    cancel: &CancellationToken,
    is_pause: &(dyn Fn() -> bool + Send + Sync),
    on_progress: &(dyn Fn(BtProgress) + Send + Sync),
) -> Result<BtRunOutcome> {
    tokio::fs::create_dir_all(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut events = client.subscribe();
    let mut opts = Map::new();
    opts.insert(
        "dir".into(),
        Value::String(dest.to_string_lossy().into_owned()),
    );
    opts.insert("seed-time".into(), Value::String("0".into()));
    opts.insert("bt-seed-unverified".into(), Value::String("false".into()));
    opts.insert(
        "bt-remove-unselected-file".into(),
        Value::String("true".into()),
    );
    opts.insert("continue".into(), Value::String("true".into()));
    let select_str = torrent_meta::format_select_file(selected_files);
    if let Some(sel) = &select_str {
        opts.insert("select-file".into(), Value::String(sel.clone()));
    }

    let mut active_gid = if let Some(bytes) = torrent_bytes {
        client.add_torrent(bytes, opts).await?
    } else if let Some(uri) = magnet_uri {
        opts.insert("bt-save-metadata".into(), Value::String("true".into()));
        opts.insert("follow-torrent".into(), Value::String("true".into()));
        client.add_uri(&[uri], opts).await?
    } else {
        bail!("BitTorrent download has neither .torrent metadata nor magnet URI");
    };

    let mut fetching_metadata = torrent_bytes.is_none();
    let mut poll = tokio::time::interval(PROGRESS_POLL);

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                client.stop_and_purge(&active_gid).await;
                return Ok(if is_pause() { BtRunOutcome::Paused } else { BtRunOutcome::Canceled });
            }
            ev = events.recv() => match ev {
                Ok(Aria2Event::BtDownloadComplete { gid }) if gid == active_gid => {
                    let status = client.tell_status(&active_gid).await.unwrap_or_default();
                    client.stop_and_purge(&active_gid).await;
                    return Ok(finish_bt(&status, dest));
                }
                Ok(Aria2Event::DownloadComplete { gid }) if gid == active_gid => {
                    let status = client.tell_status(&active_gid).await?;
                    if let Some(next_gid) = follow_if_needed(client, &active_gid, &status, &select_str).await {
                        active_gid = next_gid;
                        fetching_metadata = false;
                        continue;
                    }
                    client.stop_and_purge(&active_gid).await;
                    return Ok(finish_bt(&status, dest));
                }
                Ok(Aria2Event::DownloadError { gid }) if gid == active_gid => {
                    let status = client.tell_status(&active_gid).await.ok();
                    client.stop_and_purge(&active_gid).await;
                    let msg = status.and_then(|s| s.error_message)
                        .unwrap_or_else(|| "BitTorrent download failed".into());
                    bail!("{msg}");
                }
                Ok(Aria2Event::DownloadStop { gid }) if gid == active_gid => {
                    client.remove_download_result(&active_gid).await;
                    return Ok(if is_pause() { BtRunOutcome::Paused } else { BtRunOutcome::Canceled });
                }
                Err(broadcast::error::RecvError::Closed) => {
                    bail!("Lost connection to local aria2 process");
                }
                _ => {}
            },
            _ = poll.tick() => {
                let status = client.tell_status(&active_gid).await?;
                if let Some(next_gid) = follow_if_needed(client, &active_gid, &status, &select_str).await {
                    active_gid = next_gid;
                    fetching_metadata = false;
                    continue;
                }
                match status.status.as_str() {
                    "complete" => {
                        client.stop_and_purge(&active_gid).await;
                        return Ok(finish_bt(&status, dest));
                    }
                    "error" => {
                        client.stop_and_purge(&active_gid).await;
                        let msg = status.error_message
                            .unwrap_or_else(|| "BitTorrent download failed".into());
                        bail!("{msg}");
                    }
                    "removed" => {
                        client.remove_download_result(&active_gid).await;
                        return Ok(if is_pause() { BtRunOutcome::Paused } else { BtRunOutcome::Canceled });
                    }
                    _ => {
                        if status.bittorrent_name.is_some() && status.total_length > 0 {
                            fetching_metadata = false;
                        }
                        let output_path = resolve_bt_output_path_opt(&status, dest);
                        on_progress(BtProgress {
                            gid: active_gid.clone(),
                            fetching_metadata,
                            title: status.bittorrent_name,
                            output_path,
                            bytes_done: status.completed_length,
                            bytes_total: status.total_length,
                            download_speed: status.download_speed,
                            peers: status.connections,
                            seeders: status.num_seeders,
                        });
                    }
                }
            }
        }
    }
}

async fn follow_if_needed(
    client: &Aria2Client,
    old_gid: &str,
    status: &Aria2TaskStatus,
    select_str: &Option<String>,
) -> Option<String> {
    let next_gid = status.followed_by.first()?.clone();
    client.remove_download_result(old_gid).await;
    if let Some(sel) = select_str {
        let mut ch = Map::new();
        ch.insert("select-file".into(), Value::String(sel.clone()));
        let _ = client.change_option(&next_gid, ch).await;
    }
    Some(next_gid)
}

fn finish_bt(status: &Aria2TaskStatus, dest: &Path) -> BtRunOutcome {
    BtRunOutcome::Done {
        output_path: resolve_bt_output_path(status, dest),
        total_bytes: status.total_length.max(status.completed_length),
        title: status.bittorrent_name.clone(),
    }
}

struct RunningAria2 {
    child: Child,
    client: Arc<Aria2Client>,
    conf_path: PathBuf,
}

impl Drop for RunningAria2 {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.conf_path);
    }
}

/// Lease guard keeping the on-demand `aria2c` daemon alive while active BT tasks
/// or metadata previews run, and scheduling idle shutdown once all leases drop.
pub(crate) struct Aria2Lease {
    supervisor: Arc<Aria2Supervisor>,
    pub client: Arc<Aria2Client>,
}

impl Drop for Aria2Lease {
    fn drop(&mut self) {
        if self.supervisor.leases.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.supervisor.schedule_idle_shutdown();
        }
    }
}

/// On-demand supervisor owning the local `aria2c` daemon.
#[derive(Default)]
pub(crate) struct Aria2Supervisor {
    running: Mutex<Option<RunningAria2>>,
    leases: AtomicUsize,
    idle_cancel: parking_lot::Mutex<Option<CancellationToken>>,
}

impl Aria2Supervisor {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn has_active_leases(&self) -> bool {
        self.leases.load(Ordering::Acquire) > 0
    }

    /// Acquire a live `aria2c` client, launching the daemon lazily if needed.
    pub(crate) async fn acquire(
        self: &Arc<Self>,
        custom_path: &str,
        limit_bytes_per_sec: u64,
    ) -> Result<Aria2Lease> {
        if let Some(timer) = self.idle_cancel.lock().take() {
            timer.cancel();
        }
        let mut guard = self.running.lock().await;
        if let Some(existing) = guard.as_mut() {
            let exited = existing.child.try_wait().ok().flatten().is_some();
            if !exited && existing.client.is_alive() {
                self.leases.fetch_add(1, Ordering::AcqRel);
                return Ok(Aria2Lease {
                    supervisor: self.clone(),
                    client: existing.client.clone(),
                });
            }
            let _ = existing.child.kill().await;
            *guard = None;
        }

        let (program, on_host) = match discover_aria2(custom_path).await {
            Aria2Status::Ready { path, on_host } => (path, on_host),
            Aria2Status::Missing { install_hint, .. } => {
                bail!("aria2c was not found. {install_hint}")
            }
        };

        let launched = spawn_and_connect(&program, on_host, limit_bytes_per_sec).await?;
        let client = launched.client.clone();
        *guard = Some(launched);
        self.leases.fetch_add(1, Ordering::AcqRel);
        Ok(Aria2Lease {
            supervisor: self.clone(),
            client,
        })
    }

    /// Push an updated download bandwidth cap to `aria2c` if currently running.
    pub(crate) async fn set_global_download_limit(&self, bytes_per_sec: u64) {
        let client = {
            let guard = self.running.lock().await;
            guard.as_ref().map(|r| r.client.clone())
        };
        if let Some(client) = client
            && client.is_alive()
            && let Err(err) = client.change_global_download_limit(bytes_per_sec).await
        {
            tracing::debug!("updating aria2 download speed limit: {err:#}");
        }
    }

    /// Shut down `aria2c` immediately (called on app quit, offline mode, or opt-out).
    pub(crate) async fn shutdown(&self) {
        if let Some(timer) = self.idle_cancel.lock().take() {
            timer.cancel();
        }
        let Some(mut running) = self.running.lock().await.take() else {
            return;
        };
        if running.client.is_alive() {
            let _ =
                tokio::time::timeout(Duration::from_secs(2), running.client.force_shutdown()).await;
        }
        if tokio::time::timeout(Duration::from_secs(2), running.child.wait())
            .await
            .is_err()
        {
            let _ = running.child.kill().await;
            let _ = running.child.wait().await;
        }
        let _ = tokio::fs::remove_file(&running.conf_path).await;
    }

    fn schedule_idle_shutdown(self: &Arc<Self>) {
        let cancel = CancellationToken::new();
        if let Some(prev) = self.idle_cancel.lock().replace(cancel.clone()) {
            prev.cancel();
        }
        let sup = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(IDLE_SHUTDOWN) => {
                    if sup.leases.load(Ordering::Acquire) == 0 {
                        sup.shutdown().await;
                    }
                }
            }
        });
    }
}

/// Build the `aria2.conf` content with loopback RPC secret and zero-seed defaults.
pub(crate) fn build_aria2_conf(
    port: u16,
    secret: &str,
    cache_dir: &Path,
    limit_bytes_per_sec: u64,
) -> String {
    let dht4 = cache_dir.join("dht.dat");
    let dht6 = cache_dir.join("dht6.dat");
    format!(
        "enable-rpc=true\n\
         rpc-listen-all=false\n\
         rpc-listen-port={port}\n\
         rpc-secret={secret}\n\
         seed-time=0\n\
         bt-seed-unverified=false\n\
         bt-save-metadata=true\n\
         bt-remove-unselected-file=true\n\
         http-accept-gzip=false\n\
         file-allocation=none\n\
         enable-dht=true\n\
         bt-enable-lpd=true\n\
         dht-file-path={}\n\
         dht-file-path6={}\n\
         bt-stop-timeout=300\n\
         auto-file-renaming=false\n\
         allow-overwrite=false\n\
         continue=true\n\
         max-overall-download-limit={limit_bytes_per_sec}\n",
        dht4.display(),
        dht6.display()
    )
}

/// Build the `CommandSpec` to spawn `aria2c` natively or on a Flatpak host.
/// The RPC secret is never placed in `argv`; only `--conf-path` is passed.
pub(crate) fn build_spawn_spec(
    program: &Path,
    conf_path: &Path,
    on_host: bool,
    pid: u32,
) -> CommandSpec {
    let spec = CommandSpec::new(program)
        .arg(format!("--conf-path={}", conf_path.display()))
        .console(ConsoleWindow::Hidden);
    if on_host {
        host::tied_command(spec)
    } else {
        spec.arg(format!("--stop-with-process={pid}"))
    }
}

async fn spawn_and_connect(
    program: &Path,
    on_host: bool,
    limit_bytes_per_sec: u64,
) -> Result<RunningAria2> {
    let cache_dir = Paths::get().cache.join("aria2");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .with_context(|| format!("creating {}", cache_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ =
            tokio::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o700)).await;
    }

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..STARTUP_PORT_RETRIES {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Could not bind an ephemeral loopback port for aria2")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let secret = format!(
            "{:032x}{:032x}",
            uuid::Uuid::new_v4().as_u128(),
            uuid::Uuid::new_v4().as_u128()
        );
        let conf_path = cache_dir.join(format!("rpc-{}.conf", uuid::Uuid::new_v4().simple()));
        write_secret_conf(
            &conf_path,
            &build_aria2_conf(port, &secret, &cache_dir, limit_bytes_per_sec),
        )?;

        let spec = build_spawn_spec(program, &conf_path, on_host, std::process::id());
        let mut child = match process::spawn(&spec, None) {
            Ok(c) => c,
            Err(err) => {
                let _ = tokio::fs::remove_file(&conf_path).await;
                return Err(err).context("Could not start aria2c");
            }
        };

        let ws_url = format!("ws://127.0.0.1:{port}/jsonrpc");
        let mut connected: Option<Arc<Aria2Client>> = None;
        for _ in 0..HANDSHAKE_ATTEMPTS {
            if let Ok(Some(status)) = child.try_wait() {
                last_err = Some(anyhow::anyhow!(
                    "aria2c exited early on port {port} ({status})"
                ));
                break;
            }
            if let Ok(client) = Aria2Client::connect(&ws_url, secret.clone()).await
                && client.get_version().await.is_ok()
            {
                connected = Some(Arc::new(client));
                break;
            }
            tokio::time::sleep(HANDSHAKE_INTERVAL).await;
        }

        if let Some(client) = connected {
            let _ = tokio::fs::remove_file(&conf_path).await;
            return Ok(RunningAria2 {
                child,
                client,
                conf_path,
            });
        }

        tracing::debug!(
            "aria2 startup attempt {} on port {port} failed",
            attempt + 1
        );
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = tokio::fs::remove_file(&conf_path).await;
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Could not connect to local aria2c RPC")))
}

fn write_secret_conf(path: &Path, content: &str) -> Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all()?;
    Ok(())
}
