//! The graphics mods Irori puts into a game folder, and what they share.
//!
//! Every graphics mod goes through the same layer: [`crate::detect`] finds
//! the program that draws the game and the folder its files go to,
//! [`crate::fetch`] downloads from the mod's official source into a cache,
//! [`crate::journal`] writes the files with a manifest of their hashes,
//! turns the mod on and off by renaming one file, and removes only what it
//! wrote and nobody changed. A [`GraphicsMod`] names which mod a manifest
//! belongs to, where its manifest is kept and where its downloads are
//! cached.

use std::path::PathBuf;

use irori_core::{GameId, Paths};
use serde::{Deserialize, Serialize};

/// A graphics mod Irori can install into a game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsMod {
    /// ReShade itself. Manifests written before Irori knew other mods name
    /// no mod and are ReShade's.
    #[default]
    Reshade,
}

impl GraphicsMod {
    /// The mod's name; never translated.
    pub fn name(self) -> &'static str {
        match self {
            GraphicsMod::Reshade => "ReShade",
        }
    }

    /// Whether this is ReShade (for `skip_serializing_if`, so ReShade's
    /// manifests keep the shape they had).
    pub fn is_reshade(&self) -> bool {
        *self == GraphicsMod::Reshade
    }

    /// Where a game's install manifest of this mod lives.
    pub fn journal_path(self, id: &GameId) -> PathBuf {
        let file = format!("{}.json", irori_core::paths::sanitize(&id.app_name));
        let base = Paths::get().data.join("reshade");
        match self {
            GraphicsMod::Reshade => base.join(id.runner.id()).join(file),
        }
    }

    /// Where this mod's downloads are cached.
    pub fn cache_dir(self) -> PathBuf {
        let base = Paths::get().cache.join("reshade");
        match self {
            GraphicsMod::Reshade => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_serializes_the_mods() {
        assert_eq!(GraphicsMod::default(), GraphicsMod::Reshade);
        assert_eq!(GraphicsMod::Reshade.name(), "ReShade");
        assert_eq!(
            serde_json::to_string(&GraphicsMod::Reshade).unwrap(),
            "\"reshade\""
        );
    }
}
