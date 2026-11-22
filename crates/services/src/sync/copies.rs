//! Cloud copies: Irori's scheduled backups, Ludusavi's save backups and the
//! user's screenshots, copied to the sync target (a folder, WebDAV or an
//! rclone remote), and brought back on another computer.
//!
//! Each computer writes only under `Irori sync/devices/<device id>/`:
//! `device.json` (its name and system), `backups/`, `saves/` (Ludusavi's
//! backup folder as it is) and `screenshots/<game>/`. Only files that are
//! new or changed since they were last sent to this target are sent
//! (`state/cloud-copies.json`), so a copy costs nothing when nothing
//! changed. Each kind is Off, Manual ("Send now") or Automatic: automatic
//! copies ride the periodic tick, at most every half hour and never while a
//! game runs, and wait longer after each failure. Store sign-ins never go:
//! Irori's backups leave them out.
//!
//! [`Backend::cloud_copy_sizes`] says how much each kind holds on this
//! computer and how much of it hasn't reached the current target yet, for
//! the Sync page's sizes and space bar.
//!
//! Bringing back never overwrites: backups land in the backup folder (the
//! Backups list then offers to restore them), saves in Ludusavi's backup
//! folder (only games it has no backups of), screenshots in
//! `Pictures/Irori/<computer>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::{CopyMode, Paths, json_store};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::target::Target;
use super::{SyncDevice, default_device_name, os};
use crate::Backend;

/// What is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyKind {
    Backups,
    Saves,
    Screenshots,
}

impl CopyKind {
    pub const ALL: [CopyKind; 3] = [CopyKind::Backups, CopyKind::Saves, CopyKind::Screenshots];

    /// Its folder under a device's.
    fn dir(self) -> &'static str {
        match self {
            CopyKind::Backups => "backups",
            CopyKind::Saves => "saves",
            CopyKind::Screenshots => "screenshots",
        }
    }
}

/// How often automatic copies run.
const EVERY: Duration = Duration::from_secs(30 * 60);
/// Waits after failures in a row, then the last one again.
const RETRY: [Duration; 4] = [
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];

/// A local file to copy: its path under the kind's folder and what tells a
/// change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
}

/// What a file was when it was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sent {
    pub size: u64,
    pub modified: i64,
}

/// How the last copy of a kind went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyReport {
    /// RFC 3339 time it ran.
    pub at: String,
    pub sent: usize,
    /// Files already sent and unchanged.
    pub unchanged: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// Target key → kind → relative path → what was sent.
    sent: BTreeMap<String, BTreeMap<CopyKind, BTreeMap<String, Sent>>>,
    last: BTreeMap<CopyKind, CopyReport>,
}

/// How much one kind of cloud copy holds on this computer, and how much of
/// it waits to be sent to the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopySize {
    pub kind: CopyKind,
    pub files: usize,
    pub bytes: u64,
    pub waiting_files: usize,
    pub waiting_bytes: u64,
}

/// [`CopySize`] of `local` files of `kind`, `sent` being what the target
/// already has.
pub fn size_of(kind: CopyKind, local: &[LocalFile], sent: &BTreeMap<String, Sent>) -> CopySize {
    let waiting = to_send(local, sent);
    CopySize {
        kind,
        files: local.len(),
        bytes: local.iter().map(|f| f.size).sum(),
        waiting_files: waiting.len(),
        waiting_bytes: waiting.iter().map(|f| f.size).sum(),
    }
}

/// The cloud copies' state.
#[derive(Default)]
struct CopiesService {
    state: Mutex<Option<State>>,
    /// One copy or bring-back at a time.
    running: tokio::sync::Mutex<()>,
    /// When the next automatic copy may run, and failures in a row.
    schedule: Mutex<(Option<std::time::Instant>, usize)>,
}

/// The one cloud copies service of the process: its state is a file of
/// Irori's state folder, like sync's.
static SERVICE: once_cell::sync::Lazy<CopiesService> = once_cell::sync::Lazy::new(Default::default);

