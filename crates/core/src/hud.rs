//! Native Performance HUD domain model (MangoHud on Linux, Metal HUD on macOS).
//!
//! All types in this module compile unconditionally on every target OS so
//! cross-platform validation (`cargo check`, `cargo test`) covers Linux, macOS
//! and Windows code paths together. Only [`HudBackend::current`] branches on
//! `target_os`.

use serde::{Deserialize, Serialize};

/// Default MangoHud toggle shortcut matching MangoHud's factory default.
pub const DEFAULT_TOGGLE_HUD: &str = "Shift_R+F12";

/// Active HUD backend for the host operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudBackend {
    MangoHud,
    MetalHud,
    Unsupported,
}

impl HudBackend {
    /// Backend supported on the current host OS.
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::MangoHud
        }
        #[cfg(target_os = "macos")]
        {
            Self::MetalHud
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Self::Unsupported
        }
    }

    /// Whether the host platform supports a native Performance HUD.
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// Global HUD policy configured in application Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GlobalHudMode {
    /// Do not inject or remove any HUD environment variables.
    #[default]
    Off,
    /// Apply Irori's managed profile (MangoHud.conf on Linux, Metal HUD on macOS).
    IroriProfile,
    /// Linux: enable MangoHud using the user's external `~/.config/MangoHud/MangoHud.conf`.
    /// macOS: enable Metal HUD with default alignment.
    SystemDefault,
}

/// Per-game HUD override mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GameHudMode {
    /// Follow the global Performance HUD configuration.
    #[default]
    InheritGlobal,
    /// Use a custom profile stored specifically for this game.
    Custom,
    /// Use the system's default HUD configuration for this game.
    SystemDefault,
    /// Explicitly disable the HUD for this game (`DISABLE_MANGOHUD=1` / `MTL_HUD_ENABLED=0`).
    Disabled,
}

/// Quick one-click presets for MangoHud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudPreset {
    MinimalFps,
    HorizontalCompact,
    #[default]
    BalancedGaming,
    FullDiagnostics,
    Custom,
}

/// Screen anchor position for MangoHud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudPosition {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    MiddleLeft,
    MiddleRight,
}

impl HudPosition {
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
        Self::TopCenter,
        Self::BottomCenter,
        Self::MiddleLeft,
        Self::MiddleRight,
    ];

    pub const fn as_mangohud_value(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
            Self::TopCenter => "top-center",
            Self::BottomCenter => "bottom-center",
            Self::MiddleLeft => "middle-left",
            Self::MiddleRight => "middle-right",
        }
    }
}

/// Vertical or horizontal layout for MangoHud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudOrientation {
    #[default]
    Vertical,
    Horizontal,
}

/// MangoHud frame limiter pacing method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FpsLimitMethod {
    #[default]
    Late,
    Early,
}

impl FpsLimitMethod {
    pub const fn as_mangohud_value(self) -> &'static str {
        match self {
            Self::Late => "late",
            Self::Early => "early",
        }
    }
}

/// Color palette selection for MangoHud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudColorPalette {
    #[default]
    MangoHudDefault,
    SyncIroriTheme,
    Monochrome,
    Custom,
}

/// Left or right modifier side for X11 keysyms (`Shift_L` vs `Shift_R`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModifierSide {
    #[default]
    Left,
    Right,
}

/// Resilient GPU selection persisted without fragile raw PCI bus addresses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GpuSelection {
    #[default]
    AutoDedicated,
    AllGpus,
    ByHardwareId {
        #[serde(default)]
        vendor_id: String,
        #[serde(default)]
        device_id: String,
        #[serde(default)]
        subvendor_id: Option<String>,
        #[serde(default)]
        subdevice_id: Option<String>,
    },
}

/// Pure RGB hex color palette decoupled from GPUI types.
///
/// Every field holds a 6-digit uppercase hexadecimal `RRGGBB` string without a
/// leading `#` and without an alpha suffix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeHudColors {
    pub background_color: String,
    pub text_color: String,
    pub gpu_color: String,
    pub cpu_color: String,
    pub vram_color: String,
    pub ram_color: String,
    pub engine_color: String,
    pub frametime_color: String,
    pub io_color: String,
    pub battery_color: String,
    pub accent_color: String,
}

impl Default for ThemeHudColors {
    fn default() -> Self {
        Self {
            background_color: "020202".into(),
            text_color: "FFFFFF".into(),
            gpu_color: "2E9762".into(),
            cpu_color: "2E97CB".into(),
            vram_color: "AD64C1".into(),
            ram_color: "C26693".into(),
            engine_color: "EB5B5B".into(),
            frametime_color: "00FF00".into(),
            io_color: "A491D3".into(),
            battery_color: "FF9078".into(),
            accent_color: "EB5B5B".into(),
        }
    }
}

