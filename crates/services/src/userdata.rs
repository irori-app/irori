//! Launcher-owned per-user library data: favourites, hidden games, recently
//! played, custom categories (manual and smart, with their pins and
//! colours), playtime, play statuses, backlog goals, title/art overrides and
//! the play journal.
//!
//! A game owned on several stores is one library entry, but each copy keeps
//! its own values. Favourite, hidden, play status and rating describe the
//! game rather than the copy, so setting one writes every copy of the entry
//! ([`UserDataService::set_copy_groups`]), and combining copies shares the
//! values they had ([`UserData::share_between`]): whichever copy is the
//! default shows the same. Categories, playtime, journal notes and
//! overrides stay per copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use irori_core::game::{GameOverrides, PlaytimeRecord};
use irori_core::{Event, EventBus, GameId, GameInfo, Paths, json_store, util};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::journal::{self, Journal, JournalEntry, LeftOff, LegacyJournal};
use crate::play_status::{
    self, EffectiveStatus, PlayStatus, PlayStatusRecord, StatusChange, StatusHistory,
};
use crate::smart_categories::{CategoryLook, CategoryRef, SmartCategory};

/// Persisted shape. Game keys are `GameId::key()` strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserData {
    pub favourites: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
    /// Most recent first.
    pub recent: Vec<String>,
    /// Games the user explicitly removed from Recent / Continue playing so the
    /// `playtime` fallback does not put them back until they are launched again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub recent_dismissed: BTreeSet<String>,
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
    /// Every real change of each game's play status, oldest first.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub status_history: StatusHistory,
    /// How many games the user means to beat, by calendar year.
    pub backlog_goals: BTreeMap<i32, u32>,
    /// The user's own rating of each game, from 1 to 5 stars. Unrated games
    /// have no entry.
    #[serde(default)]
    pub ratings: BTreeMap<String, u8>,
    /// The user's notes, per game and day, several a day.
    #[serde(default, rename = "journal_notes")]
    pub journal: Journal,
    /// Where the user left off in each game.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub left_off: BTreeMap<String, LeftOff>,
    /// Notes as Irori kept them before several a day (one per game and
    /// day): read from older files (and from files an older Irori rewrote
    /// or edited) and moved into `journal` on load, and written beside
    /// `journal_notes` as their mirror so an older Irori keeps every note.
    #[serde(
        default,
        rename = "journal",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub legacy_journal: LegacyJournal,
    /// Categories filled from rules, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub smart_categories: BTreeMap<String, SmartCategory>,
    /// Pin and colour of manual categories, by name. Smart categories keep
    /// theirs in themselves.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub category_looks: BTreeMap<String, CategoryLook>,
    /// Seconds the user added to (or, negative, took from) a game's
    /// playtime ("Edit time played"), by game key. Its `playtime` record
    /// already includes them; what was counted (sessions, store totals) is
    /// the record without them ([`UserData::counted_seconds`]), which is
    /// what store totals and other computers' time are compared with, so an
    /// edit is never counted twice.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub playtime_adjust: BTreeMap<String, i64>,
    /// Genres and companies renamed, merged or removed across the library
    /// ("Manage genres and companies…").
    #[serde(
        default,
        skip_serializing_if = "crate::field_rules::FieldRules::is_empty"
    )]
    pub field_rules: crate::field_rules::FieldRules,
    /// Non-installed games the user explicitly follows in News.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub news_followed: BTreeSet<String>,
    /// Games the user muted from the unified News feed.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub news_muted: BTreeSet<String>,
}

/// A playtime record's length in seconds.
pub fn record_seconds(r: &PlaytimeRecord) -> u64 {
    r.total_minutes * 60 + r.extra_seconds
}

/// Set a record's length to `seconds`.
fn set_record_seconds(r: &mut PlaytimeRecord, seconds: u64) {
    r.total_minutes = seconds / 60;
    r.extra_seconds = seconds % 60;
}

/// `counted` seconds with the user's `adjust` applied, never below zero.
pub fn adjusted(counted: u64, adjust: i64) -> u64 {
    (counted as i64).saturating_add(adjust).max(0) as u64
}

/// Store `o` as the overrides of `key`, or none when it is empty.
fn put_overrides(d: &mut UserData, key: &str, o: GameOverrides) {
    if o.is_empty() {
        d.overrides.remove(key);
    } else {
        d.overrides.insert(key.to_string(), o);
    }
}

/// Change the overrides of `key` in place.
fn edit_overrides(d: &mut UserData, key: &str, f: impl FnOnce(&mut GameOverrides)) {
    let mut o = d.overrides.get(key).cloned().unwrap_or_default();
    f(&mut o);
    put_overrides(d, key, o);
}

/// Most stars a game can be rated.
pub const MAX_RATING: u8 = 5;

/// The favourite, hidden, category and play-status values some games had,
/// to put back exactly when a bulk edit is undone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BulkSnapshot {
    keys: Vec<String>,
    favourites: BTreeSet<String>,
    hidden: BTreeSet<String>,
    /// Every category that existed, with which of the games were in it.
    categories: BTreeMap<String, BTreeSet<String>>,
    /// Every smart category's manual inclusions among `keys`.
    smart_included: BTreeMap<String, BTreeSet<String>>,
    /// Every smart category's manual exclusions among `keys`.
    smart_excluded: BTreeMap<String, BTreeSet<String>>,
    play_status: BTreeMap<String, PlayStatusRecord>,
    status_history: StatusHistory,
}

impl BulkSnapshot {
    /// How many games it covers.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl UserData {
    /// Recently played game keys, newest first: Irori's own history, or
    /// recorded playtime when that history is empty (such as right after
    /// importing from Heroic). Games explicitly removed by the user stay out
    /// until launched again.
    pub fn recent_keys(&self) -> Vec<String> {
        let keys: Vec<String> = if self.recent.is_empty() {
            let mut played: Vec<(&String, &str)> = self
                .playtime
                .iter()
                .filter_map(|(k, r)| Some((k, r.last_played.as_deref()?)))
                .collect();
            // RFC 3339 timestamps in one offset sort chronologically as text.
            played.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            played.into_iter().map(|(k, _)| k.clone()).collect()
        } else {
            self.recent.clone()
        };
        keys.into_iter()
            .filter(|k| !self.recent_dismissed.contains(k))
            .collect()
    }

