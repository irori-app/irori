//! Achievements across the library. Every answer a store gives
//! ([`Backend::achievements`]) is summed up per game in
//! `state/achievements.json`, which the stats page totals without asking
//! any store. Each game's unlock history there only grows: an unlock a
//! later answer leaves out (another account, a store that forgot it) stays,
//! with the earliest time known. After a session the game's achievements
//! are asked for again and, when `iroriAchievementNotices` is on, new
//! unlocks are announced.
//!
//! While a game runs, with notices on, unlocks are announced as they
//! happen ([`Event::AchievementsUnlocked`], shown over the game by the UI).
//! Services follow the store's own signal where it has one
//! ([`AchievementSignal`]: GOG's Galaxy service reports unlocks, Steam
//! writes the game's stats file) and ask nothing in between; otherwise
//! they ask every few minutes, and only during the session.
//!
//! With `iroriFileAchievements` on, games whose files hold an emulator's
//! achievements answer from those files instead of their store
//! ([`crate::file_achievements`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::util::{parse_rfc3339, unix_now};
use irori_core::{
    Achievement, AchievementSignal, Event, GameId, Notice, NoticeLevel, Paths, json_store,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::Backend;

/// Unlocks kept per game for "recently unlocked" lists.
const RECENT_PER_GAME: usize = 5;
/// Names an unlock notice lists before "and N more".
const NOTICE_NAMES: usize = 3;

/// One unlocked achievement, as the totals keep it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Unlock {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// RFC 3339; empty when the store gave no date.
    pub at: String,
    pub rarity: f32,
    pub points: u32,
    pub hardcore: bool,
}

/// What one game's achievements add up to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameAchievements {
    pub total: usize,
    pub unlocked: usize,
    pub hardcore: usize,
    pub points_total: u64,
    pub points_unlocked: u64,
    /// Unix time of the answer.
    pub updated: u64,
    /// Every unlocked id, to tell new unlocks from old ones.
    pub unlocked_ids: BTreeSet<String>,
    /// The latest unlocks, newest first.
    pub recent: Vec<Unlock>,
    /// The rarest unlock.
    pub rarest: Option<Unlock>,
    /// Every unlock ever answered, newest first (undated ones last). Only
    /// grows: see [`GameAchievements::keep_history`].
    pub history: Vec<Unlock>,
}

impl GameAchievements {
    pub fn from_list(list: &[Achievement]) -> Self {
        let unlock = |a: &Achievement| Unlock {
            id: a.id.clone(),
            name: a.name.clone(),
            icon: a.image_unlocked.clone(),
            at: a.unlock_time().unwrap_or_default().to_string(),
            rarity: a.rarity,
            points: a.points,
            hardcore: a.hardcore_unlocked_at.is_some(),
        };
        let mut unlocked: Vec<&Achievement> = list.iter().filter(|a| a.is_unlocked()).collect();
        unlocked.sort_by_key(|a| std::cmp::Reverse(a.unlock_time().and_then(parse_rfc3339)));
        let rarest = unlocked
            .iter()
            .filter(|a| a.rarity > 0.0)
            .min_by(|a, b| a.rarity.total_cmp(&b.rarity))
            .map(|a| unlock(a));
        let history: Vec<Unlock> = unlocked.iter().map(|a| unlock(a)).collect();
        Self {
            total: list.len(),
            unlocked: unlocked.len(),
            hardcore: list
                .iter()
                .filter(|a| a.hardcore_unlocked_at.is_some())
                .count(),
            points_total: list.iter().map(|a| a.points as u64).sum(),
            points_unlocked: unlocked.iter().map(|a| a.points as u64).sum(),
            updated: unix_now(),
            unlocked_ids: unlocked.iter().map(|a| a.id.clone()).collect(),
            recent: history.iter().take(RECENT_PER_GAME).cloned().collect(),
            rarest,
            history,
        }
    }

    pub fn is_perfect(&self) -> bool {
        self.total > 0 && self.unlocked == self.total
    }

