//! Settings schema. Field names follow Heroic's JSON keys (including its
//! `enviromentOptions` spelling) so Heroic's `config.json` and `GamesConfig/*.json`
//! deserialize directly during import.
//!
//! Global defaults live in [`AppSettings::game`]. Per-game settings are stored as
//! a sparse override map ([`GameSettingsOverrides`]) layered on top of the
//! defaults, so changing a global default reaches every game that has not
//! overridden that field.

use serde::{Deserialize, Deserializer, Serializer};
use serde_json::Value;

mod app;
mod game;
mod overrides;
mod prefs;
mod ui;

#[cfg(test)]
mod tests;

pub use app::*;
pub use game::*;
pub use overrides::*;
pub use prefs::*;
pub use ui::*;

fn ser_false_or_string<S: Serializer>(v: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(name) => s.serialize_str(name),
        None => s.serialize_bool(false),
    }
}

fn de_false_or_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    })
}
