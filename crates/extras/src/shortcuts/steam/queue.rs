//! Steam shortcut changes asked for while Steam runs (always the case in
//! Steam's Game Mode). Steam rewrites `shortcuts.vdf` when it exits, so
//! nothing is written while it runs: the change waits in
//! `state/steam-shortcut-queue.json` and [`apply`] makes it once Steam has
//! closed. A later change of a game replaces an earlier one, so adding a
//! game and then removing it leaves only the removal.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, Result};
use irori_core::{AppSettings, GameId, GameInfo, Paths, Runner, json_store};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{LauncherCommand, SteamGame, SteamUser, SyncReport};

/// What waits to happen to one game's shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Add,
    Remove,
}

/// The changes waiting for Steam to close.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    /// Every one of Irori's shortcuts goes, before the changes in `games`.
    #[serde(default)]
    pub remove_all: bool,
    /// By game key (`runner:app`).
    #[serde(default)]
    pub games: BTreeMap<String, Change>,
}

impl Queue {
    /// Add (or keep) the shortcuts of `ids`.
    pub fn add<'a>(&mut self, ids: impl IntoIterator<Item = &'a GameId>) {
        for id in ids {
            self.games.insert(id.key(), Change::Add);
        }
    }

    /// Remove the shortcuts of `ids`, or every one of Irori's with `None`.
    pub fn remove(&mut self, ids: Option<&HashSet<GameId>>) {
        match ids {
            Some(ids) => {
                for id in ids {
                    self.games.insert(id.key(), Change::Remove);
                }
            }
            None => {
                self.remove_all = true;
                self.games.clear();
            }
        }
    }

    /// How many changes wait: one per game, plus one for "remove all".
    pub fn len(&self) -> usize {
        self.games.len() + usize::from(self.remove_all)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The games whose shortcuts are to be added.
    pub fn adds(&self) -> Vec<GameId> {
        self.of(Change::Add).collect()
    }

    /// The games whose shortcuts are to be removed; `None` for every one of
    /// Irori's.
    pub fn removals(&self) -> Option<HashSet<GameId>> {
        (!self.remove_all).then(|| self.of(Change::Remove).collect())
    }

    fn of(&self, change: Change) -> impl Iterator<Item = GameId> + '_ {
        self.games
            .iter()
            .filter(move |(_, c)| **c == change)
            .filter_map(|(key, _)| game_id(key))
    }
}

fn game_id(key: &str) -> Option<GameId> {
    let (runner, app) = key.split_once(':')?;
    Some(GameId::new(Runner::from_id(runner)?, app))
}

/// Guards the file: a change and [`apply`] never interleave.
static LOCK: Mutex<()> = Mutex::const_new(());
/// The length of the queue on disk, for screens that can't read files.
static WAITING: AtomicUsize = AtomicUsize::new(0);

fn queue_path() -> PathBuf {
    Paths::get().state_dir().join("steam-shortcut-queue.json")
}

async fn read() -> Queue {
    let path = queue_path();
    tokio::task::spawn_blocking(move || json_store::read_or_default::<Queue>(&path))
        .await
        .unwrap_or_default()
}

async fn write(queue: Queue) -> Result<()> {
    let path = queue_path();
    let len = queue.len();
    tokio::task::spawn_blocking(move || {
        if queue.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            json_store::write(&path, &queue)
        }
    })
    .await
    .context("saving the waiting Steam shortcut changes stopped")??;
    WAITING.store(len, Ordering::Relaxed);
    Ok(())
}

/// How many shortcut changes wait for Steam to close (as last read or
/// written; [`load`] reads it at start).
pub fn waiting() -> usize {
    WAITING.load(Ordering::Relaxed)
}

/// Read the queue left by an earlier run; returns its length.
pub async fn load() -> usize {
    let _guard = LOCK.lock().await;
    let len = read().await.len();
    WAITING.store(len, Ordering::Relaxed);
    len
}

/// Change the queue on disk.
pub async fn change(f: impl FnOnce(&mut Queue)) -> Result<()> {
    let _guard = LOCK.lock().await;
    let mut queue = read().await;
    f(&mut queue);
    write(queue).await
}

/// Make the waiting changes when Steam is closed: removals first, then the
/// additions of the games `lookup` still finds. `Ok(None)` when nothing
/// waits or Steam still runs. The queue is emptied after a try even when it
/// fails (a missing or broken Steam folder doesn't heal by retrying every
/// minute); the error says what went wrong.
pub async fn apply(
    app: &AppSettings,
    lookup: impl Fn(&GameId) -> Option<Arc<GameInfo>>,
) -> Result<Option<SyncReport>> {
    let _guard = LOCK.lock().await;
    let queue = read().await;
    if queue.is_empty() {
        WAITING.store(0, Ordering::Relaxed);
        return Ok(None);
    }
    if super::is_running(&super::steam_path(app)).await {
        return Ok(None);
    }
    let result = apply_queue(&queue, app, lookup).await;
    write(Queue::default()).await?;
    result.map(Some)
}

