//! The library's field rules ("Manage genres and companies…"): genres,
//! developers and publishers renamed, merged into another or removed,
//! across the whole library. Stores spell the same genre several ways
//! ("RPG", "Role-playing (RPG)", "Rollenspiel"); a rule says once what
//! every game should show instead.
//!
//! Rules are kept in the user data by the value as it was written
//! ([`FieldRules`], matched without regard to case or spacing) and laid
//! over each game at the library rebuild, before the user's own per-game
//! details (which win). An empty replacement removes the value. Rules travel
//! with sync, one register per rule.

use std::collections::BTreeMap;
use std::sync::Arc;

use irori_core::GameInfo;
use serde::{Deserialize, Serialize};

/// Which values a rule is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Genre,
    /// Developers and publishers.
    Company,
}

impl Field {
    /// The register key prefix in sync.
    pub fn id(self) -> &'static str {
        match self {
            Field::Genre => "genre",
            Field::Company => "company",
        }
    }
}

/// Every rule, by field and the value it replaces (as [`rule_key`] folds
/// it); the replacement, empty for "remove".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldRules {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub genres: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub companies: BTreeMap<String, String>,
}

/// How a value is matched: case, outer and doubled spaces ignored.
pub fn rule_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl FieldRules {
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty() && self.companies.is_empty()
    }

    pub fn of(&self, field: Field) -> &BTreeMap<String, String> {
        match field {
            Field::Genre => &self.genres,
            Field::Company => &self.companies,
        }
    }

    fn of_mut(&mut self, field: Field) -> &mut BTreeMap<String, String> {
        match field {
            Field::Genre => &mut self.genres,
            Field::Company => &mut self.companies,
        }
    }

    /// Make `value` show as `to` (empty removes it), or with `None` as
    /// itself again. A rule that would give a value back unchanged is
    /// dropped.
    pub fn set(&mut self, field: Field, value: &str, to: Option<&str>) {
        let key = rule_key(value);
        if key.is_empty() {
            return;
        }
        match to.map(str::trim) {
            Some(to) if to != value.trim() => {
                self.of_mut(field).insert(key, to.to_string());
            }
            _ => {
                self.of_mut(field).remove(&key);
            }
        }
    }

    /// Take the rules of `other` this has none for (a backup's).
    pub fn merge_missing(&mut self, other: &FieldRules) {
        for (field, theirs) in [
            (Field::Genre, &other.genres),
            (Field::Company, &other.companies),
        ] {
            let mine = self.of_mut(field);
            for (key, to) in theirs {
                mine.entry(key.clone()).or_insert_with(|| to.clone());
            }
        }
    }

    /// What `value` shows as: `Some(replacement)` (empty for removed) when
    /// a rule names it.
    pub fn rule(&self, field: Field, value: &str) -> Option<&str> {
        self.of(field).get(&rule_key(value)).map(String::as_str)
    }

    /// `value` as the rules make it; `None` when removed.
    fn map(&self, field: Field, value: &str) -> Option<String> {
        match self.rule(field, value) {
            Some("") => None,
            Some(to) => Some(to.to_string()),
            None => Some(value.to_string()),
        }
    }

    /// Lay the rules over `game`'s genres, developer and publisher.
    /// Genres merged into one show once.
    pub fn apply(&self, game: &mut GameInfo) {
        if self.is_empty() {
            return;
        }
        if !self.genres.is_empty() && !game.genres.is_empty() {
            let mut out: Vec<String> = Vec::with_capacity(game.genres.len());
            for genre in &game.genres {
                if let Some(g) = self.map(Field::Genre, genre)
                    && !out.iter().any(|o| rule_key(o) == rule_key(&g))
                {
                    out.push(g);
                }
            }
            game.genres = out;
        }
        if !self.companies.is_empty() {
            for slot in [&mut game.developer, &mut game.publisher] {
                if let Some(v) = slot.as_deref() {
                    *slot = self.map(Field::Company, v);
                }
            }
        }
    }
}

/// One value the library holds, with how many games have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    pub value: String,
    pub games: usize,
}

