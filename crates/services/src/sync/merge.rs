//! How library data from several computers becomes one, without losing any.
//!
//! Every value the user sets (a favourite, a rating, a play status, a
//! category and its games, a custom title, a yearly goal) is a register:
//! the value with when and on which computer it was last set. Each sync
//! first records what changed here since the last one, then keeps, per
//! register, the one set last. Clearing a value is a change like any other,
//! so a removal reaches the other computers instead of coming back.
//!
//! Changes are noted within a minute of being made, also while the other
//! computers are out of reach. Their times come from a hybrid clock: never
//! earlier than the newest time seen from any computer. A change made after
//! seeing another computer's change therefore wins even when this
//! computer's clock is behind. Only edits made on two computers before
//! either saw the other's are decided by the clocks alone; the computer
//! whose edit lost reports it as a conflict.
//!
//! Values set before sync was turned on count as older than any change
//! made with sync (time 0), except play statuses, which carry their date.
//!
//! Playtime is never summed twice. Sessions are merged as a set (a session
//! is its game and start time). Each computer also publishes, per game, the
//! time it knows of beyond its sessions (store totals, imports): its total
//! minus its sessions. The merged total is the largest of those plus every
//! session, so time played anywhere counts once.

use std::collections::{BTreeMap, BTreeSet};

use irori_core::game::{GameOverrides, PlaytimeRecord};
use serde::{Deserialize, Serialize};

use crate::play_status::PlayStatusRecord;
use crate::sessions::Session;
use crate::userdata::{MAX_RATING, UserData};

/// Milliseconds since the Unix epoch.
pub type Millis = i64;

/// A value with when and where it was last set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reg<T> {
    pub value: T,
    pub at: Millis,
    /// The device that set it.
    #[serde(default)]
    pub by: String,
    /// The newest time of another device's value this one replaced, on
    /// the device that set it. A value set after ours that never saw ours
    /// was set at about the same time: a conflict.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub over: Millis,
}

fn is_zero(n: &Millis) -> bool {
    *n == 0
}

impl<T> Reg<T> {
    /// Whether this was set after `other`; equal times go to the larger
    /// device id, so every computer picks the same one.
    fn after<U>(&self, other: &Reg<U>) -> bool {
        (self.at, &self.by) > (other.at, &other.by)
    }

    fn seen(&self) -> Seen<'_> {
        Seen {
            at: self.at,
            by: &self.by,
            over: self.over,
        }
    }
}

/// A register's dating, apart from its value.
#[derive(Clone, Copy)]
struct Seen<'a> {
    at: Millis,
    by: &'a str,
    over: Millis,
}

/// Registers by key; a missing register holds the default value (off,
/// none).
pub type Lww<K, T> = BTreeMap<K, Reg<T>>;

/// Every register a device publishes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Registers {
    pub favourites: Lww<String, bool>,
    pub hidden: Lww<String, bool>,
    pub ratings: Lww<String, Option<u8>>,
    pub play_status: Lww<String, Option<PlayStatusRecord>>,
    /// Whether each category exists (it may be empty).
    pub categories: Lww<String, bool>,
    /// Category name → game key → member.
    pub category_games: BTreeMap<String, Lww<String, bool>>,
    /// Custom titles and art from the web (see [`shared_overrides`]).
    pub overrides: Lww<String, Option<GameOverrides>>,
    pub backlog_goals: Lww<i32, Option<u32>>,
    /// Games whose playtime was removed (a store said it was a DLC's).
    pub playtime_removed: Lww<String, bool>,
}

