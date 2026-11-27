//! Categories of both kinds: manual ones (the user puts games in them,
//! `UserData::categories`) and smart ones, filled from rules
//! ([`SmartCategory`], `UserData::smart_categories`). Both can be pinned and
//! given a colour ([`CategoryLook`]); a [`CategoryRef`] names either kind.
//!
//! A smart category is kept the way it was written: rules, match modes and
//! fields this version doesn't know stay in it (`extra` maps, unknown rules,
//! and the whole entry when it can't be read at all), so saving it again or
//! syncing it never strips what a newer Irori wrote. What this version can't
//! read counts as *unknown* when the category is evaluated ([`eval`]).

pub mod eval;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use irori_core::{GameMode, Runner};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::play_status::EffectiveStatus;

pub use eval::{
    CategoryIndex, HltbTimes, IndexKey, Inputs, Truth, count_matches, days_ago, matches_rules_only,
};

/// One category of either kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryRef {
    /// A manual category, by name.
    Manual(String),
    /// A smart category, by id (its name may change).
    Smart(String),
}

/// How a category looks in lists: pinned ones come first (and get a shelf in
/// Big Picture), and a colour and icon mark it on tabs, covers and menus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoryLook {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// One of [`COLOURS`]; a name this version doesn't know is kept and
    /// shown without a colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// One of [`ICONS`]; a name this version doesn't know is kept and
    /// falls back to the category kind's default icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Fields a newer Irori wrote.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CategoryLook {
    pub fn is_empty(&self) -> bool {
        !self.pinned && self.colour.is_none() && self.icon.is_none() && self.extra.is_empty()
    }
}

/// The curated named colours a category can have, as stored. Screens map the
/// classic theme names to theme tokens and derive the extended hues to match
/// the active theme. Custom colours are stored as lowercase `#rrggbb` strings
/// (see [`parse_hex_colour`]).
pub const COLOURS: [&str; 14] = [
    "red", "orange", "amber", "yellow", "lime", "green", "teal", "cyan", "sky", "blue", "indigo",
    "purple", "magenta", "pink",
];

/// Parse a `#rrggbb` custom category colour into a 24-bit RGB integer.
pub fn parse_hex_colour(value: &str) -> Option<u32> {
    let digits = value.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Normalize a 6-digit hex colour (with or without leading `#`) to lowercase `#rrggbb`.
pub fn format_hex_colour(value: &str) -> Option<String> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", digits.to_ascii_lowercase()))
}

/// The curated icons a collection can have, as stored slugs. Screens map each
/// to the matching Lucide `IconName`.
pub const ICONS: [&str; 64] = [
    // Combat & action
    "flame",
    "swords",
    "shield",
    "axe",
    "bow-arrow",
    "crosshair",
    "bomb",
    "zap",
    "biohazard",
    // Fantasy, horror & sci-fi
    "sparkles",
    "wand-sparkles",
    "castle",
    "skull",
    "ghost",
    "moon-star",
    "rocket",
    "orbit",
    "bot",
    "flask-conical",
    // Strategy, puzzle & adventure
    "chess-knight",
    "dices",
    "puzzle",
    "brain",
    "compass",
    "map",
    "pickaxe",
    "wrench",
    "key",
    "eye",
    // Genres, vehicles & media
    "gamepad-2",
    "joystick",
    "tv",
    "car",
    "plane",
    "anchor",
    "music",
    "headphones",
    "book-open",
    "scroll-text",
    "clapperboard",
    "drama",
    // Cozy, nature & creatures
    "coffee",
    "utensils",
    "trees",
    "sprout",
    "flower-2",
    "mountain",
    "tent",
    "snowflake",
    "sun",
    "paw-print",
    "cat",
    // Social, awards & organization
    "users",
    "trophy",
    "medal",
    "crown",
    "gem",
    "heart",
    "star",
    "bookmark",
    "clock",
    "hourglass",
    "circle-check",
    "flag",
];

