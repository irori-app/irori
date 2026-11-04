//! Launch extras: programs started alongside a game (`iroriLaunchPrograms`)
//! and launch presets (`iroriLaunchPresets`), named sets of launch settings
//! applied to games or to the defaults.
//!
//! A preset holds only the keys it sets, and applying one writes only those
//! keys as overrides, so every setting it leaves out keeps following the
//! defaults. Lists (variables, wrappers, programs) merge into the list the
//! game already has instead of replacing it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::platform::HostPlatform;
use crate::settings::{GameSettings, GameSettingsOverrides};

/// A program started with a game: a trainer, an overlay, voice chat, a
/// script. Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchProgram {
    /// Shown in settings and the game log; the file name when empty.
    pub name: String,
    /// The program: a file, or a command found on `PATH`.
    pub path: String,
    /// Its arguments, split like a shell does.
    pub args: String,
    /// Seconds after the game starts; 0 starts it with the game.
    pub delay_seconds: u32,
    /// Run it with the game's Wine or Proton build, in the game's prefix
    /// (Windows programs). Off runs it as a program of this computer.
    pub in_prefix: bool,
    /// Stop it when the game's session ends. Off leaves it running.
    pub stop_with_game: bool,
    /// Off keeps it in the list without starting it.
    pub enabled: bool,
}

impl Default for LaunchProgram {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            args: String::new(),
            delay_seconds: 0,
            in_prefix: false,
            stop_with_game: true,
            enabled: true,
        }
    }
}

impl LaunchProgram {
    /// Longest delay offered, in seconds.
    pub const MAX_DELAY: u32 = 600;

    /// The name shown for it: its own, else the program's file name.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        let path = self.path.trim();
        path.rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(path)
            .to_string()
    }

    /// Whether it names a Windows program (`.exe`, `.bat`, `.msi`, `.lnk`),
    /// which runs in the game's prefix outside Windows.
    pub fn looks_like_windows(&self) -> bool {
        let lower = self.path.trim().to_ascii_lowercase();
        [".exe", ".bat", ".cmd", ".msi", ".lnk"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    }
}

/// Settings keys a launch preset may hold, in the order they are listed.
pub const PRESET_KEYS: &[&str] = &[
    "wineVersion",
    "enviromentOptions",
    "wrapperOptions",
    "iroriLaunchPrograms",
    "gamescope",
    "showMangohud",
    "useGameMode",
    "nvidiaPrime",
    "enableEsync",
    "enableFsync",
    "enableWoW64",
    "enableWineWayland",
    "enableHDR",
    "enableDXVKFpsLimit",
    "DXVKFpsCap",
    "autoInstallDxvkNvapi",
];

/// List keys whose entries merge by a key field (variables by name,
/// wrappers by command, programs by path) when a preset is applied.
const MERGED_LISTS: &[(&str, &str)] = &[
    ("enviromentOptions", "key"),
    ("wrapperOptions", "exe"),
    ("iroriLaunchPrograms", "path"),
];

/// A named set of launch settings. Irori-only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchPreset {
    /// Stable id: `builtin:<name>` for Irori's own, a random one for the
    /// user's.
    pub id: String,
    pub name: String,
    /// The settings it sets, by settings key (only [`PRESET_KEYS`]).
    pub values: Map<String, Value>,
}

impl LaunchPreset {
    /// A preset of the launch settings `overrides` sets. For a game these are
    /// its per-game overrides; for the defaults, the default settings that
    /// differ from Irori's own ([`GameSettingsOverrides::diff`]).
    pub fn capture(id: String, name: String, overrides: &GameSettingsOverrides) -> Self {
        let values = PRESET_KEYS
            .iter()
            .filter_map(|k| Some((k.to_string(), overrides.0.get(*k)?.clone())))
            .collect();
        Self { id, name, values }
    }

    /// Whether it is one of Irori's own presets.
    pub fn is_builtin(&self) -> bool {
        self.id.starts_with("builtin:")
    }