impl Registers {
    /// The newest time in any register.
    pub fn newest(&self) -> Millis {
        fn max<K, T>(regs: &Lww<K, T>) -> Millis {
            regs.values().map(|r| r.at).max().unwrap_or(0)
        }
        [
            max(&self.favourites),
            max(&self.hidden),
            max(&self.ratings),
            max(&self.play_status),
            max(&self.categories),
            self.category_games.values().map(max).max().unwrap_or(0),
            max(&self.overrides),
            max(&self.backlog_goals),
            max(&self.playtime_removed),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
}

/// What a device knows of a game's playtime beyond its sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaytimeShare {
    /// Seconds not covered by any session (store totals, imports).
    pub floor_seconds: u64,
    pub first_played: Option<String>,
    pub last_played: Option<String>,
}

/// What one device publishes: its registers, playtime and sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shared {
    pub registers: Registers,
    pub playtime: BTreeMap<String, PlaytimeShare>,
    pub sessions: Vec<Session>,
}

/// Which value a conflict is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictField {
    Favourite,
    Hidden,
    Rating,
    PlayStatus,
    Category,
    CategoryGame,
    Custom,
    Goal,
    Playtime,
}

/// A change made here that lost to one made on another computer at about
/// the same time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub field: ConflictField,
    /// The game key, category name or year.
    pub subject: String,
    /// The category of a [`ConflictField::CategoryGame`] conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The device whose change was kept.
    pub kept_from: String,
    pub kept_at: Millis,
}

/// The result of one merge.
#[derive(Debug, Clone, Default)]
pub struct Merged {
    pub data: UserData,
    pub registers: Registers,
    pub playtime: BTreeMap<String, PlaytimeShare>,
    /// Every session, oldest first, without duplicates.
    pub sessions: Vec<Session>,
    /// Sessions that came from other devices.
    pub sessions_added: usize,
    /// Values changed here by other devices.
    pub changed: usize,
    pub conflicts: Vec<Conflict>,
    /// Newest time seen, for the clock.
    pub clock: Millis,
}

/// This device's side of a merge.
pub struct Local<'a> {
    pub device: &'a str,
    pub data: &'a UserData,
    pub sessions: &'a [Session],
    /// The registers as of the last sync, with the changes recorded since.
    pub registers: &'a Registers,
    /// Games with playtime after the last sync.
    pub known_playtime: &'a BTreeSet<String>,
    /// The wall clock now.
    pub now: Millis,
    /// The newest time this device has seen.
    pub clock: Millis,
    /// [`Local::clock`] at the end of the last sync; changes made here
    /// later haven't reached the other devices yet. `None` before the first
    /// sync.
    pub synced: Option<Millis>,
}

/// When a change recorded now is dated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dating {
    /// Values set before this device first synced: time 0, or the date
    /// they carry.
    BeforeSync,
    /// A change made now, after everything seen so far.
    At(Millis),
}

impl Dating {
    /// The dating for changes found now on a device that has `synced`
    /// before (or not) and has seen times up to `clock`.
    pub fn for_now(synced: Option<Millis>, now: Millis, clock: Millis) -> Self {
        match synced {
            None => Dating::BeforeSync,
            Some(_) => Dating::At(now.max(clock + 1)),
        }
    }

    fn at(self, carried: Option<&str>) -> Millis {
        match self {
            Dating::BeforeSync => carried.and_then(parse_millis).unwrap_or(0).max(0),
            Dating::At(t) => t,
        }
    }
}

