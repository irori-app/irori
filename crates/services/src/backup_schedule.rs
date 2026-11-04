//! Backups Irori makes on its own: daily or weekly, and before a new
//! version of Irori first runs, into a folder the user picks, keeping the
//! newest few. They are the same files as a backup saved by hand (see
//! [`crate::backup`]), with the downloaded game art beside them when asked
//! for. The state between runs is `state/backups.json`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use irori_core::{BackupPrefs, BackupSchedule, Event, Notice, NoticeLevel, Paths, json_store};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{Backend, BackupParts};

const PREFIX: &str = "irori-backup-";
/// Backups made before an update that are kept, beside the scheduled ones.
const KEEP_BEFORE_UPDATE: usize = 3;
/// Most backups listed.
const MAX_LISTED: usize = 50;
/// How long a failed backup waits before the next try.
const RETRY: Duration = Duration::from_secs(60 * 60);

/// What kind of backup a file in the backup folder is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupKind {
    Scheduled,
    /// Made before this version of Irori first ran.
    BeforeUpdate(String),
    /// Saved by hand, or by something else.
    Other,
}

/// A backup in the backup folder.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub kind: BackupKind,
    /// RFC 3339 time it was written.
    pub created: String,
    pub size: u64,
    /// The game art was copied beside it.
    pub art: bool,
}

/// What the settings page shows about scheduled backups.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupStatus {
    pub folder: PathBuf,
    /// RFC 3339 time of the last scheduled backup that worked.
    pub last_ok: Option<String>,
    /// RFC 3339 time of the last try, and why it failed.
    pub last_try: Option<String>,
    pub last_error: Option<String>,
    pub running: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct ScheduleState {
    last_ok: Option<String>,
    last_try: Option<String>,
    last_error: Option<String>,
    /// The version of Irori that ran last.
    irori_version: String,
    /// The user was told the last backup failed.
    told: bool,
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("backups.json")
}

/// Scheduled backups' share of the backend.
pub struct BackupScheduler {
    state: Mutex<ScheduleState>,
    running: tokio::sync::Mutex<()>,
}

impl BackupScheduler {
    pub(crate) fn load() -> Self {
        Self {
            state: Mutex::new(json_store::read_or_default(&state_file())),
            running: tokio::sync::Mutex::new(()),
        }
    }

    fn save(&self, state: &ScheduleState) {
        if let Err(e) = json_store::write(&state_file(), state) {
            tracing::warn!("saving the backup state: {e:#}");
        }
    }
}

/// Where scheduled backups go.
pub fn backup_folder(prefs: &BackupPrefs) -> PathBuf {
    match prefs.folder.trim() {
        "" => Paths::get().data.join("backups"),
        folder => irori_core::paths::expand_home(folder),
    }
}

fn parse(t: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(t).ok()
}

/// An RFC 3339 time as `YYYY-MM-DD HH:MM` in this computer's time zone.
pub fn local_time(t: &str) -> String {
    parse(t)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| t.to_string())
}

/// Whether a scheduled backup is due at `now`.
fn due(
    schedule: BackupSchedule,
    last_ok: Option<&str>,
    last_try: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let period = match schedule {
        BackupSchedule::Off => return false,
        BackupSchedule::Daily => chrono::Duration::days(1),
        BackupSchedule::Weekly => chrono::Duration::days(7),
    };
    let since = |t: Option<&str>| t.and_then(parse).map(|t| now.signed_duration_since(t));
    let retry = chrono::Duration::from_std(RETRY).expect("an hour fits");
    if since(last_try).is_some_and(|d| d < retry && d >= chrono::Duration::zero()) {
        return false;
    }
    // A time in the future (a clock set back) counts as long ago.
    since(last_ok).is_none_or(|d| d >= period || d < chrono::Duration::zero())
}

