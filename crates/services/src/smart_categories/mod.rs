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

pub use eval::{CategoryIndex, HltbTimes, IndexKey, Truth, count_matches, days_ago};

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
/// Big Picture), and a colour marks it on covers and in menus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoryLook {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// One of [`COLOURS`]; a name this version doesn't know is kept and
    /// shown without a colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// Fields a newer Irori wrote.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CategoryLook {
    pub fn is_empty(&self) -> bool {
        !self.pinned && self.colour.is_none() && self.extra.is_empty()
    }
}

/// The colours a category can have, as stored. Screens map each to a theme
/// colour of the same name.
pub const COLOURS: [&str; 6] = ["red", "yellow", "green", "cyan", "blue", "magenta"];

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

/// A category filled from rules: games pass `rules` (all of them, or any,
/// by `matching`) and none of `leave_out`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartCategory {
    pub name: String,
    pub matching: Match,
    pub rules: Vec<Condition>,
    /// Games any of these match are left out.
    pub leave_out: Vec<Condition>,
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    colour: Option<String>,
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
            pinned: false,
            colour: None,
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
            pinned: self.look.pinned,
            colour: self.look.colour.clone(),
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
                look: CategoryLook {
                    pinned: s.pinned,
                    colour: s.colour,
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

    /// This category with the pin and colour of `look`, keeping the rest.
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
        }
        next.look = look;
        next
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