/// Whether a game must pass every rule or one of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Match {
    #[default]
    All,
    Any,
    /// A mode a newer Irori wrote; the category's answer is unknown.
    Unknown(Value),
}

impl Serialize for Match {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Match::All => s.serialize_str("all"),
            Match::Any => s.serialize_str("any"),
            Match::Unknown(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Match {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v.as_str() {
            Some("all") => Match::All,
            Some("any") => Match::Any,
            _ => Match::Unknown(v),
        })
    }
}

/// What a rule asks of a game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "rule",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Rule {
    /// Owned on any of these stores (any copy of the game).
    Store { stores: BTreeSet<Runner> },
    /// Has any of these store tags (compared like the library's tag filter).
    Tags { tags: Vec<String> },
    /// Plays any of these ways.
    Mode { modes: BTreeSet<GameMode> },
    /// Is in any of these play statuses (as stats count them).
    Status { statuses: BTreeSet<EffectiveStatus> },
    /// Every board that rated it allows this age; unrated games pass only
    /// with `include_unrated`.
    AgeRating {
        max_age: u8,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        include_unrated: bool,
    },
    /// Installed, or not.
    Installed { installed: bool },
    /// Played at least `min_minutes` and at most `max_minutes`.
    Playtime {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_minutes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_minutes: Option<u64>,
    },
    /// Last played at least `min_days` and at most `max_days` local
    /// calendar days ago. Games never played don't match.
    LastPlayed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_days: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_days: Option<u32>,
    },
    /// HowLongToBeat's main-story time is at least `min_hours` and at most
    /// `max_hours`. Games without a known time don't match.
    Hltb {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_hours: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_hours: Option<u32>,
    },
    /// Emulated on any of these systems (systems catalogue ids).
    System { systems: BTreeSet<String> },
    /// In any of these manual categories.
    Category { names: BTreeSet<String> },
    /// Found by this search, as the library's search box finds games.
    Query { query: String },
    /// Marked as a favourite.
    Favourite {},
    /// Has any of these genres (compared without case).
    Genre { genres: Vec<String> },
    /// Added to the library at least `min_days` and at most `max_days`
    /// local calendar days ago: when the account got it, where the store
    /// says, else when Irori first listed it.
    DateAdded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_days: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_days: Option<u32>,
    },
    /// Released from `min_year` to `max_year`. Games without a release
    /// date don't match.
    ReleaseYear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_year: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_year: Option<i32>,
    },
    /// The user gave it `min_stars` to `max_stars`. Unrated games don't
    /// match.
    Rating {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_stars: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_stars: Option<u8>,
    },
    /// Its review score, 0–100, is from `min_score` to `max_score`. Games
    /// without one don't match.
    Score {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_score: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_score: Option<u8>,
    },
    /// Installed and taking `min_gib` to `max_gib` whole GiB. Games without
    /// a known size don't match.
    Size {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_gib: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_gib: Option<u32>,
    },
    /// An update is waiting.
    Update {},
    /// Starts without an internet connection.
    Offline {},
    /// Played `min` to `max` times (sessions; starts that failed don't
    /// count).
    TimesPlayed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
    /// Its sessions last `min_minutes` to `max_minutes` on average. Games
    /// never played don't match.
    AverageSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_minutes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_minutes: Option<u64>,
    },
    /// First played at least `min_days` and at most `max_days` local
    /// calendar days ago. Games never played don't match.
    FirstPlayed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_days: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_days: Option<u32>,
    },
    /// Played `min_percent` to `max_percent` of HowLongToBeat's main-story
    /// time. Games without a known time don't match.
    HltbProgress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_percent: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_percent: Option<u32>,
    },
    /// Hidden in the library.
    Hidden {},
    /// Has a note in the journal.
    Notes {},
    /// Made by a developer whose name holds any of these (compared like
    /// store tags: case, spaces and punctuation don't matter).
    Developer { names: Vec<String> },
    /// Published by a company whose name holds any of these.
    Publisher { names: Vec<String> },
    /// Owned on more than one store (copies shown as one entry).
    Copies {},
    /// Plays with a controller: any support, or with `full` only full
    /// support. Games no source says anything of don't match.
    Controller {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        full: bool,
    },
}

