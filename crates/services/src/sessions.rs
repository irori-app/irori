//! History of play sessions: one JSON object per line in
//! `state/sessions.jsonl`. Appending keeps a long history cheap to write; the
//! totals in `library.json` stay the source of truth for playtime.
//!
//! How often and how long each game was played ([`SessionStats`]) is read
//! from the history once ([`load_stats`], off the UI thread) and then kept
//! in step by every append and replace, so sorting and smart categories
//! read it from memory.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use irori_core::Paths;
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

/// One finished session of one game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `GameId::key()` of the game.
    pub game: String,
    /// RFC 3339 time the game was launched.
    pub started: String,
    /// How long it ran.
    pub seconds: u64,
    /// It exited with an error within seconds of starting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crashed: bool,
    /// Exit code of a crashed session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

fn file() -> PathBuf {
    Paths::get().state_dir().join("sessions.jsonl")
}

/// Add a session to the history.
pub fn append(session: &Session) -> Result<()> {
    let _held = STATS_WRITE.lock();
    append_to(&file(), session)?;
    let current = stats();
    if current.loaded {
        let mut next = (*current).clone();
        next.add(session);
        publish(next);
    }
    Ok(())
}

/// Every recorded session, oldest first. Lines that do not parse (a write
/// cut short) are skipped.
pub fn read_all() -> Vec<Session> {
    read_from(&file())
}

/// Replace the whole history (a restore merged another one into it).
pub fn replace_all(list: &[Session]) -> Result<()> {
    let _held = STATS_WRITE.lock();
    let mut text = String::new();
    for s in list {
        text.push_str(&serde_json::to_string(s)?);
        text.push('\n');
    }
    irori_core::json_store::write_bytes(&file(), text.as_bytes())?;
    publish(SessionStats::from_sessions(list));
    Ok(())
}

/// How often and how long one game was played.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plays {
    /// Sessions, not counting starts that failed within seconds.
    pub count: u32,
    /// Seconds those sessions lasted.
    pub seconds: u64,
}

impl Plays {
    /// The average session in whole minutes; `None` before the first.
    pub fn average_minutes(&self) -> Option<u64> {
        (self.count > 0).then(|| self.seconds / u64::from(self.count) / 60)
    }
}

/// [`Plays`] per game key, from the whole history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStats {
    /// Changes with every change, so cached answers know to look again.
    pub revision: u64,
    /// Whether the history was read; before that no game's plays are
    /// known, which isn't "never played".
    pub loaded: bool,
    games: HashMap<String, Plays>,
}

impl SessionStats {
    /// The stats of `list`, as read.
    pub fn from_sessions(list: &[Session]) -> Self {
        let mut stats = SessionStats {
            loaded: true,
            ..Default::default()
        };
        for s in list {
            stats.add(s);
        }
        stats
    }

    fn add(&mut self, session: &Session) {
        // A start that failed isn't a time the game was played.
        if session.crashed {
            return;
        }
        let plays = self.games.entry(session.game.clone()).or_default();
        plays.count += 1;
        plays.seconds += session.seconds;
    }

    /// The plays of game `key`: zero for a game never played, `None` while
    /// the history isn't read.
    pub fn get(&self, key: &str) -> Option<Plays> {
        self.loaded
            .then(|| self.games.get(key).copied().unwrap_or_default())
    }
}

static STATS: Lazy<RwLock<Arc<SessionStats>>> = Lazy::new(Default::default);
/// Held while the history file and the stats change together.
static STATS_WRITE: Lazy<Mutex<()>> = Lazy::new(Default::default);

fn publish(mut stats: SessionStats) {
    let mut current = STATS.write();
    stats.revision = current.revision + 1;
    *current = Arc::new(stats);
}

/// The stats as last read or kept; empty and not `loaded` until
/// [`load_stats`] ran. Reads nothing from disk.
pub fn stats() -> Arc<SessionStats> {
    STATS.read().clone()
}

/// Read the history into [`stats`]. Blocking: call it off the UI thread
/// and the async runtime.
pub fn load_stats() {
    let _held = STATS_WRITE.lock();
    publish(SessionStats::from_sessions(&read_all()));
}

fn append_to(path: &Path, session: &Session) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(session)?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())?;
    Ok(())
}

fn read_from(path: &Path) -> Vec<Session> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_back_skipping_torn_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("sessions.jsonl");
        let first = Session {
            game: "gog:1".into(),
            started: "2026-10-23T10:00:00Z".into(),
            seconds: 3600,
            crashed: false,
            exit_code: None,
        };
        let second = Session {
            game: "legendary:Fowl".into(),
            started: "2026-10-23T12:00:00Z".into(),
            seconds: 3,
            crashed: true,
            exit_code: None,
        };
        append_to(&path, &first).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"game\":\"gog:2\",\"sta")
            .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        append_to(&path, &second).unwrap();
        assert_eq!(read_from(&path), vec![first, second]);
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"crashed\":false")
        );
    }

    #[test]
    fn counts_plays_without_failed_starts() {
        let session = |game: &str, seconds: u64, crashed: bool| Session {
            game: game.into(),
            started: "2026-10-23T10:00:00Z".into(),
            seconds,
            crashed,
            exit_code: None,
        };
        let stats = SessionStats::from_sessions(&[
            session("gog:1", 3600, false),
            session("gog:1", 1800, false),
            session("gog:1", 2, true),
            session("gog:2", 600, false),
        ]);
        let one = stats.get("gog:1").unwrap();
        assert_eq!((one.count, one.seconds), (2, 5400));
        assert_eq!(one.average_minutes(), Some(45));
        assert_eq!(stats.get("gog:2").unwrap().count, 1);
        assert_eq!(stats.get("gog:3"), Some(Plays::default()));
        assert_eq!(Plays::default().average_minutes(), None);
        // Before the history is read nothing is known.
        assert_eq!(SessionStats::default().get("gog:1"), None);
    }

    #[test]
    fn missing_history_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_from(&dir.path().join("none.jsonl")).is_empty());
    }
}
