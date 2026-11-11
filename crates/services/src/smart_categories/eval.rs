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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone};
use irori_core::{GameId, GameInfo, GameMode, Runner};

use super::{CategoryRef, Match, Rule, SmartCategory};
use crate::play_status::EffectiveStatus;
use crate::search::{ParsedQuery, Query, tag_key};
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
}

impl IndexKey {
    pub fn new(library: &LibrarySnapshot, user: &Arc<UserData>, hltb: &HltbTimes) -> Self {
        Self {
            revision: library.revision,
            user: Arc::as_ptr(user) as usize,
            hltb: hltb.revision,
            today: Some(chrono::Local::now().date_naive()),
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
        hltb: &HashMap<String, u32>,
        tz: &Tz,
    ) -> Self {
        let today = key
            .today
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let facts = Facts {
            library,
            user,
            hltb,
            today,
            tz,
        };
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
    hltb: &HashMap<String, u32>,
    today: NaiveDate,
    tz: &Tz,
) -> usize {
    let facts = Facts {
        library,
        user,
        hltb,
        today,
        tz,
    };
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
    today: NaiveDate,
    tz: &'a Tz,
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
    Playtime(Option<u64>, Option<u64>),
    LastPlayed(Option<u32>, Option<u32>),
    Hltb(Option<u32>, Option<u32>),
    System(&'a BTreeSet<String>),
    Category(Vec<&'a BTreeSet<String>>),
    Query(Box<(ParsedQuery, Option<Query>)>),
    Favourite,
    Genre(HashSet<String>),
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
                parsed.modes_match(&game.game_modes)
                    && parsed.tags_match(&game.store_tags)
                    && text.as_ref().is_none_or(|text| {
                        // The search index holds entries: score the entry.
                        let entry = &f.library.games[ix];
                        f.library.search().score(ix, text, entry).is_some()
                    })
            }
            Test::Favourite => f.user.favourites.contains(&key()),
            Test::Genre(wanted) => game.genres.iter().any(|g| wanted.contains(&genre_key(g))),
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
