//! Running sessions kept on disk (`state/running.json`). A game outlives an
//! Irori that closes, crashes or updates while it runs; when Irori starts
//! again it follows such a game once more, so its session and playtime are
//! recorded, its mods taken out and [`irori_core::Event::GameExited`] sent
//! when it ends.
//!
//! Only games Irori starts and follows itself are kept: by their process
//! group on Unix and their descendants on Windows, or, for third-party
//! launchers' games, by the processes running from their folder. A game
//! another program owns (Steam) is that program's to follow.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use irori_core::{GameId, Paths, StatusKind};
use serde::{Deserialize, Serialize};

use crate::Backend;
use crate::games::{RunningGame, SessionEnd};

/// One game that was running when the record was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    /// [`GameId::key`].
    pub game: String,
    /// The program Irori started, the leader of its process group on Unix.
    pub pid: u32,
    /// When the session started (RFC 3339), as the history records it.
    pub started_at: String,
    /// When the program started (Unix seconds, as the system reports it),
    /// to tell it from a later process that reused its pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start: Option<u64>,
    /// The folder whose processes are the game, for games another launcher
    /// starts and Irori follows by their folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Mods were applied for the session; they come out when it ends.
    #[serde(default)]
    pub mods: bool,
}

/// Every read and write of the file, so two sessions ending at once never
/// lose each other's change.
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn file() -> PathBuf {
    Paths::get().state_dir().join("running.json")
}

/// The sessions recorded as running.
pub(crate) fn read() -> Vec<Record> {
    let _lock = LOCK.lock();
    irori_core::json_store::read_or_default(&file())
}

fn update(change: impl FnOnce(&mut Vec<Record>)) {
    let _lock = LOCK.lock();
    let path = file();
    let mut records: Vec<Record> = irori_core::json_store::read_or_default(&path);
    let before = records.clone();
    change(&mut records);
    if records == before {
        return;
    }
    let written = if records.is_empty() {
        std::fs::remove_file(&path).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e.into()),
        })
    } else {
        irori_core::json_store::write(&path, &records)
    };
    if let Err(e) = written {
        tracing::warn!("keeping the running games in {}: {e:#}", path.display());
    }
}

/// Note a session that just started, in place of an older one of its game.
pub(crate) fn remember(record: Record) {
    update(|records| {
        records.retain(|r| r.game != record.game);
        records.push(record);
    });
}

/// Forget the session of `id`, which ended.
pub(crate) fn forget(id: &GameId) {
    let key = id.key();
    update(|records| records.retain(|r| r.game != key));
}

/// When process `pid` started (Unix seconds), or `None` when none runs.
/// Blocking.
pub(crate) fn process_start(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).map(|p| p.start_time())
}

/// How far apart two readings of one process's start may be: systems
/// report it in whole seconds, from clocks that drift a little.
const SAME_START: u64 = 2;

/// Whether a recorded session still runs and is the one recorded. `alive`:
/// what follows it sees something running. `recorded` and `now`: when the
/// recorded program started, as recorded and as the process with its pid
/// reports now (`None` when that process is gone, which a launcher that
/// handed over is). A process with the pid that started at another time
/// reused it, and what runs is no longer the game.
pub(crate) fn still_ours(recorded: Option<u64>, now: Option<u64>, alive: bool) -> bool {
    match (recorded, now) {
        _ if !alive => false,
        (Some(recorded), Some(now)) => recorded.abs_diff(now) <= SAME_START,
        _ => true,
    }
}

/// What tells whether the recorded game still runs. Blocking when called.
fn probe(record: &Record) -> crate::follow::Probe {
    if let Some(folder) = &record.folder {
        return crate::follow::folder_probe(folder.clone());
    }
    let handoff = crate::follow::Handoff::resume(record.pid, started_unix(record));
    let (pid, recorded) = (record.pid, record.process_start);
    // Unix: the process group, the program itself included. Windows has no
    // group: the program, then what it started.
    Arc::new(move || {
        let program = process_start(pid).is_some_and(|now| still_ours(recorded, Some(now), true));
        program || handoff.running()
    })
}