impl ThemeHudColors {
    /// Clean monochrome palette (high-contrast white/silver on near-black).
    pub fn monochrome() -> Self {
        Self {
            background_color: "0B0B0C".into(),
            text_color: "F2F2F2".into(),
            gpu_color: "D4D4D8".into(),
            cpu_color: "D4D4D8".into(),
            vram_color: "A1A1AA".into(),
            ram_color: "A1A1AA".into(),
            engine_color: "E4E4E7".into(),
            frametime_color: "F4F4F5".into(),
            io_color: "A1A1AA".into(),
            battery_color: "D4D4D8".into(),
            accent_color: "FFFFFF".into(),
        }
    }
}

/// Complete MangoHud configuration managed by Irori.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MangoHudProfile {
    pub preset: HudPreset,

    // GPU group
    pub gpu_stats: bool,
    pub gpu_temp: bool,
    pub gpu_junction_temp: bool,
    pub gpu_mem_temp: bool,
    pub gpu_core_clock: bool,
    pub gpu_power: bool,
    pub gpu_fan: bool,
    pub vram: bool,

    // CPU & Memory group
    pub cpu_stats: bool,
    pub cpu_temp: bool,
    pub cpu_mhz: bool,
    pub cpu_power: bool,
    pub ram: bool,

    // FPS & Frame Pacing group
    pub fps: bool,
    pub frametime: bool,
    pub frame_timing: bool,
    pub present_mode: bool,

    // Runtime & Extras group
    pub wine: bool,
    pub winesync: bool,
    pub fsr: bool,
    pub hdr: bool,
    pub gamemode: bool,
    pub throttling_status: bool,
    pub gamepad_battery: bool,

    // Layout & Shortcut
    pub position: HudPosition,
    pub orientation: HudOrientation,
    pub round_corners: u8,
    pub hud_compact: bool,
    pub hud_no_margin: bool,
    pub font_scale: f32,
    pub background_alpha: f32,
    pub alpha: f32,
    pub toggle_hud: String,
    pub no_display: bool,

    // Frame Limiter
    pub fps_limit: Option<u32>,
    pub fps_limit_method: FpsLimitMethod,

    // Colors & Theme
    pub palette: HudColorPalette,
    pub custom_colors: ThemeHudColors,
    pub fps_color_change: bool,

    // Hardware selection and labels
    pub gpu_selection: GpuSelection,
    pub gpu_label_override: String,
    pub cpu_label_override: String,

    // Native Linux OpenGL legacy wrapper (`mangohud` LD_PRELOAD)
    pub opengl_legacy_wrapper: bool,
    pub mangohud_dlsym: bool,
}

impl Default for MangoHudProfile {
    fn default() -> Self {
        Self::from_preset(HudPreset::BalancedGaming)
    }
}

impl MangoHudProfile {
    /// Construct a fresh profile configured for `preset`.
    pub fn from_preset(preset: HudPreset) -> Self {
        let mut profile = Self {
            preset,
            gpu_stats: false,
            gpu_temp: false,
            gpu_junction_temp: false,
            gpu_mem_temp: false,
            gpu_core_clock: false,
            gpu_power: false,
            gpu_fan: false,
            vram: false,
            cpu_stats: false,
            cpu_temp: false,
            cpu_mhz: false,
            cpu_power: false,
            ram: false,
            fps: true,
            frametime: false,
            frame_timing: false,
            present_mode: false,
            wine: false,
            winesync: false,
            fsr: false,
            hdr: false,
            gamemode: false,
            throttling_status: false,
            gamepad_battery: false,
            position: HudPosition::TopLeft,
            orientation: HudOrientation::Vertical,
            round_corners: 8,
            hud_compact: false,
            hud_no_margin: false,
            font_scale: 1.0,
            background_alpha: 0.5,
            alpha: 1.0,
            toggle_hud: DEFAULT_TOGGLE_HUD.into(),
            no_display: false,
            fps_limit: None,
            fps_limit_method: FpsLimitMethod::Late,
            palette: HudColorPalette::MangoHudDefault,
            custom_colors: ThemeHudColors::default(),
            fps_color_change: false,
            gpu_selection: GpuSelection::AutoDedicated,
            gpu_label_override: String::new(),
            cpu_label_override: String::new(),
            opengl_legacy_wrapper: false,
            mangohud_dlsym: true,
        };
        profile.apply_preset(preset);
        profile
    }