/// Record, as set by `me`, every value of `data` that differs from its
/// register. Returns how many changed.
pub fn record(
    regs: &mut Registers,
    data: &UserData,
    known_playtime: &BTreeSet<String>,
    me: &str,
    dating: Dating,
) -> usize {
    let view = View::of(data);
    let plain = dating.at(None);
    let mut changed = 0;
    changed += note(&mut regs.favourites, &view.favourites, me, |_| plain);
    changed += note(&mut regs.hidden, &view.hidden, me, |_| plain);
    changed += note(&mut regs.ratings, &view.ratings, me, |_| plain);
    changed += note(
        &mut regs.play_status,
        &view.play_status,
        me,
        |v: &Option<PlayStatusRecord>| dating.at(v.as_ref().map(|r| r.since.as_str())),
    );
    changed += note(&mut regs.categories, &view.categories, me, |_| plain);
    let names: BTreeSet<String> = regs
        .category_games
        .keys()
        .chain(view.category_games.keys())
        .cloned()
        .collect();
    for name in names {
        let members = view.category_games.get(&name).cloned().unwrap_or_default();
        let regs = regs.category_games.entry(name).or_default();
        changed += note(regs, &members, me, |_| plain);
    }
    changed += note(&mut regs.overrides, &view.overrides, me, |_| plain);
    changed += note(&mut regs.backlog_goals, &view.backlog_goals, me, |_| plain);
    // Playtime removed here since the last sync, or back after a removal.
    for key in known_playtime {
        let removed = regs.playtime_removed.get(key).is_some_and(|r| r.value);
        if !data.playtime.contains_key(key) && !removed {
            set(&mut regs.playtime_removed, key, true, plain, me);
            changed += 1;
        }
    }
    for key in data.playtime.keys() {
        if regs.playtime_removed.get(key).is_some_and(|r| r.value) {
            set(&mut regs.playtime_removed, key, false, plain, me);
            changed += 1;
        }
    }
    changed
}

/// Merge this device's data with what other devices published.
pub fn merge(local: Local<'_>, remotes: &[&Shared]) -> Merged {
    let me = local.device;
    let dating = Dating::for_now(local.synced, local.now, local.clock);
    let mut regs = local.registers.clone();
    record(&mut regs, local.data, local.known_playtime, me, dating);
    let before = View::of(local.data);

    // Keep the newest of every register. A change of ours replaced by
    // another device's that never saw it is a conflict.
    let mut conflicts = Vec::new();
    let mut lost = |field, subject: String, category: Option<String>, ours: Seen, theirs: Seen| {
        if ours.by == me && theirs.by != me && theirs.over < ours.at {
            conflicts.push(Conflict {
                field,
                subject,
                category,
                kept_from: theirs.by.to_string(),
                kept_at: theirs.at,
            });
        }
    };
    macro_rules! take_all {
        ($field:ident, $kind:expr) => {
            for remote in remotes {
                take(
                    &mut regs.$field,
                    &remote.registers.$field,
                    |key, ours, theirs| {
                        lost($kind, key.to_string(), None, ours.seen(), theirs.seen())
                    },
                );
            }
        };
    }
    take_all!(favourites, ConflictField::Favourite);
    take_all!(hidden, ConflictField::Hidden);
    take_all!(ratings, ConflictField::Rating);
    take_all!(play_status, ConflictField::PlayStatus);
    take_all!(categories, ConflictField::Category);
    take_all!(overrides, ConflictField::Custom);
    take_all!(backlog_goals, ConflictField::Goal);
    take_all!(playtime_removed, ConflictField::Playtime);
    for remote in remotes {
        for (name, theirs) in &remote.registers.category_games {
            let ours = regs.category_games.entry(name.clone()).or_default();
            take(ours, theirs, |key, ours, theirs| {
                lost(
                    ConflictField::CategoryGame,
                    key.clone(),
                    Some(name.clone()),
                    ours.seen(),
                    theirs.seen(),
                )
            });
        }
    }

    // Sessions and playtime.
    let (sessions, sessions_added) = union_sessions(local.sessions, remotes);
    let (playtime, shares) = merge_playtime(local.data, local.sessions, &sessions, remotes, &regs);

    // The merged values, applied to this device's data.
    let mut data = local.data.clone();
    let changed = View::from_registers(&regs).apply(&mut data, &before);
    let playtime_changed = differing(&playtime, &local.data.playtime);
    data.playtime = playtime;

    // A value the data can't hold as merged (a category kept for a game
    // added to it after it was deleted, a rating out of range) is recorded
    // as changed here, so every device ends up with the same.
    let seen = remotes
        .iter()
        .map(|r| r.registers.newest())
        .chain([local.clock, regs.newest()])
        .max()
        .unwrap_or(0);
    let known = data.playtime.keys().cloned().collect();
    record(
        &mut regs,
        &data,
        &known,
        me,
        Dating::At(local.now.max(seen + 1)),
    );

    let clock = regs.newest().max(seen);
    Merged {
        data,
        registers: regs,
        playtime: shares,
        sessions,
        sessions_added,
        changed: changed + playtime_changed,
        conflicts,
        clock,
    }
}