/// How long the session has been running, from when it started.
fn elapsed(started_at: &str) -> Duration {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .ok()
        .and_then(|t| {
            (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
                .to_std()
                .ok()
        })
        .unwrap_or_default()
}

impl Backend {
    /// At start: follow again the games that were running when Irori
    /// last closed, and forget those that ended meanwhile (their session
    /// can't be told: it ended while nobody watched).
    pub(crate) async fn adopt_sessions(&self) {
        if let Ok(records) = self.spawn_blocking(read).await {
            self.adopt_records(records).await;
        }
    }

    /// [`Self::adopt_sessions`] for these records.
    pub(crate) async fn adopt_records(&self, records: Vec<Record>) {
        for record in records {
            let Some(id) = game_id(&record.game) else {
                forget_key(&record.game);
                continue;
            };
            let probe = probe(&record);
            let (pid, recorded) = (record.pid, record.process_start);
            let check = probe.clone();
            let ours = self
                .spawn_blocking(move || still_ours(recorded, process_start(pid), check()))
                .await
                .unwrap_or(false);
            if !ours || self.is_running(&id) {
                forget(&id);
                continue;
            }
            tracing::info!("following {id} again: it ran while Irori was closed");
            self.adopt(id, record, probe);
        }
    }

    /// Follow a recorded game that still runs, as its launch would have.
    fn adopt(&self, id: GameId, record: Record, probe: crate::follow::Probe) {
        let elapsed = elapsed(&record.started_at);
        let started = Instant::now()
            .checked_sub(elapsed)
            .unwrap_or_else(Instant::now);
        // Stopping ends what the program left running too.
        let handoff = record
            .folder
            .is_none()
            .then(|| crate::follow::Handoff::resume(record.pid, started_unix(&record)));
        self.running.lock().insert(
            id.clone(),
            RunningGame {
                pid: record.pid,
                started,
                stopping: false,
                watch: None,
                sandbox: None,
                handoff,
            },
        );
        self.statuses.set(&id, StatusKind::Playing);
        let open = crate::power::OpenSession::new(self);
        let b = self.clone();
        self.spawn(async move {
            let _open = open;
            crate::follow::wait_while(probe).await;
            // The session's end needs the stores: what the game is, its
            // after-exit hook.
            while !b.stores_ready() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            b.finish_session(SessionEnd {
                game: b.game(&id),
                settings: b.settings.game(&id),
                log_file: Paths::get().game_log_file(&id),
                id,
                started,
                started_at: record.started_at,
                exit: None,
                mods: record.mods,
            })
            .await;
        });
    }
}

/// Forget a record whose game can't be named.
fn forget_key(key: &str) {
    update(|records| records.retain(|r| r.game != key));
}

/// The game a [`GameId::key`] names.
fn game_id(key: &str) -> Option<GameId> {
    let (runner, app) = key.split_once(':')?;
    Some(GameId::new(irori_core::Runner::from_id(runner)?, app))
}

/// When the recorded program started (Unix seconds): as the system said,
/// else when the session began.
fn started_unix(record: &Record) -> u64 {
    record.process_start.unwrap_or_else(|| {
        chrono::DateTime::parse_from_rfc3339(&record.started_at)
            .map_or(0, |t| u64::try_from(t.timestamp()).unwrap_or(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reused_pid_is_not_the_game() {
        assert!(still_ours(Some(100), Some(100), true));
        assert!(still_ours(Some(100), Some(102), true), "clocks drift");
        assert!(!still_ours(Some(100), Some(400), true), "another process");
        assert!(
            still_ours(Some(100), None, true),
            "the launcher handed over and left"
        );
        assert!(still_ours(None, Some(400), true), "nothing to compare");
        assert!(!still_ours(Some(100), Some(100), false), "nothing runs");
    }

    #[test]
    fn records_read_without_their_optional_fields() {
        let records: Vec<Record> = serde_json::from_str(
            r#"[{"game":"gog:1","pid":7,"startedAt":"2026-01-01T00:00:00Z"}]"#,
        )
        .unwrap();
        assert_eq!(records[0].pid, 7);
        assert_eq!(records[0].folder, None);
        assert!(!records[0].mods);
    }
}
