//! Which games each category holds, worked out once per library revision,
//! user data change and HowLongToBeat change ([`CategoryIndex`]), never per
//! frame.
//!
//! Answers are three-valued ([`Truth`]): a rule this version can't read is
//! *unknown*, leaving it out keeps it unknown, and a game whose answer is
//! unknown isn't shown, so a category this version can't fully read shows
//! no games rather than a guess. A rule this version reads still decides
//! where it can: an "any" with a known match matches.
//!
//! An entry of several copies (one game on several stores) is in a category
//! when any of its copies is.
//!
//! Besides the library and the user data, answers read [`Inputs`]:
//! HowLongToBeat times, the play history's counts and the waiting updates,
//! each of which tells its changes apart ([`IndexKey`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone};
use irori_core::{GameId, GameInfo, GameMode, Runner};

use super::{CategoryRef, Match, Rule, SmartCategory};
use crate::play_status::EffectiveStatus;
use crate::search::{ParsedQuery, Query, tag_key};
use crate::sessions::SessionStats;
use crate::{LibrarySnapshot, UserData};

/// A three-valued answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    Yes,
    No,
    /// The answer depends on something this version can't read.
    Unknown,
}

impl Truth {
    pub fn from_bool(b: bool) -> Self {
        if b { Truth::Yes } else { Truth::No }
    }

    /// The opposite; unknown stays unknown.
    #[allow(clippy::should_implement_trait)] // Kleene's "not", not `!` on bools.
    pub fn not(self) -> Self {
        match self {
            Truth::Yes => Truth::No,
            Truth::No => Truth::Yes,
            Truth::Unknown => Truth::Unknown,
        }
    }

    /// Both: a known "no" decides, else an unknown leaves it unknown.
    pub fn and(self, other: Truth) -> Self {
        match (self, other) {
            (Truth::No, _) | (_, Truth::No) => Truth::No,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::Yes,
        }
    }

    /// Either: a known "yes" decides, else an unknown leaves it unknown.
    pub fn or(self, other: Truth) -> Self {
        match (self, other) {
            (Truth::Yes, _) | (_, Truth::Yes) => Truth::Yes,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::No,
        }
    }
}

/// HowLongToBeat main-story hours per game key, as far as they are known
/// (learned when a game's page looked them up). `revision` changes with
/// every change, so cached answers know when to look again.
#[derive(Debug, Clone, Default)]
pub struct HltbTimes {
    pub revision: u64,
    pub hours: Arc<HashMap<String, u32>>,
}

/// What answers read besides the library and the user data.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub hltb: HltbTimes,
    /// How often and how long games were played ([`crate::sessions::stats`]).
    pub plays: Arc<SessionStats>,
    /// Games with an update waiting; a new `Arc` per change.
    pub updates: Arc<HashSet<GameId>>,
}

/// How many local calendar days before `today` the time `stamp` was:
/// RFC 3339 times count by their day in `tz` (a game played at 23:30 local
/// time was played that day, whatever the UTC date), plain `YYYY-MM-DD`
/// dates as written.
pub fn days_ago<Tz: TimeZone>(stamp: &str, today: NaiveDate, tz: &Tz) -> Option<i64> {
    let day = match chrono::DateTime::parse_from_rfc3339(stamp) {
        Ok(at) => at.with_timezone(tz).date_naive(),
        Err(_) => NaiveDate::parse_from_str(stamp.get(..10)?, "%Y-%m-%d").ok()?,
    };
    Some((today - day).num_days())
}

/// What an index was worked out from; an index is current while these are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexKey {
    /// The library snapshot's revision.
    pub revision: u64,
    /// The user data's identity (a new `Arc` is published per change).
    pub user: usize,
    /// [`HltbTimes::revision`].
    pub hltb: u64,
    /// Local today, for "last played" rules.
    pub today: Option<NaiveDate>,
    /// [`SessionStats::revision`].
    pub plays: u64,
    /// The waiting updates' identity.
    pub updates: usize,
}

impl IndexKey {
    pub fn new(library: &LibrarySnapshot, user: &Arc<UserData>, inputs: &Inputs) -> Self {
        Self {
            revision: library.revision,
            user: Arc::as_ptr(user) as usize,
            hltb: inputs.hltb.revision,
            today: Some(chrono::Local::now().date_naive()),
            plays: inputs.plays.revision,
            updates: Arc::as_ptr(&inputs.updates) as *const () as usize,
        }
    }
}