    /// Move what the user keeps about the game `from` to the game `to`, for
    /// one game that now has another library entry (a ROM played as a PC
    /// game): favourite, hidden, recents, categories, playtime, title and
    /// art, date added, play status, rating and journal. What `to` already
    /// has wins; playtime adds up. Copy links stay with `from`, as they
    /// name the entries the user combined.
    pub fn carry_over(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        for set in [
            &mut self.favourites,
            &mut self.hidden,
            &mut self.recent_dismissed,
            &mut self.news_followed,
            &mut self.news_muted,
        ] {
            if set.remove(from) {
                set.insert(to.to_string());
            }
        }
        if self.recent.iter().any(|k| k == from) {
            self.recent.retain(|k| k != to);
            if let Some(at) = self.recent.iter().position(|k| k == from) {
                self.recent[at] = to.to_string();
            }
        }
        for games in self.categories.values_mut() {
            if games.remove(from) {
                games.insert(to.to_string());
            }
        }
        if let Some(adjust) = self.playtime_adjust.remove(from) {
            *self.playtime_adjust.entry(to.to_string()).or_default() += adjust;
        }
        if let Some(old) = self.playtime.remove(from) {
            let new = self.playtime.entry(to.to_string()).or_default();
            let seconds = new.extra_seconds + old.extra_seconds;
            new.total_minutes += old.total_minutes + seconds / 60;
            new.extra_seconds = seconds % 60;
            new.last_played = new.last_played.take().max(old.last_played);
            new.first_played = match (new.first_played.take(), old.first_played) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        if let Some(mut o) = self.overrides.remove(from) {
            o.copy_of = None;
            o.default_copy = Default::default();
            if !self.overrides.contains_key(to) && o != GameOverrides::default() {
                self.overrides.insert(to.to_string(), o);
            }
        }
        fn take<V>(map: &mut BTreeMap<String, V>, from: &str, to: &str) {
            if let Some(v) = map.remove(from)
                && !map.contains_key(to)
            {
                map.insert(to.to_string(), v);
            }
        }
        take(&mut self.first_seen, from, to);
        take(&mut self.play_status, from, to);
        if let Some(changes) = self.status_history.remove(from) {
            let from_history = StatusHistory::from([(to.to_string(), changes)]);
            play_status::merge_history(&mut self.status_history, &from_history);
        }
        take(&mut self.ratings, from, to);
        take(&mut self.left_off, from, to);
        journal::carry_over(&mut self.journal, from, to);
    }

    /// Bring data written by older versions into today's shape: the
    /// one-note-a-day journal becomes whole-day notes, and a day an older
    /// Irori edited is kept beside the notes ([`journal::read`]). The
    /// older key then holds the mirror of the notes again.
    pub fn migrate(&mut self) {
        let legacy = std::mem::take(&mut self.legacy_journal);
        let notes = std::mem::take(&mut self.journal);
        self.journal = journal::read(notes, legacy);
        self.legacy_journal = journal::mirror(&self.journal);
    }

    /// Give every copy in `keys` (the copies of one entry, the default
    /// first) the same favourite, hidden flag, play status and rating: any
    /// copy's favourite or hidden flag, the status set last, the first
    /// rating.
    pub fn share_between(&mut self, keys: &[String]) {
        for set in [
            &mut self.favourites,
            &mut self.hidden,
            &mut self.news_followed,
            &mut self.news_muted,
        ] {
            if keys.iter().any(|k| set.contains(k)) {
                set.extend(keys.iter().cloned());
            }
        }
        let latest = keys
            .iter()
            .filter_map(|k| self.play_status.get(k))
            .max_by_key(|r| util::parse_rfc3339(&r.since).unwrap_or(i64::MIN))
            .cloned();
        if let Some(record) = latest {
            for key in keys {
                self.play_status.insert(key.clone(), record.clone());
            }
        }
        if let Some(stars) = keys.iter().find_map(|k| self.ratings.get(k).copied()) {
            for key in keys {
                self.ratings.insert(key.clone(), stars);
            }
        }
    }

    /// The time Irori counted for a game (sessions, store totals, other
    /// computers'), without the user's own edit.
    pub fn counted_seconds(&self, key: &str) -> u64 {
        let total = self.playtime.get(key).map_or(0, record_seconds);
        let adjust = self.playtime_adjust.get(key).copied().unwrap_or(0);
        (total as i64).saturating_sub(adjust).max(0) as u64
    }

    /// Make a game's playtime `total` seconds ("Edit time played"), or with
    /// `None` what was counted again. Sessions played later add to it.
    pub fn set_playtime(&mut self, key: &str, total: Option<u64>) {
        let counted = self.counted_seconds(key);
        let adjust = total.map_or(0, |t| t as i64 - counted as i64);
        if adjust == 0 {
            self.playtime_adjust.remove(key);
        } else {
            self.playtime_adjust.insert(key.to_string(), adjust);
        }
        let seconds = adjusted(counted, adjust);
        match self.playtime.get_mut(key) {
            Some(record) => set_record_seconds(record, seconds),
            None if seconds > 0 => {
                let mut record = PlaytimeRecord::default();
                set_record_seconds(&mut record, seconds);
                self.playtime.insert(key.to_string(), record);
            }
            None => {}
        }
    }

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

    /// The name of a category of either kind, while it exists.
    pub fn category_name(&self, category: &CategoryRef) -> Option<&str> {
        match category {
            CategoryRef::Manual(name) => {
                self.categories.get_key_value(name).map(|(n, _)| n.as_str())
            }
            CategoryRef::Smart(id) => self.smart_categories.get(id).map(|c| c.name.as_str()),
        }
    }

    /// The pin and colour of a category of either kind.
    pub fn category_look(&self, category: &CategoryRef) -> CategoryLook {
        match category {
            CategoryRef::Manual(name) => self.category_looks.get(name).cloned(),
            CategoryRef::Smart(id) => self.smart_categories.get(id).map(|c| c.look.clone()),
        }
        .unwrap_or_default()
    }

    /// Every category of both kinds with its name: pinned ones first, then
    /// by name (ignoring case), the kinds mixed.
    pub fn categories_in_order(&self) -> Vec<(CategoryRef, String)> {
        let mut all: Vec<(CategoryRef, String)> = self
            .categories
            .keys()
            .map(|n| (CategoryRef::Manual(n.clone()), n.clone()))
            .chain(
                self.smart_categories
                    .iter()
                    .map(|(id, c)| (CategoryRef::Smart(id.clone()), c.name.clone())),
            )
            .collect();
        all.sort_by_cached_key(|(r, name)| {
            (
                !self.category_look(r).pinned,
                name.to_lowercase(),
                r.clone(),
            )
        });
        all
    }

    /// The user's notes on a game for one day, in the day's order.
    pub fn journal_day(&self, key: &str, day: chrono::NaiveDate) -> &[JournalEntry] {
        self.journal
            .get(key)
            .and_then(|days| days.get(&day))
            .map_or(&[], Vec::as_slice)
    }

    /// One note of a game, with its day.
    pub fn journal_note(&self, key: &str, id: &str) -> Option<(chrono::NaiveDate, &JournalEntry)> {
        self.journal
            .get(key)?
            .iter()
            .find_map(|(day, notes)| notes.iter().find(|n| n.id == id).map(|n| (*day, n)))
    }
}

impl UserData {
    /// What `ids` are now, for [`UserData::restore`].
    pub fn capture(&self, ids: &[GameId]) -> BulkSnapshot {
        self.capture_keys(ids.iter().map(GameId::key).collect())
    }

    /// [`UserData::capture`] of games by key.
    fn capture_keys(&self, keys: Vec<String>) -> BulkSnapshot {
        let pick = |set: &BTreeSet<String>| -> BTreeSet<String> {
            keys.iter().filter(|k| set.contains(*k)).cloned().collect()
        };
        BulkSnapshot {
            favourites: pick(&self.favourites),
            hidden: pick(&self.hidden),
            categories: self
                .categories
                .iter()
                .map(|(name, games)| (name.clone(), pick(games)))
                .collect(),
            smart_included: self
                .smart_categories
                .iter()
                .map(|(id, cat)| (id.clone(), pick(&cat.included_games)))
                .collect(),
            smart_excluded: self
                .smart_categories
                .iter()
                .map(|(id, cat)| (id.clone(), pick(&cat.excluded_games)))
                .collect(),
            play_status: keys
                .iter()
                .filter_map(|k| Some((k.clone(), self.play_status.get(k)?.clone())))
                .collect(),
            status_history: keys
                .iter()
                .filter_map(|k| Some((k.clone(), self.status_history.get(k)?.clone())))
                .collect(),
            keys,
        }
    }

