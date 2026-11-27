//! What the download queue still has to write on each disk, against the space
//! free there, so the downloads page can say a disk won't hold the queue
//! before the download that fills it starts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use irori_core::GameId;

use crate::Backend;
use crate::downloads::{DownloadItem, DownloadKind, DownloadState};
use crate::drives::{
    DetectedDrive, extra_drives_from_system, resolve_disk_label, snapshot_system_disks,
};

/// One disk the queue downloads to, or an install drive shown when idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueDisk {
    /// The first folder on this disk (or mount root for an auto-detected drive).
    pub folder: PathBuf,
    /// Formatted disk label (for example `C: System`, `D: Asgard`,
    /// `/ · Reletech P400 EVO 2TB`, `/mnt/Asgard`), when resolved.
    pub label: Option<String>,
    /// Bytes the queue still downloads there.
    pub needed: u64,
    /// Bytes free there, when they could be read.
    pub free: Option<u64>,
    /// Total disk capacity in bytes, when it could be read.
    pub total: Option<u64>,
}

impl QueueDisk {
    /// The disk holds less than the queue still downloads to it.
    pub fn is_short(&self) -> bool {
        self.free.is_some_and(|free| free < self.needed)
    }

    /// Concise display label for the UI: the resolved disk name when known,
    /// otherwise the folder path.
    pub fn display_label(&self) -> String {
        self.label
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.folder.display().to_string())
    }
}

/// The folder and the bytes each waiting or running install still downloads.
/// Updates (whose size stores tell only while they run), offline installers
/// and installs of unknown size need nothing that can be told. The running
/// item counts only what is left of it, from `percent_done`.
pub(crate) fn queue_needs(
    active: &[DownloadItem],
    percent_done: impl Fn(&GameId) -> Option<f32>,
) -> Vec<(PathBuf, u64)> {
    active
        .iter()
        .filter_map(|item| {
            let DownloadKind::Install(req) = &item.kind else {
                return None;
            };
            if item.download_size == 0 || req.installer.is_some() || req.path.as_os_str().is_empty()
            {
                return None;
            }
            let left = match item.state {
                DownloadState::Running | DownloadState::Paused => {
                    remaining(item.download_size, percent_done(&item.id))
                }
                _ => item.download_size,
            };
            Some((req.path.clone(), left))
        })
        .collect()
}

/// Bytes still to download of `total`, `percent` of it done.
fn remaining(total: u64, percent: Option<f32>) -> u64 {
    match percent {
        Some(p) if p > 0.0 => {
            let left = f64::from(100.0 - p.clamp(0.0, 100.0)) / 100.0;
            (total as f64 * left).round() as u64
        }
        _ => total,
    }
}

/// Add the needs up per disk, in the order the queue reaches each disk.
/// `disk_of` names the disk a folder is on (two folders on one disk share a
/// name) and its `(free, total)` bytes; a folder whose disk can't be told is a
/// disk of its own with its space unknown.
pub(crate) fn per_disk(
    needs: Vec<(PathBuf, u64)>,
    disk_of: impl Fn(&Path) -> Option<(String, Option<u64>, Option<u64>)>,
) -> Vec<QueueDisk> {
    let mut disks: Vec<(String, QueueDisk)> = Vec::new();
    for (folder, bytes) in needs {
        let (key, free, total) =
            disk_of(&folder).unwrap_or_else(|| (folder.display().to_string(), None, None));
        match disks.iter_mut().find(|(k, _)| *k == key) {
            Some((_, disk)) => disk.needed = disk.needed.saturating_add(bytes),
            None => disks.push((
                key,
                QueueDisk {
                    folder,
                    label: None,
                    needed: bytes,
                    free,
                    total,
                },
            )),
        }
    }
    disks.into_iter().map(|(_, disk)| disk).collect()
}

/// The disk holding `path` (its nearest existing folder) and its `(free, total)` bytes.
fn this_disk(path: &Path) -> Option<(String, Option<u64>, Option<u64>)> {
    let existing = path.ancestors().find(|p| p.is_dir())?;
    let free = fs4::available_space(existing).ok();
    let total = fs4::total_space(existing).ok();
    Some((disk_key(existing)?, free, total))
}

