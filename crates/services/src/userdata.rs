//! Launcher-owned per-user library data: favourites, hidden games, recently
//! played, custom categories, playtime, play statuses, backlog goals and
//! title/art overrides.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use irori_core::game::{GameOverrides, PlaytimeRecord};
use irori_core::{Event, EventBus, GameId, GameInfo, Paths, json_store, util};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::play_status::{EffectiveStatus, PlayStatus, PlayStatusRecord};

/// Persisted shape. Game keys are `GameId::key()` strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserData {
    pub favourites: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
    /// Most recent first.
    pub recent: Vec<String>,
    /// Category name → game keys.
    pub categories: BTreeMap<String, BTreeSet<String>>,
    pub playtime: BTreeMap<String, PlaytimeRecord>,
    pub overrides: BTreeMap<String, GameOverrides>,
    /// RFC 3339 time each game first appeared in the library: its "date
    /// added" where the store does not say when the account got it.
    pub first_seen: BTreeMap<String, String>,
    /// Games hidden because they are hidden on the store's website. Each is
    /// hidden once; unhiding it here sticks, and unhiding it on the store
    /// unhides it here.
    pub store_hidden_imported: BTreeSet<String>,
    /// The play status the user set per game, with when they set it.
    pub play_status: BTreeMap<String, PlayStatusRecord>,
    /// How many games the user means to beat, by calendar year.
    pub backlog_goals: BTreeMap<i32, u32>,
    /// The user's own rating of each game, from 1 to 5 stars. Unrated games
    /// have no entry.
    #[serde(default)]
    pub ratings: BTreeMap<String, u8>,
}

/// Most stars a game can be rated.
pub const MAX_RATING: u8 = 5;

impl UserData {
    /// The status the user set for a game, if any.
    pub fn play_status(&self, key: &str) -> Option<PlayStatus> {
        self.play_status.get(key).map(|r| r.status)
    }

    /// The status filters and stats group a game under (see
    /// [`EffectiveStatus`]).
    pub fn effective_status(&self, key: &str) -> EffectiveStatus {
        match self.play_status(key) {
            Some(status) => status.into(),
            None if self
                .playtime
                .get(key)
                .is_some_and(|r| r.total_minutes > 0 || r.last_played.is_some()) =>
            {
                EffectiveStatus::Played
            }
            None => EffectiveStatus::NotPlayed,
        }
    }
}

/// Set (or clear) the play status of `keys`, dated `now`. A game already
/// in that status keeps the date it got it.
fn apply_play_status(d: &mut UserData, keys: &[String], status: Option<PlayStatus>, now: &str) {
    for key in keys {
        match status {
            Some(status) if d.play_status(key) == Some(status) => {}
            Some(status) => {
                d.play_status.insert(
                    key.clone(),
                    PlayStatusRecord {
                        status,
                        since: now.to_string(),
                    },
                );
            }
            None => {
                d.play_status.remove(key);
            }
        }
    }
}

/// Which of a store's playtime totals name games, and which stored records
/// name DLC instead. Epic reports playtime per DLC as well, each repeating
/// the base game's time; counted as games they doubled the totals and showed
/// up under raw app names. Totals for app names the store does not list are
/// dropped too, as nothing in the library could show them.
fn split_store_playtime(
    games: &[Arc<GameInfo>],
    totals: &std::collections::HashMap<String, u64>,
) -> (std::collections::HashMap<String, u64>, BTreeSet<String>) {
    let playable: BTreeSet<&str> = games
        .iter()
        .filter(|g| !g.is_dlc)
        .map(|g| g.app_name.as_str())
        .collect();
    let dlc = games
        .iter()
        .flat_map(|g| {
            g.dlcs
                .iter()
                .map(|d| d.app_name.as_str())
                .chain(g.is_dlc.then_some(g.app_name.as_str()))
        })
        .filter(|app| !playable.contains(app))
        .map(str::to_string)
        .collect();
    let totals = totals
        .iter()
        .filter(|(app, _)| playable.contains(app.as_str()))
        .map(|(app, minutes)| (app.clone(), *minutes))
        .collect();
    (totals, dlc)
}