    /// Put the captured games back as they were. Categories made since keep
    /// existing unless the restore empties them; categories deleted since
    /// stay deleted.
    pub fn restore(&mut self, snap: &BulkSnapshot) {
        let set = |set: &mut BTreeSet<String>, key: &String, on: bool| {
            if on {
                set.insert(key.clone());
            } else {
                set.remove(key);
            }
        };
        let mut emptied = Vec::new();
        for (name, games) in self.categories.iter_mut() {
            let before = snap.categories.get(name);
            let had = !games.is_empty();
            for key in &snap.keys {
                set(games, key, before.is_some_and(|b| b.contains(key)));
            }
            if before.is_none() && had && games.is_empty() {
                emptied.push(name.clone());
            }
        }
        for name in emptied {
            self.categories.remove(&name);
        }
        for (id, cat) in self.smart_categories.iter_mut() {
            if cat.is_unreadable() {
                continue;
            }
            let before_in = snap.smart_included.get(id);
            let before_out = snap.smart_excluded.get(id);
            for key in &snap.keys {
                set(
                    &mut cat.included_games,
                    key,
                    before_in.is_some_and(|b| b.contains(key)),
                );
                set(
                    &mut cat.excluded_games,
                    key,
                    before_out.is_some_and(|b| b.contains(key)),
                );
            }
        }
        for key in &snap.keys {
            set(&mut self.favourites, key, snap.favourites.contains(key));
            set(&mut self.hidden, key, snap.hidden.contains(key));
            match snap.play_status.get(key) {
                Some(record) => {
                    self.play_status.insert(key.clone(), record.clone());
                }
                None => {
                    self.play_status.remove(key);
                }
            }
            // An undone change never happened: it leaves no history.
            match snap.status_history.get(key) {
                Some(changes) => {
                    self.status_history.insert(key.clone(), changes.clone());
                }
                None => {
                    self.status_history.remove(key);
                }
            }
        }
    }
}

/// Set (or clear) the play status of `keys`, dated `now`. A game already
/// in that status keeps the date it got it; every real change is added to
/// its history.
fn apply_play_status(d: &mut UserData, keys: &[String], status: Option<PlayStatus>, now: &str) {
    for key in keys {
        if d.play_status(key) == status {
            continue;
        }
        match status {
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
        play_status::record(
            d.status_history.entry(key.clone()).or_default(),
            StatusChange {
                at: now.to_string(),
                status,
            },
        );
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
        .filter(|(key, minutes)| *minutes > data.counted_seconds(key) / 60)
        .collect()
}

/// The store's last-played times that are later than Irori's, for the
/// store's games, as `(key, RFC 3339)` in Irori's own spelling.
fn later_last_played(
    data: &UserData,
    runner: irori_core::Runner,
    games: &[Arc<GameInfo>],
    times: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let playable: BTreeSet<&str> = games
        .iter()
        .filter(|g| !g.is_dlc)
        .map(|g| g.app_name.as_str())
        .collect();
    let mut later: Vec<(String, String)> = times
        .iter()
        .filter(|(app, _)| playable.contains(app.as_str()))
        .filter_map(|(app, at)| {
            let secs = util::parse_rfc3339(at)?;
            let key = GameId::new(runner, app.as_str()).key();
            let known = data
                .playtime
                .get(&key)
                .and_then(|r| r.last_played.as_deref())
                .and_then(util::parse_rfc3339);
            (known.is_none_or(|k| secs > k))
                .then(|| util::rfc3339_from_unix(secs).map(|at| (key, at)))
                .flatten()
        })
        .collect();
    later.sort();
    later
}

/// Rename manual category `from` to `to`, merging into `to` when it exists,
/// and follow it in its look and in smart categories' rules.
fn rename_category(d: &mut UserData, from: &str, to: &str) {
    let Some(set) = d.categories.remove(from) else {
        return;
    };
    d.categories.entry(to.to_string()).or_default().extend(set);
    if let Some(look) = d.category_looks.remove(from) {
        d.category_looks.entry(to.to_string()).or_insert(look);
    }
    for smart in d.smart_categories.values_mut() {
        smart.rename_category(from, to);
    }
}

/// Set a category's look; false when it doesn't exist or nothing changed.
fn set_look(d: &mut UserData, category: &CategoryRef, look: CategoryLook) -> bool {
    match category {
        CategoryRef::Manual(name) => {
            if !d.categories.contains_key(name) || d.category_look(category) == look {
                return false;
            }
            if look.is_empty() {
                d.category_looks.remove(name);
            } else {
                d.category_looks.insert(name.clone(), look);
            }
            true
        }
        CategoryRef::Smart(id) => match d.smart_categories.get_mut(id) {
            Some(smart) if smart.look != look => {
                *smart = smart.with_look(look);
                true
            }
            _ => false,
        },
    }
}

/// Put `keys` in `set` (`on`) or take them out.
fn toggle(set: &mut BTreeSet<String>, keys: &[String], on: bool) {
    for key in keys {
        if on {
            set.insert(key.clone());
        } else {
            set.remove(key);
        }
    }
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

fn exclude_smart_game(d: &mut UserData, smart_id: &str, game_key: &str) -> bool {
    if game_key.is_empty() {
        return false;
    }
    d.smart_categories
        .get_mut(smart_id)
        .filter(|c| !c.is_unreadable())
        .is_some_and(|c| {
            let removed_in = c.included_games.remove(game_key);
            let added_out = c.excluded_games.insert(game_key.to_owned());
            removed_in || added_out
        })
}

fn restore_smart_game(d: &mut UserData, smart_id: &str, game_key: &str) -> bool {
    d.smart_categories
        .get_mut(smart_id)
        .filter(|c| !c.is_unreadable())
        .is_some_and(|c| c.excluded_games.remove(game_key))
}

fn add_smart_games(
    d: &mut UserData,
    smart_id: &str,
    plans: &[(String, Vec<String>, bool)],
) -> bool {
    let Some(cat) = d
        .smart_categories
        .get_mut(smart_id)
        .filter(|c| !c.is_unreadable())
    else {
        return false;
    };
    let mut changed = false;
    for (primary_key, copy_keys, pin_manually) in plans {
        if primary_key.is_empty() {
            continue;
        }
        for key in copy_keys {
            changed |= cat.excluded_games.remove(key);
        }
        if *pin_manually {
            changed |= cat.included_games.insert(primary_key.clone());
        }
    }
    changed
}

fn remove_smart_games(
    d: &mut UserData,
    smart_id: &str,
    plans: &[(String, Vec<String>, bool)],
) -> bool {
    let Some(cat) = d
        .smart_categories
        .get_mut(smart_id)
        .filter(|c| !c.is_unreadable())
    else {
        return false;
    };
    let mut changed = false;
    for (primary_key, copy_keys, exclude_rule_match) in plans {
        if primary_key.is_empty() {
            continue;
        }
        for key in copy_keys {
            changed |= cat.included_games.remove(key);
        }
        if *exclude_rule_match {
            changed |= cat.excluded_games.insert(primary_key.clone());
        }
    }
    changed
}

pub struct UserDataService {
    data: RwLock<Arc<UserData>>,
    bus: EventBus,
    /// Each copy's key → the keys of every copy of its entry, as the
    /// library last grouped them.
    copies: RwLock<Arc<HashMap<String, Arc<[String]>>>>,
}

/// `keys` with every other copy of their entries (`groups`), each once,
/// in order.
fn with_copies(
    groups: &HashMap<String, Arc<[String]>>,
    keys: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in keys {
        let group = groups.get(&key).map(|g| g.to_vec());
        for k in group.unwrap_or_else(|| vec![key]) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

fn file() -> PathBuf {
    Paths::get().state_dir().join("library.json")
}

impl UserDataService {
    pub fn load(bus: EventBus) -> Self {
        Self {
            data: RwLock::new(Arc::new({
                let mut data: UserData = json_store::read_or_default(&file());
                data.migrate();
                data
            })),
            bus,
            copies: RwLock::default(),
        }
    }

    /// Follow how the library groups copies (each group the copies of one
    /// entry), so a favourite, hidden flag, play status or rating set on
    /// one copy is set on all of them.
    pub fn set_copy_groups(&self, groups: &[Arc<[GameId]>]) {
        let mut map = HashMap::new();
        for group in groups {
            let keys: Arc<[String]> = group.iter().map(GameId::key).collect();
            for key in keys.iter() {
                map.insert(key.clone(), keys.clone());
            }
        }
        *self.copies.write() = Arc::new(map);
    }

    /// The keys of `ids` and of every other copy of their entries.
    fn keys_with_copies(&self, ids: &[GameId]) -> Vec<String> {
        with_copies(&self.copies.read(), ids.iter().map(GameId::key))
    }

    pub fn snapshot(&self) -> Arc<UserData> {
        self.data.read().clone()
    }

    /// Mutate, persist, and announce. `game` scopes the change event.
    pub fn update(&self, game: Option<&GameId>, f: impl FnOnce(&mut UserData)) {
        self.update_if(game, |d| {
            f(d);
            true
        });
    }

    /// [`UserDataService::update`] when `f` returns true; otherwise its
    /// changes are dropped and nothing is written or announced.
    pub fn update_if(&self, game: Option<&GameId>, f: impl FnOnce(&mut UserData) -> bool) {
        let snapshot = {
            let mut guard = self.data.write();
            let mut next = (**guard).clone();
            if !f(&mut next) {
                return;
            }
            // An older Irori reads the notes from their mirror.
            next.legacy_journal = journal::mirror(&next.journal);
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

    /// Mark or unmark a game (every copy of its entry) as a favourite.
    pub fn set_favourite(&self, id: &GameId, on: bool) {
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| toggle(&mut d.favourites, &keys, on));
    }

    /// Mark or unmark several games as favourites in one change.
    pub fn set_favourites(&self, ids: &[GameId], on: bool) {
        let keys = self.keys_with_copies(ids);
        self.update(None, |d| toggle(&mut d.favourites, &keys, on));
    }

    /// Hide or unhide several games in one change.
    pub fn set_hidden_many(&self, ids: &[GameId], on: bool) {
        let keys = self.keys_with_copies(ids);
        self.update(None, |d| toggle(&mut d.hidden, &keys, on));
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
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| {
            for key in keys {
                rate(d, key, stars);
            }
        });
    }

    /// Hide or unhide a game (every copy of its entry).
    pub fn set_hidden(&self, id: &GameId, on: bool) {
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| toggle(&mut d.hidden, &keys, on));
    }

    /// Follow or unfollow a game (every copy of its entry) in News.
    pub fn set_news_followed(&self, id: &GameId, on: bool) {
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| {
            toggle(&mut d.news_followed, &keys, on);
            if on {
                toggle(&mut d.news_muted, &keys, false);
            }
        });
        self.bus.emit(Event::NewsChanged);
    }

    /// Mute or unmute a game (every copy of its entry) in the unified News feed.
    pub fn set_news_muted(&self, id: &GameId, on: bool) {
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| {
            toggle(&mut d.news_muted, &keys, on);
            if on {
                toggle(&mut d.news_followed, &keys, false);
            }
        });
        self.bus.emit(Event::NewsChanged);
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
        self.update(None, |d| rename_category(d, from, to));
    }

    pub fn delete_category(&self, name: &str) {
        self.update(None, |d| {
            d.categories.remove(name);
            d.category_looks.remove(name);
        });
    }

    /// Save a smart category: a new one with `id` `None` (its id is
    /// returned), else the one of `id` replaced.
    pub fn save_smart_category(&self, id: Option<&str>, category: SmartCategory) -> String {
        let id = id.map_or_else(crate::smart_categories::new_id, str::to_owned);
        let key = id.clone();
        self.update(None, move |d| {
            d.smart_categories.insert(key, category);
        });
        id
    }

    pub fn delete_smart_category(&self, id: &str) {
        self.update_if(None, |d| d.smart_categories.remove(id).is_some());
    }

    /// Manually leave `game_key` out of smart category `smart_id`.
    pub fn exclude_from_smart_category(&self, smart_id: &str, game_key: &str) {
        self.update_if(None, |d| exclude_smart_game(d, smart_id, game_key));
    }

    /// Put a manually excluded `game_key` back into smart category `smart_id`.
    pub fn restore_to_smart_category(&self, smart_id: &str, game_key: &str) {
        self.update_if(None, |d| restore_smart_game(d, smart_id, game_key));
    }

    /// Add `ids` to smart category `smart_id`, clearing any manual exclusions
    /// on their copies and inserting into `included_games` whenever
    /// `pin_manually` returns true (game does not already match by rules).
    pub fn add_to_smart_category_many(
        &self,
        smart_id: &str,
        ids: &[GameId],
        pin_manually: impl Fn(&GameId) -> bool,
    ) {
        let plans: Vec<(String, Vec<String>, bool)> = ids
            .iter()
            .map(|id| {
                (
                    id.key(),
                    self.keys_with_copies(std::slice::from_ref(id)),
                    pin_manually(id),
                )
            })
            .collect();
        self.update_if(None, |d| add_smart_games(d, smart_id, &plans));
    }

    /// Remove `ids` from smart category `smart_id`, clearing manual inclusions
    /// on their copies and inserting into `excluded_games` whenever
    /// `exclude_rule_match` returns true (game would still match by rules).
    pub fn remove_from_smart_category_many(
        &self,
        smart_id: &str,
        ids: &[GameId],
        exclude_rule_match: impl Fn(&GameId) -> bool,
    ) {
        let plans: Vec<(String, Vec<String>, bool)> = ids
            .iter()
            .map(|id| {
                (
                    id.key(),
                    self.keys_with_copies(std::slice::from_ref(id)),
                    exclude_rule_match(id),
                )
            })
            .collect();
        self.update_if(None, |d| remove_smart_games(d, smart_id, &plans));
    }

    /// Pin, unpin or colour a category of either kind.
    pub fn set_category_look(&self, category: &CategoryRef, look: CategoryLook) {
        self.update_if(None, |d| set_look(d, category, look));
    }

    /// Set a game's play status, or clear it with `None`.
    pub fn set_play_status(&self, id: &GameId, status: Option<PlayStatus>) {
        let now = util::now_rfc3339();
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| apply_play_status(d, &keys, status, &now));
    }

    /// Set or clear the play status of several games in one change.
    pub fn set_play_status_many(&self, ids: &[GameId], status: Option<PlayStatus>) {
        let now = util::now_rfc3339();
        let keys = self.keys_with_copies(ids);
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

    /// The current values of `ids`, to undo a bulk edit with [`Self::restore`].
    pub fn capture(&self, ids: &[GameId]) -> BulkSnapshot {
        self.snapshot().capture_keys(self.keys_with_copies(ids))
    }

    /// Put games back as [`Self::capture`] saw them.
    pub fn restore(&self, snap: &BulkSnapshot) {
        self.update(None, |d| d.restore(snap));
    }

    /// Write one of a game's notes on `day`: the note of `entry.id`, or a
    /// new one when it has none. A note with nothing in it is removed.
    /// Returns the note's id.
    pub fn set_journal_entry(
        &self,
        id: &GameId,
        day: chrono::NaiveDate,
        entry: JournalEntry,
    ) -> String {
        let now = util::now_rfc3339();
        let mut note = String::new();
        self.update(Some(id), |d| {
            note = journal::put(&mut d.journal, &id.key(), day, entry, &now);
        });
        note
    }

    /// Delete one of a game's notes; returns it with its day, to put back
    /// with [`Self::restore_journal_entry`].
    pub fn remove_journal_entry(
        &self,
        id: &GameId,
        note: &str,
    ) -> Option<(chrono::NaiveDate, JournalEntry)> {
        let mut removed = None;
        self.update_if(Some(id), |d| {
            removed = journal::remove(&mut d.journal, &id.key(), note);
            removed.is_some()
        });
        removed
    }

    /// Write where the user left off in a game, or clear it with an empty
    /// text. Returns what was there before, to put back with
    /// [`Self::restore_left_off`].
    pub fn set_left_off(&self, id: &GameId, text: &str) -> Option<LeftOff> {
        let now = util::now_rfc3339();
        let key = id.key();
        let mut before = None;
        self.update_if(Some(id), |d| {
            before = d.left_off.get(&key).cloned();
            let next = LeftOff::new(text, &now);
            if before.as_ref().map(|b| &b.text) == next.as_ref().map(|n| &n.text) {
                return false;
            }
            match next {
                Some(next) => d.left_off.insert(key.clone(), next),
                None => d.left_off.remove(&key),
            };
            true
        });
        before
    }

    /// Put back where the user left off as [`Self::set_left_off`] found it
    /// (Undo).
    pub fn restore_left_off(&self, id: &GameId, before: Option<LeftOff>) {
        let key = id.key();
        self.update(Some(id), |d| match before {
            Some(before) => {
                d.left_off.insert(key, before);
            }
            None => {
                d.left_off.remove(&key);
            }
        });
    }

    /// Put back a note [`Self::remove_journal_entry`] took, as it was.
    pub fn restore_journal_entry(&self, id: &GameId, day: chrono::NaiveDate, entry: JournalEntry) {
        let updated = entry.updated.clone();
        self.update(Some(id), |d| {
            journal::put(&mut d.journal, &id.key(), day, entry, &updated);
        });
    }

    /// Drop one game (every copy of its entry) from the recently played list
    /// and keep it out of the playtime fallback until it is launched again.
    pub fn remove_recent(&self, id: &GameId) {
        let keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| {
            d.recent.retain(|k| !keys.contains(k));
            d.recent_dismissed.extend(keys);
        });
    }

    pub fn clear_recent(&self) {
        self.update(None, |d| {
            d.recent.clear();
            let played: Vec<String> = d
                .playtime
                .iter()
                .filter(|(_, r)| r.last_played.is_some())
                .map(|(k, _)| k.clone())
                .collect();
            d.recent_dismissed.extend(played);
        });
    }

    /// Replace a game's title, art and game modes. How it joins other
    /// stores' copies stays: [`UserDataService::combine_copies`] and its
    /// siblings change that; so do its links and details
    /// ([`UserDataService::set_metadata`]).
    pub fn set_overrides(&self, id: &GameId, mut o: GameOverrides) {
        self.update(Some(id), |d| {
            if let Some(old) = d.overrides.get(&id.key()) {
                o.copy_of = old.copy_of.clone();
                o.default_copy = old.default_copy;
                o.links = old.links.clone();
                o.developer = old.developer.clone();
                o.publisher = old.publisher.clone();
                o.genres = old.genres.clone();
            }
            put_overrides(d, &id.key(), o);
        });
    }

    /// Change the developer, publisher or genres of several games at once
    /// ("Edit details…"); what `edit` leaves out stays.
    pub fn set_metadata(&self, ids: &[GameId], edit: &irori_core::MetadataEdit) {
        if ids.is_empty() || edit.is_empty() {
            return;
        }
        // A game's event, so the library is rebuilt with the new details.
        self.update(ids.first(), |d| {
            for id in ids {
                edit_overrides(d, &id.key(), |o| edit.apply(o));
            }
        });
    }

    /// Make `value` of `field` show as `to` across the library (empty
    /// removes it), or with `None` as itself again.
    pub fn set_field_rule(&self, field: crate::field_rules::Field, value: &str, to: Option<&str>) {
        self.update_if(None, |d| {
            let before = d.field_rules.clone();
            d.field_rules.set(field, value, to);
            d.field_rules != before
        });
    }

    /// Keep `links` with `id` (its "Links" section), in their order; an
    /// empty list removes them.
    pub fn set_links(&self, id: &GameId, links: Vec<irori_core::GameLink>) {
        self.update(Some(id), |d| {
            edit_overrides(d, &id.key(), |o| o.links = links)
        });
    }

    /// Make `ids` (two or more games, from different stores) one library
    /// entry: each joins the first. The first stays the default copy unless
    /// another one is installed.
    pub fn combine_copies(&self, ids: &[GameId]) {
        let Some((first, rest)) = ids.split_first() else {
            return;
        };
        let keys: Vec<String> = ids.iter().map(GameId::key).collect();
        // A game's event, so the library is rebuilt with the new entry.
        self.update(Some(first), |d| {
            d.share_between(&keys);
            let target = first.key();
            edit_overrides(d, &target, |o| {
                if o.copy_of.as_deref() == Some("") {
                    o.copy_of = None;
                }
                o.default_copy = true;
            });
            for id in rest {
                edit_overrides(d, &id.key(), |o| {
                    o.copy_of = Some(target.clone());
                    o.default_copy = false;
                });
            }
        });
    }

    /// Take `id` out of the entry it shares with `group` (every copy of
    /// the entry, `id` among them): it stands alone from now on, and the
    /// others stay together.
    pub fn separate_copy(&self, id: &GameId, group: &[GameId]) {
        let key = id.key();
        let rest: Vec<String> = group
            .iter()
            .map(GameId::key)
            .filter(|k| *k != key)
            .collect();
        self.update(Some(id), |d| {
            edit_overrides(d, &key, |o| {
                o.copy_of = Some(String::new());
                o.default_copy = false;
            });
            // Copies joined to this one join another of the rest instead.
            if let Some(anchor) = rest.first() {
                for other in &rest {
                    edit_overrides(d, other, |o| {
                        if o.copy_of.as_deref() == Some(key.as_str()) {
                            o.copy_of = (other != anchor).then(|| anchor.clone());
                        }
                    });
                }
            }
        });
    }

    /// Show every copy of `group` as an entry of its own.
    pub fn separate_copies(&self, group: &[GameId]) {
        let Some(first) = group.first() else {
            return;
        };
        self.update(Some(first), |d| {
            for id in group {
                edit_overrides(d, &id.key(), |o| {
                    o.copy_of = Some(String::new());
                    o.default_copy = false;
                });
            }
        });
    }

    /// Make `id` the copy its entry plays and installs (when no other copy
    /// is installed instead).
    pub fn set_default_copy(&self, id: &GameId, group: &[GameId]) {
        let key = id.key();
        self.update(Some(id), |d| {
            for other in group {
                let k = other.key();
                edit_overrides(d, &k, |o| o.default_copy = k == key);
            }
        });
    }

    /// Make a game's playtime `total` seconds, or with `None` what Irori
    /// counted ([`UserData::set_playtime`]). Hold the sync's playtime guard
    /// around it ([`crate::Backend::set_playtime`]).
    pub fn set_playtime(&self, id: &GameId, total: Option<u64>) {
        let key = id.key();
        self.update(Some(id), |d| d.set_playtime(&key, total));
    }

    /// Record a session start: recent list and first/last played.
    pub fn record_launch(&self, id: &GameId, max_recent: usize) {
        let now = util::now_rfc3339();
        let copy_keys = self.keys_with_copies(std::slice::from_ref(id));
        self.update(Some(id), |d| {
            let key = id.key();
            for k in &copy_keys {
                d.recent_dismissed.remove(k);
            }
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
                    let adjust = d.playtime_adjust.get(key).copied().unwrap_or(0);
                    let record = d.playtime.entry(key.clone()).or_default();
                    set_record_seconds(record, adjusted(minutes * 60, adjust));
                }
                for key in &stale {
                    d.playtime.remove(key);
                    d.playtime_adjust.remove(key);
                }
            });
        }
        grown.len()
    }