/// What kind of backup the file `name` is, if it is one.
fn kind_of(name: &str) -> Option<BackupKind> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(".json")?;
    let stamp = |s: &str| {
        s.len() == 17
            && s.bytes().enumerate().all(|(i, b)| match i {
                4 | 7 | 10 => b == b'-',
                _ => b.is_ascii_digit(),
            })
    };
    if stamp(stem) {
        return Some(BackupKind::Scheduled);
    }
    if let Some((s, version)) = stem.split_once("-before-")
        && stamp(s)
        && !version.is_empty()
    {
        return Some(BackupKind::BeforeUpdate(version.to_string()));
    }
    Some(BackupKind::Other)
}

/// The name of a new backup made at `now`.
fn file_name(now: chrono::DateTime<chrono::Local>, kind: &BackupKind) -> String {
    let stamp = now.format("%Y-%m-%d-%H%M%S");
    match kind {
        BackupKind::BeforeUpdate(version) => {
            format!(
                "{PREFIX}{stamp}-before-{}.json",
                irori_core::paths::sanitize(version)
            )
        }
        _ => format!("{PREFIX}{stamp}.json"),
    }
}

/// The folder beside a backup holding its game art.
fn art_dir(backup: &Path) -> PathBuf {
    backup.with_extension("art")
}

/// The backups in `folder`, newest first.
pub fn list(folder: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut out: Vec<(SystemTime, BackupEntry)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = kind_of(&name)?;
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let path = entry.path();
            Some((
                modified,
                BackupEntry {
                    art: art_dir(&path).is_dir(),
                    created: chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339(),
                    size: meta.len(),
                    kind,
                    path,
                },
            ))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.path.cmp(&a.1.path)));
    out.truncate(MAX_LISTED);
    out.into_iter().map(|(_, e)| e).collect()
}

/// Remove scheduled backups beyond the newest `keep`, and backups made
/// before updates beyond the newest few. Backups saved by hand stay.
/// Returns how many were removed.
fn prune(folder: &Path, keep: usize) -> Result<usize> {
    let mut scheduled = Vec::new();
    let mut before = Vec::new();
    for entry in std::fs::read_dir(folder)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match kind_of(&name) {
            Some(BackupKind::Scheduled) => scheduled.push(name),
            Some(BackupKind::BeforeUpdate(_)) => before.push(name),
            _ => {}
        }
    }
    // Names start with the time they were made.
    scheduled.sort_by(|a, b| b.cmp(a));
    before.sort_by(|a, b| b.cmp(a));
    let mut removed = 0;
    let old = scheduled
        .iter()
        .skip(keep.max(1))
        .chain(before.iter().skip(KEEP_BEFORE_UPDATE));
    for name in old {
        let path = folder.join(name);
        std::fs::remove_file(&path).with_context(|| format!("Cannot remove {}", path.display()))?;
        let art = art_dir(&path);
        if art.is_dir() {
            // Only the art folder Irori made beside its own backup.
            std::fs::remove_dir_all(&art)
                .with_context(|| format!("Cannot remove {}", art.display()))?;
        }
        removed += 1;
    }
    Ok(removed)
}

