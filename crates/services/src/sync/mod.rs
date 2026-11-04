//! Library sync between computers (a desktop and a Steam Deck, Linux and
//! Windows on one PC) through a folder something else syncs, or WebDAV.
//!
//! Each device writes one file, `<device id>.json` in the `Irori sync`
//! folder, with everything it knows, and reads the others'. What is merged
//! and how is in [`merge`]. Synced: favourites, hidden games, ratings, play
//! statuses, categories, custom titles and web art, yearly goals, playtime
//! and sessions. Install paths, per-game and app settings, recently played
//! and store sign-ins stay on each device.
//!
//! Changes are noted on every periodic tick while sync is on, also when the
//! other devices are out of reach, so they keep the time they were made.
//! The state between syncs is `state/sync.json`; the WebDAV login is kept
//! with `irori_secrets`.

pub mod merge;
pub mod target;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::{Event, Notice, NoticeLevel, Paths, SyncMode, json_store, util};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{Backend, UserData, sessions};
pub use merge::{ConflictField, Millis};
use merge::{Local, Registers, Shared};
use target::Target;
pub use target::WebDavLogin;

/// Marks a file as an Irori sync file.
const FORMAT: &str = "irori-sync";
/// Newest layout this version reads and writes.
const VERSION: u32 = 1;
/// How often other devices' changes are fetched while nothing changes here.
const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Conflicts kept for the user to read.
const MAX_CONFLICTS: usize = 30;

/// Another device that syncs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncDevice {
    pub id: String,
    pub name: String,
    /// `linux`, `windows`, `macos`.
    pub os: String,
    pub irori_version: String,
    /// RFC 3339 time it last wrote its file.
    pub updated: String,
}

/// A change made here that lost to one made at about the same time on
/// another device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub field: ConflictField,
    /// The game key, category name or year.
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The device whose change was kept, by name.
    pub device: String,
    /// RFC 3339 time of the change that was kept.
    pub at: String,
}

/// How the last sync went.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncReport {
    /// RFC 3339 time it ran.
    pub at: String,
    /// Why it failed.
    pub error: Option<String>,
    pub devices: Vec<SyncDevice>,
    /// Values other devices changed here.
    pub changed: usize,
    pub sessions_added: usize,
    /// Files that could not be used: unreadable, or from a newer Irori.
    pub skipped: Vec<String>,
    /// Some files came from a newer Irori.
    pub newer_version: bool,
}

/// What the settings page shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncStatus {
    pub device_id: String,
    pub device_name: String,
    pub last: Option<SyncReport>,
    /// RFC 3339 time of the last sync that worked.
    pub last_success: Option<String>,
    /// Newest first.
    pub conflicts: Vec<SyncConflict>,
    pub running: bool,
}

/// What this device keeps between syncs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SyncState {
    device_id: String,
    registers: Registers,
    known_playtime: BTreeSet<String>,
    clock: Millis,
    synced: Option<Millis>,
    last: Option<SyncReport>,
    last_success: Option<String>,
    conflicts: Vec<SyncConflict>,
}

/// A device's file in the sync folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct DeviceFile {
    format: String,
    version: u32,
    device: SyncDevice,
    #[serde(flatten)]
    shared: Shared,
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("sync.json")
}

fn login_file() -> PathBuf {
    Paths::get().config.join("sync").join("webdav.json")
}

/// A new device id: random enough that two devices never share one.
fn new_device_id() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut id = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        h.write_u32(std::process::id());
        id.push_str(&format!("{:016x}", h.finish()));
    }
    id
}

/// What other devices call this one when the user hasn't named it.
pub fn default_device_name() -> String {
    sysinfo::System::host_name()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Irori".into())
}

fn os() -> &'static str {
    std::env::consts::OS
}

fn now_millis() -> Millis {
    chrono::Utc::now().timestamp_millis()
}

fn rfc(ms: Millis) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// Sync's share of the backend.
pub struct SyncService {
    state: Mutex<SyncState>,
    /// One sync at a time.
    running: tokio::sync::Mutex<()>,
    busy: AtomicBool,
    /// The library data as of the last time changes were noted.
    noted: Mutex<Option<Arc<UserData>>>,
    /// When the last sync was tried.
    last_try: Mutex<Option<std::time::Instant>>,
    /// Held while a session's playtime and history are written, so a sync
    /// never sees one without the other.
    playtime: Mutex<()>,
    /// The user was told sync failed; not again until it works.
    told: AtomicBool,
}

impl SyncService {
    pub(crate) fn load() -> Self {
        let mut state: SyncState = json_store::read_or_default(&state_file());
        if state.device_id.is_empty() {
            state.device_id = new_device_id();
        }
        Self {
            state: Mutex::new(state),
            running: tokio::sync::Mutex::new(()),
            busy: AtomicBool::new(false),
            noted: Mutex::new(None),
            last_try: Mutex::new(None),
            playtime: Mutex::new(()),
            told: AtomicBool::new(false),
        }
    }