/// Store totals larger than Irori's, by game key.
fn grown_totals(
    data: &UserData,
    runner: irori_core::Runner,
    totals: &std::collections::HashMap<String, u64>,
) -> Vec<(String, u64)> {
    totals
        .iter()
        .map(|(app, minutes)| (GameId::new(runner, app.as_str()).key(), *minutes))
        .filter(|(key, minutes)| *minutes > data.playtime.get(key).map_or(0, |r| r.total_minutes))
        .collect()
}

fn rate(d: &mut UserData, key: String, stars: Option<u8>) {
    match stars.filter(|s| *s > 0) {
        Some(s) => {
            d.ratings.insert(key, s.min(MAX_RATING));
        }
        None => {
            d.ratings.remove(&key);
        }
    }
}

/// What [`UserDataService::sync_library`] changes, worked out from the
/// current data and library.
#[derive(Debug, Default, PartialEq, Eq)]
struct LibrarySync {
    /// Keys seen for the first time.
    new: Vec<String>,
    /// Hidden on the store and not yet followed here.
    hide: Vec<String>,
    /// Hidden here because of the store, which no longer hides them.
    unhide: Vec<String>,
}

impl LibrarySync {
    fn plan(data: &UserData, games: &[Arc<GameInfo>]) -> Self {
        let mut plan = Self::default();
        let mut store_hidden = BTreeSet::new();
        for g in games {
            let key = g.id().key();
            if !data.first_seen.contains_key(&key) {
                plan.new.push(key.clone());
            }
            if g.store_hidden {
                // Hidden here already (by the user or earlier): nothing to do.
                if !data.store_hidden_imported.contains(&key) && !data.hidden.contains(&key) {
                    plan.hide.push(key.clone());
                }
                store_hidden.insert(key);
            } else if data.store_hidden_imported.contains(&key) {
                plan.unhide.push(key);
            }
        }
        plan
    }

    fn is_empty(&self) -> bool {
        self.new.is_empty() && self.hide.is_empty() && self.unhide.is_empty()
    }

    fn apply(&self, d: &mut UserData, now: &str) {
        for key in &self.new {
            d.first_seen.insert(key.clone(), now.to_string());
        }
        for key in &self.hide {
            d.hidden.insert(key.clone());
            d.store_hidden_imported.insert(key.clone());
        }
        for key in &self.unhide {
            // Only games still hidden: one the user unhid here stays unhidden.
            d.hidden.remove(key);
            d.store_hidden_imported.remove(key);
        }
    }
}

pub struct UserDataService {
    data: RwLock<Arc<UserData>>,
    bus: EventBus,
}

fn file() -> PathBuf {
    Paths::get().state_dir().join("library.json")
}

impl UserDataService {
    pub fn load(bus: EventBus) -> Self {
        Self {
            data: RwLock::new(Arc::new(json_store::read_or_default(&file()))),
            bus,
        }
    }

    pub fn snapshot(&self) -> Arc<UserData> {
        self.data.read().clone()
    }

    /// Mutate, persist, and announce. `game` scopes the change event.
    pub fn update(&self, game: Option<&GameId>, f: impl FnOnce(&mut UserData)) {
        let snapshot = {
            let mut guard = self.data.write();
            let mut next = (**guard).clone();
            f(&mut next);
            *guard = Arc::new(next);
            guard.clone()
        };
        if let Err(e) = json_store::write(&file(), &*snapshot) {
            tracing::error!("saving library data: {e:#}");
        }
        match game {
            Some(id) => self.bus.emit(Event::GameChanged(id.clone())),
            None => self.bus.emit(Event::SettingsChanged),
        }
    }

    pub fn set_favourite(&self, id: &GameId, on: bool) {
        self.update(Some(id), |d| {
            if on {
                d.favourites.insert(id.key());
            } else {
                d.favourites.remove(&id.key());
            }
        });
    }

    /// Mark or unmark several games as favourites in one change.
    pub fn set_favourites(&self, ids: &[GameId], on: bool) {
        self.update(None, |d| {
            for id in ids {
                if on {
                    d.favourites.insert(id.key());
                } else {
                    d.favourites.remove(&id.key());
                }
            }
        });
    }

    /// Hide or unhide several games in one change.
    pub fn set_hidden_many(&self, ids: &[GameId], on: bool) {
        self.update(None, |d| {
            for id in ids {
                if on {
                    d.hidden.insert(id.key());
                } else {
                    d.hidden.remove(&id.key());
                }
            }
        });
    }

