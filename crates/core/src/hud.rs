//! Native Performance HUD domain model (MangoHud on Linux, Metal HUD on macOS).
//!
//! All types in this module compile unconditionally on every target OS so
//! cross-platform validation (`cargo check`, `cargo test`) covers Linux, macOS
//! and Windows code paths together. Only [`HudBackend::current`] branches on
//! `target_os`.

use serde::{Deserialize, Serialize};

/// Default MangoHud toggle shortcut matching MangoHud's factory default.
pub const DEFAULT_TOGGLE_HUD: &str = "Shift_R+F12";

/// Default MangoHud preset cycling shortcut matching MangoHud's factory default.
pub const DEFAULT_TOGGLE_PRESET: &str = "Shift_R+F10";

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
    /// Older settings saved as `systemDefault` load as this mode.
    #[serde(alias = "systemDefault")]
    IroriProfile,
}

/// Per-game HUD override mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GameHudMode {
    /// Follow the global Performance HUD configuration.
    #[default]
    InheritGlobal,
    /// Use a custom profile stored specifically for this game.
    /// Older settings saved as `systemDefault` load as this mode.
    #[serde(alias = "systemDefault")]
    Custom,
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
    CustomHorizontal,
}

impl HudPreset {
    /// Built-in Irori presets in their canonical in-game cycle order.
    pub const BUILTIN: [Self; 4] = [
        Self::MinimalFps,
        Self::HorizontalCompact,
        Self::BalancedGaming,
        Self::FullDiagnostics,
    ];

    /// Stable numeric identifier written to MangoHud `presets.conf` (`[preset N]`).
    pub const fn mangohud_preset_id(self) -> u8 {
        match self {
            Self::MinimalFps => 1,
            Self::HorizontalCompact => 2,
            Self::BalancedGaming => 3,
            Self::FullDiagnostics => 4,
            Self::Custom => 5,
            Self::CustomHorizontal => 6,
        }
    }
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

/// Hardware vendor identified deterministically from Linux PCI vendor IDs
/// (`1002`, `10de`, `8086`) or x86 CPUID `vendor_id` (`GenuineIntel`,
/// `AuthenticAMD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HardwareVendor {
    Amd,
    Nvidia,
    Intel,
    #[default]
    Unknown,
}

impl HardwareVendor {
    /// Whether the vendor was positively identified.
    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Official brand-aligned 6-digit uppercase hex color legible on dark HUDs.
    pub const fn brand_hex(self, fallback: &'static str) -> &'static str {
        match self {
            Self::Amd => "ED1C24",
            Self::Nvidia => "76B900",
            Self::Intel => "0095FF",
            Self::Unknown => fallback,
        }
    }
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

    /// Hardware-aware default palette tinting GPU/VRAM and CPU labels by their
    /// detected silicon vendors (AMD red, NVIDIA green, Intel blue).
    pub fn for_hardware(gpu: HardwareVendor, cpu: HardwareVendor) -> Self {
        let gpu_hex = gpu.brand_hex("2E9762");
        let cpu_hex = cpu.brand_hex("2E97CB");
        Self {
            background_color: "000000".into(),
            text_color: "FFFFFF".into(),
            gpu_color: gpu_hex.into(),
            cpu_color: cpu_hex.into(),
            vram_color: gpu_hex.into(),
            ram_color: "00C23D".into(),
            engine_color: "FFFFFF".into(),
            frametime_color: "00FF00".into(),
            io_color: "A491D3".into(),
            battery_color: "00FF00".into(),
            accent_color: gpu_hex.into(),
        }
    }
}

/// Metric and layout toggles that presets overwrite, saved so switching back
/// to [`HudPreset::Custom`] restores the user's own selection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomPresetSnapshot {
    pub gpu_stats: bool,
    pub gpu_temp: bool,
    pub gpu_junction_temp: bool,
    pub gpu_mem_temp: bool,
    pub gpu_core_clock: bool,
    pub gpu_mem_clock: bool,
    pub gpu_power: bool,
    pub gpu_fan: bool,
    pub vram: bool,
    pub cpu_stats: bool,
    pub cpu_temp: bool,
    pub cpu_mhz: bool,
    pub cpu_power: bool,
    pub ram: bool,
    pub fps: bool,
    pub fps_avg: bool,
    pub fps_1_low: bool,
    pub frametime: bool,
    pub frame_timing: bool,
    pub present_mode: bool,
    pub wine: bool,
    pub winesync: bool,
    pub fsr: bool,
    pub hdr: bool,
    pub gamemode: bool,
    pub throttling_status: bool,
    pub gamepad_battery: bool,
    pub orientation: HudOrientation,
    pub hud_compact: bool,
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
    pub gpu_mem_clock: bool,
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
    pub fps_use_label: bool,
    pub fps_avg: bool,
    pub fps_1_low: bool,
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
    pub toggle_preset: String,
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