impl Rule {
    /// The kind's name as stored (`"store"`, `"lastPlayed"`).
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::Store { .. } => "store",
            Rule::Tags { .. } => "tags",
            Rule::Mode { .. } => "mode",
            Rule::Status { .. } => "status",
            Rule::AgeRating { .. } => "ageRating",
            Rule::Installed { .. } => "installed",
            Rule::Playtime { .. } => "playtime",
            Rule::LastPlayed { .. } => "lastPlayed",
            Rule::Hltb { .. } => "hltb",
            Rule::System { .. } => "system",
            Rule::Category { .. } => "category",
            Rule::Query { .. } => "query",
            Rule::Favourite {} => "favourite",
            Rule::Genre { .. } => "genre",
            Rule::DateAdded { .. } => "dateAdded",
            Rule::ReleaseYear { .. } => "releaseYear",
            Rule::Rating { .. } => "rating",
            Rule::Score { .. } => "score",
            Rule::Size { .. } => "size",
            Rule::Update {} => "update",
            Rule::Offline {} => "offline",
            Rule::TimesPlayed { .. } => "timesPlayed",
            Rule::AverageSession { .. } => "averageSession",
            Rule::FirstPlayed { .. } => "firstPlayed",
            Rule::HltbProgress { .. } => "hltbProgress",
            Rule::Hidden {} => "hidden",
            Rule::Controller { .. } => "controller",
            Rule::Notes {} => "notes",
            Rule::Developer { .. } => "developer",
            Rule::Publisher { .. } => "publisher",
            Rule::Copies {} => "copies",
        }
    }
}

/// One row of a smart category: a rule and whatever a newer Irori added to
/// it. `rule` is `None` for a rule this version can't read; its answer is
/// unknown, and everything it holds is kept.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Condition {
    pub rule: Option<Rule>,
    /// Keys this version doesn't read (all of them when `rule` is `None`).
    pub extra: Map<String, Value>,
    /// A row that isn't even an object, kept as it was.
    odd: Option<Value>,
}

impl Condition {
    pub fn new(rule: Rule) -> Self {
        Self {
            rule: Some(rule),
            ..Default::default()
        }
    }

    /// This row with another rule, keeping what a newer Irori added to it.
    pub fn with_rule(&self, rule: Rule) -> Self {
        let mut extra = self.extra.clone();
        if self.rule.is_none() {
            // The unknown rule's own fields go with it.
            extra.clear();
        }
        Self {
            rule: Some(rule),
            extra,
            odd: None,
        }
    }

    pub fn is_readable(&self) -> bool {
        self.rule.is_some()
    }
}

impl Serialize for Condition {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if let Some(odd) = &self.odd {
            return odd.serialize(s);
        }
        let mut map = self.extra.clone();
        if let Some(rule) = &self.rule {
            match serde_json::to_value(rule).map_err(serde::ser::Error::custom)? {
                Value::Object(known) => map.extend(known),
                other => return other.serialize(s),
            }
        }
        map.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let Value::Object(map) = value else {
            return Ok(Condition {
                odd: Some(value),
                ..Default::default()
            });
        };
        let Ok(rule) = serde_json::from_value::<Rule>(Value::Object(map.clone())) else {
            return Ok(Condition {
                rule: None,
                extra: map,
                odd: None,
            });
        };
        // What the rule writes back is its own; the rest is a newer Irori's.
        let known = match serde_json::to_value(&rule) {
            Ok(Value::Object(known)) => known,
            _ => Map::new(),
        };
        let extra = map
            .into_iter()
            .filter(|(k, _)| !known.contains_key(k))
            .collect();
        Ok(Condition {
            rule: Some(rule),
            extra,
            odd: None,
        })
    }
}