fn service() -> &'static CopiesService {
    &SERVICE
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("cloud-copies.json")
}

impl CopiesService {
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock();
        let state = state.get_or_insert_with(|| json_store::read_or_default(&state_file()));
        let out = f(state);
        if let Err(e) = json_store::write(&state_file(), &*state) {
            tracing::warn!("saving the cloud copies' state: {e:#}");
        }
        out
    }
}

/// The files under `root`, with paths relative to it, skipping links and
/// files being written.
pub fn walk(root: &Path) -> Vec<LocalFile> {
    fn go(dir: &Path, rel: &str, out: &mut Vec<LocalFile>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.ends_with(".tmp") || name.ends_with(".irori-part") {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if meta.is_dir() {
                go(&entry.path(), &child, out);
            } else if meta.is_file() {
                out.push(LocalFile {
                    rel: child,
                    path: entry.path(),
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs() as i64),
                });
            }
        }
    }
    let mut out = Vec::new();
    go(root, "", &mut out);
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// The files of `local` not yet sent as they are now.
pub fn to_send<'a>(local: &'a [LocalFile], sent: &BTreeMap<String, Sent>) -> Vec<&'a LocalFile> {
    local
        .iter()
        .filter(|f| {
            sent.get(&f.rel)
                != Some(&Sent {
                    size: f.size,
                    modified: f.modified,
                })
        })
        .collect()
}

/// Which saves folders (one per game, the first part of each path) may be
/// brought back: those `local` (Ludusavi's backup folder) has nothing of,
/// so a game's own backups and its `mapping.yaml` never mix with another
/// computer's.
pub fn saves_to_bring(remote: &[String], local_root: &Path) -> Vec<String> {
    let mut games: Vec<String> = remote
        .iter()
        .filter_map(|rel| rel.split('/').next().map(str::to_string))
        .collect();
    games.sort();
    games.dedup();
    games.retain(|g| !local_root.join(g).exists());
    games
}

/// A plain name for a folder: the game key without `:` and other
/// characters a cloud may refuse.
fn folder_name(key: &str) -> String {
    irori_core::paths::sanitize(&key.replace(':', "-"))
}

/// What the settings page shows of one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyStatus {
    pub kind: CopyKind,
    pub mode: CopyMode,
    pub last: Option<CopyReport>,
}

impl Backend {
    fn copy_mode(&self, kind: CopyKind) -> CopyMode {
        let prefs = self.settings.app().sync.copies.clone();
        match kind {
            CopyKind::Backups => prefs.backups,
            CopyKind::Saves => prefs.saves,
            CopyKind::Screenshots => prefs.screenshots,
        }
    }

    /// How each kind of cloud copy stands.
    pub fn cloud_copies_status(&self) -> Vec<CopyStatus> {
        let last = service().with(|s| s.last.clone());
        CopyKind::ALL
            .iter()
            .map(|kind| CopyStatus {
                kind: *kind,
                mode: self.copy_mode(*kind),
                last: last.get(kind).cloned(),
            })
            .collect()
    }

    /// The local files of `kind`. Reads the disk.
    async fn local_files(&self, kind: CopyKind) -> Result<Vec<LocalFile>> {
        match kind {
            CopyKind::Backups => {
                let folder = crate::backup_schedule::backup_folder(&self.settings.app().backups);
                Ok(self
                    .spawn_blocking(move || {
                        crate::backup_schedule::list(&folder)
                            .into_iter()
                            .filter_map(|e| {
                                let meta = std::fs::metadata(&e.path).ok()?;
                                Some(LocalFile {
                                    rel: e.path.file_name()?.to_string_lossy().into_owned(),
                                    size: meta.len(),
                                    modified: meta
                                        .modified()
                                        .ok()?
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .ok()?
                                        .as_secs()
                                        as i64,
                                    path: e.path,
                                })
                            })
                            .collect()
                    })
                    .await?)
            }
            CopyKind::Saves => {
                let Some(root) = self.ludusavi_backup_root().await else {
                    return Ok(Vec::new());
                };
                Ok(self.spawn_blocking(move || walk(&root)).await?)
            }
            CopyKind::Screenshots => {
                let b = self.clone();
                Ok(self
                    .spawn_blocking(move || {
                        b.screenshots()
                            .shots
                            .into_iter()
                            .filter(|s| !s.video)
                            .filter_map(|s| {
                                let meta = std::fs::metadata(&s.path).ok()?;
                                let name = s.path.file_name()?.to_string_lossy().into_owned();
                                Some(LocalFile {
                                    rel: format!("{}/{name}", folder_name(&s.game)),
                                    size: meta.len(),
                                    modified: s.taken,
                                    path: s.path,
                                })
                            })
                            .collect()
                    })
                    .await?)
            }
        }
    }