    /// Add several games to `category`, or take them out, in one change.
    pub fn set_category_many(&self, ids: &[GameId], category: &str, on: bool) {
        self.update(None, |d| {
            let games = d.categories.entry(category.to_string()).or_default();
            for id in ids {
                if on {
                    games.insert(id.key());
                } else {
                    games.remove(&id.key());
                }
            }
        });
    }

    /// Rate a game from 1 to [`MAX_RATING`] stars, or clear its rating with
    /// `None` (or 0). Larger values count as the most stars.
    pub fn set_rating(&self, id: &GameId, stars: Option<u8>) {
        self.update(Some(id), |d| rate(d, id.key(), stars));
    }

    pub fn set_hidden(&self, id: &GameId, on: bool) {
        self.update(Some(id), |d| {
            if on {
                d.hidden.insert(id.key());
            } else {
                d.hidden.remove(&id.key());
            }
        });
    }

    pub fn set_category(&self, id: &GameId, category: &str, on: bool) {
        self.update(Some(id), |d| {
            let set = d.categories.entry(category.to_string()).or_default();
            if on {
                set.insert(id.key());
            } else {
                set.remove(&id.key());
            }
        });
    }

    pub fn create_category(&self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.update(None, |d| {
            d.categories.entry(name.to_string()).or_default();
        });
    }

    pub fn rename_category(&self, from: &str, to: &str) {
        let to = to.trim();
        if to.is_empty() || from == to {
            return;
        }
        self.update(None, |d| {
            if let Some(set) = d.categories.remove(from) {
                d.categories.entry(to.to_string()).or_default().extend(set);
            }
        });
    }

    pub fn delete_category(&self, name: &str) {
        self.update(None, |d| {
            d.categories.remove(name);
        });
    }

    /// Set a game's play status, or clear it with `None`.
    pub fn set_play_status(&self, id: &GameId, status: Option<PlayStatus>) {
        let now = util::now_rfc3339();
        self.update(Some(id), |d| {
            apply_play_status(d, &[id.key()], status, &now)
        });
    }

    /// Set or clear the play status of several games in one change.
    pub fn set_play_status_many(&self, ids: &[GameId], status: Option<PlayStatus>) {
        let now = util::now_rfc3339();
        let keys: Vec<String> = ids.iter().map(GameId::key).collect();
        self.update(None, |d| apply_play_status(d, &keys, status, &now));
    }

    /// Set how many games the user means to beat in `year`; `None` or zero
    /// removes the goal.
    pub fn set_backlog_goal(&self, year: i32, goal: Option<u32>) {
        self.update(None, |d| match goal.filter(|g| *g > 0) {
            Some(goal) => {
                d.backlog_goals.insert(year, goal);
            }
            None => {
                d.backlog_goals.remove(&year);
            }
        });
    }

    /// Drop one game from the recently played list.
    pub fn remove_recent(&self, id: &GameId) {
        let key = id.key();
        self.update(Some(id), |d| d.recent.retain(|k| *k != key));
    }

    pub fn clear_recent(&self) {
        self.update(None, |d| d.recent.clear());
    }

    pub fn set_overrides(&self, id: &GameId, o: GameOverrides) {
        self.update(Some(id), |d| {
            if o.is_empty() {
                d.overrides.remove(&id.key());
            } else {
                d.overrides.insert(id.key(), o);
            }
        });
    }

    /// Record a session start: recent list and first/last played.
    pub fn record_launch(&self, id: &GameId, max_recent: usize) {
        let now = util::now_rfc3339();
        self.update(Some(id), |d| {
            let key = id.key();
            d.recent.retain(|k| k != &key);
            d.recent.insert(0, key.clone());
            d.recent.truncate(max_recent.max(1) * 4);
            let rec = d.playtime.entry(key).or_default();
            rec.last_played = Some(now.clone());
            rec.first_played.get_or_insert(now);
        });
    }

    /// Add a finished session's length.
    pub fn record_session(&self, id: &GameId, seconds: u64) {
        self.update(Some(id), |d| {
            let rec = d.playtime.entry(id.key()).or_default();
            let total = rec.extra_seconds + seconds;
            rec.total_minutes += total / 60;
            rec.extra_seconds = total % 60;
        });
    }