    /// Last manual selection made under [`HudPreset::Custom`] (vertical).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_custom: Option<CustomPresetSnapshot>,
    /// Last manual selection made under [`HudPreset::CustomHorizontal`] (horizontal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_custom_horizontal: Option<CustomPresetSnapshot>,
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
            gpu_mem_clock: false,
            gpu_power: false,
            gpu_fan: false,
            vram: false,
            cpu_stats: false,
            cpu_temp: false,
            cpu_mhz: false,
            cpu_power: false,
            ram: false,
            fps: true,
            fps_use_label: false,
            fps_avg: false,
            fps_1_low: false,
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
            toggle_preset: DEFAULT_TOGGLE_PRESET.into(),
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
            saved_custom: None,
            saved_custom_horizontal: None,
        };
        profile.apply_preset(preset);
        profile
    }

    /// Apply a preset's metric and layout toggles while preserving the user's
    /// hotkey, GPU selection, custom labels, palette and legacy wrapper choice.
    ///
    /// Leaving [`HudPreset::Custom`] or [`HudPreset::CustomHorizontal`] stores
    /// the current selection in its dedicated slot, and choosing it again
    /// restores that selection.
    pub fn apply_preset(&mut self, preset: HudPreset) {
        if self.preset == HudPreset::Custom && preset != HudPreset::Custom {
            let mut snapshot = self.custom_snapshot();
            snapshot.orientation = HudOrientation::Vertical;
            self.saved_custom = Some(snapshot);
        } else if self.preset == HudPreset::CustomHorizontal
            && preset != HudPreset::CustomHorizontal
        {
            let mut snapshot = self.custom_snapshot();
            snapshot.orientation = HudOrientation::Horizontal;
            self.saved_custom_horizontal = Some(snapshot);
        }

        if preset == HudPreset::Custom {
            self.preset = HudPreset::Custom;
            if let Some(mut snapshot) = self.saved_custom.clone() {
                snapshot.orientation = HudOrientation::Vertical;
                self.restore_custom_snapshot(&snapshot);
                self.saved_custom = Some(snapshot);
            } else {
                self.orientation = HudOrientation::Vertical;
                self.saved_custom = Some(self.custom_snapshot());
            }
            return;
        }

        if preset == HudPreset::CustomHorizontal {
            self.preset = HudPreset::CustomHorizontal;
            if let Some(mut snapshot) = self.saved_custom_horizontal.clone() {
                snapshot.orientation = HudOrientation::Horizontal;
                self.restore_custom_snapshot(&snapshot);
                self.saved_custom_horizontal = Some(snapshot);
            } else {
                self.reset_metric_toggles();
                self.apply_horizontal_compact_metrics();
                self.saved_custom_horizontal = Some(self.custom_snapshot());
            }
            return;
        }

        self.preset = preset;
        self.reset_metric_toggles();

        match preset {
            HudPreset::MinimalFps => {
                self.fps = true;
                self.orientation = HudOrientation::Vertical;
                self.hud_compact = true;
            }
            HudPreset::HorizontalCompact => {
                self.apply_horizontal_compact_metrics();
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
                self.gpu_mem_clock = true;
                self.gpu_power = true;
                self.gpu_fan = true;
                self.vram = true;
                self.cpu_stats = true;
                self.cpu_temp = true;
                self.cpu_mhz = true;
                self.cpu_power = true;
                self.ram = true;
                self.fps = true;
                self.fps_avg = true;
                self.fps_1_low = true;
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
            HudPreset::Custom | HudPreset::CustomHorizontal => {}
        }
    }

    fn reset_metric_toggles(&mut self) {
        self.gpu_stats = false;
        self.gpu_temp = false;
        self.gpu_junction_temp = false;
        self.gpu_mem_temp = false;
        self.gpu_core_clock = false;
        self.gpu_mem_clock = false;
        self.gpu_power = false;
        self.gpu_fan = false;
        self.vram = false;
        self.cpu_stats = false;
        self.cpu_temp = false;
        self.cpu_mhz = false;
        self.cpu_power = false;
        self.ram = false;
        self.fps = false;
        self.fps_avg = false;
        self.fps_1_low = false;
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
    }

    fn apply_horizontal_compact_metrics(&mut self) {
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

    /// True when any GPU sub-sensor requires MangoHud's `gpu_stats` master flag.
    pub const fn gpu_stats_locked(&self) -> bool {
        self.gpu_temp
            || self.gpu_junction_temp
            || self.gpu_mem_temp
            || self.gpu_core_clock
            || self.gpu_mem_clock
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
            && !self.fps_avg
            && !self.fps_1_low
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

    /// Transition the preset indicator to [`HudPreset::Custom`] (vertical) or
    /// [`HudPreset::CustomHorizontal`] (horizontal) after a manual edit,
    /// saving the updated selection to its dedicated slot.
    pub fn mark_custom(&mut self) {
        if self.orientation == HudOrientation::Horizontal {
            self.preset = HudPreset::CustomHorizontal;
            self.saved_custom_horizontal = Some(self.custom_snapshot());
        } else {
            self.preset = HudPreset::Custom;
            self.orientation = HudOrientation::Vertical;
            self.saved_custom = Some(self.custom_snapshot());
        }
    }

    /// Switch layout orientation via the Layout tab buttons, routing to
    /// [`HudPreset::Custom`] (vertical) or [`HudPreset::CustomHorizontal`]
    /// (horizontal).
    ///
    /// Saves the active custom slot before switching. If the target slot
    /// already has a saved snapshot, it is restored without being overwritten;
    /// otherwise the current metrics are copied into the target slot with the
    /// requested orientation.
    pub fn set_orientation(&mut self, orientation: HudOrientation) {
        if self.preset == HudPreset::Custom {
            let mut snapshot = self.custom_snapshot();
            snapshot.orientation = HudOrientation::Vertical;
            self.saved_custom = Some(snapshot);
        } else if self.preset == HudPreset::CustomHorizontal {
            let mut snapshot = self.custom_snapshot();
            snapshot.orientation = HudOrientation::Horizontal;
            self.saved_custom_horizontal = Some(snapshot);
        }

        match orientation {
            HudOrientation::Vertical => {
                self.preset = HudPreset::Custom;
                if let Some(mut snapshot) = self.saved_custom.clone() {
                    snapshot.orientation = HudOrientation::Vertical;
                    self.restore_custom_snapshot(&snapshot);
                    self.saved_custom = Some(snapshot);
                } else {
                    self.orientation = HudOrientation::Vertical;
                    self.saved_custom = Some(self.custom_snapshot());
                }
            }
            HudOrientation::Horizontal => {
                self.preset = HudPreset::CustomHorizontal;
                if let Some(mut snapshot) = self.saved_custom_horizontal.clone() {
                    snapshot.orientation = HudOrientation::Horizontal;
                    self.restore_custom_snapshot(&snapshot);
                    self.saved_custom_horizontal = Some(snapshot);
                } else {
                    self.orientation = HudOrientation::Horizontal;
                    self.saved_custom_horizontal = Some(self.custom_snapshot());
                }
            }
        }
    }

    /// Hydrate this profile's custom preset slots from globally shared
    /// snapshots, also updating the live metric toggles when the active preset
    /// is [`HudPreset::Custom`] or [`HudPreset::CustomHorizontal`].
    pub fn hydrate_global_customs(
        &mut self,
        vertical: Option<&CustomPresetSnapshot>,
        horizontal: Option<&CustomPresetSnapshot>,
    ) {
        if let Some(snap) = vertical {
            let mut snap = snap.clone();
            snap.orientation = HudOrientation::Vertical;
            if self.preset == HudPreset::Custom {
                self.restore_custom_snapshot(&snap);
            }
            self.saved_custom = Some(snap);
        }
        if let Some(snap) = horizontal {
            let mut snap = snap.clone();
            snap.orientation = HudOrientation::Horizontal;
            if self.preset == HudPreset::CustomHorizontal {
                self.restore_custom_snapshot(&snap);
            }
            self.saved_custom_horizontal = Some(snap);
        }
    }

    /// Whether this profile has a distinct vertical custom selection to include
    /// in MangoHud's in-game preset cycle (`[preset 5]`).
    pub const fn has_custom_preset(&self) -> bool {
        matches!(self.preset, HudPreset::Custom) || self.saved_custom.is_some()
    }

    /// Whether this profile has a distinct horizontal custom selection to
    /// include in MangoHud's in-game preset cycle (`[preset 6]`).
    pub const fn has_custom_horizontal_preset(&self) -> bool {
        matches!(self.preset, HudPreset::CustomHorizontal) || self.saved_custom_horizontal.is_some()
    }

    /// Ordered preset cycle starting with `self.preset` so MangoHud launches
    /// directly into the active preset and cycles cleanly through Irori's own
    /// presets on `toggle_preset`.
    pub fn preset_cycle(&self) -> Vec<HudPreset> {
        let mut cycle = Vec::with_capacity(6);
        cycle.extend_from_slice(&HudPreset::BUILTIN);
        if self.has_custom_preset() {
            cycle.push(HudPreset::Custom);
        }
        if self.has_custom_horizontal_preset() {
            cycle.push(HudPreset::CustomHorizontal);
        }
        if let Some(pos) = cycle.iter().position(|&p| p == self.preset) {
            cycle.rotate_left(pos);
        }
        cycle
    }

    /// Capture the toggles that presets overwrite.
    pub fn custom_snapshot(&self) -> CustomPresetSnapshot {
        CustomPresetSnapshot {
            gpu_stats: self.gpu_stats,
            gpu_temp: self.gpu_temp,
            gpu_junction_temp: self.gpu_junction_temp,
            gpu_mem_temp: self.gpu_mem_temp,
            gpu_core_clock: self.gpu_core_clock,
            gpu_mem_clock: self.gpu_mem_clock,
            gpu_power: self.gpu_power,
            gpu_fan: self.gpu_fan,
            vram: self.vram,
            cpu_stats: self.cpu_stats,
            cpu_temp: self.cpu_temp,
            cpu_mhz: self.cpu_mhz,
            cpu_power: self.cpu_power,
            ram: self.ram,
            fps: self.fps,
            fps_avg: self.fps_avg,
            fps_1_low: self.fps_1_low,
            frametime: self.frametime,
            frame_timing: self.frame_timing,
            present_mode: self.present_mode,
            wine: self.wine,
            winesync: self.winesync,
            fsr: self.fsr,
            hdr: self.hdr,
            gamemode: self.gamemode,
            throttling_status: self.throttling_status,
            gamepad_battery: self.gamepad_battery,
            orientation: self.orientation,
            hud_compact: self.hud_compact,
        }
    }

    /// Write a previously captured selection back into this profile.
    pub fn restore_custom_snapshot(&mut self, snapshot: &CustomPresetSnapshot) {
        self.gpu_stats = snapshot.gpu_stats;
        self.gpu_temp = snapshot.gpu_temp;
        self.gpu_junction_temp = snapshot.gpu_junction_temp;
        self.gpu_mem_temp = snapshot.gpu_mem_temp;
        self.gpu_core_clock = snapshot.gpu_core_clock;
        self.gpu_mem_clock = snapshot.gpu_mem_clock;
        self.gpu_power = snapshot.gpu_power;
        self.gpu_fan = snapshot.gpu_fan;
        self.vram = snapshot.vram;
        self.cpu_stats = snapshot.cpu_stats;
        self.cpu_temp = snapshot.cpu_temp;
        self.cpu_mhz = snapshot.cpu_mhz;
        self.cpu_power = snapshot.cpu_power;
        self.ram = snapshot.ram;
        self.fps = snapshot.fps;
        self.fps_avg = snapshot.fps_avg;
        self.fps_1_low = snapshot.fps_1_low;
        self.frametime = snapshot.frametime;
        self.frame_timing = snapshot.frame_timing;
        self.present_mode = snapshot.present_mode;
        self.wine = snapshot.wine;
        self.winesync = snapshot.winesync;
        self.fsr = snapshot.fsr;
        self.hdr = snapshot.hdr;
        self.gamemode = snapshot.gamemode;
        self.throttling_status = snapshot.throttling_status;
        self.gamepad_battery = snapshot.gamepad_battery;
        self.orientation = snapshot.orientation;
        self.hud_compact = snapshot.hud_compact;
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
    /// Globally shared vertical custom preset (`[preset 5]`), available across
    /// global and per-game HUD profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_preset: Option<CustomPresetSnapshot>,
    /// Globally shared horizontal custom preset (`[preset 6]`), available
    /// across global and per-game HUD profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_horizontal_preset: Option<CustomPresetSnapshot>,
}

impl GlobalHudSettings {
    /// Shared vertical custom preset snapshot, falling back to legacy
    /// `mangohud.saved_custom` when `custom_preset` has not been written yet.
    pub fn effective_custom_preset(&self) -> Option<CustomPresetSnapshot> {
        self.custom_preset
            .clone()
            .or_else(|| self.mangohud.saved_custom.clone())
            .or_else(|| {
                (self.mangohud.preset == HudPreset::Custom).then(|| self.mangohud.custom_snapshot())
            })
            .map(|mut snapshot| {
                snapshot.orientation = HudOrientation::Vertical;
                snapshot
            })
    }

    /// Shared horizontal custom preset snapshot, falling back to
    /// `mangohud.saved_custom_horizontal` when `custom_horizontal_preset` has
    /// not been written yet.
    pub fn effective_custom_horizontal_preset(&self) -> Option<CustomPresetSnapshot> {
        self.custom_horizontal_preset
            .clone()
            .or_else(|| self.mangohud.saved_custom_horizontal.clone())
            .or_else(|| {
                (self.mangohud.preset == HudPreset::CustomHorizontal)
                    .then(|| self.mangohud.custom_snapshot())
            })
            .map(|mut snapshot| {
                snapshot.orientation = HudOrientation::Horizontal;
                snapshot
            })
    }

    /// Hydrate `profile` with the latest shared vertical and horizontal custom
    /// snapshots from global settings.
    pub fn hydrate_profile(&self, profile: &mut MangoHudProfile) {
        let vertical = self.effective_custom_preset();
        let horizontal = self.effective_custom_horizontal_preset();
        profile.hydrate_global_customs(vertical.as_ref(), horizontal.as_ref());
    }
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
            let mut profile = global.mangohud.clone();
            global.hydrate_profile(&mut profile);
            self.custom_mangohud = Some(profile);
        } else if let Some(profile) = self.custom_mangohud.as_mut() {
            global.hydrate_profile(profile);
        }
        if self.custom_metal.is_none() {
            self.custom_metal = Some(global.metal.clone());
        }
    }

    /// Replace this game's custom HUD profiles with a fresh copy of `global`.
    pub fn restore_from_global(&mut self, global: &GlobalHudSettings) {
        let mut profile = global.mangohud.clone();
        global.hydrate_profile(&mut profile);
        self.custom_mangohud = Some(profile);
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
            GameHudMode::Custom => {
                let mut profile = game
                    .custom_mangohud
                    .clone()
                    .unwrap_or_else(|| global.mangohud.clone());
                global.hydrate_profile(&mut profile);
                ResolvedHud::MangoHudProfile(Box::new(profile))
            }
            GameHudMode::InheritGlobal => match global.mode {
                GlobalHudMode::IroriProfile => {
                    let mut profile = global.mangohud.clone();
                    global.hydrate_profile(&mut profile);
                    ResolvedHud::MangoHudProfile(Box::new(profile))
                }
                // The legacy `showMangohud` toggle counts as "On".
                GlobalHudMode::Off if legacy_show_mangohud => {
                    let mut profile = global.mangohud.clone();
                    global.hydrate_profile(&mut profile);
                    ResolvedHud::MangoHudProfile(Box::new(profile))
                }
                GlobalHudMode::Off => ResolvedHud::Off,
            },
        },
        HudBackend::MetalHud => match game.mode {
            GameHudMode::Disabled => ResolvedHud::Disabled,
            GameHudMode::Custom => ResolvedHud::MetalHud(
                game.custom_metal
                    .clone()
                    .unwrap_or_else(|| global.metal.clone()),
            ),
            GameHudMode::InheritGlobal => match global.mode {
                GlobalHudMode::IroriProfile => ResolvedHud::MetalHud(global.metal.clone()),
                // The legacy `showFps` toggle counts as "On".
                GlobalHudMode::Off if legacy_show_fps => {
                    ResolvedHud::MetalHud(global.metal.clone())
                }
                GlobalHudMode::Off => ResolvedHud::Off,
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
        assert_eq!(global.mangohud.toggle_preset, DEFAULT_TOGGLE_PRESET);
        assert_eq!(global.metal.alignment, MetalHudAlignment::TopRight);

        let game: GameHudSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(game.mode, GameHudMode::InheritGlobal);
        assert!(game.custom_mangohud.is_none());
        assert!(game.custom_metal.is_none());
    }

    #[test]
    fn preset_cycle_rotates_active_preset_first_and_tracks_custom() {
        let mut profile = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        profile.toggle_preset = "Shift_L+F9".into();
        assert!(!profile.has_custom_preset());
        assert_eq!(
            profile.preset_cycle(),
            vec![
                HudPreset::BalancedGaming,
                HudPreset::FullDiagnostics,
                HudPreset::MinimalFps,
                HudPreset::HorizontalCompact,
            ]
        );

        profile.gpu_fan = true;
        profile.mark_custom();
        assert!(profile.has_custom_preset());
        assert_eq!(
            profile.preset_cycle(),
            vec![
                HudPreset::Custom,
                HudPreset::MinimalFps,
                HudPreset::HorizontalCompact,
                HudPreset::BalancedGaming,
                HudPreset::FullDiagnostics,
            ]
        );

        // Switching back to a built-in preset keeps toggle_preset and saved_custom in the cycle.
        profile.apply_preset(HudPreset::HorizontalCompact);
        assert_eq!(profile.toggle_preset, "Shift_L+F9");
        assert!(profile.has_custom_preset());
        assert_eq!(
            profile.preset_cycle(),
            vec![
                HudPreset::HorizontalCompact,
                HudPreset::BalancedGaming,
                HudPreset::FullDiagnostics,
                HudPreset::Custom,
                HudPreset::MinimalFps,
            ]
        );
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

    #[test]
    fn legacy_system_default_values_load_as_on_and_custom() {
        let global: GlobalHudSettings =
            serde_json::from_str(r#"{"mode":"systemDefault"}"#).unwrap();
        assert_eq!(global.mode, GlobalHudMode::IroriProfile);
        let game: GameHudSettings = serde_json::from_str(r#"{"mode":"systemDefault"}"#).unwrap();
        assert_eq!(game.mode, GameHudMode::Custom);
    }

    #[test]
    fn legacy_toggle_with_global_off_resolves_to_irori_profile() {
        let global = GlobalHudSettings::default();
        let game = GameHudSettings::default();
        let resolved = resolve_effective_hud(&global, &game, true, false, HudBackend::MangoHud);
        assert_eq!(
            resolved,
            ResolvedHud::MangoHudProfile(Box::new(global.mangohud.clone()))
        );
        let resolved = resolve_effective_hud(&global, &game, false, false, HudBackend::MangoHud);
        assert_eq!(resolved, ResolvedHud::Off);
        let resolved = resolve_effective_hud(&global, &game, false, true, HudBackend::MetalHud);
        assert_eq!(resolved, ResolvedHud::MetalHud(global.metal.clone()));
    }

    #[test]
    fn custom_selection_survives_preset_switch_and_json_round_trip() {
        let mut profile = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        profile.gpu_fan = true;
        profile.ram = false;
        profile.hud_compact = true;
        profile.mark_custom();
        assert_eq!(profile.preset, HudPreset::Custom);
        assert_eq!(profile.orientation, HudOrientation::Vertical);
        let expected_vertical = profile.custom_snapshot();

        // Editing from a horizontal preset targets CustomHorizontal independently.
        profile.apply_preset(HudPreset::HorizontalCompact);
        profile.gpu_power = true;
        profile.mark_custom();
        assert_eq!(profile.preset, HudPreset::CustomHorizontal);
        assert_eq!(profile.orientation, HudOrientation::Horizontal);
        let expected_horizontal = profile.custom_snapshot();

        profile.apply_preset(HudPreset::BalancedGaming);
        assert!(!profile.gpu_fan);
        assert!(profile.ram);
        assert_eq!(profile.orientation, HudOrientation::Vertical);

        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("savedCustom"));
        assert!(json.contains("savedCustomHorizontal"));
        let mut restored: MangoHudProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, profile);

        restored.apply_preset(HudPreset::Custom);
        assert_eq!(restored.preset, HudPreset::Custom);
        assert_eq!(restored.custom_snapshot(), expected_vertical);
        assert!(restored.gpu_fan);
        assert!(!restored.ram);
        assert_eq!(restored.orientation, HudOrientation::Vertical);
        assert!(restored.hud_compact);

        restored.apply_preset(HudPreset::CustomHorizontal);
        assert_eq!(restored.preset, HudPreset::CustomHorizontal);
        assert_eq!(restored.custom_snapshot(), expected_horizontal);
        assert!(restored.gpu_power);
        assert_eq!(restored.orientation, HudOrientation::Horizontal);
    }

    #[test]
    fn set_orientation_switches_between_custom_slots_without_clobbering() {
        let mut profile = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        profile.gpu_fan = true;
        profile.mark_custom();
        assert_eq!(profile.preset, HudPreset::Custom);
        assert!(profile.saved_custom_horizontal.is_none());

        // Switching to Horizontal when CustomHorizontal is empty copies current metrics.
        profile.set_orientation(HudOrientation::Horizontal);
        assert_eq!(profile.preset, HudPreset::CustomHorizontal);
        assert_eq!(profile.orientation, HudOrientation::Horizontal);
        assert!(profile.gpu_fan);

        // Customize horizontal metrics differently.
        profile.gpu_fan = false;
        profile.cpu_power = true;
        profile.mark_custom();

        // Switching back to Vertical restores Custom without losing CustomHorizontal.
        profile.set_orientation(HudOrientation::Vertical);
        assert_eq!(profile.preset, HudPreset::Custom);
        assert_eq!(profile.orientation, HudOrientation::Vertical);
        assert!(profile.gpu_fan);
        assert!(!profile.cpu_power);

        // Switching to Horizontal restores the saved CustomHorizontal snapshot intact.
        profile.set_orientation(HudOrientation::Horizontal);
        assert_eq!(profile.preset, HudPreset::CustomHorizontal);
        assert_eq!(profile.orientation, HudOrientation::Horizontal);
        assert!(!profile.gpu_fan);
        assert!(profile.cpu_power);
    }

    #[test]
    fn preset_cycle_includes_both_custom_slots_and_rotates_active() {
        let mut profile = MangoHudProfile::from_preset(HudPreset::HorizontalCompact);
        profile.gpu_power = true;
        profile.mark_custom();
        assert!(!profile.has_custom_preset());
        assert!(profile.has_custom_horizontal_preset());
        assert_eq!(
            profile.preset_cycle(),
            vec![
                HudPreset::CustomHorizontal,
                HudPreset::MinimalFps,
                HudPreset::HorizontalCompact,
                HudPreset::BalancedGaming,
                HudPreset::FullDiagnostics,
            ]
        );

        // Also define vertical Custom so both 5 and 6 enter the cycle.
        profile.apply_preset(HudPreset::BalancedGaming);
        profile.gpu_fan = true;
        profile.mark_custom();
        assert!(profile.has_custom_preset());
        assert!(profile.has_custom_horizontal_preset());
        assert_eq!(
            profile.preset_cycle(),
            vec![
                HudPreset::Custom,
                HudPreset::CustomHorizontal,
                HudPreset::MinimalFps,
                HudPreset::HorizontalCompact,
                HudPreset::BalancedGaming,
                HudPreset::FullDiagnostics,
            ]
        );
    }

    #[test]
    fn global_custom_slots_propagate_to_per_game_custom_and_fallback_legacy_saved_custom() {
        let mut global = GlobalHudSettings {
            mode: GlobalHudMode::IroriProfile,
            ..Default::default()
        };
        let mut vert = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        vert.gpu_fan = true;
        vert.mark_custom();
        let mut horiz = MangoHudProfile::from_preset(HudPreset::HorizontalCompact);
        horiz.cpu_power = true;
        horiz.mark_custom();
        global.custom_preset = vert.saved_custom.clone();
        global.custom_horizontal_preset = horiz.saved_custom_horizontal.clone();

        let mut game_profile = MangoHudProfile::from_preset(HudPreset::CustomHorizontal);
        game_profile.fps_limit = Some(90);
        game_profile.saved_custom = None;
        game_profile.saved_custom_horizontal = None;
        let game = GameHudSettings {
            mode: GameHudMode::Custom,
            custom_mangohud: Some(game_profile),
            custom_metal: None,
        };

        let ResolvedHud::MangoHudProfile(resolved) =
            resolve_effective_hud(&global, &game, false, false, HudBackend::MangoHud)
        else {
            panic!("expected MangoHudProfile");
        };
        assert_eq!(resolved.fps_limit, Some(90));
        assert_eq!(resolved.preset, HudPreset::CustomHorizontal);
        assert!(resolved.cpu_power);
        assert!(resolved.has_custom_preset());
        assert!(resolved.has_custom_horizontal_preset());
        assert!(resolved.saved_custom.as_ref().unwrap().gpu_fan);

        // Legacy profile without GlobalHudSettings.custom_preset falls back to mangohud.saved_custom.
        let mut legacy_global = GlobalHudSettings {
            mode: GlobalHudMode::IroriProfile,
            ..Default::default()
        };
        legacy_global.mangohud.gpu_fan = true;
        legacy_global.mangohud.mark_custom();
        legacy_global
            .mangohud
            .apply_preset(HudPreset::BalancedGaming);
        assert!(legacy_global.custom_preset.is_none());
        assert!(legacy_global.effective_custom_preset().unwrap().gpu_fan);
    }

    #[test]
    fn profile_without_saved_custom_omits_the_field() {
        let profile = MangoHudProfile::default();
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("savedCustom"));
        assert!(!json.contains("savedCustomHorizontal"));
        let mut profile = profile;
        let before = profile.custom_snapshot();
        profile.apply_preset(HudPreset::Custom);
        assert_eq!(profile.custom_snapshot(), before);
    }
}