/// Record, as set by `me`, every value that differs from its register.
/// Returns how many changed.
fn note<K: Ord + Clone, T: Default + PartialEq + Clone>(
    regs: &mut Lww<K, T>,
    current: &BTreeMap<K, T>,
    me: &str,
    at: impl Fn(&T) -> Millis,
) -> usize {
    let keys: BTreeSet<K> = regs.keys().chain(current.keys()).cloned().collect();
    let absent = T::default();
    let mut changed = 0;
    for key in keys {
        let now = current.get(&key).unwrap_or(&absent);
        let was = regs.get(&key).map_or(&absent, |r| &r.value);
        if now != was {
            let when = at(now);
            set(regs, &key, now.clone(), when, me);
            changed += 1;
        }
    }
    changed
}

fn set<K: Ord + Clone, T>(regs: &mut Lww<K, T>, key: &K, value: T, at: Millis, by: &str) {
    let over = regs
        .get(key)
        .map_or(0, |r| if r.by == by { r.over } else { r.at });
    regs.insert(
        key.clone(),
        Reg {
            value,
            at,
            by: by.to_string(),
            over,
        },
    );
}

/// Take every register of `theirs` set after ours; `replaced` hears of each
/// one that replaces a different value of ours.
fn take<K: Ord + Clone, T: Clone + PartialEq>(
    ours: &mut Lww<K, T>,
    theirs: &Lww<K, T>,
    mut replaced: impl FnMut(&K, &Reg<T>, &Reg<T>),
) {
    for (key, reg) in theirs {
        match ours.get(key) {
            Some(mine) if !reg.after(mine) => {}
            Some(mine) => {
                if mine.value != reg.value {
                    replaced(key, mine, reg);
                }
                ours.insert(key.clone(), reg.clone());
            }
            None => {
                ours.insert(key.clone(), reg.clone());
            }
        }
    }
}

/// The values the user sets, as maps from key to non-default value.
#[derive(Debug, Default, PartialEq)]
struct View {
    favourites: BTreeMap<String, bool>,
    hidden: BTreeMap<String, bool>,
    ratings: BTreeMap<String, Option<u8>>,
    play_status: BTreeMap<String, Option<PlayStatusRecord>>,
    categories: BTreeMap<String, bool>,
    category_games: BTreeMap<String, BTreeMap<String, bool>>,
    overrides: BTreeMap<String, Option<GameOverrides>>,
    backlog_goals: BTreeMap<i32, Option<u32>>,
}

fn on(keys: &BTreeSet<String>) -> BTreeMap<String, bool> {
    keys.iter().map(|k| (k.clone(), true)).collect()
}

fn values<K: Ord + Clone, T: Clone + Default + PartialEq>(regs: &Lww<K, T>) -> BTreeMap<K, T> {
    regs.iter()
        .filter(|(_, r)| r.value != T::default())
        .map(|(k, r)| (k.clone(), r.value.clone()))
        .collect()
}