    /// How much each kind holds here and how much waits to be sent, for
    /// every kind whatever its mode. Reads the disk.
    pub async fn cloud_copy_sizes(&self) -> Result<Vec<CopySize>> {
        // Without a target yet (an rclone remote not chosen), everything waits.
        let key = self.sync_target().await.ok().flatten().map(|t| t.key());
        let mut sizes = Vec::new();
        for kind in CopyKind::ALL {
            let local = self.local_files(kind).await?;
            let sent = key
                .as_ref()
                .and_then(|key| {
                    service().with(|s| s.sent.get(key).and_then(|k| k.get(&kind)).cloned())
                })
                .unwrap_or_default();
            sizes.push(size_of(kind, &local, &sent));
        }
        Ok(sizes)
    }

    /// Send the kinds whose mode is Manual or Automatic (or only `only`)
    /// now. Refused while a game runs.
    pub async fn send_cloud_copies(&self, only: Option<CopyKind>) -> Result<Vec<CopyReport>> {
        if !self.running_games().is_empty() {
            bail!("Cloud copies wait until the game is closed");
        }
        let _one = service().running.lock().await;
        let target = self.sync_target().await?.context("Sync is off")?;
        let me = self.sync.state.lock().device_id.clone();
        let base = format!("devices/{me}");
        self.write_device_file(&target, &base).await?;
        let mut reports = Vec::new();
        for kind in CopyKind::ALL {
            if only.is_some_and(|o| o != kind) || self.copy_mode(kind) == CopyMode::Off {
                continue;
            }
            let report = match self.send_kind(&target, &base, kind).await {
                Ok(report) => report,
                Err(e) => CopyReport {
                    at: irori_core::util::now_rfc3339(),
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                },
            };
            service().with(|s| s.last.insert(kind, report.clone()));
            reports.push(report);
        }
        self.bus.emit(irori_core::Event::SettingsChanged);
        match reports.iter().find_map(|r| r.error.clone()) {
            Some(e) => bail!("{e}"),
            None => Ok(reports),
        }
    }

    async fn write_device_file(&self, target: &Target, base: &str) -> Result<()> {
        let name = self.settings.app().sync.device_name.trim().to_string();
        let device = SyncDevice {
            id: self.sync.state.lock().device_id.clone(),
            name: if name.is_empty() {
                default_device_name()
            } else {
                name
            },
            os: os().into(),
            irori_version: env!("CARGO_PKG_VERSION").into(),
            updated: irori_core::util::now_rfc3339(),
        };
        let scratch = super::rclone::Scratch::new()?;
        tokio::fs::write(scratch.path(), serde_json::to_vec(&device)?).await?;
        target
            .put_file(&format!("{base}/device.json"), scratch.path())
            .await
    }