    /// Keep what `before` knew that this answer doesn't: unlocks it leaves
    /// out stay in the history (and count as seen), and an unlock keeps the
    /// earliest time either gave it. Names and icons follow this answer.
    pub fn keep_history(&mut self, before: &GameAchievements) {
        // Totals written before the history was kept have only the latest.
        let earlier = if before.history.is_empty() {
            &before.recent
        } else {
            &before.history
        };
        let by_id: HashMap<String, usize> = self
            .history
            .iter()
            .enumerate()
            .map(|(ix, u)| (u.id.clone(), ix))
            .collect();
        for old in earlier {
            match by_id.get(&old.id) {
                Some(&ix) => {
                    let now = &mut self.history[ix];
                    let earlier_time = match (parse_rfc3339(&old.at), parse_rfc3339(&now.at)) {
                        (Some(a), Some(b)) => a < b,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if earlier_time {
                        now.at = old.at.clone();
                    }
                }
                None => self.history.push(old.clone()),
            }
        }
        self.history
            .sort_by_key(|u| std::cmp::Reverse(parse_rfc3339(&u.at).unwrap_or(i64::MIN)));
        self.unlocked_ids
            .extend(before.unlocked_ids.iter().cloned());
        self.unlocked_ids
            .extend(self.history.iter().map(|u| u.id.clone()));
        self.recent = self.history.iter().take(RECENT_PER_GAME).cloned().collect();
    }

    /// The unlocks to count by time: the history, or the latest few in
    /// totals written before it was kept.
    fn dated_unlocks(&self) -> &[Unlock] {
        if self.history.is_empty() {
            &self.recent
        } else {
            &self.history
        }
    }
}

/// Every game's achievements known so far, keyed by [`GameId::key`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AchievementIndex {
    pub games: BTreeMap<String, GameAchievements>,
}

/// Achievements of the whole library.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AchievementTotals {
    /// Games with achievements.
    pub games: usize,
    pub total: usize,
    pub unlocked: usize,
    pub hardcore: usize,
    pub perfect: usize,
    pub points: u64,
    /// The latest unlocks across games, newest first, with their game key.
    pub recent: Vec<(String, Unlock)>,
    /// The rarest unlock across games, with its game key.
    pub rarest: Option<(String, Unlock)>,
}

impl AchievementIndex {
    /// Sum up the games `counts` accepts (games still in the library).
    pub fn totals(&self, counts: impl Fn(&str) -> bool, recent: usize) -> AchievementTotals {
        let mut t = AchievementTotals::default();
        let mut all_recent: Vec<(String, Unlock)> = Vec::new();
        for (key, g) in self.games.iter().filter(|(k, g)| g.total > 0 && counts(k)) {
            t.games += 1;
            t.total += g.total;
            t.unlocked += g.unlocked;
            t.hardcore += g.hardcore;
            t.points += g.points_unlocked;
            t.perfect += g.is_perfect() as usize;
            all_recent.extend(g.recent.iter().map(|u| (key.clone(), u.clone())));
            if let Some(r) = &g.rarest
                && t.rarest.as_ref().is_none_or(|(_, b)| r.rarity < b.rarity)
            {
                t.rarest = Some((key.clone(), r.clone()));
            }
        }
        all_recent.sort_by_key(|(_, u)| std::cmp::Reverse(parse_rfc3339(&u.at)));
        all_recent.truncate(recent);
        t.recent = all_recent;
        t
    }

    /// Unlocks in `[from, until)` (Unix times) across `counts`' games.
    pub fn unlocked_between(&self, counts: impl Fn(&str) -> bool, from: i64, until: i64) -> usize {
        self.games
            .iter()
            .filter(|(k, _)| counts(k))
            .flat_map(|(_, g)| g.dated_unlocks())
            .filter_map(|u| parse_rfc3339(&u.at))
            .filter(|t| (from..until).contains(t))
            .count()
    }
}