    /// Take a store's own last-played times (RFC 3339 by app name) into
    /// account: each game keeps the later of Irori's last session and the
    /// store's, so a game played outside Irori sorts as recently played.
    /// Only the store's games (not DLC) count. Writes only when a time
    /// moved forward; returns how many did.
    pub fn merge_store_last_played(
        &self,
        runner: irori_core::Runner,
        games: &[Arc<GameInfo>],
        times: &std::collections::HashMap<String, String>,
    ) -> usize {
        let later = later_last_played(&self.snapshot(), runner, games, times);
        if !later.is_empty() {
            self.update(None, |d| {
                for (key, at) in &later {
                    let rec = d.playtime.entry(key.clone()).or_default();
                    rec.last_played = Some(at.clone());
                    rec.first_played.get_or_insert_with(|| at.clone());
                }
            });
        }
        later.len()
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

    /// Move what the user keeps about `from` to `to` ([`UserData::carry_over`]).
    pub fn carry_over(&self, from: &GameId, to: &GameId) {
        let (from_key, to_key) = (from.key(), to.key());
        self.update(Some(to), |d| d.carry_over(&from_key, &to_key));
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

    #[test]
    fn carrying_a_game_over_moves_what_the_user_keeps() {
        let (rom, pc) = ("emulation:abc", "sideload:xyz");
        let mut d = UserData::default();
        d.favourites.insert(rom.into());
        d.recent = vec!["gog:1".into(), rom.into(), pc.into()];
        d.categories
            .entry("RPG".into())
            .or_default()
            .insert(rom.into());
        d.playtime.insert(
            rom.into(),
            PlaytimeRecord {
                total_minutes: 90,
                extra_seconds: 50,
                last_played: Some("2026-05-01T10:00:00+00:00".into()),
                first_played: Some("2025-01-01T10:00:00+00:00".into()),
            },
        );
        d.playtime.insert(
            pc.into(),
            PlaytimeRecord {
                total_minutes: 10,
                extra_seconds: 20,
                last_played: Some("2026-06-01T10:00:00+00:00".into()),
                first_played: None,
            },
        );
        d.overrides.insert(
            rom.into(),
            GameOverrides {
                title: Some("Mine".into()),
                copy_of: Some("gog:1".into()),
                ..Default::default()
            },
        );
        d.ratings.insert(rom.into(), 4);
        d.ratings.insert(pc.into(), 2);
        d.carry_over(rom, pc);

        assert!(d.favourites.contains(pc) && !d.favourites.contains(rom));
        assert_eq!(d.recent, vec!["gog:1".to_string(), pc.to_string()]);
        assert!(d.categories["RPG"].contains(pc));
        let time = &d.playtime[pc];
        assert_eq!((time.total_minutes, time.extra_seconds), (101, 10));
        assert_eq!(
            time.last_played.as_deref(),
            Some("2026-06-01T10:00:00+00:00")
        );
        assert_eq!(
            time.first_played.as_deref(),
            Some("2025-01-01T10:00:00+00:00")
        );
        assert!(!d.playtime.contains_key(rom));
        let o = &d.overrides[pc];
        assert_eq!(o.title.as_deref(), Some("Mine"));
        assert_eq!(o.copy_of, None);
        // What the new entry already had wins.
        assert_eq!(d.ratings.get(pc), Some(&2));
        assert!(!d.ratings.contains_key(rom));
    }

    #[test]
    fn recent_keys_respects_dismissed_even_when_recent_falls_back_to_playtime() {
        let played = |when: &str| PlaytimeRecord {
            total_minutes: 30,
            last_played: Some(when.into()),
            ..Default::default()
        };
        let mut d = UserData::default();
        d.playtime
            .insert("gog:1".into(), played("2026-10-01T10:00:00Z"));
        d.playtime
            .insert("legendary:a".into(), played("2026-10-20T10:00:00Z"));

        // Empty `recent` falls back to `playtime` sorted newest-first.
        assert_eq!(d.recent_keys(), vec!["legendary:a", "gog:1"]);

        // Dismissing a game removes it from the playtime fallback too.
        d.recent_dismissed.insert("legendary:a".into());
        assert_eq!(d.recent_keys(), vec!["gog:1"]);

        // Dismissing the remaining game leaves the list empty instead of resurrecting it.
        d.recent_dismissed.insert("gog:1".into());
        assert!(d.recent_keys().is_empty());

        // Launching a dismissed game again removes it from `recent_dismissed`.
        d.recent_dismissed.remove("gog:1");
        d.recent.insert(0, "gog:1".into());
        assert_eq!(d.recent_keys(), vec!["gog:1"]);
    }

    #[test]
    fn restoring_a_capture_undoes_bulk_edits_exactly() {
        let a = GameId::new(Runner::Gog, "a");
        let b = GameId::new(Runner::Gog, "b");
        let other = GameId::new(Runner::Gog, "other");
        let mut d = UserData::default();
        d.favourites.insert(a.key());
        d.hidden.insert(b.key());
        d.categories
            .insert("RPG".into(), [a.key(), other.key()].into());
        d.categories.insert("Empty".into(), BTreeSet::new());
        d.play_status.insert(
            a.key(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2024-01-01T00:00:00Z".into(),
            },
        );
        let before = d.clone();
        let snap = d.capture(&[a.clone(), b.clone()]);
        assert_eq!(snap.len(), 2);

        // A bulk pass over both games: favourite, unhide, recategorize,
        // new status, and a category made on the way.
        d.favourites.insert(b.key());
        d.hidden.remove(&b.key());
        d.categories.get_mut("RPG").unwrap().remove(&a.key());
        d.categories.get_mut("Empty").unwrap().insert(b.key());
        d.categories.insert("New".into(), [a.key(), b.key()].into());
        d.categories.insert("Made later".into(), BTreeSet::new());
        apply_play_status(
            &mut d,
            &[a.key(), b.key()],
            Some(PlayStatus::Playing),
            "2026-11-04T10:00:00Z",
        );
        // Another game changed meanwhile keeps its change.
        d.favourites.insert(other.key());

        d.restore(&snap);
        let mut expected = before;
        expected.favourites.insert(other.key());
        expected
            .categories
            .insert("Made later".into(), BTreeSet::new());
        assert_eq!(d.favourites, expected.favourites);
        assert_eq!(d.hidden, expected.hidden);
        assert_eq!(d.categories, expected.categories);
        assert_eq!(d.play_status, expected.play_status);
    }

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
    fn edits_and_combining_reach_every_copy() {
        let groups = HashMap::from([
            (
                "steam:1".to_string(),
                Arc::from(["steam:1".to_string(), "gog:2".to_string()]),
            ),
            (
                "gog:2".to_string(),
                Arc::from(["steam:1".to_string(), "gog:2".to_string()]),
            ),
        ]);
        assert_eq!(
            with_copies(&groups, ["gog:2".to_string(), "epic:3".to_string()]),
            ["steam:1", "gog:2", "epic:3"]
        );

        let mut d = UserData::default();
        d.favourites.insert("gog:2".into());
        d.ratings.insert("steam:1".into(), 4);
        d.ratings.insert("gog:2".into(), 2);
        d.play_status.insert(
            "steam:1".into(),
            PlayStatusRecord {
                status: PlayStatus::Playing,
                since: "2026-01-01T00:00:00Z".into(),
            },
        );
        d.play_status.insert(
            "gog:2".into(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2026-03-01T00:00:00Z".into(),
            },
        );
        d.share_between(&["steam:1".to_string(), "gog:2".to_string()]);
        assert!(d.favourites.contains("steam:1"));
        assert!(!d.hidden.contains("steam:1"));
        assert_eq!(d.play_status("steam:1"), Some(PlayStatus::Beaten));
        assert_eq!(d.ratings["gog:2"], 4, "the default copy's rating");
    }

    #[test]
    fn an_edited_playtime_follows_what_was_counted() {
        let mut data = UserData::default();
        data.playtime.insert(
            "legendary:Fowl".into(),
            PlaytimeRecord {
                total_minutes: 120,
                last_played: Some("2026-10-01T10:00:00Z".into()),
                ..Default::default()
            },
        );
        // Set lower than counted: the store's own total, still below what
        // was counted, doesn't bring it back.
        data.set_playtime("legendary:Fowl", Some(30 * 60));
        assert_eq!(data.playtime["legendary:Fowl"].total_minutes, 30);
        assert_eq!(data.counted_seconds("legendary:Fowl"), 120 * 60);
        assert_eq!(
            data.playtime["legendary:Fowl"].last_played.as_deref(),
            Some("2026-10-01T10:00:00Z")
        );
        let totals = std::collections::HashMap::from([("Fowl".to_string(), 100u64)]);
        assert!(grown_totals(&data, Runner::Legendary, &totals).is_empty());
        // Time played since counts on top of the edit.
        let totals = std::collections::HashMap::from([("Fowl".to_string(), 150u64)]);
        assert_eq!(
            grown_totals(&data, Runner::Legendary, &totals),
            [("legendary:Fowl".to_string(), 150)]
        );
        // A game never played can be given time, and the edit undone.
        data.set_playtime("gog:2", Some(90));
        assert_eq!(data.playtime["gog:2"].extra_seconds, 30);
        data.set_playtime("gog:2", None);
        assert_eq!(data.counted_seconds("gog:2"), 0);
        assert!(!data.playtime_adjust.contains_key("gog:2"));
        // Carried over with the time.
        data.carry_over("legendary:Fowl", "sideload:x");
        assert_eq!(data.playtime_adjust["sideload:x"], -90 * 60);
        assert_eq!(data.counted_seconds("sideload:x"), 120 * 60);
    }

    #[test]
    fn store_last_played_only_moves_forward_for_games() {
        let mut data = UserData::default();
        data.playtime.insert(
            "launchers:xbox:Old".into(),
            PlaytimeRecord {
                last_played: Some("2026-10-01T10:00:00Z".into()),
                ..Default::default()
            },
        );
        let game = |app: &str, is_dlc| {
            Arc::new(GameInfo {
                app_name: app.into(),
                is_dlc,
                ..Default::default()
            })
        };
        let games = [
            game("xbox:Old", false),
            game("xbox:New", false),
            game("xbox:Dlc", true),
        ];
        let times = std::collections::HashMap::from([
            // Earlier than Irori's own session: kept as it is.
            ("xbox:Old".to_string(), "2026-09-01T10:00:00Z".to_string()),
            // Xbox's seven fractional digits.
            (
                "xbox:New".to_string(),
                "2026-10-08T01:55:12.4411936Z".to_string(),
            ),
            ("xbox:Dlc".to_string(), "2026-10-09T00:00:00Z".to_string()),
            ("xbox:Gone".to_string(), "2026-10-09T00:00:00Z".to_string()),
            ("xbox:Bad".to_string(), "yesterday".to_string()),
        ]);
        let later = later_last_played(&data, Runner::Launchers, &games, &times);
        assert_eq!(
            later,
            [(
                "launchers:xbox:New".to_string(),
                "2026-10-08T01:55:12Z".to_string()
            )]
        );
        let mut newer = times.clone();
        newer.insert("xbox:Old".into(), "2026-10-02T00:00:00Z".into());
        let later = later_last_played(&data, Runner::Launchers, &games, &newer);
        assert_eq!(later.len(), 2);
        assert_eq!(later[1].0, "launchers:xbox:Old");
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
        // Every real change was kept; setting the same status again wasn't.
        let kept: Vec<_> = data.status_history["gog:1"]
            .iter()
            .map(|c| (c.at.as_str(), c.status))
            .collect();
        assert_eq!(
            kept,
            [
                ("monday", Some(PlayStatus::Playing)),
                ("friday", Some(PlayStatus::Beaten)),
                ("later", None)
            ]
        );
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

    #[test]
    fn category_looks_follow_renames_and_order_pinned_categories_first() {
        use crate::smart_categories::{Match, Rule};
        let mut d = UserData::default();
        d.categories.insert("RPG".into(), BTreeSet::new());
        d.categories.insert("Arcade".into(), BTreeSet::new());
        let smart = SmartCategory::new(
            "Big RPGs",
            Match::All,
            vec![Rule::Category {
                names: ["RPG".to_string()].into(),
            }],
            vec![],
        );
        d.smart_categories.insert("s-1".into(), smart);
        let pinned = CategoryLook {
            pinned: true,
            colour: Some("green".into()),
            ..Default::default()
        };
        assert!(set_look(
            &mut d,
            &CategoryRef::Manual("RPG".into()),
            pinned.clone()
        ));
        assert!(!set_look(
            &mut d,
            &CategoryRef::Manual("RPG".into()),
            pinned.clone()
        ));
        assert!(!set_look(
            &mut d,
            &CategoryRef::Manual("Gone".into()),
            pinned.clone()
        ));
        assert!(set_look(
            &mut d,
            &CategoryRef::Smart("s-1".into()),
            pinned.clone()
        ));
        let order: Vec<String> = d
            .categories_in_order()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(order, ["Big RPGs", "RPG", "Arcade"]);

        rename_category(&mut d, "RPG", "Role-playing");
        assert_eq!(d.category_looks["Role-playing"], pinned);
        assert!(!d.category_looks.contains_key("RPG"));
        assert_eq!(
            d.smart_categories["s-1"].rules[0].rule,
            Some(Rule::Category {
                names: ["Role-playing".to_string()].into()
            })
        );
        // Taking the look off removes the entry.
        let manual = CategoryRef::Manual("Role-playing".into());
        assert!(set_look(&mut d, &manual, CategoryLook::default()));
        assert!(d.category_looks.is_empty());
        assert_eq!(d.category_name(&manual), Some("Role-playing"));
        assert_eq!(
            d.category_name(&CategoryRef::Smart("s-1".into())),
            Some("Big RPGs")
        );
    }

    #[test]
    fn a_one_note_a_day_journal_is_read_and_written_in_the_new_shape() {
        let old = r#"{"favourites":["gog:1"],"journal":{"gog:1":{"2026-11-01":{"text":"Old note","updated":"2026-11-01T20:00:00Z"}}}}"#;
        let mut d: UserData = serde_json::from_str(old).unwrap();
        d.migrate();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(d.journal_day("gog:1", day)[0].text, "Old note");
        assert!(d.journal_note("gog:1", "day-2026-11-01").is_some());
        let written = serde_json::to_value(&d).unwrap();
        assert!(written["journal_notes"]["gog:1"]["2026-11-01"].is_array());
        // The older key stays, for an older Irori.
        assert_eq!(
            written["journal"]["gog:1"]["2026-11-01"]["text"],
            "Old note"
        );
        assert_eq!(written["favourites"][0], "gog:1");
    }

    #[test]
    fn a_library_file_round_trips_and_keeps_what_an_older_irori_edited() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let mut d = UserData::default();
        for (time, text) in [((20, 5), "Found the lift"), ((21, 40), "Beat the knight")] {
            journal::put(
                &mut d.journal,
                "gog:1",
                day,
                JournalEntry {
                    time: chrono::NaiveTime::from_hms_opt(time.0, time.1, 0),
                    text: text.into(),
                    ..Default::default()
                },
                "2026-11-02T22:00:00Z",
            );
        }
        // As `UserDataService::update_if` writes it.
        d.legacy_journal = journal::mirror(&d.journal);
        let mut file = serde_json::to_value(&d).unwrap();
        // An older Irori sees one note with both texts.
        let older: LegacyJournal = serde_json::from_value(file["journal"].clone()).unwrap();
        assert_eq!(
            older["gog:1"][&day].text,
            "20:05\nFound the lift\n\n21:40\nBeat the knight"
        );
        // Read back unchanged.
        let mut back: UserData = serde_json::from_value(file.clone()).unwrap();
        back.migrate();
        assert_eq!(back.journal, d.journal);

        // An older Irori edits the day. Whether `journal_notes` (which it
        // doesn't know) is still there or its rewrite dropped it, the edit
        // is kept.
        file["journal"]["gog:1"]["2026-11-02"]["text"] =
            "Found the lift; beat the knight twice".into();
        let mut edited: UserData = serde_json::from_value(file.clone()).unwrap();
        edited.migrate();
        let notes = edited.journal_day("gog:1", day);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].text, "Found the lift; beat the knight twice");
        assert_eq!(notes[1].text, "Found the lift");
        file.as_object_mut().unwrap().remove("journal_notes");
        let mut rewritten: UserData = serde_json::from_value(file).unwrap();
        rewritten.migrate();
        assert_eq!(
            rewritten.journal_day("gog:1", day)[0].text,
            "Found the lift; beat the knight twice"
        );
    }