impl View {
    fn of(d: &UserData) -> Self {
        Self {
            favourites: on(&d.favourites),
            hidden: on(&d.hidden),
            ratings: d
                .ratings
                .iter()
                .filter(|(_, s)| **s > 0)
                .map(|(k, s)| (k.clone(), Some(*s)))
                .collect(),
            play_status: d
                .play_status
                .iter()
                .map(|(k, r)| (k.clone(), Some(r.clone())))
                .collect(),
            categories: d.categories.keys().map(|k| (k.clone(), true)).collect(),
            category_games: d
                .categories
                .iter()
                .map(|(name, games)| (name.clone(), on(games)))
                .collect(),
            overrides: d
                .overrides
                .iter()
                .filter_map(|(k, o)| Some((k.clone(), Some(shared_overrides(o)?))))
                .collect(),
            backlog_goals: d
                .backlog_goals
                .iter()
                .filter(|(_, g)| **g > 0)
                .map(|(y, g)| (*y, Some(*g)))
                .collect(),
        }
    }

    fn from_registers(regs: &Registers) -> Self {
        // A category deleted on one device stays when a game was added to
        // it later on another: that game's place in it isn't lost.
        let mut categories = values(&regs.categories);
        for (name, members) in &regs.category_games {
            let deleted = regs.categories.get(name);
            let revived = members
                .values()
                .any(|m| m.value && deleted.is_none_or(|d| m.after(d)));
            if revived {
                categories.insert(name.clone(), true);
            }
        }
        let category_games = categories
            .keys()
            .map(|name| {
                let members = regs
                    .category_games
                    .get(name)
                    .map(values)
                    .unwrap_or_default();
                (name.clone(), members)
            })
            .collect();
        Self {
            favourites: values(&regs.favourites),
            hidden: values(&regs.hidden),
            ratings: values(&regs.ratings),
            play_status: values(&regs.play_status),
            categories,
            category_games,
            overrides: values(&regs.overrides),
            backlog_goals: values(&regs.backlog_goals),
        }
    }

    /// How many values differ from `other`'s.
    fn differences(&self, other: &View) -> usize {
        let members = |a: &BTreeMap<String, BTreeMap<String, bool>>,
                       b: &BTreeMap<String, BTreeMap<String, bool>>| {
            let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            names
                .into_iter()
                .map(|n| {
                    let empty = BTreeMap::new();
                    differing(a.get(n).unwrap_or(&empty), b.get(n).unwrap_or(&empty))
                })
                .sum::<usize>()
        };
        differing(&self.favourites, &other.favourites)
            + differing(&self.hidden, &other.hidden)
            + differing(&self.ratings, &other.ratings)
            + differing(&self.play_status, &other.play_status)
            + differing(&self.categories, &other.categories)
            + members(&self.category_games, &other.category_games)
            + differing(&self.overrides, &other.overrides)
            + differing(&self.backlog_goals, &other.backlog_goals)
    }

    /// Put these values into `d`, whose values were `before`. Returns how
    /// many changed.
    fn apply(&self, d: &mut UserData, before: &View) -> usize {
        let changed = self.differences(before);
        d.favourites = self.favourites.keys().cloned().collect();
        d.hidden = self.hidden.keys().cloned().collect();
        d.ratings = self
            .ratings
            .iter()
            .filter_map(|(k, s)| Some((k.clone(), (*s)?.clamp(1, MAX_RATING))))
            .collect();
        d.play_status = self
            .play_status
            .iter()
            .filter_map(|(k, r)| Some((k.clone(), r.clone()?)))
            .collect();
        d.categories = self
            .category_games
            .iter()
            .map(|(name, games)| (name.clone(), games.keys().cloned().collect()))
            .collect();
        let mut overrides = BTreeMap::new();
        let keys: BTreeSet<&String> = d.overrides.keys().chain(self.overrides.keys()).collect();
        for key in keys {
            let local = d.overrides.get(key).cloned().unwrap_or_default();
            let shared = self.overrides.get(key).cloned().flatten();
            let o = with_local_art(shared.unwrap_or_default(), &local);
            if !o.is_empty() {
                overrides.insert(key.clone(), o);
            }
        }
        d.overrides = overrides;
        d.backlog_goals = self
            .backlog_goals
            .iter()
            .filter_map(|(y, g)| Some((*y, (*g)?)))
            .collect();
        changed
    }
}