/// Copy the files of `from` into `to`, keeping files already there.
/// Returns how many were copied.
fn copy_missing(from: &Path, to: &Path) -> Result<usize> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(0);
    };
    std::fs::create_dir_all(to).with_context(|| format!("Cannot create {}", to.display()))?;
    let mut copied = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copied += copy_missing(&entry.path(), &target)?;
        } else if kind.is_file() && !target.exists() {
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("Cannot copy to {}", target.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn art_cache() -> PathBuf {
    Paths::get().images_dir().join("orig")
}

fn parts(prefs: &BackupPrefs) -> BackupParts {
    BackupParts {
        settings: prefs.settings,
        game_settings: prefs.game_settings,
        library: prefs.library,
        playtime: prefs.playtime,
    }
}

impl Backend {
    /// How scheduled backups stand.
    pub fn backup_status(&self) -> BackupStatus {
        let state = self.backups.state.lock();
        BackupStatus {
            folder: backup_folder(&self.settings.app().backups),
            last_ok: state.last_ok.clone(),
            last_try: state.last_try.clone(),
            last_error: state.last_error.clone(),
            running: self.backups.running.try_lock().is_err(),
        }
    }

    /// The backups in the backup folder, newest first. Reads the folder:
    /// call it off the UI thread.
    pub fn scheduled_backups(&self) -> Vec<BackupEntry> {
        list(&backup_folder(&self.settings.app().backups))
    }

    /// Back up now into the backup folder, as a scheduled backup.
    pub async fn back_up_now(&self) -> Result<PathBuf> {
        self.run_backup(BackupKind::Scheduled).await
    }

    /// Back up when the schedule says so. Runs on the periodic tick.
    pub(crate) async fn back_up_when_due(&self) {
        let prefs = self.settings.app().backups.clone();
        let is_due = {
            let state = self.backups.state.lock();
            due(
                prefs.schedule,
                state.last_ok.as_deref(),
                state.last_try.as_deref(),
                chrono::Utc::now(),
            )
        };
        if is_due {
            // Failures are reported through the status and a notice.
            let _ = self.run_backup(BackupKind::Scheduled).await;
        }
    }

    /// When Irori runs in a new version for the first time, back up what
    /// the old one wrote.
    pub(crate) async fn back_up_before_update(&self) {
        let version = env!("CARGO_PKG_VERSION");
        let last = {
            let mut state = self.backups.state.lock();
            let last = std::mem::replace(&mut state.irori_version, version.to_string());
            if last != version {
                self.backups.save(&state);
            }
            last
        };
        // A first run has nothing older to keep.
        if last.is_empty() || last == version || !self.settings.app().backups.before_updates {
            return;
        }
        let _ = self
            .run_backup(BackupKind::BeforeUpdate(version.to_string()))
            .await;
    }

    async fn run_backup(&self, kind: BackupKind) -> Result<PathBuf> {
        let _one = self.backups.running.lock().await;
        let b = self.clone();
        let result = self
            .spawn_blocking(move || b.write_scheduled_backup(&kind))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
        {
            let mut state = self.backups.state.lock();
            state.last_try = Some(irori_core::util::now_rfc3339());
            match &result {
                Ok(_) => {
                    state.last_ok = state.last_try.clone();
                    state.last_error = None;
                    state.told = false;
                }
                Err(e) => {
                    tracing::warn!("backing up: {e:#}");
                    state.last_error = Some(format!("{e:#}"));
                    if !state.told {
                        state.told = true;
                        self.bus.notice(
                            Notice::new(NoticeLevel::Warning, "Irori couldn’t back up")
                                .message(format!("{e:#}"))
                                .keys("shell.notice.backup_failed", None, &[]),
                        );
                    }
                }
            }
            self.backups.save(&state);
        }
        self.bus.emit(Event::SettingsChanged);
        result
    }

    fn write_scheduled_backup(&self, kind: &BackupKind) -> Result<PathBuf> {
        let prefs = self.settings.app().backups.clone();
        let folder = backup_folder(&prefs);
        if prefs.folder.trim().is_empty() {
            std::fs::create_dir_all(&folder)
                .with_context(|| format!("Cannot create {}", folder.display()))?;
        } else if !folder.is_dir() {
            // A drive or share that isn't there now: don't create a folder
            // where it is mounted.
            bail!("The backup folder {} isn’t available", folder.display());
        }
        let mut parts = parts(&prefs);
        if !parts.any() {
            parts = BackupParts::ALL;
        }
        let path = folder.join(file_name(chrono::Local::now(), kind));
        self.write_backup(&path, parts)?;
        if prefs.art {
            copy_missing(&art_cache(), &art_dir(&path).join("orig"))?;
        }
        prune(&folder, prefs.keep.clamp(1, MAX_LISTED as u32) as usize)?;
        Ok(path)
    }

    /// Bring back the game art saved beside a backup, keeping art already
    /// here. Returns how many files were copied; none when the backup has no
    /// art.
    pub fn restore_backup_art(&self, backup: &Path) -> Result<usize> {
        copy_missing(&art_dir(backup).join("orig"), &art_cache())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(t: &str) -> chrono::DateTime<chrono::Utc> {
        parse(t).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    fn schedules_fall_due_after_their_period() {
        let now = utc("2026-11-04T12:00:00Z");
        let weekly = BackupSchedule::Weekly;
        assert!(due(weekly, None, None, now));
        assert!(!due(weekly, Some("2026-11-01T12:00:00Z"), None, now));
        assert!(due(weekly, Some("2026-10-28T12:00:00Z"), None, now));
        assert!(due(
            BackupSchedule::Daily,
            Some("2026-11-03T11:00:00Z"),
            None,
            now
        ));
        assert!(!due(BackupSchedule::Off, None, None, now));
        // A failure waits an hour before trying again.
        assert!(!due(weekly, None, Some("2026-11-04T11:30:00Z"), now));
        assert!(due(weekly, None, Some("2026-11-04T10:30:00Z"), now));
        // A clock set back doesn't stop backups for a week.
        assert!(due(weekly, Some("2027-01-01T00:00:00Z"), None, now));
    }

    #[test]
    fn tells_backups_apart_by_name() {
        assert_eq!(
            kind_of("irori-backup-2026-11-04-153000.json"),
            Some(BackupKind::Scheduled)
        );
        assert_eq!(
            kind_of("irori-backup-2026-11-04-153000-before-0.6.0.json"),
            Some(BackupKind::BeforeUpdate("0.6.0".into()))
        );
        assert_eq!(
            kind_of("irori-backup-2026-11-04.json"),
            Some(BackupKind::Other)
        );
        assert_eq!(kind_of("irori-backup-2026-11-04-153000.art"), None);
        assert_eq!(kind_of("notes.json"), None);
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-04T15:30:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Local);
        let name = file_name(at, &BackupKind::BeforeUpdate("0.6.0".into()));
        assert_eq!(
            kind_of(&name),
            Some(BackupKind::BeforeUpdate("0.6.0".into()))
        );
    }

    #[test]
    fn pruning_keeps_the_newest_and_what_the_user_saved() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let names = [
            "irori-backup-2026-10-01-100000.json",
            "irori-backup-2026-10-08-100000.json",
            "irori-backup-2026-10-15-100000.json",
            "irori-backup-2026-10-22-100000.json",
            "irori-backup-2026-01-01-100000-before-0.2.0.json",
            "irori-backup-2026-02-01-100000-before-0.3.0.json",
            "irori-backup-2026-03-01-100000-before-0.4.0.json",
            "irori-backup-2026-04-01-100000-before-0.5.0.json",
            "irori-backup-2026-09-30.json",
            "holiday photos.json",
        ];
        for name in names {
            std::fs::write(folder.join(name), "{}").unwrap();
        }
        let art = folder
            .join("irori-backup-2026-10-01-100000.art")
            .join("orig");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(art.join("abc"), "img").unwrap();

        assert_eq!(prune(folder, 2).unwrap(), 3);
        let mut left: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "holiday photos.json",
                "irori-backup-2026-02-01-100000-before-0.3.0.json",
                "irori-backup-2026-03-01-100000-before-0.4.0.json",
                "irori-backup-2026-04-01-100000-before-0.5.0.json",
                "irori-backup-2026-09-30.json",
                "irori-backup-2026-10-15-100000.json",
                "irori-backup-2026-10-22-100000.json",
            ]
        );
        let listed = list(folder);
        assert_eq!(listed.len(), 6);
        assert!(listed.iter().all(|e| !e.art));
    }

    #[test]
    fn art_copies_keep_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("a"), "new").unwrap();
        std::fs::write(from.join("sub").join("b"), "b").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("a"), "kept").unwrap();
        assert_eq!(copy_missing(&from, &to).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(to.join("a")).unwrap(), "kept");
        assert_eq!(
            std::fs::read_to_string(to.join("sub").join("b")).unwrap(),
            "b"
        );
        assert_eq!(copy_missing(&dir.path().join("none"), &to).unwrap(), 0);
    }
}
