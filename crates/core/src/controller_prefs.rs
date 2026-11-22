//! Controller preferences (`iroriController`): how controller input behaves
//! everywhere in Irori. Kept apart from `settings.rs` because Big Picture,
//! the in-game overlay and the desktop screens all read it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Controller preferences (`iroriController`). Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ControllerPrefs {
    /// Hide the mouse pointer on controller input until the mouse moves
    /// (`hidePointer`, on unless turned off).
    pub hide_pointer: bool,
    /// The face button that confirms (`confirmButton`).
    pub confirm_button: ConfirmButton,
    /// The family the buttons are shown as, in hints and drawings
    /// (`glyphs`).
    pub glyphs: GlyphStyle,
    /// Buttons given another job (`buttons`): a button's id (`south`,
    /// `east`, `west`, `north`, `left_shoulder`, `right_shoulder`, `start`,
    /// `select`) to a job (`confirm`, `back`, `alt`, `menu`, `prev_page`,
    /// `next_page`, or `none`). Buttons not listed keep their own job;
    /// entries a newer Irori writes stay here unread.
    pub buttons: BTreeMap<String, String>,
}

impl Default for ControllerPrefs {
    fn default() -> Self {
        Self {
            hide_pointer: true,
            confirm_button: ConfirmButton::Auto,
            glyphs: GlyphStyle::Auto,
            buttons: BTreeMap::new(),
        }
    }
}

/// The face button that confirms (`confirmButton`). Values a newer Irori
/// writes read as [`ConfirmButton::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmButton {
    /// The bottom one, or the right one on Nintendo's layout.
    #[default]
    Auto,
    /// The bottom one (A on Xbox, ✕ on PlayStation, B on Nintendo).
    South,
    /// The right one (B on Xbox, ○ on PlayStation, A on Nintendo).
    East,
}

impl<'de> Deserialize<'de> for ConfirmButton {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "south" => ConfirmButton::South,
            "east" => ConfirmButton::East,
            _ => ConfirmButton::Auto,
        })
    }
}

/// The family buttons are shown as (`glyphs`). Values a newer Irori writes
/// read as [`GlyphStyle::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphStyle {
    /// As the controller in hand prints them.
    #[default]
    Auto,
    Xbox,
    PlayStation,
    Nintendo,
}

impl<'de> Deserialize<'de> for GlyphStyle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "xbox" => GlyphStyle::Xbox,
            "play_station" | "playstation" => GlyphStyle::PlayStation,
            "nintendo" => GlyphStyle::Nintendo,
            _ => GlyphStyle::Auto,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_read_as_the_defaults() {
        let prefs: ControllerPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, ControllerPrefs::default());
        assert!(prefs.hide_pointer);
        let prefs: ControllerPrefs = serde_json::from_str(r#"{"hidePointer":false}"#).unwrap();
        assert!(!prefs.hide_pointer);
    }

    #[test]
    fn choices_round_trip_and_unknown_ones_read_as_automatic() {
        let prefs = ControllerPrefs {
            confirm_button: ConfirmButton::East,
            glyphs: GlyphStyle::PlayStation,
            buttons: [("west".to_owned(), "menu".to_owned())].into(),
            ..ControllerPrefs::default()
        };
        let json = serde_json::to_value(&prefs).unwrap();
        assert_eq!(json["confirmButton"], "east");
        assert_eq!(json["glyphs"], "play_station");
        assert_eq!(json["buttons"]["west"], "menu");
        let back: ControllerPrefs = serde_json::from_value(json).unwrap();
        assert_eq!(back, prefs);
        let newer: ControllerPrefs =
            serde_json::from_str(r#"{"confirmButton":"left","glyphs":"steam"}"#).unwrap();
        assert_eq!(newer.confirm_button, ConfirmButton::Auto);
        assert_eq!(newer.glyphs, GlyphStyle::Auto);
    }
}