    async fn send_kind(&self, target: &Target, base: &str, kind: CopyKind) -> Result<CopyReport> {
        let local = self.local_files(kind).await?;
        let key = target.key();
        let sent = service().with(|s| {
            s.sent
                .get(&key)
                .and_then(|k| k.get(&kind))
                .cloned()
                .unwrap_or_default()
        });
        let pending: Vec<LocalFile> = to_send(&local, &sent).into_iter().cloned().collect();
        let mut report = CopyReport {
            at: irori_core::util::now_rfc3339(),
            unchanged: local.len() - pending.len(),
            ..Default::default()
        };
        for file in pending {
            // A game started meanwhile: stop, and go on next time.
            if !self.running_games().is_empty() {
                break;
            }
            target
                .put_file(&format!("{base}/{}/{}", kind.dir(), file.rel), &file.path)
                .await
                .with_context(|| format!("Couldn’t send {}", file.rel))?;
            service().with(|s| {
                s.sent
                    .entry(key.clone())
                    .or_default()
                    .entry(kind)
                    .or_default()
                    .insert(
                        file.rel.clone(),
                        Sent {
                            size: file.size,
                            modified: file.modified,
                        },
                    )
            });
            report.sent += 1;
        }
        Ok(report)
    }

    /// Automatic copies, from the periodic tick: when one kind is
    /// Automatic, no game runs and the wait since the last one is over.
    pub(crate) async fn cloud_copies_when_due(&self) {
        let app = self.settings.app();
        if app.sync.mode == irori_core::SyncMode::Off {
            return;
        }
        let automatic: Vec<CopyKind> = CopyKind::ALL
            .into_iter()
            .filter(|k| self.copy_mode(*k) == CopyMode::Automatic)
            .collect();
        if automatic.is_empty() || !self.running_games().is_empty() {
            return;
        }
        if service()
            .schedule
            .lock()
            .0
            .is_some_and(|next| std::time::Instant::now() < next)
        {
            return;
        }
        let mut failed = false;
        for kind in automatic {
            if let Err(e) = self.send_cloud_copies(Some(kind)).await {
                tracing::info!("cloud copy of {kind:?}: {e:#}");
                failed = true;
            }
        }
        let mut schedule = service().schedule.lock();
        let wait = if failed {
            schedule.1 += 1;
            RETRY[(schedule.1 - 1).min(RETRY.len() - 1)]
        } else {
            schedule.1 = 0;
            EVERY
        };
        schedule.0 = Some(std::time::Instant::now() + wait);
    }

    /// The other computers that keep copies there.
    pub async fn cloud_devices(&self) -> Result<Vec<SyncDevice>> {
        let target = self.sync_target().await?.context("Sync is off")?;
        let me = self.sync.state.lock().device_id.clone();
        let mut out = Vec::new();
        for entry in target.list_dir("devices").await? {
            if !entry.is_dir || entry.name == me {
                continue;
            }
            let scratch = super::rclone::Scratch::new()?;
            let read = target
                .get_file(
                    &format!("devices/{}/device.json", entry.name),
                    scratch.path(),
                )
                .await
                .and_then(|()| Ok(std::fs::read(scratch.path())?));
            match read.map(|b| serde_json::from_slice::<SyncDevice>(&b)) {
                Ok(Ok(device)) if device.id == entry.name => out.push(device),
                _ => tracing::info!("cloud copies: skipping devices/{}", entry.name),
            }
        }
        out.sort_by(|a, b| b.updated.cmp(&a.updated));
        Ok(out)
    }

