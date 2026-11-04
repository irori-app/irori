//! Achievements across the library. Every answer a store gives
//! ([`Backend::achievements`]) is summed up per game in
//! `state/achievements.json`, which the stats page totals without asking
//! any store. After a session the game's achievements are asked for again
//! and, when `iroriAchievementNotices` is on, new unlocks are announced.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::util::{parse_rfc3339, unix_now};
use irori_core::{Achievement, GameId, Notice, NoticeLevel, Paths, json_store};
use parking_lot::RwLock;
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
            recent: unlocked
                .iter()
                .take(RECENT_PER_GAME)
                .map(|a| unlock(a))
                .collect(),
            rarest,
        }
    }

    pub fn is_perfect(&self) -> bool {
        self.total > 0 && self.unlocked == self.total
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
            .flat_map(|(_, g)| &g.recent)
            .filter_map(|u| parse_rfc3339(&u.at))
            .filter(|t| (from..until).contains(t))
            .count()
    }
}

/// The index and where it lives.
pub(crate) struct AchievementStore {
    path: PathBuf,
    index: RwLock<Option<Arc<AchievementIndex>>>,
    /// One read-modify-write at a time.
    write: tokio::sync::Mutex<()>,
}

impl Default for AchievementStore {
    fn default() -> Self {
        Self {
            path: Paths::get().state_dir().join("achievements.json"),
            index: RwLock::new(None),
            write: tokio::sync::Mutex::new(()),
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
        let before = if list.is_empty() {
            index.games.remove(&key)
        } else {
            index.games.insert(key, GameAchievements::from_list(list))
        };
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
        let store = self
            .store(id.runner)
            .with_context(|| format!("{} is not available", id.runner.store_name()))?;
        let list = store.achievements(&id.app_name, language).await?;
        self.achievement_store.record(id.key(), &list).await;
        Ok(Arc::new(list))
    }

    /// What every game's achievements add up to, as last answered.
    pub async fn achievement_index(&self) -> Arc<AchievementIndex> {
        self.achievement_store.current().await
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
        let Some(store) = self.store(id.runner) else {
            return;
        };
        let known = self
            .achievement_store
            .current()
            .await
            .games
            .get(&id.key())
            .cloned();
        store.forget_achievements(&id.app_name).await;
        let language = self.settings.app().ui_language.clone();
        let list = match store.achievements(&id.app_name, &language).await {
            Ok(list) => list,
            Err(e) => {
                tracing::debug!("achievements of {id} after playing: {e:#}");
                return;
            }
        };
        let before = self.achievement_store.record(id.key(), &list).await;
        if !self.settings.app().achievement_notices {
            return;
        }
        // Without an earlier answer every unlock would look new.
        let Some(before) = known.or(before) else {
            return;
        };
        let fresh = new_unlocks(&before, &list);
        if let Some(notice) = unlock_notice(id, title, &fresh) {
            self.bus.notice(notice);
        }
    }
}

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