    /// Apply a preset's metric and layout toggles while preserving the user's
    /// hotkey, GPU selection, custom labels, palette and legacy wrapper choice.
    pub fn apply_preset(&mut self, preset: HudPreset) {
        self.preset = preset;
        if preset == HudPreset::Custom {
            return;
        }
        // Reset all metric toggles first.
        self.gpu_stats = false;
        self.gpu_temp = false;
        self.gpu_junction_temp = false;
        self.gpu_mem_temp = false;
        self.gpu_core_clock = false;
        self.gpu_power = false;
        self.gpu_fan = false;
        self.vram = false;
        self.cpu_stats = false;
        self.cpu_temp = false;
        self.cpu_mhz = false;
        self.cpu_power = false;
        self.ram = false;
        self.fps = false;
        self.frametime = false;
        self.frame_timing = false;
        self.present_mode = false;
        self.wine = false;
        self.winesync = false;
        self.fsr = false;
        self.hdr = false;
        self.gamemode = false;
        self.throttling_status = false;
        self.gamepad_battery = false;

        match preset {
            HudPreset::MinimalFps => {
                self.fps = true;
                self.orientation = HudOrientation::Vertical;
                self.hud_compact = true;
            }
            HudPreset::HorizontalCompact => {
                self.gpu_stats = true;
                self.gpu_temp = true;
                self.vram = true;
                self.cpu_stats = true;
                self.cpu_temp = true;
                self.ram = true;
                self.fps = true;
                self.frametime = true;
                self.orientation = HudOrientation::Horizontal;
                self.hud_compact = true;
            }
            HudPreset::BalancedGaming => {
                self.gpu_stats = true;
                self.gpu_temp = true;
                self.gpu_power = true;
                self.vram = true;
                self.cpu_stats = true;
                self.cpu_temp = true;
                self.ram = true;
                self.fps = true;
                self.frametime = true;
                self.frame_timing = true;
                self.orientation = HudOrientation::Vertical;
                self.hud_compact = false;
            }
            HudPreset::FullDiagnostics => {
                self.gpu_stats = true;
                self.gpu_temp = true;
                self.gpu_junction_temp = true;
                self.gpu_mem_temp = true;
                self.gpu_core_clock = true;
                self.gpu_power = true;
                self.gpu_fan = true;
                self.vram = true;
                self.cpu_stats = true;
                self.cpu_temp = true;
                self.cpu_mhz = true;
                self.cpu_power = true;
                self.ram = true;
                self.fps = true;
                self.frametime = true;
                self.frame_timing = true;
                self.present_mode = true;
                self.wine = true;
                self.winesync = true;
                self.fsr = true;
                self.hdr = true;
                self.gamemode = true;
                self.throttling_status = true;
                self.gamepad_battery = true;
                self.orientation = HudOrientation::Vertical;
                self.hud_compact = false;
            }
            HudPreset::Custom => {}
        }
    }

    /// True when any GPU sub-sensor requires MangoHud's `gpu_stats` master flag.
    pub const fn gpu_stats_locked(&self) -> bool {
        self.gpu_temp
            || self.gpu_junction_temp
            || self.gpu_mem_temp
            || self.gpu_core_clock
            || self.gpu_power
            || self.gpu_fan
    }

    /// Effective `gpu_stats` state emitted to MangoHud and reflected in the UI.
    pub const fn effective_gpu_stats(&self) -> bool {
        self.gpu_stats || self.gpu_stats_locked()
    }

    /// True when any CPU sub-sensor requires MangoHud's `cpu_stats` master flag.
    pub const fn cpu_stats_locked(&self) -> bool {
        self.cpu_temp || self.cpu_mhz || self.cpu_power
    }

    /// Effective `cpu_stats` state emitted to MangoHud and reflected in the UI.
    pub const fn effective_cpu_stats(&self) -> bool {
        self.cpu_stats || self.cpu_stats_locked()
    }

    /// Whether the profile qualifies for MangoHud's `fps_only=1` minimal mode.
    pub const fn is_minimal_fps_only(&self) -> bool {
        matches!(self.preset, HudPreset::MinimalFps)
            && self.fps
            && !self.effective_gpu_stats()
            && !self.vram
            && !self.effective_cpu_stats()
            && !self.ram
            && !self.frametime
            && !self.frame_timing
            && !self.present_mode
            && !self.wine
            && !self.winesync
            && !self.fsr
            && !self.hdr
            && !self.gamemode
            && !self.throttling_status
            && !self.gamepad_battery
    }