    #[test]
    fn excludes_and_restores_games_in_smart_category() {
        let mut d = UserData::default();
        let smart = SmartCategory::new("Souls", crate::Match::Any, vec![], vec![]);
        d.smart_categories.insert("s-souls".into(), smart);

        assert!(exclude_smart_game(&mut d, "s-souls", "steam:570"));
        // Inserting the same key again or an empty key is a no-op.
        assert!(!exclude_smart_game(&mut d, "s-souls", "steam:570"));
        assert!(!exclude_smart_game(&mut d, "s-souls", ""));
        assert!(
            d.smart_categories["s-souls"]
                .excluded_games
                .contains("steam:570")
        );

        assert!(restore_smart_game(&mut d, "s-souls", "steam:570"));
        assert!(!restore_smart_game(&mut d, "s-souls", "steam:570"));
        assert!(
            !d.smart_categories["s-souls"]
                .excluded_games
                .contains("steam:570")
        );

        // Missing or unreadable smart categories are safe no-ops.
        assert!(!exclude_smart_game(&mut d, "s-missing", "steam:570"));
        let unreadable: SmartCategory =
            serde_json::from_value(serde_json::json!({"name": "Future", "rules": "not-an-array"}))
                .unwrap();
        assert!(unreadable.is_unreadable());
        d.smart_categories.insert("s-future".into(), unreadable);
        assert!(!exclude_smart_game(&mut d, "s-future", "steam:570"));
        assert!(!restore_smart_game(&mut d, "s-future", "steam:570"));
    }

