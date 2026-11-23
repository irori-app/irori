//! Steam's overlay for games Steam starts through Irori's non-Steam
//! shortcuts (Linux, per game `iroriSteamOverlay`, off by default).
//!
//! Steam shows its overlay in a non-Steam game it starts by preloading its
//! `gameoverlayrenderer.so` into the shortcut's command and naming the
//! shortcut in `SteamOverlayGameId`. Irori's shortcuts run
//! `irori --launch <game> --wait`, which hands the game to the running Irori,
//! so the game would start without either. The waiting invocation reads
//! what Steam set for it ([`SteamLaunch::from_env`]) and passes it with the
//! request; the game then gets it ([`SteamLaunch::apply`]).
//!
//! Irori never looks for the overlay itself or preloads anything Steam
//! didn't hand over for that launch: only Steam's own launch path turns it
//! on, and only while that Steam runs. Under gamescope the nested session
//! hides the overlay, and the sandbox keeps the game from reaching Steam,
//! so neither gets it.

use std::path::Path;

use irori_core::{EnvVar, GameSettings, Runner};
use serde::{Deserialize, Serialize};

/// The file Steam preloads for its overlay (one per architecture).
const OVERLAY_LIBRARY: &str = "gameoverlayrenderer.so";

/// What Steam set for a launch of one of its non-Steam shortcuts that
/// matters to its overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteamLaunch {
    /// Steam's overlay libraries from `LD_PRELOAD`, absolute paths.
    pub preload: Vec<String>,
    /// `SteamOverlayGameId`: the shortcut's game id, digits only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_game_id: Option<String>,
}

/// Why a game started without Steam's overlay although it asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// The setting is off for this game.
    Off,
    /// Steam starts its own games with its overlay.
    SteamGame,
    /// The game runs in gamescope, whose nested session hides the overlay.
    Gamescope,
    /// The sandbox keeps the game from Steam.
    Sandbox,
}