    /// Transition the preset indicator to [`HudPreset::Custom`] after a manual edit.
    pub fn mark_custom(&mut self) {
        self.preset = HudPreset::Custom;
    }
}

/// Corner alignment for Apple's Metal Performance HUD (`MTL_HUD_ALIGNMENT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetalHudAlignment {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl MetalHudAlignment {
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// Exact hyphen-free value expected by `MTL_HUD_ALIGNMENT`.
    pub const fn as_env_value(self) -> &'static str {
        match self {
            Self::TopLeft => "topleft",
            Self::TopRight => "topright",
            Self::BottomLeft => "bottomleft",
            Self::BottomRight => "bottomright",
        }
    }
}

/// Configuration for Apple's Metal Performance HUD on macOS.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MetalHudSettings {
    pub alignment: MetalHudAlignment,
}

/// Global HUD settings stored in [`crate::AppSettings`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GlobalHudSettings {
    pub mode: GlobalHudMode,
    pub mangohud: MangoHudProfile,
    pub metal: MetalHudSettings,
    /// Last theme palette extracted on the UI thread so headless/CLI launches
    /// still honor `SyncIroriTheme` without depending on GPUI.
    pub cached_theme_colors: Option<ThemeHudColors>,
}

/// Per-game HUD override stored in [`crate::GameSettings`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameHudSettings {
    pub mode: GameHudMode,
    pub custom_mangohud: Option<MangoHudProfile>,
    pub custom_metal: Option<MetalHudSettings>,
}

impl GameHudSettings {
    /// Ensure custom profiles are initialized from `global` when switching to
    /// [`GameHudMode::Custom`].
    pub fn ensure_custom_from(&mut self, global: &GlobalHudSettings) {
        if self.custom_mangohud.is_none() {
            self.custom_mangohud = Some(global.mangohud.clone());
        }
        if self.custom_metal.is_none() {
            self.custom_metal = Some(global.metal.clone());
        }
    }

    /// Replace this game's custom HUD profiles with a fresh copy of `global`.
    pub fn restore_from_global(&mut self, global: &GlobalHudSettings) {
        self.custom_mangohud = Some(global.mangohud.clone());
        self.custom_metal = Some(global.metal.clone());
    }
}

/// Final resolved HUD state for a specific game launch.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedHud {
    /// Do not inject or remove any HUD variables.
    Off,
    /// Explicitly suppress HUD for this game (`DISABLE_MANGOHUD=1` / `MTL_HUD_ENABLED=0`).
    Disabled,
    /// Linux: enable MangoHud without a managed config file (uses user's system config).
    MangoHudSystemDefault {
        opengl_legacy_wrapper: bool,
        mangohud_dlsym: bool,
    },
    /// Linux: enable MangoHud with a specific Irori-managed profile.
    MangoHudProfile(Box<MangoHudProfile>),
    /// macOS: enable Metal Performance HUD with the selected corner alignment.
    MetalHud(MetalHudSettings),
}

/// Resolve the effective HUD behavior combining global settings, per-game
/// overrides, and backwards-compatible legacy toggles (`showMangohud` on Linux,
/// `showFps` on macOS).
pub fn resolve_effective_hud(
    global: &GlobalHudSettings,
    game: &GameHudSettings,
    legacy_show_mangohud: bool,
    legacy_show_fps: bool,
    backend: HudBackend,
) -> ResolvedHud {
    match backend {
        HudBackend::Unsupported => ResolvedHud::Off,
        HudBackend::MangoHud => match game.mode {
            GameHudMode::Disabled => ResolvedHud::Disabled,
            GameHudMode::SystemDefault => ResolvedHud::MangoHudSystemDefault {
                opengl_legacy_wrapper: game
                    .custom_mangohud
                    .as_ref()
                    .map_or(global.mangohud.opengl_legacy_wrapper, |p| {
                        p.opengl_legacy_wrapper
                    }),
                mangohud_dlsym: game
                    .custom_mangohud
                    .as_ref()
                    .map_or(global.mangohud.mangohud_dlsym, |p| p.mangohud_dlsym),
            },
            GameHudMode::Custom => ResolvedHud::MangoHudProfile(Box::new(
                game.custom_mangohud
                    .clone()
                    .unwrap_or_else(|| global.mangohud.clone()),
            )),
            GameHudMode::InheritGlobal => match global.mode {
                GlobalHudMode::IroriProfile => {
                    ResolvedHud::MangoHudProfile(Box::new(global.mangohud.clone()))
                }
                GlobalHudMode::SystemDefault => ResolvedHud::MangoHudSystemDefault {
                    opengl_legacy_wrapper: global.mangohud.opengl_legacy_wrapper,
                    mangohud_dlsym: global.mangohud.mangohud_dlsym,
                },
                GlobalHudMode::Off => {
                    if legacy_show_mangohud {
                        ResolvedHud::MangoHudSystemDefault {
                            opengl_legacy_wrapper: false,
                            mangohud_dlsym: true,
                        }
                    } else {
                        ResolvedHud::Off
                    }
                }
            },
        },
        HudBackend::MetalHud => match game.mode {
            GameHudMode::Disabled => ResolvedHud::Disabled,
            GameHudMode::SystemDefault => ResolvedHud::MetalHud(MetalHudSettings::default()),
            GameHudMode::Custom => ResolvedHud::MetalHud(
                game.custom_metal
                    .clone()
                    .unwrap_or_else(|| global.metal.clone()),
            ),
            GameHudMode::InheritGlobal => match global.mode {
                GlobalHudMode::IroriProfile => ResolvedHud::MetalHud(global.metal.clone()),
                GlobalHudMode::SystemDefault => ResolvedHud::MetalHud(MetalHudSettings::default()),
                GlobalHudMode::Off => {
                    if legacy_show_fps {
                        ResolvedHud::MetalHud(MetalHudSettings::default())
                    } else {
                        ResolvedHud::Off
                    }
                }
            },
        },
    }
}

