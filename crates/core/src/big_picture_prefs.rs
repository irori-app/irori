//! Big Picture preferences that belong to this computer rather than to a
//! theme (`iroriBigPicture`): navigation sounds and the idle screen.

use serde::{Deserialize, Serialize};

/// Big Picture's own preferences (`iroriBigPicture`). Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BigPicturePrefs {
    /// Short tones when the focus moves, something opens or closes
    /// (`sounds`, off unless chosen).
    pub sounds: NavSounds,
    /// Their loudness in percent (`soundVolume`, 1–100).
    pub sound_volume: u32,
}

impl Default for BigPicturePrefs {
    fn default() -> Self {
        Self {
            sounds: NavSounds::Off,
            sound_volume: 40,
        }
    }
}

impl BigPicturePrefs {
    /// [`Self::sound_volume`] as a factor, kept to 1–100 %.
    pub fn sound_gain(&self) -> f32 {
        self.sound_volume.clamp(1, 100) as f32 / 100.0
    }
}

/// Big Picture's navigation sounds (`sounds`). Values a newer Irori writes
/// read as [`NavSounds::Off`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NavSounds {
    #[default]
    Off,
    /// Rounded sine tones.
    Soft,
    /// Brighter, shorter blips, as older consoles made.
    Classic,
}

impl<'de> Deserialize<'de> for NavSounds {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "soft" => NavSounds::Soft,
            "classic" => NavSounds::Classic,
            _ => NavSounds::Off,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_leniently_and_keeps_the_volume_in_range() {
        let prefs: BigPicturePrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, BigPicturePrefs::default());
        let prefs: BigPicturePrefs =
            serde_json::from_str(r#"{"sounds":"classic","soundVolume":400}"#).unwrap();
        assert_eq!(prefs.sounds, NavSounds::Classic);
        assert_eq!(prefs.sound_gain(), 1.0);
        let prefs: BigPicturePrefs = serde_json::from_str(r#"{"sounds":"chiptune"}"#).unwrap();
        assert_eq!(prefs.sounds, NavSounds::Off);
        assert_eq!(serde_json::to_value(NavSounds::Soft).unwrap(), "soft");
    }
}