/// The games of every category, and how many each shows.
#[derive(Debug, Clone, Default)]
pub struct CategoryIndex {
    pub key: IndexKey,
    /// Every copy of the entries in each smart category, by id.
    smart: HashMap<String, HashSet<GameId>>,
    /// Shown (not hidden) entries per category of either kind.
    counts: HashMap<CategoryRef, usize>,
    /// Smart categories with a part this version can't read.
    unreadable: BTreeSet<String>,
}

impl CategoryIndex {
    /// Work out every category of `user` over `library` as of `key`
    /// (whose `today` is read once for the whole pass).
    pub fn build<Tz: TimeZone>(
        key: IndexKey,
        library: &LibrarySnapshot,
        user: &UserData,
        inputs: &Inputs,
        tz: &Tz,
    ) -> Self {
        let today = key
            .today
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let facts = Facts::new(library, user, inputs, today, tz);
        let mut smart = HashMap::new();
        let mut counts = HashMap::new();
        let mut unreadable = BTreeSet::new();
        for (id, category) in &user.smart_categories {
            if !category.is_readable() {
                unreadable.insert(id.clone());
            }
            let (members, shown) = Compiled::new(category, user).members(&facts);
            smart.insert(id.clone(), members);
            counts.insert(CategoryRef::Smart(id.clone()), shown);
        }
        for (name, games) in &user.categories {
            let shown = library
                .games
                .iter()
                .filter(|g| !user.hidden.contains(&g.id().key()))
                .filter(|g| {
                    copies(library, g)
                        .iter()
                        .any(|c| games.contains(&c.id().key()))
                })
                .count();
            counts.insert(CategoryRef::Manual(name.clone()), shown);
        }
        Self {
            key,
            smart,
            counts,
            unreadable,
        }
    }

    /// Whether `game` (any copy of an entry) is in smart category `id`.
    pub fn contains(&self, id: &str, game: &GameId) -> bool {
        self.smart.get(id).is_some_and(|s| s.contains(game))
    }

    /// The smart categories `game` is in.
    pub fn smart_of(&self, game: &GameId) -> Vec<&str> {
        self.smart
            .iter()
            .filter(|(_, games)| games.contains(game))
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// How many games a category shows (hidden ones aside).
    pub fn count(&self, category: &CategoryRef) -> usize {
        self.counts.get(category).copied().unwrap_or(0)
    }

    /// Whether this version reads every part of smart category `id`.
    pub fn is_readable(&self, id: &str) -> bool {
        !self.unreadable.contains(id)
    }
}

/// How many entries `category` would show (hidden ones aside), for an
/// editor's preview before it is saved.
pub fn count_matches<Tz: TimeZone>(
    category: &SmartCategory,
    library: &LibrarySnapshot,
    user: &UserData,
    inputs: &Inputs,
    today: NaiveDate,
    tz: &Tz,
) -> usize {
    let facts = Facts::new(library, user, inputs, today, tz);
    Compiled::new(category, user).members(&facts).1
}

/// Every copy of the entry `entry` heads (itself alone when it has none).
fn copies<'a>(library: &'a LibrarySnapshot, entry: &'a Arc<GameInfo>) -> Vec<&'a Arc<GameInfo>> {
    let ids = library.copies_of(&entry.id());
    if ids.is_empty() {
        return vec![entry];
    }
    ids.iter().filter_map(|id| library.get(id)).collect()
}

/// What answers depend on.
struct Facts<'a, Tz: TimeZone> {
    library: &'a LibrarySnapshot,
    user: &'a UserData,
    hltb: &'a HashMap<String, u32>,
    plays: &'a SessionStats,
    updates: &'a HashSet<GameId>,
    today: NaiveDate,
    tz: &'a Tz,
}

impl<'a, Tz: TimeZone> Facts<'a, Tz> {
    fn new(
        library: &'a LibrarySnapshot,
        user: &'a UserData,
        inputs: &'a Inputs,
        today: NaiveDate,
        tz: &'a Tz,
    ) -> Self {
        Self {
            library,
            user,
            hltb: &inputs.hltb.hours,
            plays: &inputs.plays,
            updates: &inputs.updates,
            today,
            tz,
        }
    }

    /// Days since `stamp` within `min..=max` days ago; an unreadable or
    /// missing time doesn't match.
    fn days_within(&self, stamp: Option<&str>, min: Option<u32>, max: Option<u32>) -> bool {
        stamp
            .and_then(|at| days_ago(at, self.today, self.tz))
            .is_some_and(|days| within(days, min.map(i64::from), max.map(i64::from)))
    }
}

