use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::merged_lists;

use super::GameSettings;

/// Sparse per-game settings: only the keys a user changed for that game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Map<String, Value>", into = "Map<String, Value>")]
pub struct GameSettingsOverrides(pub Map<String, Value>);

impl From<Map<String, Value>> for GameSettingsOverrides {
    /// Irori was called Ember before its first release; its own keys were
    /// `ember…`, so files written then keep their overrides.
    fn from(map: Map<String, Value>) -> Self {
        Self(
            map.into_iter()
                .map(|(k, v)| match k.strip_prefix("ember") {
                    Some(rest) if rest.starts_with(char::is_uppercase) => {
                        (format!("irori{rest}"), v)
                    }
                    _ => (k, v),
                })
                .collect(),
        )
    }
}

impl From<GameSettingsOverrides> for Map<String, Value> {
    fn from(overrides: GameSettingsOverrides) -> Self {
        overrides.0
    }
}

impl GameSettingsOverrides {
    /// Effective settings: `defaults` with these overrides applied.
    pub fn apply(&self, defaults: &GameSettings) -> GameSettings {
        if self.0.is_empty() {
            return defaults.clone();
        }
        let mut base = serde_json::to_value(defaults).expect("settings serialize");
        if let Value::Object(map) = &mut base {
            for (k, v) in &self.0 {
                // Emulator options merge per emulator and option, so a game
                // that sets one option keeps the defaults of the others.
                if k == "iroriEngineOptions"
                    && let (Some(Value::Object(base)), Value::Object(set)) = (map.get_mut(k), v)
                {
                    for (engine, options) in set {
                        match (base.get_mut(engine), options) {
                            (Some(Value::Object(b)), Value::Object(o)) => {
                                b.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())))
                            }
                            _ => {
                                base.insert(engine.clone(), options.clone());
                            }
                        }
                    }
                    continue;
                }
                // Variables, wrappers and programs hold the game's own
                // entries, laid over the defaults' (`merged_lists`).
                if let Some(field) = merged_lists::key_field(k) {
                    let merged = merged_lists::merge(map.get(k), v, field);
                    map.insert(k.clone(), merged);
                    continue;
                }
                map.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).unwrap_or_else(|_| defaults.clone())
    }

    /// Overrides that turn `defaults` into `effective`: every key whose value
    /// differs, lists that merge by key as the game's own entries.
    pub fn diff(defaults: &GameSettings, effective: &GameSettings) -> Self {
        let (Value::Object(d), Value::Object(e)) = (
            serde_json::to_value(defaults).expect("settings serialize"),
            serde_json::to_value(effective).expect("settings serialize"),
        ) else {
            return Self::default();
        };
        Self(
            e.into_iter()
                .filter_map(|(k, v)| {
                    let value = override_of(&d, &k, v)?;
                    Some((k, value))
                })
                .collect(),
        )
    }

    /// What to store for `key` so the game reads `value` over `defaults`:
    /// `None` when that is the default, the game's own entries for a list
    /// that merges by key, else `value` itself.
    pub fn override_value(defaults: &GameSettings, key: &str, value: Value) -> Option<Value> {
        let Value::Object(d) = serde_json::to_value(defaults).expect("settings serialize") else {
            return Some(value);
        };
        override_of(&d, key, value)
    }

    pub fn is_overridden(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_string(), value);
    }

    pub fn reset(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn override_of(defaults: &Map<String, Value>, key: &str, value: Value) -> Option<Value> {
    match merged_lists::key_field(key) {
        Some(field) => merged_lists::own_entries(defaults.get(key), &value, field),
        None => (defaults.get(key) != Some(&value)).then_some(value),
    }
}