    /// Every file under `rel` there, as paths relative to it.
    async fn remote_files(&self, target: &Target, rel: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut folders = vec![String::new()];
        while let Some(sub) = folders.pop() {
            let dir = if sub.is_empty() {
                rel.to_string()
            } else {
                format!("{rel}/{sub}")
            };
            for entry in target.list_dir(&dir).await? {
                // Names come from the target: keep them plain.
                if entry.name.contains(['/', '\\']) || entry.name == ".." || entry.name == "." {
                    continue;
                }
                let child = if sub.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{sub}/{}", entry.name)
                };
                if entry.is_dir {
                    folders.push(child);
                } else {
                    out.push(child);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Bring `kind` of `device`'s copies to this computer, never replacing
    /// a file here. Returns how many files came.
    pub async fn bring_back_cloud_copies(
        &self,
        device: &SyncDevice,
        kind: CopyKind,
    ) -> Result<usize> {
        let _one = service().running.lock().await;
        let target = self.sync_target().await?.context("Sync is off")?;
        if device.id.contains(['/', '\\']) || device.id.starts_with('.') {
            bail!("That computer’s name for itself can’t be used");
        }
        let base = format!("devices/{}/{}", device.id, kind.dir());
        let files = self.remote_files(&target, &base).await?;
        let (root, files) = match kind {
            CopyKind::Backups => (
                crate::backup_schedule::backup_folder(&self.settings.app().backups),
                files,
            ),
            CopyKind::Saves => {
                let root = self.ludusavi_backup_root().await.context(
                    "Ludusavi’s backup folder isn’t known. Choose it in Settings → Saves.",
                )?;
                let games = saves_to_bring(&files, &root);
                let files = files
                    .into_iter()
                    .filter(|f| {
                        games
                            .iter()
                            .any(|g| f.split('/').next() == Some(g.as_str()))
                    })
                    .collect();
                (root, files)
            }
            CopyKind::Screenshots => {
                let pictures = dirs::picture_dir().unwrap_or_else(|| Paths::get().data.clone());
                let name = irori_core::paths::sanitize(&device.name);
                (pictures.join("Irori").join(name), files)
            }
        };
        let mut came = 0;
        for rel in files {
            let to = rel.split('/').fold(root.clone(), |p, part| {
                p.join(irori_core::paths::sanitize(part))
            });
            if to.exists() {
                continue;
            }
            target.get_file(&format!("{base}/{rel}"), &to).await?;
            came += 1;
        }
        Ok(came)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_count_what_is_here_and_what_waits() {
        let file = |rel: &str, size: u64, modified: i64| LocalFile {
            rel: rel.into(),
            path: rel.into(),
            size,
            modified,
        };
        let local = [file("a", 10, 1), file("b", 20, 1), file("c", 30, 1)];
        let sent: BTreeMap<String, Sent> = [
            (
                "a".to_string(),
                Sent {
                    size: 10,
                    modified: 1,
                },
            ),
            // Changed since it was sent: waits again.
            (
                "b".to_string(),
                Sent {
                    size: 5,
                    modified: 1,
                },
            ),
        ]
        .into();
        let size = size_of(CopyKind::Saves, &local, &sent);
        assert_eq!((size.files, size.bytes), (3, 60));
        assert_eq!((size.waiting_files, size.waiting_bytes), (2, 50));
        let none = size_of(CopyKind::Backups, &[], &sent);
        assert_eq!((none.files, none.bytes, none.waiting_files), (0, 0, 0));
    }

    #[test]
    fn sends_only_new_and_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Celeste/backup-1")).unwrap();
        std::fs::write(root.join("Celeste/mapping.yaml"), b"name: Celeste").unwrap();
        std::fs::write(root.join("Celeste/backup-1/a.sav"), b"save").unwrap();
        std::fs::write(root.join("Celeste/mapping.yaml.tmp"), b"half").unwrap();
        let local = walk(root);
        let rels: Vec<&str> = local.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["Celeste/backup-1/a.sav", "Celeste/mapping.yaml"]);

        let mut sent = BTreeMap::new();
        assert_eq!(to_send(&local, &sent).len(), 2);
        sent.insert(
            local[0].rel.clone(),
            Sent {
                size: local[0].size,
                modified: local[0].modified,
            },
        );
        sent.insert(
            local[1].rel.clone(),
            Sent {
                size: 1,
                modified: local[1].modified,
            },
        );
        let pending = to_send(&local, &sent);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].rel, "Celeste/mapping.yaml");
    }

    #[test]
    fn brings_back_only_games_without_local_backups() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Celeste")).unwrap();
        let remote = [
            "Celeste/mapping.yaml".to_string(),
            "Hades/mapping.yaml".to_string(),
            "Hades/backup-1/a.sav".to_string(),
        ];
        assert_eq!(saves_to_bring(&remote, dir.path()), ["Hades"]);
        assert_eq!(folder_name("gog:1207658691"), "gog-1207658691");
    }
}