const GIB: u64 = 1 << 30;

/// The year a release date names ("2017-03-03", "2017").
fn release_year(game: &GameInfo) -> Option<i32> {
    game.release_date.as_deref()?.trim().get(..4)?.parse().ok()
}

/// Whether `company` holds any of the `wanted` keys ([`tag_key`]).
fn company_matches(company: Option<&str>, wanted: &[String]) -> bool {
    company.is_some_and(|c| {
        let key = tag_key(c);
        wanted.iter().any(|w| key.contains(w.as_str()))
    })
}

/// A rule made ready to test many games.
enum Test<'a> {
    Unknown,
    Store(&'a BTreeSet<Runner>),
    Tags(HashSet<String>),
    Mode(&'a BTreeSet<GameMode>),
    Status(&'a BTreeSet<EffectiveStatus>),
    Age(u8, bool),
    Installed(bool),
    Controller(bool),
    Playtime(Option<u64>, Option<u64>),
    LastPlayed(Option<u32>, Option<u32>),
    Hltb(Option<u32>, Option<u32>),
    System(&'a BTreeSet<String>),
    Category(Vec<&'a BTreeSet<String>>),
    Query(Box<(ParsedQuery, Option<Query>)>),
    Favourite,
    Genre(HashSet<String>),
    DateAdded(Option<u32>, Option<u32>),
    ReleaseYear(Option<i32>, Option<i32>),
    Rating(Option<u8>, Option<u8>),
    Score(Option<u8>, Option<u8>),
    Size(Option<u32>, Option<u32>),
    Update,
    Offline,
    TimesPlayed(Option<u32>, Option<u32>),
    AverageSession(Option<u64>, Option<u64>),
    FirstPlayed(Option<u32>, Option<u32>),
    HltbProgress(Option<u32>, Option<u32>),
    Hidden,
    Notes,
    Developer(Vec<String>),
    Publisher(Vec<String>),
    Copies,
}

/// The keys of the company names a rule holds; empty names name nothing.
fn company_keys(names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|n| tag_key(n))
        .filter(|k| !k.is_empty())
        .collect()
}

fn genre_key(genre: &str) -> String {
    genre.trim().to_lowercase()
}

fn within<T: PartialOrd>(value: T, min: Option<T>, max: Option<T>) -> bool {
    min.is_none_or(|m| value >= m) && max.is_none_or(|m| value <= m)
}

impl<'a> Test<'a> {
    fn new(rule: Option<&'a Rule>, user: &'a UserData) -> Self {
        let Some(rule) = rule else {
            return Test::Unknown;
        };
        match rule {
            Rule::Store { stores } => Test::Store(stores),
            Rule::Tags { tags } => Test::Tags(tags.iter().map(|t| tag_key(t)).collect()),
            Rule::Mode { modes } => Test::Mode(modes),
            Rule::Status { statuses } => Test::Status(statuses),
            Rule::AgeRating {
                max_age,
                include_unrated,
            } => Test::Age(*max_age, *include_unrated),
            Rule::Installed { installed } => Test::Installed(*installed),
            Rule::Controller { full } => Test::Controller(*full),
            Rule::Playtime {
                min_minutes,
                max_minutes,
            } => Test::Playtime(*min_minutes, *max_minutes),
            Rule::LastPlayed { min_days, max_days } => Test::LastPlayed(*min_days, *max_days),
            Rule::Hltb {
                min_hours,
                max_hours,
            } => Test::Hltb(*min_hours, *max_hours),
            Rule::System { systems } => Test::System(systems),
            Rule::Category { names } => Test::Category(
                names
                    .iter()
                    .filter_map(|n| user.categories.get(n))
                    .collect(),
            ),
            Rule::Query { query } => {
                let parsed = ParsedQuery::new(query);
                let text = Query::new(&parsed.text);
                Test::Query(Box::new((parsed, text)))
            }
            Rule::Favourite {} => Test::Favourite,
            Rule::Genre { genres } => Test::Genre(genres.iter().map(|g| genre_key(g)).collect()),
            Rule::DateAdded { min_days, max_days } => Test::DateAdded(*min_days, *max_days),
            Rule::ReleaseYear { min_year, max_year } => Test::ReleaseYear(*min_year, *max_year),
            Rule::Rating {
                min_stars,
                max_stars,
            } => Test::Rating(*min_stars, *max_stars),
            Rule::Score {
                min_score,
                max_score,
            } => Test::Score(*min_score, *max_score),
            Rule::Size { min_gib, max_gib } => Test::Size(*min_gib, *max_gib),
            Rule::Update {} => Test::Update,
            Rule::Offline {} => Test::Offline,
            Rule::TimesPlayed { min, max } => Test::TimesPlayed(*min, *max),
            Rule::AverageSession {
                min_minutes,
                max_minutes,
            } => Test::AverageSession(*min_minutes, *max_minutes),
            Rule::FirstPlayed { min_days, max_days } => Test::FirstPlayed(*min_days, *max_days),
            Rule::HltbProgress {
                min_percent,
                max_percent,
            } => Test::HltbProgress(*min_percent, *max_percent),
            Rule::Hidden {} => Test::Hidden,
            Rule::Notes {} => Test::Notes,
            Rule::Developer { names } => Test::Developer(company_keys(names)),
            Rule::Publisher { names } => Test::Publisher(company_keys(names)),
            Rule::Copies {} => Test::Copies,
        }
    }

    /// The answer for `game`, one copy of the entry at `ix`.
    fn test<Tz: TimeZone>(&self, f: &Facts<'_, Tz>, game: &GameInfo, ix: usize) -> Truth {
        let key = || game.id().key();
        Truth::from_bool(match self {
            Test::Unknown => return Truth::Unknown,
            Test::Store(stores) => stores.contains(&game.runner()),
            Test::Tags(wanted) => game.store_tags.iter().any(|t| wanted.contains(&tag_key(t))),
            Test::Mode(modes) => game.game_modes.iter().any(|m| modes.contains(m)),
            Test::Status(statuses) => statuses.contains(&f.user.effective_status(&key())),
            Test::Age(limit, unrated) => {
                match irori_core::age_rating::strictest_age(&game.age_ratings) {
                    Some(age) => age <= *limit,
                    None => *unrated,
                }
            }
            Test::Installed(installed) => game.is_installed == *installed,
            Test::Controller(full) => game.controller_support.is_some_and(|c| {
                c.meets(if *full {
                    irori_core::ControllerSupport::Full
                } else {
                    irori_core::ControllerSupport::Partial
                })
            }),
            Test::Playtime(min, max) => {
                let minutes = f.user.playtime.get(&key()).map_or(0, |r| r.total_minutes);
                within(minutes, *min, *max)
            }
            Test::LastPlayed(min, max) => f
                .user
                .playtime
                .get(&key())
                .and_then(|r| r.last_played.as_deref())
                .and_then(|at| days_ago(at, f.today, f.tz))
                .is_some_and(|days| within(days, min.map(i64::from), max.map(i64::from))),
            Test::Hltb(min, max) => f
                .hltb
                .get(&key())
                .is_some_and(|hours| within(*hours, *min, *max)),
            Test::System(systems) => game.system.as_ref().is_some_and(|s| systems.contains(s)),
            Test::Category(sets) => {
                let key = key();
                sets.iter().any(|m| m.contains(&key))
            }
            Test::Query(q) => {
                let (parsed, text) = &**q;
                // Smart categories' games aren't known while categories are
                // worked out (one may name another, or itself).
                if !parsed.smart.is_empty() {
                    return Truth::Unknown;
                }
                parsed.modes_match(&game.game_modes)
                    && parsed.tags_match(&game.store_tags)
                    && parsed.categories_match(f.user, &[key()])
                    && text.as_ref().is_none_or(|text| {
                        // The search index holds entries: score the entry.
                        let entry = &f.library.games[ix];
                        f.library.search().score(ix, text, entry).is_some()
                    })
            }
            Test::Favourite => f.user.favourites.contains(&key()),
            Test::Genre(wanted) => game.genres.iter().any(|g| wanted.contains(&genre_key(g))),
            Test::DateAdded(min, max) => {
                let key = key();
                let added = game
                    .date_added
                    .as_deref()
                    .or_else(|| f.user.first_seen.get(&key).map(String::as_str));
                f.days_within(added, *min, *max)
            }
            Test::ReleaseYear(min, max) => {
                release_year(game).is_some_and(|y| within(y, *min, *max))
            }
            Test::Rating(min, max) => f
                .user
                .ratings
                .get(&key())
                .is_some_and(|stars| within(*stars, *min, *max)),
            Test::Score(min, max) => game.rating.is_some_and(|s| within(s, *min, *max)),
            Test::Size(min, max) => game
                .install
                .as_ref()
                .map(|i| i.install_size_bytes)
                .filter(|bytes| game.is_installed && *bytes > 0)
                .is_some_and(|bytes| within(bytes / GIB, min.map(u64::from), max.map(u64::from))),
            Test::Update => f.updates.contains(&game.id()),
            Test::Offline => game.can_run_offline,
            // Before the history is read, plays are unknown, not none.
            Test::TimesPlayed(min, max) => match f.plays.get(&key()) {
                Some(plays) => within(plays.count, *min, *max),
                None => return Truth::Unknown,
            },
            Test::AverageSession(min, max) => match f.plays.get(&key()) {
                Some(plays) => plays
                    .average_minutes()
                    .is_some_and(|m| within(m, *min, *max)),
                None => return Truth::Unknown,
            },
            Test::FirstPlayed(min, max) => {
                let record = f.user.playtime.get(&key());
                f.days_within(record.and_then(|r| r.first_played.as_deref()), *min, *max)
            }
            Test::HltbProgress(min, max) => {
                let key = key();
                let played = f.user.playtime.get(&key).map_or(0, |r| r.total_minutes);
                f.hltb
                    .get(&key)
                    .filter(|hours| **hours > 0)
                    .is_some_and(|hours| {
                        let percent = played * 100 / (u64::from(*hours) * 60);
                        within(percent, min.map(u64::from), max.map(u64::from))
                    })
            }
            Test::Hidden => f.user.hidden.contains(&key()),
            Test::Notes => f
                .user
                .journal
                .get(&key())
                .is_some_and(|days| days.values().any(|note| !note.is_empty())),
            Test::Developer(wanted) => company_matches(game.developer.as_deref(), wanted),
            Test::Publisher(wanted) => company_matches(game.publisher.as_deref(), wanted),
            Test::Copies => f.library.copies_of(&f.library.games[ix].id()).len() > 1,
        })
    }
}

/// A smart category made ready to test many games.
struct Compiled<'a> {
    /// `None` when the whole entry or its match mode can't be read.
    all: Option<bool>,
    rules: Vec<Test<'a>>,
    leave_out: Vec<Test<'a>>,
}

impl<'a> Compiled<'a> {
    fn new(category: &'a SmartCategory, user: &'a UserData) -> Self {
        let all = match (&category.matching, category.is_unreadable()) {
            (_, true) | (Match::Unknown(_), _) => None,
            (Match::All, false) => Some(true),
            (Match::Any, false) => Some(false),
        };
        let tests = |rows: &'a [super::Condition]| {
            rows.iter()
                .map(|c| Test::new(c.rule.as_ref(), user))
                .collect()
        };
        Self {
            all,
            rules: tests(&category.rules),
            leave_out: tests(&category.leave_out),
        }
    }

    /// The answer for one copy of the entry at `ix`.
    fn test<Tz: TimeZone>(&self, f: &Facts<'_, Tz>, game: &GameInfo, ix: usize) -> Truth {
        let Some(all) = self.all else {
            return Truth::Unknown;
        };
        // An empty "all" and an empty "any" both take every game, so a
        // category of "leave out" rows alone shows the rest.
        let kept = if self.rules.is_empty() {
            Truth::Yes
        } else if all {
            self.rules
                .iter()
                .fold(Truth::Yes, |t, r| t.and(r.test(f, game, ix)))
        } else {
            self.rules
                .iter()
                .fold(Truth::No, |t, r| t.or(r.test(f, game, ix)))
        };
        let left_out = self
            .leave_out
            .iter()
            .fold(Truth::No, |t, r| t.or(r.test(f, game, ix)));
        kept.and(left_out.not())
    }

    /// Every copy of the entries in the category, and how many of those
    /// entries aren't hidden.
    fn members<Tz: TimeZone>(&self, f: &Facts<'_, Tz>) -> (HashSet<GameId>, usize) {
        let mut members = HashSet::new();
        let mut shown = 0;
        for (ix, entry) in f.library.games.iter().enumerate() {
            let copies = copies(f.library, entry);
            let truth = copies
                .iter()
                .fold(Truth::No, |t, c| t.or(self.test(f, c, ix)));
            if truth == Truth::Yes {
                members.extend(copies.iter().map(|c| c.id()));
                if !f.user.hidden.contains(&entry.id().key()) {
                    shown += 1;
                }
            }
        }
        (members, shown)
    }
}