    /// Take a store's own playtime totals (minutes by app name) into account:
    /// each game keeps the larger of Irori's total and the store's, so time
    /// played in the store's launcher shows here and nothing is counted
    /// twice. Writes only when a total grew.
    ///
    /// Only the store's games count (see [`split_store_playtime`]); records
    /// an earlier sync kept for its DLC are removed.
    pub fn merge_store_playtime(
        &self,
        runner: irori_core::Runner,
        games: &[Arc<GameInfo>],
        totals: &std::collections::HashMap<String, u64>,
    ) -> usize {
        let (totals, dlc) = split_store_playtime(games, totals);
        let data = self.snapshot();
        let grown = grown_totals(&data, runner, &totals);
        let stale: Vec<String> = dlc
            .iter()
            .map(|app| GameId::new(runner, app.as_str()).key())
            .filter(|key| data.playtime.contains_key(key))
            .collect();
        if !grown.is_empty() || !stale.is_empty() {
            self.update(None, |d| {
                for (key, minutes) in &grown {
                    d.playtime.entry(key.clone()).or_default().total_minutes = *minutes;
                }
                for key in &stale {
                    d.playtime.remove(key);
                }
            });
        }
        grown.len()
    }

    /// Note games appearing for the first time and follow the hidden flags
    /// stores report, writing only when something changed.
    pub fn sync_library(&self, games: &[Arc<GameInfo>]) {
        let plan = LibrarySync::plan(&self.snapshot(), games);
        if plan.is_empty() {
            return;
        }
        let now = util::now_rfc3339();
        self.update(None, |d| plan.apply(d, &now));
    }

    /// Forget everything about a game except playtime (uninstall).
    pub fn forget(&self, id: &GameId) {
        self.update(Some(id), |d| {
            let key = id.key();
            d.recent.retain(|k| k != &key);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn game(app: &str, store_hidden: bool) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(Runner::Gog),
            app_name: app.into(),
            store_hidden,
            ..Default::default()
        })
    }

