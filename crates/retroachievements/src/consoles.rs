//! RetroAchievements' console ids for Irori's system ids, and how each
//! system's games are hashed.

use crate::hash::Method;

/// A system RetroAchievements has achievements for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Console {
    /// RetroAchievements' console id (`API_GetGameList.php?i=`).
    pub id: u32,
    pub method: Method,
}

/// The console of an Irori system id (`assets/engines/systems.json`), when
/// RetroAchievements has one and Irori can hash its games.
pub fn console_for(system: &str) -> Option<Console> {
    let (id, method) = match system {
        "genesis" => (1, Method::Whole),
        "n64" => (2, Method::N64),
        "snes" => (3, Method::Snes),
        "gb" => (4, Method::Whole),
        "gba" => (5, Method::Whole),
        "gbc" => (6, Method::Whole),
        "nes" => (7, Method::Nes),
        "pce" => (8, Method::PcEngine),
        "segacd" => (9, Method::SegaCd),
        "sms" => (11, Method::Whole),
        "psx" => (12, Method::PlayStation),
        "lynx" => (13, Method::Lynx),
        "gamegear" => (15, Method::Whole),
        "nds" => (18, Method::NintendoDs),
        "ps2" => (21, Method::PlayStation2),
        "atari2600" => (25, Method::Whole),
        "arcade" => (27, Method::Arcade),
        "virtualboy" => (28, Method::Whole),
        "psp" => (41, Method::Psp),
        "atari7800" => (51, Method::Atari7800),
        _ => return None,
    };
    Some(Console { id, method })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_systems() {
        assert_eq!(console_for("psx").unwrap().id, 12);
        assert_eq!(console_for("nes").unwrap().method, Method::Nes);
        assert!(console_for("dos").is_none());
        assert!(console_for("ps5").is_none());
    }
}