    /// Hold while recording a session's playtime and history together.
    pub(crate) fn playtime_guard(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.playtime.lock()
    }

    fn save(&self, state: &SyncState) {
        if let Err(e) = json_store::write(&state_file(), state) {
            tracing::warn!("saving the sync state: {e:#}");
        }
    }
}

impl Backend {
    /// How sync stands, for the settings page.
    pub fn sync_status(&self) -> SyncStatus {
        let state = self.sync.state.lock();
        let name = self.settings.app().sync.device_name.trim().to_string();
        SyncStatus {
            device_id: state.device_id.clone(),
            device_name: if name.is_empty() {
                default_device_name()
            } else {
                name
            },
            last: state.last.clone(),
            last_success: state.last_success.clone(),
            conflicts: state.conflicts.clone(),
            running: self.sync.busy.load(Ordering::Relaxed),
        }
    }

    /// Forget the conflicts the user has read.
    pub fn clear_sync_conflicts(&self) {
        let mut state = self.sync.state.lock();
        state.conflicts.clear();
        self.sync.save(&state);
        drop(state);
        self.bus.emit(Event::SettingsChanged);
    }

    /// The saved WebDAV user name, if any. Reads a secret: call it off the
    /// UI thread.
    pub fn webdav_username(&self) -> Option<String> {
        irori_secrets::load::<WebDavLogin>(&login_file())
            .map(|l| l.username)
            .filter(|u| !u.is_empty())
    }

    /// Keep the WebDAV login, or forget it with `None`.
    pub fn set_webdav_login(&self, login: Option<WebDavLogin>) -> Result<()> {
        match login {
            Some(login) => irori_secrets::write_json(&login_file(), &login)
                .context("The WebDAV login couldn’t be saved"),
            None => match std::fs::remove_file(login_file()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        }
    }

    fn sync_target(&self) -> Result<Option<Target>> {
        let prefs = self.settings.app().sync.clone();
        Ok(match prefs.mode {
            SyncMode::Off => None,
            SyncMode::Folder => Some(Target::Folder(irori_core::paths::expand_home(
                prefs.folder.trim(),
            ))),
            SyncMode::Webdav => {
                let url = prefs.webdav_url.trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    bail!("Enter the address of a WebDAV folder");
                }
                let login = irori_secrets::read_json::<WebDavLogin>(&login_file())
                    .context("The WebDAV login couldn’t be opened")?
                    .unwrap_or_default();
                Some(Target::WebDav { url, login })
            }
        })
    }

    /// Note changes made here, and sync when they or the interval call for
    /// it. Runs on the periodic tick.
    pub(crate) async fn sync_when_due(&self) {
        if self.settings.app().sync.mode == SyncMode::Off {
            return;
        }
        let noted_changes = self.note_sync_changes();
        let due = self
            .sync
            .last_try
            .lock()
            .is_none_or(|t| t.elapsed() >= INTERVAL);
        if noted_changes || due {
            // Failures are reported through the status and a notice.
            let _ = self.sync_now().await;
        }
    }

    /// Record what changed here since the last look. Returns whether
    /// anything did.
    fn note_sync_changes(&self) -> bool {
        let data = self.userdata.snapshot();
        {
            let noted = self.sync.noted.lock();
            if noted.as_ref().is_some_and(|n| Arc::ptr_eq(n, &data)) {
                return false;
            }
        }
        let mut state = self.sync.state.lock();
        let dating = merge::Dating::for_now(state.synced, now_millis(), state.clock);
        let device = state.device_id.clone();
        let known = state.known_playtime.clone();
        let changed = merge::record(&mut state.registers, &data, &known, &device, dating);
        if changed > 0 {
            state.clock = state.clock.max(state.registers.newest());
            self.sync.save(&state);
        }
        *self.sync.noted.lock() = Some(data);
        changed > 0
    }

    /// Sync now: read the other devices' files, merge, apply and publish.
    pub async fn sync_now(&self) -> Result<SyncReport> {
        let _one = self.sync.running.lock().await;
        *self.sync.last_try.lock() = Some(std::time::Instant::now());
        self.sync.busy.store(true, Ordering::Relaxed);
        let result = self.sync_inner().await;
        self.sync.busy.store(false, Ordering::Relaxed);
        let report = match &result {
            Ok(report) => {
                self.sync.told.store(false, Ordering::Relaxed);
                report.clone()
            }
            Err(e) => {
                tracing::warn!("sync: {e:#}");
                if !self.sync.told.swap(true, Ordering::Relaxed) {
                    self.bus.notice(
                        Notice::new(NoticeLevel::Warning, "Library sync failed")
                            .message(format!("{e:#}"))
                            .keys("shell.notice.sync_failed", None, &[]),
                    );
                }
                SyncReport {
                    at: util::now_rfc3339(),
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                }
            }
        };
        {
            let mut state = self.sync.state.lock();
            if report.error.is_some() {
                // Keep the devices last seen.
                let devices = state
                    .last
                    .as_ref()
                    .map(|l| l.devices.clone())
                    .unwrap_or_default();
                state.last = Some(SyncReport {
                    devices,
                    ..report.clone()
                });
            } else {
                state.last = Some(report.clone());
                state.last_success = Some(report.at.clone());
            }
            self.sync.save(&state);
        }
        self.bus.emit(Event::SettingsChanged);
        result
    }