/// What tells disks apart: the device on Unix, the drive or share elsewhere.
#[cfg(unix)]
fn disk_key(existing: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(existing)
        .ok()
        .map(|m| m.dev().to_string())
}

#[cfg(not(unix))]
fn disk_key(existing: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    match existing.components().next()? {
        Component::Prefix(prefix) => Some(match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                format!("{}:", (drive as char).to_ascii_uppercase())
            }
            _ => prefix.as_os_str().to_string_lossy().to_ascii_uppercase(),
        }),
        _ => Some(existing.display().to_string()),
    }
}

/// Combine configured install disks with auto-detected extra writable drives,
/// preserving configured disks first and skipping any extra drive whose
/// underlying disk key is already present.
pub(crate) fn merge_install_disks(
    mut configured: Vec<QueueDisk>,
    extras: Vec<DetectedDrive>,
    key_of: impl Fn(&Path) -> Option<String>,
) -> Vec<QueueDisk> {
    let mut seen_keys: HashSet<String> = configured
        .iter()
        .filter_map(|d| key_of(&d.folder))
        .collect();
    for drive in extras {
        if let Some(key) = key_of(&drive.mount)
            && !seen_keys.insert(key)
        {
            continue;
        }
        configured.push(QueueDisk {
            folder: drive.mount,
            label: Some(drive.disk_label),
            needed: 0,
            free: Some(drive.free),
            total: Some(drive.total),
        });
    }
    configured
}

impl Backend {
    /// What the queue still downloads to each disk, against the space free
    /// there. Reads the disks, so call it off the UI thread, and only when
    /// the queue changes.
    pub fn download_disks(&self) -> Vec<QueueDisk> {
        let active = self.downloads().active;
        let needs = queue_needs(&active, |id| {
            self.statuses
                .get(id)
                .and_then(|s| s.progress)
                .or_else(|| active.iter().find(|i| &i.id == id).and_then(|i| i.progress))
                .and_then(|p| p.percent)
        });
        if needs.is_empty() {
            return Vec::new();
        }
        let system_disks = snapshot_system_disks();
        let mut disks = per_disk(needs, this_disk);
        for disk in &mut disks {
            disk.label = resolve_disk_label(&disk.folder, &system_disks);
        }
        disks
    }