/// Unlocks of a running game waiting for the UI, at most this many per game.
const LIVE_PER_GAME: usize = 50;

/// The index and where it lives.
pub(crate) struct AchievementStore {
    path: PathBuf,
    index: RwLock<Option<Arc<AchievementIndex>>>,
    /// One read-modify-write at a time.
    write: tokio::sync::Mutex<()>,
    /// Unlocks a running game made that the UI hasn't shown yet, by game key.
    live: Mutex<HashMap<String, Vec<Achievement>>>,
}

impl Default for AchievementStore {
    fn default() -> Self {
        Self {
            path: Paths::get().state_dir().join("achievements.json"),
            index: RwLock::new(None),
            write: tokio::sync::Mutex::new(()),
            live: Mutex::new(HashMap::new()),
        }
    }
}

impl AchievementStore {
    async fn current(&self) -> Arc<AchievementIndex> {
        if let Some(i) = self.index.read().clone() {
            return i;
        }
        let path = self.path.clone();
        let loaded: AchievementIndex =
            tokio::task::spawn_blocking(move || json_store::read_or_default(&path))
                .await
                .unwrap_or_default();
        let loaded = Arc::new(loaded);
        let mut slot = self.index.write();
        slot.get_or_insert(loaded).clone()
    }

    /// Record a game's answer; returns what it was before.
    async fn record(&self, key: String, list: &[Achievement]) -> Option<GameAchievements> {
        let _one = self.write.lock().await;
        let mut index = (*self.current().await).clone();
        let before = index.games.get(&key).cloned();
        let mut now = GameAchievements::from_list(list);
        if let Some(before) = &before {
            now.keep_history(before);
        }
        if now.history.is_empty() && list.is_empty() {
            index.games.remove(&key);
        } else {
            index.games.insert(key, now);
        }
        let index = Arc::new(index);
        *self.index.write() = Some(index.clone());
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || json_store::write(&path, &*index)).await;
        if let Ok(Err(e)) = result {
            tracing::warn!("saving achievement totals: {e:#}");
        }
        before
    }
}

/// Achievements of `list` unlocked that `before` didn't have.
pub fn new_unlocks<'a>(before: &GameAchievements, list: &'a [Achievement]) -> Vec<&'a Achievement> {
    list.iter()
        .filter(|a| a.is_unlocked() && !before.unlocked_ids.contains(&a.id))
        .collect()
}

impl Backend {
    /// A game's achievements from its store, recorded for the totals.
    pub async fn achievements(&self, id: &GameId, language: &str) -> Result<Arc<Vec<Achievement>>> {
        let list = match self.file_achievements(id, language).await {
            Some(answer) => answer?,
            None => {
                let store = self
                    .store(id.runner)
                    .with_context(|| format!("{} is not available", id.runner.store_name()))?;
                store.achievements(&id.app_name, language).await?
            }
        };
        self.achievement_store.record(id.key(), &list).await;
        Ok(Arc::new(list))
    }

    /// What every game's achievements add up to, as last answered.
    pub async fn achievement_index(&self) -> Arc<AchievementIndex> {
        self.achievement_store.current().await
    }

    /// How many of a game's achievements are unlocked, of how many, as last
    /// answered; `None` before an answer was read this run.
    pub fn achievement_progress(&self, id: &GameId) -> Option<(usize, usize)> {
        let index = self.achievement_store.index.read().clone()?;
        let game = index.games.get(&id.key())?;
        (game.total > 0).then_some((game.unlocked, game.total))
    }

    /// Every unlock of a game Irori has seen, newest first.
    pub async fn achievement_history(&self, id: &GameId) -> Vec<Unlock> {
        self.achievement_store
            .current()
            .await
            .games
            .get(&id.key())
            .map(|g| g.history.clone())
            .unwrap_or_default()
    }