    #[test]
    fn ratings_are_clamped_cleared_and_optional_on_disk() {
        let mut data: UserData = serde_json::from_str(r#"{"favourites":["gog:1"]}"#).unwrap();
        assert!(data.ratings.is_empty());
        rate(&mut data, "gog:1".into(), Some(4));
        rate(&mut data, "gog:2".into(), Some(12));
        assert_eq!(data.ratings["gog:1"], 4);
        assert_eq!(data.ratings["gog:2"], MAX_RATING);
        rate(&mut data, "gog:1".into(), Some(0));
        rate(&mut data, "gog:2".into(), None);
        assert!(data.ratings.is_empty());
    }

    #[test]
    fn follows_store_hidden_flags_without_overriding_the_user() {
        let mut data = UserData::default();
        let games = vec![game("1", true), game("2", false)];
        let plan = LibrarySync::plan(&data, &games);
        assert_eq!(plan.new, ["gog:1", "gog:2"]);
        assert_eq!(plan.hide, ["gog:1"]);
        plan.apply(&mut data, "2026-10-23T10:00:00Z");
        assert!(data.hidden.contains("gog:1"));
        assert_eq!(data.first_seen["gog:2"], "2026-10-23T10:00:00Z");
        // Nothing left to do on the next rebuild.
        assert!(LibrarySync::plan(&data, &games).is_empty());

        // The user unhides it here: it stays unhidden.
        data.hidden.remove("gog:1");
        assert!(LibrarySync::plan(&data, &games).is_empty());

        // Unhidden on the store: the import is forgotten.
        data.hidden.insert("gog:1".into());
        let plan = LibrarySync::plan(&data, &[game("1", false), game("2", false)]);
        assert_eq!(plan.unhide, ["gog:1"]);
        plan.apply(&mut data, "later");
        assert!(!data.hidden.contains("gog:1"));
        assert!(data.store_hidden_imported.is_empty());
    }

    #[test]
    fn store_playtime_counts_games_not_their_dlc() {
        // Epic reports Civilization VI's DLC with the base game's time.
        let base = Arc::new(GameInfo {
            app_name: "Kinglet".into(),
            dlcs: vec![irori_core::game::DlcInfo {
                app_name: "KingletAztec".into(),
                title: "Aztec".into(),
                is_installed: true,
                owned: true,
            }],
            ..Default::default()
        });
        let dlc = Arc::new(GameInfo {
            app_name: "KingletNubia".into(),
            is_dlc: true,
            ..Default::default()
        });
        let totals = std::collections::HashMap::from([
            ("Kinglet".to_string(), 2283u64),
            ("KingletAztec".to_string(), 2283),
            ("KingletNubia".to_string(), 248),
            ("Gone".to_string(), 10),
        ]);
        let (kept, dlc) = split_store_playtime(&[base, dlc], &totals);
        assert_eq!(
            kept,
            std::collections::HashMap::from([("Kinglet".to_string(), 2283)])
        );
        assert_eq!(
            dlc.into_iter().collect::<Vec<_>>(),
            ["KingletAztec", "KingletNubia"]
        );
    }

    #[test]
    fn store_playtime_only_ever_raises_totals() {
        let mut data = UserData::default();
        data.playtime.insert(
            "legendary:Fowl".into(),
            PlaytimeRecord {
                total_minutes: 120,
                ..Default::default()
            },
        );
        let totals = std::collections::HashMap::from([
            ("Fowl".to_string(), 90u64),
            ("Sugar".to_string(), 600),
        ]);
        let grown = grown_totals(&data, Runner::Legendary, &totals);
        assert_eq!(grown, [("legendary:Sugar".to_string(), 600)]);
    }

    #[test]
    fn play_status_keeps_its_date_until_it_changes() {
        let mut data = UserData::default();
        let keys = ["gog:1".to_string()];
        apply_play_status(&mut data, &keys, Some(PlayStatus::Playing), "monday");
        apply_play_status(&mut data, &keys, Some(PlayStatus::Playing), "tuesday");
        assert_eq!(data.play_status["gog:1"].since, "monday");
        apply_play_status(&mut data, &keys, Some(PlayStatus::Beaten), "friday");
        assert_eq!(
            data.play_status["gog:1"],
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "friday".into()
            }
        );
        apply_play_status(&mut data, &keys, None, "later");
        assert!(data.play_status.is_empty());
    }

    #[test]
    fn games_without_a_status_count_as_played_from_their_playtime() {
        let mut data = UserData::default();
        data.playtime.insert(
            "gog:1".into(),
            PlaytimeRecord {
                total_minutes: 30,
                ..Default::default()
            },
        );
        assert_eq!(data.effective_status("gog:1"), EffectiveStatus::Played);
        assert_eq!(data.effective_status("gog:2"), EffectiveStatus::NotPlayed);
        apply_play_status(
            &mut data,
            &["gog:1".into()],
            Some(PlayStatus::OnHold),
            "now",
        );
        assert_eq!(data.effective_status("gog:1"), EffectiveStatus::OnHold);
    }

    #[test]
    fn older_library_files_read_without_statuses() {
        let data: UserData = serde_json::from_str(r#"{"favourites":["gog:1"]}"#).unwrap();
        assert!(data.play_status.is_empty() && data.backlog_goals.is_empty());
        let mut data = data;
        data.backlog_goals.insert(2026, 12);
        apply_play_status(
            &mut data,
            &["gog:1".into()],
            Some(PlayStatus::PlanToPlay),
            "now",
        );
        let text = serde_json::to_string(&data).unwrap();
        assert!(text.contains(r#""backlog_goals":{"2026":12}"#), "{text}");
        assert!(text.contains(r#""status":"planToPlay""#), "{text}");
        let back: UserData = serde_json::from_str(&text).unwrap();
        assert_eq!(back.play_status, data.play_status);
        assert_eq!(back.backlog_goals, data.backlog_goals);
    }

    #[test]
    fn a_game_the_user_hid_themselves_is_never_unhidden() {
        let mut data = UserData::default();
        data.hidden.insert("gog:1".into());
        let plan = LibrarySync::plan(&data, &[game("1", true)]);
        assert!(plan.hide.is_empty());
        plan.apply(&mut data, "now");
        let plan = LibrarySync::plan(&data, &[game("1", false)]);
        assert!(plan.unhide.is_empty());
        assert!(data.hidden.contains("gog:1"));
    }
}