/// A category filled from rules and/or manual picks: games in `included_games`
/// or passing `rules` (all of them, or any, by `matching`, unless `manual_only`
/// leaves empty `rules` empty) and none of `leave_out` or `excluded_games`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartCategory {
    pub name: String,
    pub matching: Match,
    pub rules: Vec<Condition>,
    /// Games any of these match are left out.
    pub leave_out: Vec<Condition>,
    /// Individual games (by `GameId::key()`) manually included in this category.
    pub included_games: BTreeSet<String>,
    /// Individual games (by `GameId::key()`) manually excluded from this category.
    pub excluded_games: BTreeSet<String>,
    /// When true and `rules` is empty, only `included_games` belong to this
    /// category (rather than matching every game in the library).
    pub manual_only: bool,
    pub look: CategoryLook,
    /// Fields a newer Irori wrote.
    pub extra: Map<String, Value>,
    /// The whole entry as written, when this version can't read it at all.
    unreadable: Option<Value>,
}

/// The stored shape of a readable [`SmartCategory`].
#[derive(Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stored {
    name: String,
    #[serde(rename = "match")]
    matching: Match,
    rules: Vec<Condition>,
    leave_out: Vec<Condition>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    included_games: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    excluded_games: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    manual_only: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    colour: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            name: String::new(),
            matching: Match::All,
            rules: Vec::new(),
            leave_out: Vec::new(),
            included_games: BTreeSet::new(),
            excluded_games: BTreeSet::new(),
            manual_only: false,
            pinned: false,
            colour: None,
            icon: None,
            extra: Map::new(),
        }
    }
}

impl Serialize for SmartCategory {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if let Some(raw) = &self.unreadable {
            return raw.serialize(s);
        }
        let mut extra = self.extra.clone();
        extra.extend(self.look.extra.clone());
        Stored {
            name: self.name.clone(),
            matching: self.matching.clone(),
            rules: self.rules.clone(),
            leave_out: self.leave_out.clone(),
            included_games: self.included_games.clone(),
            excluded_games: self.excluded_games.clone(),
            manual_only: self.manual_only,
            pinned: self.look.pinned,
            colour: self.look.colour.clone(),
            icon: self.look.icon.clone(),
            extra,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for SmartCategory {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        match serde_json::from_value::<Stored>(value.clone()) {
            Ok(s) => Ok(SmartCategory {
                name: s.name,
                matching: s.matching,
                rules: s.rules,
                leave_out: s.leave_out,
                included_games: s.included_games,
                excluded_games: s.excluded_games,
                manual_only: s.manual_only,
                look: CategoryLook {
                    pinned: s.pinned,
                    colour: s.colour,
                    icon: s.icon,
                    extra: Map::new(),
                },
                extra: s.extra,
                unreadable: None,
            }),
            Err(_) => Ok(SmartCategory {
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                look: CategoryLook {
                    pinned: value.get("pinned").and_then(Value::as_bool) == Some(true),
                    colour: value
                        .get("colour")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    icon: value.get("icon").and_then(Value::as_str).map(str::to_owned),
                    extra: Map::new(),
                },
                unreadable: Some(value),
                ..Default::default()
            }),
        }
    }
}

impl SmartCategory {
    /// A new category of `rules`.
    pub fn new(name: &str, matching: Match, rules: Vec<Rule>, leave_out: Vec<Rule>) -> Self {
        Self {
            name: name.trim().to_owned(),
            matching,
            rules: rules.into_iter().map(Condition::new).collect(),
            leave_out: leave_out.into_iter().map(Condition::new).collect(),
            ..Default::default()
        }
    }

    /// Whether this version reads every part of it: its match mode and
    /// every rule. Categories it can't fully read show no games where the
    /// answer depends on the unread part.
    pub fn is_readable(&self) -> bool {
        self.unreadable.is_none()
            && !matches!(self.matching, Match::Unknown(_))
            && self
                .rules
                .iter()
                .chain(&self.leave_out)
                .all(Condition::is_readable)
    }