    /// The unlocks a running game made since the UI last took them, oldest
    /// first (see [`Event::AchievementsUnlocked`]).
    pub fn take_live_unlocks(&self, id: &GameId) -> Vec<Achievement> {
        self.achievement_store
            .live
            .lock()
            .remove(&id.key())
            .unwrap_or_default()
    }

    /// Ask the stores for the achievements of `ids`, one game after
    /// another, so the totals cover games not opened yet. Returns how many
    /// have achievements; a game whose store fails is skipped.
    pub async fn scan_achievements(&self, ids: Vec<GameId>) -> usize {
        let language = self.settings.app().ui_language.clone();
        let mut found = 0;
        for id in ids {
            if !self.is_online() {
                break;
            }
            match self.achievements(&id, &language).await {
                Ok(list) if !list.is_empty() => found += 1,
                Ok(_) => {}
                Err(e) => tracing::debug!("achievements of {id}: {e:#}"),
            }
        }
        found
    }

    /// After a session: ask again (unlocks happen while playing) and, when
    /// the setting is on, announce what the session unlocked.
    pub(crate) async fn after_session_achievements(&self, id: &GameId, title: &str) {
        // The game is gone: nothing is shown over it any more.
        self.achievement_store.live.lock().remove(&id.key());
        self.check_unlocks(id, title, Announce::Notice).await;
    }

    /// While a game runs, with `iroriAchievementNotices` on: announce new
    /// unlocks as they happen, so they show over the game rather than only
    /// after it. The store's own signal is followed where it has one (see
    /// the module docs) and nothing is asked in between; otherwise unlocks
    /// the game keeps in its own files are read every minute and a store is
    /// asked every few.
    pub(crate) async fn watch_achievements_while_playing(
        &self,
        id: GameId,
        title: String,
        stop: tokio_util::sync::CancellationToken,
    ) {
        // Once at the start, so the first unlock is told from older ones.
        let source = loop {
            if self.is_online() {
                break self.check_unlocks(&id, &title, Announce::Quietly).await;
            }
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(LIVE_STORE) => {}
            }
        };
        let signal = match source {
            // No achievements to watch.
            None => return,
            Some(Source::Files) => AchievementSignal::None,
            Some(Source::Store) => match self.store(id.runner) {
                Some(store) => store.achievement_signal(&id.app_name).await,
                None => return,
            },
        };
        let (tx, mut signals) = tokio::sync::mpsc::channel::<()>(8);
        let mut every = Some(if source == Some(Source::Files) {
            LIVE_FILES
        } else {
            LIVE_STORE
        });
        let _watcher = match &signal {
            AchievementSignal::None => None,
            AchievementSignal::Events => {
                self.forward_signals(id.clone(), tx.clone(), stop.clone());
                every = None;
                None
            }
            AchievementSignal::Files(files) => {
                let watcher = watch_files(files, tx.clone());
                if watcher.is_some() {
                    every = None;
                }
                watcher
            }
        };
        drop(tx);
        tracing::debug!(
            "following {id}'s achievements: {}",
            match (&signal, every) {
                (_, Some(_)) => "asking now and then",
                (AchievementSignal::Events, None) => "the store's reports",
                _ => "the store's files",
            }
        );
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(every.unwrap_or_default()), if every.is_some() => {}
                Some(()) = signals.recv() => {
                    // A store writes its file or reports several times in a
                    // row: ask once they settle.
                    tokio::time::sleep(SETTLE).await;
                    while signals.try_recv().is_ok() {}
                }
            }
            if !self.settings.app().achievement_notices || !self.is_online() {
                continue;
            }
            if self
                .check_unlocks(&id, &title, Announce::OverGame)
                .await
                .is_none()
            {
                // The answers stopped having achievements: stop asking.
                return;
            }
        }
    }

    /// Pass the store's [`Event::AchievementSignal`]s for `id` to `tx`
    /// until `stop`.
    fn forward_signals(
        &self,
        id: GameId,
        tx: tokio::sync::mpsc::Sender<()>,
        stop: tokio_util::sync::CancellationToken,
    ) {
        let mut events = self.bus.subscribe();
        self.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = stop.cancelled() => return,
                    event = events.recv() => event,
                };
                match event {
                    Ok(Event::AchievementSignal(game)) if game == id => {
                        let _ = tx.try_send(());
                    }
                    Ok(_) => {}
                    // The missed events may have held one: ask anyway.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = tx.try_send(());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// Ask for `id`'s achievements past every cache, record them and, with
    /// notices on, announce the unlocks new since the last answer as
    /// `announce` says. Returns where the answer came from; `None` when
    /// nothing answered.
    async fn check_unlocks(&self, id: &GameId, title: &str, announce: Announce) -> Option<Source> {
        let known = self
            .achievement_store
            .current()
            .await
            .games
            .get(&id.key())
            .cloned();
        let language = self.settings.app().ui_language.clone();
        // Game files are read afresh every time; a store is asked past its cache.
        let (answer, source) = match self.file_achievements(id, &language).await {
            Some(answer) => (answer, Source::Files),
            None => {
                let store = self.store(id.runner)?;
                store.forget_achievements(&id.app_name).await;
                (
                    store.achievements(&id.app_name, &language).await,
                    Source::Store,
                )
            }
        };
        let list = match answer {
            Ok(list) => list,
            Err(e) => {
                tracing::debug!("achievements of {id}: {e:#}");
                // A failed request isn't "no achievements": keep watching.
                return Some(source);
            }
        };
        if list.is_empty() {
            return None;
        }
        let before = self.achievement_store.record(id.key(), &list).await;
        if announce == Announce::Quietly || !self.settings.app().achievement_notices {
            return Some(source);
        }
        // Without an earlier answer every unlock would look new.
        let Some(before) = known.or(before) else {
            return Some(source);
        };
        let fresh = new_unlocks(&before, &list);
        if fresh.is_empty() {
            return Some(source);
        }
        if announce == Announce::OverGame {
            {
                let mut waiting = self.achievement_store.live.lock();
                let queue = waiting.entry(id.key()).or_default();
                queue.extend(oldest_first(&fresh).into_iter().cloned());
                let over = queue.len().saturating_sub(LIVE_PER_GAME);
                queue.drain(..over);
            }
            self.bus.emit(Event::AchievementsUnlocked(id.clone()));
        } else if let Some(notice) = unlock_notice(id, title, &fresh) {
            self.bus.notice(notice);
        }
        Some(source)
    }
}