async fn apply_queue(
    queue: &Queue,
    app: &AppSettings,
    lookup: impl Fn(&GameId) -> Option<Arc<GameInfo>>,
) -> Result<SyncReport> {
    let users = super::users(&super::steam_path(app)).await?;
    let games: Vec<Arc<GameInfo>> = queue
        .adds()
        .iter()
        .filter_map(&lookup)
        .filter(|g| !g.is_dlc)
        .collect();
    let mut ids = Vec::with_capacity(games.len());
    for game in &games {
        ids.push(crate::wiki::steam_app_id(game).await);
    }
    let list: Vec<SteamGame<'_>> = games
        .iter()
        .zip(&ids)
        .map(|(game, id)| SteamGame {
            game,
            steam_app_id: id.as_deref(),
        })
        .collect();
    apply_for_users(&users, queue.removals(), &list).await
}

/// The removals, then the additions, for the given users, with Steam known
/// to be closed.
async fn apply_for_users(
    users: &[SteamUser],
    removals: Option<HashSet<GameId>>,
    games: &[SteamGame<'_>],
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    if removals.as_ref().is_none_or(|r| !r.is_empty()) {
        let removed = super::remove_for_users(users, removals.as_ref()).await?;
        report.removed = removed.removed;
        report.users_changed = removed.users_changed;
        report.errors = removed.errors;
    }
    if !games.is_empty() {
        let prepared = super::prepare(games, &LauncherCommand::detect()).await;
        let synced = super::sync_users(users, &prepared, None).await?;
        report.added = synced.added;
        report.updated = synced.updated;
        report.users_changed = report.users_changed.max(synced.users_changed);
        report.errors.extend(synced.errors);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    #[test]
    fn a_later_change_replaces_an_earlier_one() {
        let mut q = Queue::default();
        q.add([&id("1"), &id("2")]);
        q.remove(Some(&HashSet::from([id("1")])));
        assert_eq!(q.adds(), [id("2")]);
        assert_eq!(q.removals(), Some(HashSet::from([id("1")])));
        assert_eq!(q.len(), 2);
        q.add([&id("1")]);
        assert_eq!(q.removals(), Some(HashSet::new()));
    }

    #[test]
    fn removing_every_shortcut_drops_earlier_changes_and_keeps_later_adds() {
        let mut q = Queue::default();
        q.add([&id("1")]);
        q.remove(None);
        assert!(q.games.is_empty() && q.remove_all);
        assert_eq!(q.removals(), None);
        q.add([&id("2")]);
        assert_eq!(q.adds(), [id("2")]);
        assert_eq!(q.len(), 2);
    }

    #[tokio::test]
    async fn applies_removals_before_additions() {
        crate::util::test_paths();
        let dir = tempfile::tempdir().unwrap();
        let users = vec![SteamUser {
            id: "1".into(),
            config_dir: dir.path().join("1/config"),
        }];
        let game = |app: &str| GameInfo {
            runner: Some(Runner::Sideload),
            app_name: app.into(),
            title: format!("Game {app}"),
            ..Default::default()
        };
        let (a, b) = (game("a"), game("b"));
        let first = [a.clone(), b.clone()];
        let steam: Vec<SteamGame<'_>> = first
            .iter()
            .map(|game| SteamGame {
                game,
                steam_app_id: None,
            })
            .collect();
        let added = apply_for_users(&users, Some(HashSet::new()), &steam)
            .await
            .unwrap();
        assert_eq!((added.added, added.removed), (2, 0));

        let mut q = Queue::default();
        q.remove(Some(&HashSet::from([a.id()])));
        let report = apply_for_users(&users, q.removals(), &[]).await.unwrap();
        assert_eq!(report.removed, 1);
        let file = super::super::read_file(&users[0].shortcuts_file())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            super::super::sync::own_games(&file),
            HashSet::from([b.id()])
        );
    }

    #[test]
    fn reads_what_it_writes_and_skips_unknown_runners() {
        let mut q = Queue::default();
        q.add([&id("a:b")]);
        q.games.insert("nowhere:x".into(), Change::Add);
        let text = serde_json::to_string(&q).unwrap();
        let back: Queue = serde_json::from_str(&text).unwrap();
        assert_eq!(back, q);
        assert_eq!(back.adds(), [id("a:b")]);
        assert_eq!(
            serde_json::from_str::<Queue>("{}").unwrap(),
            Queue::default()
        );
    }
}