    #[test]
    fn hybrid_collection_mutations_and_bulk_snapshot_undo() {
        let mut d = UserData::default();
        let mut smart = SmartCategory::new("Souls", crate::Match::Any, vec![], vec![]);
        smart.manual_only = true;
        d.smart_categories.insert("s-souls".into(), smart);

        let before = d.capture_keys(vec!["steam:10".into(), "gog:20".into()]);

        // Add one manually (pinned) and one by clearing exclusion.
        assert!(add_smart_games(
            &mut d,
            "s-souls",
            &[("steam:10".into(), vec!["steam:10".into()], true)],
        ));
        assert!(
            d.smart_categories["s-souls"]
                .included_games
                .contains("steam:10")
        );

        // Removing a manual-only game clears included_games without polluting excluded_games.
        assert!(remove_smart_games(
            &mut d,
            "s-souls",
            &[("steam:10".into(), vec!["steam:10".into()], false)],
        ));
        assert!(d.smart_categories["s-souls"].included_games.is_empty());
        assert!(d.smart_categories["s-souls"].excluded_games.is_empty());

        // Removing a rule-matched game inserts into excluded_games, then BulkSnapshot restores both.
        assert!(add_smart_games(
            &mut d,
            "s-souls",
            &[("steam:10".into(), vec!["steam:10".into()], true)],
        ));
        assert!(remove_smart_games(
            &mut d,
            "s-souls",
            &[("gog:20".into(), vec!["gog:20".into()], true)],
        ));
        assert!(
            d.smart_categories["s-souls"]
                .included_games
                .contains("steam:10")
        );
        assert!(
            d.smart_categories["s-souls"]
                .excluded_games
                .contains("gog:20")
        );

        d.restore(&before);
        assert!(d.smart_categories["s-souls"].included_games.is_empty());
        assert!(d.smart_categories["s-souls"].excluded_games.is_empty());
    }
}