/// `fresh` in the order they were unlocked (undated ones last).
fn oldest_first<'a>(fresh: &[&'a Achievement]) -> Vec<&'a Achievement> {
    let mut out = fresh.to_vec();
    out.sort_by_key(|a| a.unlock_time().and_then(parse_rfc3339).unwrap_or(i64::MAX));
    out
}

/// Watch the folders of `files` and send to `tx` when one of them is
/// written. `None` when no folder could be watched.
fn watch_files(
    files: &[PathBuf],
    tx: tokio::sync::mpsc::Sender<()>,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;
    // By name: the store may reach the folder through a link.
    let names: HashSet<std::ffi::OsString> = files
        .iter()
        .filter_map(|f| f.file_name().map(|n| n.to_os_string()))
        .collect();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && !event.kind.is_access()
            && event
                .paths
                .iter()
                .any(|p| p.file_name().is_some_and(|n| names.contains(n)))
        {
            let _ = tx.try_send(());
        }
    })
    .ok()?;
    let folders: BTreeSet<PathBuf> = files
        .iter()
        .filter_map(|f| f.parent().map(PathBuf::from))
        .filter(|d| d.is_dir())
        .collect();
    let mut watched = false;
    for dir in folders {
        match watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            Ok(()) => watched = true,
            Err(e) => tracing::debug!("watching {}: {e}", dir.display()),
        }
    }
    watched.then_some(watcher)
}

/// Where a game's achievements were read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Files,
    Store,
}