/// Every distinct value of `field` among `games` as the stores give them
/// (before rules), most common first, then by name. Spellings that differ
/// only in case or spacing count as one, under the most common spelling.
pub fn values(games: &[Arc<GameInfo>], field: Field) -> Vec<FieldValue> {
    let mut by_key: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for game in games {
        let mut seen: Vec<String> = Vec::new();
        let names: Vec<&str> = match field {
            Field::Genre => game.genres.iter().map(String::as_str).collect(),
            Field::Company => [&game.developer, &game.publisher]
                .into_iter()
                .filter_map(|v| v.as_deref())
                .collect(),
        };
        for name in names {
            let key = rule_key(name);
            if key.is_empty() || seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            *by_key
                .entry(key)
                .or_default()
                .entry(name.trim().to_string())
                .or_default() += 1;
        }
    }
    let mut out: Vec<FieldValue> = by_key
        .into_values()
        .map(|spellings| {
            let games = spellings.values().sum();
            let value = spellings
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(v, _)| v)
                .unwrap_or_default();
            FieldValue { value, games }
        })
        .collect();
    out.sort_by(|a, b| {
        b.games
            .cmp(&a.games)
            .then_with(|| a.value.to_lowercase().cmp(&b.value.to_lowercase()))
    });
    out
}

/// Values that look like one another (the same once folded: punctuation,
/// accents, "&" and "and" ignored) but are kept apart, as `(less common,
/// more common)` pairs: merging the first into the second is suggested.
/// Values a rule already names aren't suggested.
pub fn duplicates(
    values: &[FieldValue],
    field: Field,
    rules: &FieldRules,
) -> Vec<(String, String)> {
    let fold = |v: &str| {
        crate::search::fold(&v.replace('&', " and "))
            .split_whitespace()
            .filter(|w| *w != "and")
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut first: BTreeMap<String, &FieldValue> = BTreeMap::new();
    let mut out = Vec::new();
    // Most common first, so each pair points at the more common spelling.
    for v in values {
        if rules.rule(field, &v.value).is_some() {
            continue;
        }
        let key = fold(&v.value);
        if key.is_empty() {
            continue;
        }
        match first.get(&key) {
            Some(main) => out.push((v.value.clone(), main.value.clone())),
            None => {
                first.insert(key, v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(genres: &[&str], developer: Option<&str>, publisher: Option<&str>) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            genres: genres.iter().map(|g| g.to_string()).collect(),
            developer: developer.map(str::to_string),
            publisher: publisher.map(str::to_string),
            ..Default::default()
        })
    }

    #[test]
    fn rules_rename_merge_and_remove() {
        let mut rules = FieldRules::default();
        rules.set(Field::Genre, "Role-playing (RPG)", Some("RPG"));
        rules.set(Field::Genre, "Indie", Some(""));
        rules.set(Field::Company, "Supergiant", Some("Supergiant Games"));
        rules.set(Field::Company, "Unknown", Some(""));
        // A rule that changes nothing isn't kept; None takes one back.
        rules.set(Field::Genre, "Action", Some("Action"));
        assert!(rules.rule(Field::Genre, "action").is_none());

        let mut g = (*game(
            &["role-playing  (rpg)", "RPG", "Indie", "Action"],
            Some("Supergiant"),
            Some("Unknown"),
        ))
        .clone();
        rules.apply(&mut g);
        assert_eq!(g.genres, ["RPG", "Action"]);
        assert_eq!(g.developer.as_deref(), Some("Supergiant Games"));
        assert_eq!(g.publisher, None);

        rules.set(Field::Genre, "Indie", None);
        assert!(rules.rule(Field::Genre, "Indie").is_none());
    }

    #[test]
    fn values_are_counted_once_per_game_and_duplicates_suggested() {
        let games = [
            game(&["Action", "action"], Some("Devolver"), Some("Devolver")),
            game(&["Action", "Role Playing"], Some("A & B"), None),
            game(&["Role-playing"], Some("A and B"), None),
            game(&["Role-playing"], None, None),
        ];
        let genres = values(&games, Field::Genre);
        let shown: Vec<(&str, usize)> =
            genres.iter().map(|v| (v.value.as_str(), v.games)).collect();
        assert_eq!(
            shown,
            [("Action", 2), ("Role-playing", 2), ("Role Playing", 1)]
        );
        let companies = values(&games, Field::Company);
        assert_eq!(companies[0].value, "A & B");
        assert_eq!(companies.len(), 3);

        let mut rules = FieldRules::default();
        assert_eq!(
            duplicates(&genres, Field::Genre, &rules),
            [("Role Playing".to_string(), "Role-playing".to_string())]
        );
        assert_eq!(
            duplicates(&companies, Field::Company, &rules),
            [("A and B".to_string(), "A & B".to_string())]
        );
        // Settled by a rule: no longer suggested.
        rules.set(Field::Genre, "Role Playing", Some("Role-playing"));
        assert!(duplicates(&genres, Field::Genre, &rules).is_empty());
    }
}