/// Sanitize a game identifier for safe use as a flat filename under
/// `<irori_data_dir>/mangohud/`. Any character outside `[a-zA-Z0-9_-]` becomes `_`.
pub fn sanitize_game_id_for_filename(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "game".into()
    } else {
        sanitized
    }
}

/// Strip MangoHud config delimiters and control characters from custom GPU/CPU labels.
pub fn sanitize_hud_label(label: &str) -> String {
    label
        .chars()
        .filter(|&c| !matches!(c, '#' | '=' | ',' | ';' | '+' | '\r' | '\n'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Normalize a hex color string to 6 uppercase ASCII hex digits (`RRGGBB`),
/// dropping any leading `#` and any trailing alpha digits (`RRGGBBAA`).
pub fn sanitize_hex6(input: &str, fallback: &str) -> String {
    let stripped = input.trim().trim_start_matches('#');
    let hex_only: String = stripped
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if hex_only.len() == 6 {
        hex_only
    } else {
        fallback.trim_start_matches('#').to_ascii_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_game_id_replaces_colons_slashes_and_spaces() {
        assert_eq!(sanitize_game_id_for_filename("epic:123"), "epic_123");
        assert_eq!(sanitize_game_id_for_filename("gog/456"), "gog_456");
        assert_eq!(
            sanitize_game_id_for_filename("sideload:foo/bar baz"),
            "sideload_foo_bar_baz"
        );
        assert_eq!(sanitize_game_id_for_filename(""), "game");
    }

    #[test]
    fn sanitize_hud_label_strips_delimiters() {
        assert_eq!(
            sanitize_hud_label("RX 9070 XT #1=fast,ok;+yes\r\n"),
            "RX 9070 XT 1fastokyes"
        );
    }

    #[test]
    fn sanitize_hex6_strips_hash_and_alpha() {
        assert_eq!(sanitize_hex6("#2e9762ff", "FFFFFF"), "2E9762");
        assert_eq!(sanitize_hex6("bad", "123456"), "123456");
    }

    #[test]
    fn empty_json_deserializes_with_defaults() {
        let global: GlobalHudSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(global.mode, GlobalHudMode::Off);
        assert_eq!(global.mangohud.toggle_hud, DEFAULT_TOGGLE_HUD);
        assert_eq!(global.metal.alignment, MetalHudAlignment::TopRight);

        let game: GameHudSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(game.mode, GameHudMode::InheritGlobal);
        assert!(game.custom_mangohud.is_none());
        assert!(game.custom_metal.is_none());
    }

    #[test]
    fn custom_mode_clones_global_profile_when_none() {
        let mut global = GlobalHudSettings {
            mode: GlobalHudMode::IroriProfile,
            ..Default::default()
        };
        global.mangohud.fps_limit = Some(120);
        global.metal.alignment = MetalHudAlignment::BottomLeft;

        let mut game = GameHudSettings {
            mode: GameHudMode::Custom,
            ..Default::default()
        };
        game.ensure_custom_from(&global);
        assert_eq!(game.custom_mangohud.as_ref().unwrap().fps_limit, Some(120));
        assert_eq!(
            game.custom_metal.as_ref().unwrap().alignment,
            MetalHudAlignment::BottomLeft
        );
    }
}