impl SteamLaunch {
    /// What Steam set for this launch, read through `var`: `None` outside
    /// Linux and when Steam preloaded no overlay (the invocation didn't come
    /// from Steam).
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        Self::parse(var)
    }

    fn parse(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let preload: Vec<String> = var("LD_PRELOAD")?
            .split([':', ' '])
            .map(str::trim)
            .filter(|entry| {
                let path = Path::new(entry);
                path.is_absolute()
                    && path.file_name().is_some_and(|n| n == OVERLAY_LIBRARY)
                    && !entry.contains("..")
            })
            .map(str::to_string)
            .collect();
        if preload.is_empty() {
            return None;
        }
        let overlay_game_id = var("SteamOverlayGameId")
            .or_else(|| var("SteamGameId"))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()));
        Some(Self {
            preload,
            overlay_game_id,
        })
    }

    /// Give the game Steam's overlay when its settings ask for it: Steam's
    /// libraries first in its `LD_PRELOAD` (before any of the user's) and
    /// the shortcut's id, unless the user set one.
    pub fn apply(&self, runner: Runner, settings: &mut GameSettings) -> Result<(), Skipped> {
        if !settings.steam_overlay {
            return Err(Skipped::Off);
        }
        if runner == Runner::Steam {
            return Err(Skipped::SteamGame);
        }
        if settings.gamescope.is_enabled() {
            return Err(Skipped::Gamescope);
        }
        if settings.sandbox.is_enabled() {
            return Err(Skipped::Sandbox);
        }
        let env = &mut settings.environment_options;
        let steam = self.preload.join(":");
        match env.iter_mut().find(|v| v.key.trim() == "LD_PRELOAD") {
            Some(own) if !own.value.trim().is_empty() => {
                own.value = format!("{steam}:{}", own.value.trim());
            }
            Some(own) => own.value = steam,
            None => env.push(EnvVar {
                key: "LD_PRELOAD".into(),
                value: steam,
            }),
        }
        if let Some(id) = &self.overlay_game_id
            && !env.iter().any(|v| v.key.trim() == "SteamOverlayGameId")
        {
            env.push(EnvVar {
                key: "SteamOverlayGameId".into(),
                value: id.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const STEAM: &str = "/home/me/.local/share/Steam/ubuntu12_32/gameoverlayrenderer.so:\
                         /home/me/.local/share/Steam/ubuntu12_64/gameoverlayrenderer.so";

    fn launch() -> SteamLaunch {
        SteamLaunch::parse(env(&[
            ("LD_PRELOAD", STEAM),
            ("SteamOverlayGameId", "13853614418917539840"),
            ("SteamGameId", "13853614418917539840"),
        ]))
        .unwrap()
    }

    #[test]
    fn reads_only_steams_overlay_from_a_shortcut_launch() {
        let launch = launch();
        assert_eq!(launch.preload, STEAM.split(':').collect::<Vec<_>>());
        assert_eq!(
            launch.overlay_game_id.as_deref(),
            Some("13853614418917539840")
        );
        // Anything else preloaded, relative or not the overlay, stays out.
        let other = SteamLaunch::parse(env(&[(
            "LD_PRELOAD",
            "/usr/lib/libmangohud.so libfoo.so gameoverlayrenderer.so \
             /tmp/../x/gameoverlayrenderer.so /s/ubuntu12_64/gameoverlayrenderer.so",
        )]))
        .unwrap();
        assert_eq!(other.preload, ["/s/ubuntu12_64/gameoverlayrenderer.so"]);
        assert_eq!(other.overlay_game_id, None);
        // Not started by Steam.
        assert_eq!(SteamLaunch::parse(env(&[])), None);
        assert_eq!(
            SteamLaunch::parse(env(&[("LD_PRELOAD", "/usr/lib/libmangohud.so")])),
            None
        );
        // An id that isn't a number is never passed on.
        let odd = SteamLaunch::parse(env(&[("LD_PRELOAD", STEAM), ("SteamGameId", "x; rm")]));
        assert_eq!(odd.unwrap().overlay_game_id, None);
    }

    #[test]
    fn the_game_gets_the_overlay_only_when_it_asks_and_can_show_it() {
        let mut settings = GameSettings::default();
        assert_eq!(
            launch().apply(Runner::Gog, &mut settings),
            Err(Skipped::Off)
        );
        assert!(settings.environment_options.is_empty());

        settings.steam_overlay = true;
        assert_eq!(
            launch().apply(Runner::Steam, &mut settings.clone()),
            Err(Skipped::SteamGame)
        );
        let mut gamescope = settings.clone();
        gamescope.gamescope.enabled = Some(true);
        assert_eq!(
            launch().apply(Runner::Gog, &mut gamescope),
            Err(Skipped::Gamescope)
        );
        let mut sandboxed = settings.clone();
        sandboxed.sandbox.no_network = true;
        assert_eq!(
            launch().apply(Runner::Gog, &mut sandboxed),
            Err(Skipped::Sandbox)
        );

        // Steam's libraries go before the user's own; the user's id wins.
        settings.environment_options = vec![
            EnvVar {
                key: "LD_PRELOAD".into(),
                value: "/opt/mine.so".into(),
            },
            EnvVar {
                key: "SteamOverlayGameId".into(),
                value: "1".into(),
            },
        ];
        assert_eq!(launch().apply(Runner::Sideload, &mut settings), Ok(()));
        let get = |key: &str| {
            settings
                .environment_options
                .iter()
                .find(|v| v.key == key)
                .map(|v| v.value.clone())
        };
        assert_eq!(get("LD_PRELOAD"), Some(format!("{STEAM}:/opt/mine.so")));
        assert_eq!(get("SteamOverlayGameId").as_deref(), Some("1"));

        let mut fresh = GameSettings {
            steam_overlay: true,
            ..GameSettings::default()
        };
        launch().apply(Runner::Legendary, &mut fresh).unwrap();
        assert_eq!(fresh.environment_options.len(), 2);
        assert_eq!(fresh.environment_options[0].value, STEAM);
        assert_eq!(fresh.environment_options[1].value, "13853614418917539840");
    }
}