/// How many keys have different values in `a` and `b`.
fn differing<K: Ord, T: PartialEq>(a: &BTreeMap<K, T>, b: &BTreeMap<K, T>) -> usize {
    let only_b = b.keys().filter(|k| !a.contains_key(*k)).count();
    a.iter().filter(|(k, v)| b.get(*k) != Some(*v)).count() + only_b
}

/// Whether an art override can be shown on another computer: an address on
/// the web, not a file on this one.
fn is_web(art: &Option<String>) -> bool {
    art.as_deref()
        .is_some_and(|a| a.starts_with("https://") || a.starts_with("http://"))
}

/// The part of a game's custom title, game modes, copies and art other
/// computers can use: art from files on this computer stays here.
pub fn shared_overrides(o: &GameOverrides) -> Option<GameOverrides> {
    let web = |art: &Option<String>| art.clone().filter(|_| is_web(art));
    let shared = GameOverrides {
        title: o.title.clone(),
        art_cover: web(&o.art_cover),
        art_square: web(&o.art_square),
        art_background: web(&o.art_background),
        art_logo: web(&o.art_logo),
        game_modes: o.game_modes.clone(),
        copy_of: o.copy_of.clone(),
        default_copy: o.default_copy,
    };
    (!shared.is_empty()).then_some(shared)
}

/// `shared`, keeping art from files on this computer where `shared` sets
/// none.
fn with_local_art(shared: GameOverrides, local: &GameOverrides) -> GameOverrides {
    let pick = |s: Option<String>, l: &Option<String>| match s {
        Some(s) => Some(s),
        None => l.clone().filter(|_| !is_web(l)),
    };
    GameOverrides {
        title: shared.title,
        art_cover: pick(shared.art_cover, &local.art_cover),
        art_square: pick(shared.art_square, &local.art_square),
        art_background: pick(shared.art_background, &local.art_background),
        art_logo: pick(shared.art_logo, &local.art_logo),
        game_modes: shared.game_modes,
        copy_of: shared.copy_of,
        default_copy: shared.default_copy,
    }
}

/// A session's identity across devices.
fn session_id(s: &Session) -> (String, Millis, String) {
    // The same start written with another offset is the same session.
    let at = parse_millis(&s.started).unwrap_or(Millis::MIN);
    let raw = if at == Millis::MIN {
        s.started.clone()
    } else {
        String::new()
    };
    (s.game.clone(), at, raw)
}

/// Every session of this device and the others, oldest first, and how many
/// came from the others.
fn union_sessions(local: &[Session], remotes: &[&Shared]) -> (Vec<Session>, usize) {
    let mut seen = BTreeSet::new();
    let mut all = Vec::with_capacity(local.len());
    for s in local {
        if seen.insert(session_id(s)) {
            all.push(s.clone());
        }
    }
    let mut added = 0;
    for remote in remotes {
        for s in &remote.sessions {
            if seen.insert(session_id(s)) {
                all.push(s.clone());
                added += 1;
            }
        }
    }
    all.sort_by_key(|s| (parse_millis(&s.started).unwrap_or(0), s.game.clone()));
    (all, added)
}

fn seconds_by_game(sessions: &[Session]) -> BTreeMap<&str, u64> {
    let mut out: BTreeMap<&str, u64> = BTreeMap::new();
    for s in sessions {
        *out.entry(s.game.as_str()).or_default() += s.seconds;
    }
    out
}

fn record_seconds(r: &PlaytimeRecord) -> u64 {
    r.total_minutes * 60 + r.extra_seconds
}