    /// Free and total space for configured game install folders plus
    /// auto-detected main writable drives, grouped per physical disk. Reads the
    /// disks, so call it off the UI thread.
    pub fn install_disks(&self) -> Vec<QueueDisk> {
        let configured_paths: Vec<PathBuf> = self
            .settings
            .app()
            .library_folders()
            .into_iter()
            .filter(|f| !f.path.trim().is_empty())
            .map(|f| irori_core::paths::expand_home(&f.path))
            .collect();
        let system_disks = snapshot_system_disks();
        let folders: Vec<(PathBuf, u64)> =
            configured_paths.iter().cloned().map(|p| (p, 0)).collect();
        let mut configured: Vec<QueueDisk> = per_disk(folders, this_disk)
            .into_iter()
            .filter(|d| d.free.is_some())
            .collect();
        for disk in &mut configured {
            disk.label = resolve_disk_label(&disk.folder, &system_disks);
        }
        let home = dirs::home_dir();
        let home_ref = home
            .as_deref()
            .map(|p| (p, irori_core::util::is_dir_writable(p)));
        let extras = extra_drives_from_system(&system_disks, &configured_paths, home_ref);
        merge_install_disks(configured, extras, |path| {
            path.ancestors().find(|p| p.is_dir()).and_then(disk_key)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstallRequest, Runner, UpdateRequest};

    fn install(app: &str, path: &str, size: u64, state: DownloadState) -> DownloadItem {
        DownloadItem {
            id: GameId::new(Runner::Gog, app),
            title: app.into(),
            kind: DownloadKind::Install(InstallRequest {
                path: PathBuf::from(path),
                ..Default::default()
            }),
            state,
            added_at: 0,
            started_at: None,
            finished_at: None,
            download_size: size,
            error: None,
            progress: None,
        }
    }

    #[test]
    fn needs_count_what_is_left_of_known_installs() {
        let mut update = install("u", "/games", 0, DownloadState::Queued);
        update.kind = DownloadKind::Update(UpdateRequest::default());
        let mut offline = install("o", "/games", 500, DownloadState::Queued);
        if let DownloadKind::Install(req) = &mut offline.kind {
            req.installer = Some(PathBuf::from("/setup.exe"));
        }
        let active = vec![
            install("a", "/games", 1000, DownloadState::Running),
            install("b", "/other", 300, DownloadState::Queued),
            install("c", "/games", 0, DownloadState::Queued),
            update,
            offline,
        ];
        let needs = queue_needs(&active, |id| (id.app_name.as_ref() == "a").then_some(25.0));
        assert_eq!(
            needs,
            vec![
                (PathBuf::from("/games"), 750),
                (PathBuf::from("/other"), 300),
            ]
        );
    }

    #[test]
    fn adds_up_per_disk_and_flags_short_ones() {
        let needs = vec![
            (PathBuf::from("/mnt/big/a"), 100),
            (PathBuf::from("/home/me/games"), 50),
            (PathBuf::from("/mnt/big/b"), 200),
            (PathBuf::from("/unknown"), 10),
        ];
        let disks = per_disk(needs, |p| {
            if p.starts_with("/mnt/big") {
                Some(("big".into(), Some(250), Some(1_000)))
            } else if p.starts_with("/home") {
                Some(("home".into(), Some(1_000), Some(2_000)))
            } else {
                None
            }
        });
        assert_eq!(disks.len(), 3);
        assert_eq!(disks[0].folder, PathBuf::from("/mnt/big/a"));
        assert_eq!(disks[0].needed, 300);
        assert_eq!(disks[0].total, Some(1_000));
        assert!(disks[0].is_short(), "300 needed, 250 free");
        assert!(!disks[1].is_short());
        assert_eq!(disks[2].free, None);
        assert_eq!(disks[2].total, None);
        assert!(!disks[2].is_short(), "unknown free space is never short");
    }

    #[test]
    fn reads_a_real_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (key, free, total) = this_disk(&dir.path().join("not/yet/made")).unwrap();
        assert!(!key.is_empty());
        assert!(free.is_some());
        assert!(total.is_some());
    }

    #[cfg(not(unix))]
    #[test]
    fn disk_key_deduplicates_verbatim_and_plain_drive_paths() {
        assert_eq!(
            disk_key(Path::new(r"c:\Games")),
            disk_key(Path::new(r"\\?\C:\Library"))
        );
        assert_ne!(
            disk_key(Path::new(r"C:\Games")),
            disk_key(Path::new(r"D:\Games"))
        );
    }

    #[test]
    fn merges_configured_and_extra_drives_without_duplicate_keys() {
        let configured = vec![QueueDisk {
            folder: PathBuf::from("/home/edward/Games/Heroic"),
            label: Some("/ · Reletech P400 EVO 2TB".into()),
            needed: 0,
            free: Some(1_300),
            total: Some(1_900),
        }];
        let extras = vec![
            DetectedDrive {
                label: "/mnt/alias".into(),
                disk_label: "/mnt/alias".into(),
                suggested_base: "/mnt/alias/Games/Irori".into(),
                total: 1_900,
                free: 1_300,
                mount: PathBuf::from("/mnt/alias"),
            },
            DetectedDrive {
                label: "/mnt/Asgard".into(),
                disk_label: "/mnt/Asgard".into(),
                suggested_base: "/mnt/Asgard/Games/Irori".into(),
                total: 938,
                free: 177,
                mount: PathBuf::from("/mnt/Asgard"),
            },
        ];
        let merged = merge_install_disks(configured, extras, |p| {
            if p.starts_with("/home") || p.starts_with("/mnt/alias") {
                Some("dev0".into())
            } else {
                Some("dev1".into())
            }
        });
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].display_label(), "/ · Reletech P400 EVO 2TB");
        assert_eq!(merged[1].display_label(), "/mnt/Asgard");
        assert_eq!(merged[1].folder, PathBuf::from("/mnt/Asgard"));
        assert_eq!(merged[1].free, Some(177));
        assert_eq!(merged[1].total, Some(938));
    }

    #[test]
    fn display_label_falls_back_to_folder_when_label_missing() {
        let disk = QueueDisk {
            folder: PathBuf::from("/games"),
            label: None,
            needed: 0,
            free: Some(10),
            total: Some(20),
        };
        assert_eq!(disk.display_label(), "/games");
    }
}