    /// Settings keys it sets, in [`PRESET_KEYS`] order.
    pub fn keys(&self) -> Vec<&'static str> {
        PRESET_KEYS
            .iter()
            .copied()
            .filter(|k| self.values.contains_key(*k))
            .collect()
    }

    /// The values to write over `current` (the effective settings of what the
    /// preset is applied to): its own values, with lists merged into the
    /// current lists. Keys outside [`PRESET_KEYS`] are left out.
    pub fn values_over(&self, current: &GameSettings) -> Map<String, Value> {
        let current = serde_json::to_value(current).unwrap_or_default();
        self.keys()
            .into_iter()
            .filter_map(|key| {
                let value = self.values.get(key)?;
                let merged = match MERGED_LISTS.iter().find(|(k, _)| *k == key) {
                    Some((_, field)) => merge_list(current.get(key), value, field),
                    None => value.clone(),
                };
                Some((key.to_string(), merged))
            })
            .collect()
    }

    /// Apply to one game: its overrides with the preset's values written in.
    /// Settings the preset doesn't name stay as they were, inherited ones
    /// included.
    pub fn apply_to_game(
        &self,
        defaults: &GameSettings,
        overrides: &GameSettingsOverrides,
    ) -> GameSettingsOverrides {
        let effective = overrides.apply(defaults);
        let mut out = overrides.clone();
        for (key, value) in self.values_over(&effective) {
            out.set(&key, value);
        }
        out
    }

    /// Apply to the defaults every game inherits.
    pub fn apply_to_defaults(&self, defaults: &GameSettings) -> GameSettings {
        let overrides = GameSettingsOverrides(self.values_over(defaults));
        overrides.apply(defaults)
    }
}

/// `current` with every entry of `preset` written in: an entry whose `field`
/// matches one already there replaces it, the others are added at the end.
fn merge_list(current: Option<&Value>, preset: &Value, field: &str) -> Value {
    let mut list = current
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for entry in preset.as_array().into_iter().flatten() {
        let key = entry.get(field);
        match list
            .iter_mut()
            .find(|e| key.is_some() && e.get(field) == key)
        {
            Some(existing) => *existing = entry.clone(),
            None => list.push(entry.clone()),
        }
    }
    Value::Array(list)
}