/// The merged playtime records, and the shares this device publishes.
fn merge_playtime(
    local: &UserData,
    local_sessions: &[Session],
    all_sessions: &[Session],
    remotes: &[&Shared],
    regs: &Registers,
) -> (
    BTreeMap<String, PlaytimeRecord>,
    BTreeMap<String, PlaytimeShare>,
) {
    let mine = seconds_by_game(local_sessions);
    let all = seconds_by_game(all_sessions);
    // The first and last session of each game; sessions are oldest first.
    let mut span: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for s in all_sessions {
        span.entry(s.game.as_str())
            .and_modify(|(_, last)| *last = s.started.as_str())
            .or_insert((s.started.as_str(), s.started.as_str()));
    }
    let mut shares: BTreeMap<String, PlaytimeShare> = BTreeMap::new();
    for (key, r) in &local.playtime {
        let covered = mine.get(key.as_str()).copied().unwrap_or(0);
        shares.insert(
            key.clone(),
            PlaytimeShare {
                floor_seconds: record_seconds(r).saturating_sub(covered),
                first_played: r.first_played.clone(),
                last_played: r.last_played.clone(),
            },
        );
    }
    for remote in remotes {
        for (key, theirs) in &remote.playtime {
            let share = shares.entry(key.clone()).or_default();
            share.floor_seconds = share.floor_seconds.max(theirs.floor_seconds);
            share.first_played = earliest(share.first_played.take(), theirs.first_played.clone());
            share.last_played = latest(share.last_played.take(), theirs.last_played.clone());
        }
    }
    // A game played only through sessions another device sent.
    for key in all.keys() {
        shares.entry((*key).to_string()).or_default();
    }
    shares.retain(|key, _| !regs.playtime_removed.get(key).is_some_and(|r| r.value));

    let mut records = BTreeMap::new();
    for (key, share) in &shares {
        let local_record = local.playtime.get(key);
        let seconds = (share.floor_seconds + all.get(key.as_str()).copied().unwrap_or(0))
            .max(local_record.map_or(0, record_seconds));
        let (first_session, last_session) = span
            .get(key.as_str())
            .map(|(f, l)| (Some(f.to_string()), Some(l.to_string())))
            .unwrap_or_default();
        let record = PlaytimeRecord {
            total_minutes: seconds / 60,
            extra_seconds: seconds % 60,
            first_played: earliest(share.first_played.clone(), first_session),
            last_played: latest(share.last_played.clone(), last_session),
        };
        // Keep the record as it was when merging didn't change it, so its
        // times keep the offset they were written with.
        let record = match local_record {
            Some(r) if same_record(r, &record) => r.clone(),
            _ => record,
        };
        records.insert(key.clone(), record);
    }
    (records, shares)
}

fn same_record(a: &PlaytimeRecord, b: &PlaytimeRecord) -> bool {
    let same_time = |x: &Option<String>, y: &Option<String>| match (x, y) {
        (Some(x), Some(y)) => parse_millis(x) == parse_millis(y),
        (x, y) => x == y,
    };
    record_seconds(a) == record_seconds(b)
        && same_time(&a.first_played, &b.first_played)
        && same_time(&a.last_played, &b.last_played)
}

fn earliest(a: Option<String>, b: Option<String>) -> Option<String> {
    pick_time(a, b, |x, y| x < y)
}

fn latest(a: Option<String>, b: Option<String>) -> Option<String> {
    pick_time(a, b, |x, y| x > y)
}

/// The one of two RFC 3339 times `better` prefers; a time that can't be
/// read loses to one that can.
fn pick_time(
    a: Option<String>,
    b: Option<String>,
    better: impl Fn(Millis, Millis) -> bool,
) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => match (parse_millis(&a), parse_millis(&b)) {
            (Some(x), Some(y)) if better(y, x) => Some(b),
            (None, Some(_)) => Some(b),
            _ => Some(a),
        },
        (a, b) => a.or(b),
    }
}

/// An RFC 3339 time in milliseconds.
pub fn parse_millis(t: &str) -> Option<Millis> {
    chrono::DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.timestamp_millis())
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