    /// Whether the entry couldn't be read at all (it can't be edited, only
    /// kept or deleted).
    pub fn is_unreadable(&self) -> bool {
        self.unreadable.is_some()
    }

    /// This category with the pin, colour and icon of `look`, keeping the rest.
    pub fn with_look(&self, look: CategoryLook) -> Self {
        let mut next = self.clone();
        if let Some(Value::Object(raw)) = &mut next.unreadable {
            // Even an entry this version can't read keeps its look in step.
            match look.pinned {
                true => raw.insert("pinned".into(), Value::Bool(true)),
                false => raw.remove("pinned"),
            };
            match &look.colour {
                Some(c) => raw.insert("colour".into(), Value::String(c.clone())),
                None => raw.remove("colour"),
            };
            match &look.icon {
                Some(i) => raw.insert("icon".into(), Value::String(i.clone())),
                None => raw.remove("icon"),
            };
        }
        next.look = look;
        next
    }

    /// The manual categories its rules name that `user` doesn't have (on
    /// this computer), sorted.
    pub fn missing_categories(&self, user: &crate::UserData) -> Vec<String> {
        let mut missing: Vec<String> = self
            .rules
            .iter()
            .chain(&self.leave_out)
            .filter_map(|c| match &c.rule {
                Some(Rule::Category { names }) => Some(names),
                _ => None,
            })
            .flatten()
            .filter(|n| !user.categories.contains_key(*n))
            .cloned()
            .collect();
        missing.sort();
        missing.dedup();
        missing
    }

    /// The rules after renaming manual category `from` to `to`.
    pub fn rename_category(&mut self, from: &str, to: &str) -> bool {
        let mut changed = false;
        for c in self.rules.iter_mut().chain(self.leave_out.iter_mut()) {
            if let Some(Rule::Category { names }) = &mut c.rule
                && names.remove(from)
            {
                names.insert(to.to_owned());
                changed = true;
            }
        }
        changed
    }
}

/// A new smart category id: never reused, and unlike a name it survives
/// renames, so filters, shelves and sync follow the category.
pub fn new_id() -> String {
    format!("s-{}", uuid::Uuid::new_v4().simple())
}

impl crate::Backend {
    /// What smart categories read besides the library and the user data,
    /// as it is now (no I/O).
    pub fn category_inputs(&self) -> Inputs {
        Inputs {
            hltb: self.hltb_times(),
            plays: crate::sessions::stats(),
            updates: self.updates(),
        }
    }

    /// Manually exclude `game_key` from smart category `smart_id`.
    pub fn exclude_from_smart_category(&self, smart_id: &str, game_key: &str) {
        self.userdata
            .exclude_from_smart_category(smart_id, game_key);
    }

    /// Restore a manually excluded `game_key` to smart category `smart_id`.
    pub fn restore_to_smart_category(&self, smart_id: &str, game_key: &str) {
        self.userdata.restore_to_smart_category(smart_id, game_key);
    }

    /// Add `ids` to smart category `smart_id`, clearing any manual exclusion on
    /// their copies and inserting into `included_games` whenever `pin_manually`
    /// returns true (i.e. the game does not already match by rules).
    pub fn add_to_smart_category_many(
        &self,
        smart_id: &str,
        ids: &[irori_core::GameId],
        pin_manually: impl Fn(&irori_core::GameId) -> bool,
    ) {
        self.userdata
            .add_to_smart_category_many(smart_id, ids, pin_manually);
    }

    /// Remove `ids` from smart category `smart_id`, clearing manual inclusions on
    /// their copies and inserting into `excluded_games` whenever
    /// `exclude_rule_match` returns true (i.e. the game would still match by rules).
    pub fn remove_from_smart_category_many(
        &self,
        smart_id: &str,
        ids: &[irori_core::GameId],
        exclude_rule_match: impl Fn(&irori_core::GameId) -> bool,
    ) {
        self.userdata
            .remove_from_smart_category_many(smart_id, ids, exclude_rule_match);
    }
}