    async fn sync_inner(&self) -> Result<SyncReport> {
        let b = self.clone();
        let target = self
            .spawn_blocking(move || b.sync_target())
            .await??
            .context("Sync is off")?;
        let (me, own) = {
            let state = self.sync.state.lock();
            (state.device_id.clone(), format!("{}.json", state.device_id))
        };

        // Every other device's file.
        let mut report = SyncReport {
            at: util::now_rfc3339(),
            ..Default::default()
        };
        let mut remotes = Vec::new();
        for name in target.list().await? {
            if name == own {
                continue;
            }
            let file = match target.read(&name).await.and_then(|bytes| {
                serde_json::from_slice::<DeviceFile>(&bytes).context("not a sync file")
            }) {
                Ok(file) if file.format == FORMAT => file,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("sync: skipping {name}: {e:#}");
                    report.skipped.push(name);
                    continue;
                }
            };
            if file.version > VERSION {
                report.newer_version = true;
                report.skipped.push(file.device.name.clone());
                continue;
            }
            if file.device.id != me && !report.devices.iter().any(|d| d.id == file.device.id) {
                report.devices.push(file.device.clone());
            }
            remotes.push(file);
        }

        // Merge and apply, with the session history and playtime held still.
        let b = self.clone();
        let (merged, bytes) = self
            .spawn_blocking(move || b.merge_and_apply(&remotes))
            .await??;
        report.changed = merged.changed;
        report.sessions_added = merged.sessions_added;
        if merged.changed > 0 {
            self.rebuild_library();
        }
        target.write(&own, bytes).await?;
        Ok(report)
    }

    /// Merge `remotes` into this device's data and state; returns the merge
    /// and this device's new file.
    fn merge_and_apply(&self, remotes: &[DeviceFile]) -> Result<(merge::Merged, Vec<u8>)> {
        let _playtime = self.sync.playtime_guard();
        let local_sessions = sessions::read_all();
        let names: BTreeMap<String, String> = remotes
            .iter()
            .map(|f| (f.device.id.clone(), f.device.name.clone()))
            .collect();
        let shared: Vec<&Shared> = remotes.iter().map(|f| &f.shared).collect();
        let mut state = self.sync.state.lock();
        let mut result = None;
        self.userdata.update_if(None, |d| {
            let merged = merge::merge(
                Local {
                    device: &state.device_id,
                    data: d,
                    sessions: &local_sessions,
                    registers: &state.registers,
                    known_playtime: &state.known_playtime,
                    now: now_millis(),
                    clock: state.clock,
                    synced: state.synced,
                },
                &shared,
            );
            let apply = merged.changed > 0;
            if apply {
                *d = merged.data.clone();
            }
            result = Some(merged);
            apply
        });
        let merged = result.expect("the update ran");
        if merged.sessions_added > 0 {
            sessions::replace_all(&merged.sessions)?;
        }
        state.registers = merged.registers.clone();
        state.known_playtime = merged.data.playtime.keys().cloned().collect();
        state.clock = merged.clock;
        state.synced = Some(merged.clock);
        for c in merged.conflicts.iter().rev() {
            state.conflicts.insert(
                0,
                SyncConflict {
                    field: c.field,
                    subject: c.subject.clone(),
                    category: c.category.clone(),
                    device: names.get(&c.kept_from).cloned().unwrap_or_default(),
                    at: rfc(c.kept_at),
                },
            );
        }
        state.conflicts.truncate(MAX_CONFLICTS);
        self.sync.save(&state);
        *self.sync.noted.lock() = Some(self.userdata.snapshot());

        let name = self.settings.app().sync.device_name.trim().to_string();
        let file = DeviceFile {
            format: FORMAT.into(),
            version: VERSION,
            device: SyncDevice {
                id: state.device_id.clone(),
                name: if name.is_empty() {
                    default_device_name()
                } else {
                    name
                },
                os: os().into(),
                irori_version: env!("CARGO_PKG_VERSION").into(),
                updated: util::now_rfc3339(),
            },
            shared: Shared {
                registers: merged.registers.clone(),
                playtime: merged.playtime.clone(),
                sessions: merged.sessions.clone(),
            },
        };
        let bytes = serde_json::to_vec(&file)?;
        Ok((merged, bytes))
    }
}

#[cfg(test)]
mod tests;