/// Irori's own presets that make sense on `host`, in menu order. Their names
/// and descriptions are translated by id in the interface.
pub fn builtin_presets(host: HostPlatform) -> Vec<LaunchPreset> {
    let env = |pairs: &[(&str, &str)]| -> Value {
        Value::Array(
            pairs
                .iter()
                .map(|(k, v)| serde_json::json!({"key": k, "value": v}))
                .collect(),
        )
    };
    let preset = |id: &str, values: Value| LaunchPreset {
        id: format!("builtin:{id}"),
        name: String::new(),
        values: match values {
            Value::Object(map) => map,
            _ => Map::new(),
        },
    };
    let mut out = Vec::new();
    if host == HostPlatform::Linux {
        // Render ahead one frame and expose NVAPI so DXVK and VKD3D-Proton
        // can use Reflex; GameMode keeps the CPU at speed.
        out.push(preset(
            "low-latency-nvidia",
            serde_json::json!({
                "enviromentOptions": env(&[
                    ("__GL_MaxFramesAllowed", "1"),
                    ("PROTON_ENABLE_NVAPI", "1"),
                    ("DXVK_ENABLE_NVAPI", "1"),
                ]),
                "autoInstallDxvkNvapi": true,
                "useGameMode": true,
            }),
        ));
        // The Deck's screen in desktop mode; Game Mode has its own gamescope,
        // which launches leave alone.
        out.push(preset(
            "steam-deck-handheld",
            serde_json::json!({
                "gamescope": {
                    "iroriEnabled": true,
                    "enableUpscaling": true,
                    "enableLimiter": true,
                    "enableForceGrabCursor": false,
                    "windowType": "fullscreen",
                    "gameWidth": "1280",
                    "gameHeight": "800",
                    "upscaleWidth": "1280",
                    "upscaleHeight": "800",
                    "upscaleMethod": "fsr",
                    "fpsLimiter": "60",
                    "fpsLimiterNoFocus": "15",
                    "additionalOptions": "",
                },
                "enableFsync": true,
            }),
        ));
    }
    if host != HostPlatform::Windows {
        // Games from before multi-core CPUs and long OpenGL extension lists
        // crash on either; four cores and a 2003 extension list keep them up.
        out.push(preset(
            "older-32-bit-game",
            serde_json::json!({
                "enviromentOptions": env(&[
                    ("WINE_CPU_TOPOLOGY", "4:0,1,2,3"),
                    ("MESA_EXTENSION_MAX_YEAR", "2003"),
                    ("__GL_ExtensionStringVersion", "17700"),
                ]),
                "enableWoW64": false,
            }),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{EnvVar, Wrapper};

    fn var(key: &str, value: &str) -> EnvVar {
        EnvVar {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn programs_keep_their_defaults_from_older_files() {
        let p: LaunchProgram = serde_json::from_str(r#"{"path": "/opt/discord"}"#).unwrap();
        assert!(p.enabled && p.stop_with_game && !p.in_prefix);
        assert_eq!(p.label(), "discord");
        let p = LaunchProgram {
            name: " Trainer ".into(),
            path: r"C:\Tools\trainer.EXE".into(),
            ..Default::default()
        };
        assert_eq!(p.label(), "Trainer");
        assert!(p.looks_like_windows());
        let settings: GameSettings =
            serde_json::from_str(r#"{"iroriLaunchPrograms": [{"path": "obs"}]}"#).unwrap();
        assert_eq!(settings.launch_programs[0].path, "obs");
    }

    #[test]
    fn capture_keeps_only_launch_keys_a_game_sets() {
        let mut overrides = GameSettingsOverrides::default();
        overrides.set("showMangohud", Value::Bool(true));
        overrides.set("launcherArgs", Value::String("-dx11".into()));
        overrides.set("winePrefix", Value::String("/p".into()));
        let preset = LaunchPreset::capture("u1".into(), "Mine".into(), &overrides);
        assert_eq!(preset.keys(), vec!["showMangohud"]);
        assert!(!preset.is_builtin());
    }

    #[test]
    fn applying_writes_only_its_keys_so_defaults_still_flow() {
        let mut defaults = GameSettings::default();
        let preset = LaunchPreset {
            values: serde_json::json!({"showMangohud": true})
                .as_object()
                .unwrap()
                .clone(),
            ..Default::default()
        };
        let overrides = preset.apply_to_game(&defaults, &GameSettingsOverrides::default());
        assert_eq!(overrides.0.len(), 1);
        // A later change of an unrelated default reaches the game.
        defaults.use_game_mode = true;
        let effective = overrides.apply(&defaults);
        assert!(effective.show_mangohud && effective.use_game_mode);
    }

    #[test]
    fn lists_merge_into_what_the_game_has() {
        let defaults = GameSettings {
            environment_options: vec![var("DXVK_HUD", "fps"), var("A", "1")],
            wrapper_options: vec![Wrapper {
                exe: "strace".into(),
                args: String::new(),
            }],
            ..Default::default()
        };
        let preset = LaunchPreset {
            values: serde_json::json!({
                "enviromentOptions": [{"key": "A", "value": "2"}, {"key": "B", "value": "3"}],
                "wrapperOptions": [{"exe": "strace", "args": "-f"}],
                "iroriLaunchPrograms": [{"path": "/opt/voice"}],
                "launcherArgs": "-ignored",
            })
            .as_object()
            .unwrap()
            .clone(),
            ..Default::default()
        };
        let overrides = preset.apply_to_game(&defaults, &GameSettingsOverrides::default());
        let effective = overrides.apply(&defaults);
        assert_eq!(
            effective.environment_options,
            vec![var("DXVK_HUD", "fps"), var("A", "2"), var("B", "3")]
        );
        assert_eq!(effective.wrapper_options.len(), 1);
        assert_eq!(effective.wrapper_options[0].args, "-f");
        assert_eq!(effective.launch_programs[0].path, "/opt/voice");
        assert!(effective.launcher_args.is_empty(), "not a preset key");
        assert!(!overrides.is_overridden("launcherArgs"));

        // Applying again changes nothing.
        let again = preset.apply_to_game(&defaults, &overrides);
        assert_eq!(again, overrides);
    }

    #[test]
    fn applies_to_the_defaults() {
        let defaults = GameSettings::default();
        let deck = builtin_presets(HostPlatform::Linux)
            .into_iter()
            .find(|p| p.id == "builtin:steam-deck-handheld")
            .unwrap();
        let out = deck.apply_to_defaults(&defaults);
        assert!(out.gamescope.is_enabled());
        assert_eq!(out.gamescope.game_width, "1280");
        assert_eq!(out.verbose_logs, defaults.verbose_logs);
    }

    #[test]
    fn builtin_presets_hold_only_preset_keys_and_parse() {
        for host in [
            HostPlatform::Linux,
            HostPlatform::Mac,
            HostPlatform::Windows,
        ] {
            for preset in builtin_presets(host) {
                assert!(preset.is_builtin());
                assert_eq!(preset.keys().len(), preset.values.len(), "{}", preset.id);
                let applied = preset.apply_to_defaults(&GameSettings::default());
                assert_ne!(applied, GameSettings::default(), "{}", preset.id);
            }
        }
        assert_eq!(builtin_presets(HostPlatform::Linux).len(), 3);
        assert!(builtin_presets(HostPlatform::Windows).is_empty());
    }
}