/// How [`Backend::check_unlocks`] tells of new unlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announce {
    /// Not at all: the answer is what later ones are compared with.
    Quietly,
    /// To the UI, which shows them over the running game.
    OverGame,
    /// As a notice (after the session).
    Notice,
}

/// How often achievements are read while playing: game files (local) and
/// store APIs (one request per game, kept polite).
const LIVE_FILES: std::time::Duration = std::time::Duration::from_secs(60);
const LIVE_STORE: std::time::Duration = std::time::Duration::from_secs(3 * 60);
/// How long after a store's signal it is asked, so its writes settle.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// The notice announcing `fresh` unlocks, if there are any.
pub fn unlock_notice(id: &GameId, title: &str, fresh: &[&Achievement]) -> Option<Notice> {
    let first = fresh.first()?;
    let names: Vec<&str> = fresh
        .iter()
        .take(NOTICE_NAMES)
        .map(|a| a.name.as_str())
        .collect();
    let names = names.join(", ");
    let more = fresh.len().saturating_sub(NOTICE_NAMES);
    let (title_key, message_key) = match (fresh.len(), more) {
        (1, _) => (
            "shell.notice.achievement_unlocked",
            "shell.notice.achievement_unlocked_message",
        ),
        (_, 0) => (
            "shell.notice.achievements_unlocked",
            "shell.notice.achievements_unlocked_message",
        ),
        _ => (
            "shell.notice.achievements_unlocked",
            "shell.notice.achievements_unlocked_more_message",
        ),
    };
    let english = if fresh.len() == 1 {
        format!("Achievement unlocked in {title}")
    } else {
        format!("{} achievements unlocked in {title}", fresh.len())
    };
    Some(
        Notice::new(NoticeLevel::Success, english)
            .message(if fresh.len() == 1 {
                first.name.clone()
            } else {
                names.clone()
            })
            .keys(
                title_key,
                Some(message_key),
                &[
                    ("title", title),
                    ("count", &fresh.len().to_string()),
                    ("name", &first.name),
                    ("names", &names),
                    ("more", &more.to_string()),
                ],
            )
            .game(id.clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn a(id: &str, at: Option<&str>, rarity: f32, points: u32) -> Achievement {
        Achievement {
            id: id.into(),
            name: id.to_uppercase(),
            unlocked_at: at.map(String::from),
            rarity,
            points,
            ..Default::default()
        }
    }

    #[test]
    fn sums_games_and_finds_the_rarest() {
        let g1 = GameAchievements::from_list(&[
            a("x", Some("2024-01-01T00:00:00Z"), 40.0, 5),
            a("y", Some("2024-03-01T00:00:00Z"), 2.0, 10),
            a("z", None, 1.0, 20),
        ]);
        assert_eq!(
            (g1.total, g1.unlocked, g1.points_unlocked, g1.points_total),
            (3, 2, 15, 35)
        );
        assert_eq!(g1.recent[0].id, "y");
        assert_eq!(g1.rarest.as_ref().unwrap().id, "y");
        let g2 = GameAchievements::from_list(&[a("p", Some("2024-02-01T00:00:00Z"), 0.5, 0)]);
        assert!(g2.is_perfect());
        let index = AchievementIndex {
            games: BTreeMap::from([("gog:1".into(), g1), ("steam:2".into(), g2)]),
        };
        let t = index.totals(|_| true, 2);
        assert_eq!(
            (t.games, t.total, t.unlocked, t.perfect, t.points),
            (2, 4, 3, 1, 15)
        );
        assert_eq!(
            t.recent
                .iter()
                .map(|(_, u)| u.id.as_str())
                .collect::<Vec<_>>(),
            ["y", "p"]
        );
        assert_eq!(t.rarest.unwrap().1.id, "p");
        let only_gog = index.totals(|k| k.starts_with("gog:"), 10);
        assert_eq!(only_gog.games, 1);
        let jan = parse_rfc3339("2024-01-01T00:00:00Z").unwrap();
        let feb = parse_rfc3339("2024-02-15T00:00:00Z").unwrap();
        assert_eq!(index.unlocked_between(|_| true, jan, feb), 2);
    }

    #[test]
    fn the_history_only_grows() {
        let first = GameAchievements::from_list(&[
            a("x", Some("2024-03-01T00:00:00Z"), 1.0, 0),
            a("y", Some("2024-01-01T00:00:00Z"), 1.0, 0),
        ]);
        // Another account (or a store that forgot): x is gone, y is dated
        // later, z is new.
        let mut later = GameAchievements::from_list(&[
            a("y", Some("2024-06-01T00:00:00Z"), 1.0, 0),
            a("z", Some("2024-05-01T00:00:00Z"), 1.0, 0),
            a("w", None, 1.0, 0),
        ]);
        later.keep_history(&first);
        let history: Vec<(&str, &str)> = later
            .history
            .iter()
            .map(|u| (u.id.as_str(), u.at.as_str()))
            .collect();
        assert_eq!(
            history,
            [
                ("z", "2024-05-01T00:00:00Z"),
                ("x", "2024-03-01T00:00:00Z"),
                ("y", "2024-01-01T00:00:00Z"),
            ],
            "newest first, each at its earliest time"
        );
        assert_eq!(later.unlocked, 2, "the totals follow the answer");
        assert!(
            later.unlocked_ids.contains("x"),
            "x isn't new if it returns"
        );
        assert_eq!(later.recent[0].id, "z");

        // Totals written before the history was kept: their latest unlocks.
        let mut old = first.clone();
        old.history.clear();
        let mut now = GameAchievements::from_list(&[]);
        now.keep_history(&old);
        assert_eq!(now.history.len(), 2);
        let index = AchievementIndex {
            games: BTreeMap::from([("gog:1".into(), now)]),
        };
        let since = parse_rfc3339("2024-02-01T00:00:00Z").unwrap();
        assert_eq!(index.unlocked_between(|_| true, since, i64::MAX), 1);
        assert_eq!(index.totals(|_| true, 5).games, 0, "nothing to total");
    }

    #[test]
    fn live_unlocks_come_oldest_first() {
        let list = [
            a("late", Some("2024-03-01T00:00:00Z"), 1.0, 0),
            a("undated", Some(""), 1.0, 0),
            a("early", Some("2024-01-01T00:00:00Z"), 1.0, 0),
        ];
        let refs: Vec<&Achievement> = list.iter().collect();
        let ids: Vec<&str> = oldest_first(&refs).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["early", "late", "undated"]);
    }

    #[test]
    fn announces_only_new_unlocks() {
        let before = GameAchievements::from_list(&[a("old", Some("2024-01-01T00:00:00Z"), 1.0, 0)]);
        let list = vec![
            a("old", Some("2024-01-01T00:00:00Z"), 1.0, 0),
            a("new1", Some("2024-05-01T00:00:00Z"), 1.0, 0),
            a("locked", None, 1.0, 0),
        ];
        let fresh = new_unlocks(&before, &list);
        assert_eq!(fresh.len(), 1);
        let id = GameId::new(Runner::Gog, "1");
        let n = unlock_notice(&id, "Hades", &fresh).unwrap();
        assert_eq!(n.title_key, Some("shell.notice.achievement_unlocked"));
        assert_eq!(n.message.as_deref(), Some("NEW1"));
        assert!(unlock_notice(&id, "Hades", &[]).is_none());
        let many: Vec<Achievement> = (0..5)
            .map(|i| a(&format!("n{i}"), Some(""), 1.0, 0))
            .collect();
        let refs: Vec<&Achievement> = many.iter().collect();
        let n = unlock_notice(&id, "Hades", &refs).unwrap();
        assert_eq!(
            n.message_key,
            Some("shell.notice.achievements_unlocked_more_message")
        );
        assert!(n.args.contains(&("more", "2".into())));
    }
}
