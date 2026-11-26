use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::game::{CloudSaveLocation, LaunchOption, SavesPathFound};

#[cfg(doc)]
use super::GameSettingsOverrides;
use super::{DEFAULT_SHADER_CACHE_GIB, FpsLimiter, de_false_or_string, ser_false_or_string};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WineKind {
    #[default]
    Wine,
    Proton,
    Crossover,
    Toolkit,
}

/// A Wine or Proton build usable to run Windows games.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct WineInstallation {
    /// `bin/wine` for Wine builds, the `proton` script for Proton.
    pub bin: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: WineKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib32: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wineserver: Option<String>,
}

impl WineInstallation {
    pub fn is_proton(&self) -> bool {
        self.kind == WineKind::Proton
    }

    pub fn is_set(&self) -> bool {
        !self.bin.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wrapper {
    pub exe: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GamescopeSettings {
    /// The on/off switch. `None` in settings saved before it existed:
    /// gamescope then runs when one of its options asks for it, as it did.
    /// Irori-only.
    #[serde(rename = "iroriEnabled")]
    pub enabled: Option<bool>,
    pub enable_upscaling: bool,
    pub enable_limiter: bool,
    pub enable_force_grab_cursor: bool,
    /// "fullscreen" | "borderless" | "windowed"
    pub window_type: String,
    pub game_width: String,
    pub game_height: String,
    pub upscale_width: String,
    pub upscale_height: String,
    /// "fsr" | "nis" | "integer" | "stretch" | ""
    pub upscale_method: String,
    pub fps_limiter: String,
    pub fps_limiter_no_focus: String,
    pub additional_options: String,
}

impl Default for GamescopeSettings {
    fn default() -> Self {
        Self {
            enabled: None,
            enable_upscaling: false,
            enable_limiter: false,
            enable_force_grab_cursor: false,
            window_type: "fullscreen".into(),
            game_width: String::new(),
            game_height: String::new(),
            upscale_width: String::new(),
            upscale_height: String::new(),
            upscale_method: "fsr".into(),
            fps_limiter: String::new(),
            fps_limiter_no_focus: String::new(),
            additional_options: String::new(),
        }
    }
}

impl GamescopeSettings {
    /// Whether games run inside gamescope.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(
            self.enable_limiter
                || self.enable_upscaling
                || self.enable_force_grab_cursor
                || !self.additional_options.trim().is_empty(),
        )
    }

    /// Whether the window type and resolutions apply. Without the switch
    /// they only ever applied together with upscaling.
    pub fn sets_window(&self) -> bool {
        self.enabled == Some(true) || self.enable_upscaling
    }
}

/// Settings that apply to launching a game. Global defaults and per-game
/// effective values share this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameSettings {
    pub auto_install_dxvk: bool,
    pub auto_install_vkd3d: bool,
    pub auto_install_dxvk_nvapi: bool,
    pub auto_sync_saves: bool,
    pub battl_eye_runtime: bool,
    #[serde(rename = "DXVKFpsCap")]
    pub dxvk_fps_cap: String,
    pub eac_runtime: bool,
    #[serde(rename = "enableDXVKFpsLimit")]
    pub enable_dxvk_fps_limit: bool,
    pub enable_esync: bool,
    #[serde(rename = "enableFSR")]
    pub enable_fsr: bool,
    pub enable_msync: bool,
    pub enable_fsync: bool,
    /// NTSync, the kernel's Windows synchronization driver (`/dev/ntsync`),
    /// in Proton and Wine builds that use it. On by default as those builds
    /// have it; off sets `PROTON_NO_NTSYNC`. Linux only; Irori-only.
    #[serde(rename = "iroriNtsync")]
    pub ntsync: bool,
    pub enable_wine_wayland: bool,
    #[serde(rename = "enableHDR")]
    pub enable_hdr: bool,
    #[serde(rename = "enableWoW64")]
    pub enable_wow64: bool,
    /// Proton reads controllers through SDL instead of hidraw and evdev
    /// (`PROTON_PREFER_SDL`), for controllers it otherwise misreads or
    /// misses. Linux only; Irori-only.
    #[serde(rename = "iroriProtonPreferSdl")]
    pub proton_prefer_sdl: bool,
    pub gamescope: GamescopeSettings,
    #[serde(rename = "enviromentOptions")]
    pub environment_options: Vec<EnvVar>,
    /// Wine DLL overrides, added to `WINEDLLOVERRIDES` for games run
    /// through Wine or Proton (each replaces what the variable says about
    /// its DLL). Irori-only.
    #[serde(rename = "iroriDllOverrides")]
    pub dll_overrides: Vec<crate::DllOverride>,
    /// List this game among Sunshine's applications while Irori keeps its
    /// games there. Irori-only; set per game.
    #[serde(rename = "iroriInSunshine")]
    pub in_sunshine: bool,
    /// Show this game on Discord while it runs, when Rich Presence is on.
    /// Irori-only; set per game.
    #[serde(rename = "iroriDiscordPresence")]
    pub discord_presence: bool,
    /// Linux: when Steam starts the game through its non-Steam shortcut,
    /// pass Steam's overlay on to the game (off under gamescope and the
    /// sandbox). Irori-only; set per game.
    #[serde(rename = "iroriSteamOverlay")]
    pub steam_overlay: bool,
    pub ignore_game_updates: bool,
    pub language: String,
    pub launcher_args: String,
    pub last_used_launch_option: Option<LaunchOption>,
    pub max_sharpness: Option<u8>,
    pub nvidia_prime: bool,
    pub offline_mode: bool,
    pub prefer_system_libs: bool,
    pub show_fps: bool,
    pub show_mangohud: bool,
    /// MangoHud loads with the game without drawing, until the in-game
    /// overlay shows it. Linux only; Irori-only.
    #[serde(rename = "iroriMangohudHidden")]
    pub mangohud_hidden: bool,
    /// What applies the frame limit (`enableDXVKFpsLimit`, `DXVKFpsCap`):
    /// see `irori_runtime::frame_limit`. Linux only; Irori-only.
    #[serde(rename = "iroriFpsLimiter")]
    pub fps_limiter: FpsLimiter,
    pub target_exe: String,
    pub use_game_mode: bool,
    /// Steam runtime name (`"soldier"`, `"sniper"`, …) or none. Heroic writes
    /// `false` for none.
    #[serde(
        serialize_with = "ser_false_or_string",
        deserialize_with = "de_false_or_string"
    )]
    pub steam_runtime: Option<String>,
    pub wine_crossover_bottle: String,
    pub wine_prefix: String,
    pub wine_version: WineInstallation,
    pub wrapper_options: Vec<Wrapper>,
    pub saves_path: String,
    /// What Irori found `saves_path` for when it worked the folder out
    /// itself; `None` when the user chose it. Per game; Irori-only.
    #[serde(
        rename = "iroriSavesPathFound",
        skip_serializing_if = "Option::is_none"
    )]
    pub saves_path_found: Option<SavesPathFound>,
    pub gog_saves: Vec<CloudSaveLocation>,
    pub before_launch_script_path: String,
    /// Start the game without waiting for the before-launch script, which
    /// then runs alongside it and is stopped when the session ends (for a
    /// script that never exits, such as a server). Irori-only.
    #[serde(rename = "iroriBeforeLaunchScriptAlongside")]
    pub before_launch_script_alongside: bool,
    pub after_launch_script_path: String,
    #[serde(rename = "disableUMU")]
    pub disable_umu: bool,
    /// Start the game's program directly, without Wine or Proton, though
    /// its build is Windows': for a native replacement (OpenJK,
    /// fallout2-ce) set as its program that Irori can't recognise as one.
    /// Programs of this system run without Wine anyway. Per game;
    /// Irori-only.
    #[serde(rename = "iroriNoWine")]
    pub no_wine: bool,
    pub verbose_logs: bool,
    pub advertise_avx_for_rosetta: bool,
    pub enable_quick_saves_menu: bool,
    /// macOS: render at the display's full resolution (Wine's Mac driver
    /// `RetinaMode`). Irori-only.
    #[serde(rename = "iroriRetinaMode", alias = "emberRetinaMode")]
    pub retina_mode: bool,
    /// macOS on Apple silicon: start a native Mac game's Intel code under
    /// Rosetta even when the program also has Apple silicon code (a game
    /// whose arm64 build misbehaves). Intel-only programs always run under
    /// Rosetta. Irori-only.
    #[serde(rename = "iroriRosetta")]
    pub rosetta: bool,
    /// macOS: NVIDIA DLSS translated to MetalFX, where the Wine build ships
    /// the translation (Game Porting Toolkit 3's D3DMetal, CrossOver, DXMT).
    /// Irori-only.
    #[serde(rename = "iroriMetalFxDlss")]
    pub metal_fx_dlss: bool,
    /// The emulator that plays this game (`duckstation`,
    /// `retroarch:<core>`); empty for the system's default. Irori-only.
    #[serde(rename = "iroriEngine", alias = "emberEngine")]
    pub engine: String,
    /// The installation of that emulator this game is pinned to
    /// (`managed:<version>`, `flatpak`, `system`, …); empty for the
    /// emulator's default. Irori-only.
    #[serde(rename = "iroriEngineVersion", alias = "emberEngineVersion")]
    pub engine_version: String,
    /// Emulator option values by emulator id. Per game, only the options
    /// set for that game; [`GameSettingsOverrides::apply`] merges them over
    /// the defaults option by option. Irori-only.
    #[serde(rename = "iroriEngineOptions", alias = "emberEngineOptions")]
    pub engine_options: BTreeMap<String, Map<String, Value>>,
    /// Leave the emulator's own configuration alone instead of the settings
    /// Irori manages. Irori-only.
    #[serde(rename = "iroriEngineOwnSettings", alias = "emberEngineOwnSettings")]
    pub engine_own_settings: bool,
    /// DXVK release tag for plain Wine builds; empty for the newest.
    /// Irori-only.
    #[serde(rename = "iroriDxvkVersion")]
    pub dxvk_version: String,
    /// VKD3D-Proton release tag for plain Wine builds; empty for the newest.
    /// Irori-only.
    #[serde(rename = "iroriVkd3dVersion")]
    pub vkd3d_version: String,
    /// Turn on VKD3D-Proton's descriptor heap. Irori-only.
    #[serde(rename = "iroriDescriptorHeap")]
    pub descriptor_heap: bool,
    /// Room for the GPU drivers' shader caches, in GiB: NVIDIA's
    /// (`__GL_SHADER_DISK_CACHE_SIZE`) and Mesa's
    /// (`MESA_SHADER_CACHE_MAX_SIZE`). Their own limits are small enough that
    /// large games compile their shaders again at every start. 0 leaves the
    /// drivers' limits. Linux only; Irori-only.
    #[serde(rename = "iroriShaderCacheSize")]
    pub shader_cache_size: u32,
    /// `PROTON_DLSS_UPGRADE`. Irori-only.
    #[serde(rename = "iroriDlssUpgrade")]
    pub dlss_upgrade: bool,
    /// `PROTON_DLSS_INDICATOR`. Irori-only.
    #[serde(rename = "iroriDlssIndicator")]
    pub dlss_indicator: bool,
    /// `PROTON_FSR4_UPGRADE`. Irori-only.
    #[serde(rename = "iroriFsr4Upgrade")]
    pub fsr4_upgrade: bool,
    /// `PROTON_XESS_UPGRADE`. Irori-only.
    #[serde(rename = "iroriXessUpgrade")]
    pub xess_upgrade: bool,
    /// PCI address of the graphics card the game renders on
    /// (`0000:03:00.0`); empty leaves the choice to the system. Takes
    /// precedence over `nvidiaPrime`. Linux only; Irori-only.
    #[serde(rename = "iroriGpu")]
    pub gpu: String,
    /// Windows' own graphics preference for the game's program, written to
    /// `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`: `auto` (let
    /// Windows decide), `powerSaving` or `highPerformance`; empty (or a value
    /// this Irori doesn't know) leaves Windows' setting alone. Windows only;
    /// Irori-only.
    #[serde(rename = "iroriWindowsGpu")]
    pub windows_gpu: String,
    /// PnP instance id (`PCI\VEN_…&DEV_…\…`) of a graphics card Windows
    /// hides from the game while it runs: disabled for the session through
    /// Windows' own permission prompt and enabled again when it ends, never
    /// one driving a display. Empty hides nothing. Windows only; Irori-only.
    #[serde(rename = "iroriHideGpu")]
    pub hide_gpu: String,
    /// NVIDIA Smooth Motion frame generation
    /// (`NVPRESENT_ENABLE_SMOOTH_MOTION`). Linux only; Irori-only.
    #[serde(rename = "iroriSmoothMotion")]
    pub smooth_motion: bool,
    /// NVIDIA Reflex in Vulkan games run through Wine or Proton, through
    /// DXVK-NVAPI's Reflex layer (`DXVK_NVAPI_VKREFLEX`). Linux only;
    /// Irori-only.
    #[serde(rename = "iroriNvidiaReflex")]
    pub nvidia_reflex: bool,
    /// Lossless Scaling frame generation through the lsfg-vk Vulkan layer.
    /// Linux only; Irori-only.
    #[serde(rename = "iroriLsfg")]
    pub lsfg: LsfgSettings,
    /// The Lutris install script that installed this game, so it can be
    /// reinstalled or updated later. Per game only. Irori-only.
    #[serde(
        rename = "iroriLutrisScript",
        alias = "emberLutrisScript",
        skip_serializing_if = "Option::is_none"
    )]
    pub lutris_script: Option<LutrisScriptRecord>,
    /// What the game is kept from when it runs in a bubblewrap sandbox.
    /// Linux only; Irori-only.
    #[serde(rename = "iroriSandbox")]
    pub sandbox: SandboxSettings,
    /// Programs started with the game (trainers, overlays, voice chat).
    /// Irori-only.
    #[serde(rename = "iroriLaunchPrograms")]
    pub launch_programs: Vec<crate::launch_extras::LaunchProgram>,
    /// Actions beside the game's Play button: programs and links. Per game;
    /// Irori-only.
    #[serde(rename = "iroriActions")]
    pub actions: Vec<crate::user_actions::UserAction>,
    /// Back up the game's saves with Ludusavi after every session.
    /// Irori-only.
    #[serde(rename = "iroriLudusaviAutoBackup")]
    pub ludusavi_auto_backup: bool,
    /// Also back up the game's saves with Ludusavi every this many minutes
    /// while it runs; 0 (the default) for never. Irori-only.
    #[serde(rename = "iroriLudusaviInterval")]
    pub ludusavi_interval: u32,
    /// The game's title in Ludusavi, when the user corrected the automatic
    /// match; empty for the automatic one. Per game only. Irori-only.
    #[serde(rename = "iroriLudusaviTitle")]
    pub ludusavi_title: String,
    /// The game's HowLongToBeat id, when the user chose its match ("Change
    /// match"), or `none` for no match; empty finds one. Per game only.
    /// Irori-only.
    #[serde(rename = "iroriHltbId")]
    pub hltb_id: String,
    /// The game's IGDB id, when the user chose its match ("Change match"),
    /// or `none` for no match; empty finds one by title. Per game only.
    /// Irori-only.
    #[serde(rename = "iroriIgdbId")]
    pub igdb_id: String,
    /// The game's VNDB id (`v17`), when the user chose its match ("Change
    /// VNDB match"), or `none` for no match; empty finds one by title. Per
    /// game only. Irori-only.
    #[serde(rename = "iroriVndbId")]
    pub vndb_id: String,
    /// The Steam app the game is linked to, when the user chose one, or
    /// `none` for none: Steam's tags, store details, ProtonDB and the
    /// achievements read from game files follow it, and umu names the game
    /// by it. Empty finds it (wikis, the exact title; the game's files for
    /// achievements). It never changes how the game itself starts. Per game
    /// only. Irori-only.
    #[serde(rename = "iroriSteamAppId")]
    pub steam_app_id: String,
    /// The umu database id (`GAMEID`) the game runs with under umu, when the
    /// user chose one (`umu-1091500`, or a bare Steam app id); empty finds
    /// it from the store, the Steam app id or the title. Per game only.
    /// Irori-only.
    #[serde(rename = "iroriUmuId")]
    pub umu_id: String,
    /// Keep the last moments of play ready to save as a clip. Linux only;
    /// Irori-only.
    #[serde(rename = "iroriReplay")]
    pub replay: ReplaySettings,
    /// Per-game Performance HUD override (MangoHud on Linux, Metal HUD on
    /// macOS). Inherits the global HUD profile by default. Irori-only.
    #[serde(rename = "iroriGameHud", default)]
    pub game_hud: crate::hud::GameHudSettings,
    /// Global defaults inherited by OptiScaler installations and per-game
    /// configuration. Irori-only.
    #[serde(rename = "iroriOptiscaler", default)]
    pub optiscaler: super::OptiscalerDefaults,
}

/// The sandbox a game runs in (`iroriSandbox`). Off when nothing is blocked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SandboxSettings {
    /// Start the game in a network namespace of its own: no internet, no
    /// local network, no programs listening on this computer.
    pub no_network: bool,
    /// Hide the user's home folder and other drives: the game sees its own
    /// folders and a home folder of its own.
    pub no_home: bool,
}

impl SandboxSettings {
    /// Whether the game runs sandboxed at all.
    pub fn is_enabled(&self) -> bool {
        self.no_network || self.no_home
    }
}

/// Lossless Scaling frame generation of one game (`iroriLsfg`), handed to
/// lsfg-vk through its environment variables so its own configuration file
/// stays the user's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LsfgSettings {
    pub enabled: bool,
    /// Frames shown per rendered frame (2 doubles the frame rate).
    pub multiplier: u8,
    /// Resolution the motion is estimated at, in percent (25 to 100); lower
    /// is faster and less accurate.
    pub flow_scale: u8,
    /// lsfg-vk's lighter model.
    pub performance_mode: bool,
    /// The Vulkan layer that generates the frames: [`LAYER_LSFG_VK`] or
    /// [`LAYER_MAKO`] (MAKO Renderer, lsfg-vk's descendant). Settings
    /// written before the choice existed lack the key and keep lsfg-vk.
    pub layer: String,
}

/// [`LsfgSettings::layer`] for lsfg-vk.
pub const LAYER_LSFG_VK: &str = "lsfg-vk";
/// [`LsfgSettings::layer`] for MAKO Renderer.
pub const LAYER_MAKO: &str = "mako";

/// The frame generation layer [`LsfgSettings::layer`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameGenLayer {
    LsfgVk,
    Mako,
}

impl LsfgSettings {
    /// The chosen layer. A value this Irori doesn't know (a newer one's)
    /// is no layer at all, so the game starts without frame generation
    /// rather than with a layer the user didn't pick.
    pub fn chosen_layer(&self) -> Option<FrameGenLayer> {
        match self.layer.trim() {
            "" | LAYER_LSFG_VK => Some(FrameGenLayer::LsfgVk),
            LAYER_MAKO => Some(FrameGenLayer::Mako),
            _ => None,
        }
    }

    /// The layer that runs for this game, when frame generation is on.
    pub fn active_layer(&self) -> Option<FrameGenLayer> {
        self.enabled.then(|| self.chosen_layer()).flatten()
    }
}

impl Default for LsfgSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            multiplier: 2,
            flow_scale: 100,
            performance_mode: false,
            layer: LAYER_LSFG_VK.into(),
        }
    }
}

/// The replay buffer (`iroriReplay`): gpu-screen-recorder keeps the last
/// `seconds` of a game Irori started, and a clip is saved on request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplaySettings {
    pub enabled: bool,
    /// How much play a clip holds.
    pub seconds: u32,
    /// gpu-screen-recorder's quality preset: `medium`, `high`, `very_high`
    /// or `ultra`.
    pub quality: String,
    /// `desktop` (what the speakers play), `desktop_and_mic`, `game` (only
    /// the game's own sound, through PipeWire) or `none`.
    pub audio: String,
    /// `screen` (a monitor: [`ReplaySettings::monitor`], else the first) or
    /// `portal` (the desktop asks once what to record).
    pub capture: String,
    /// Where clips go, in a folder per game; empty for Videos/Irori.
    pub folder: String,
    /// Frames a second the clip records.
    pub fps: u32,
    /// The video codec: `auto` (the recorder picks what the GPU encodes),
    /// `h264`, `hevc` or `av1`.
    pub codec: String,
    /// The largest size a clip is recorded at (`1920x1080`), scaled down
    /// from the screen; `native` keeps the screen's.
    pub resolution: String,
    /// The monitor recorded with `screen` capture, by connector name
    /// (`DP-1`); empty for the first one.
    pub monitor: String,
    /// Start the recorder again when it stops by itself during the session.
    pub restart: bool,
    /// Link each saved clip in that day's journal note of the game.
    pub journal: bool,
}

impl Default for ReplaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            seconds: 60,
            quality: "very_high".into(),
            audio: "desktop".into(),
            capture: "screen".into(),
            folder: String::new(),
            fps: 60,
            codec: "auto".into(),
            resolution: "native".into(),
            monitor: String::new(),
            restart: true,
            journal: true,
        }
    }
}

/// Which Lutris install script installed a game (`iroriLutrisScript`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LutrisScriptRecord {
    /// The game's slug on lutris.net.
    pub slug: String,
    /// The installer's slug on lutris.net, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installer_slug: Option<String>,
    /// The installer's name ("GOG", "Setup"…).
    pub version: String,
    /// lutris.net's revision (or installer id) of the script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Where the script came from: its lutris.net page or the local file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            auto_install_dxvk: true,
            auto_install_vkd3d: true,
            auto_install_dxvk_nvapi: true,
            auto_sync_saves: false,
            battl_eye_runtime: true,
            dxvk_fps_cap: String::new(),
            eac_runtime: true,
            enable_dxvk_fps_limit: false,
            enable_esync: true,
            enable_fsr: false,
            enable_msync: false,
            enable_fsync: true,
            ntsync: true,
            enable_wine_wayland: false,
            enable_hdr: false,
            enable_wow64: false,
            proton_prefer_sdl: false,
            gamescope: GamescopeSettings::default(),
            environment_options: Vec::new(),
            dll_overrides: Vec::new(),
            in_sunshine: true,
            discord_presence: true,
            steam_overlay: false,
            ignore_game_updates: false,
            language: String::new(),
            launcher_args: String::new(),
            last_used_launch_option: None,
            max_sharpness: None,
            nvidia_prime: false,
            offline_mode: false,
            prefer_system_libs: false,
            show_fps: false,
            show_mangohud: false,
            mangohud_hidden: false,
            fps_limiter: FpsLimiter::Auto,
            target_exe: String::new(),
            use_game_mode: false,
            steam_runtime: None,
            wine_crossover_bottle: "Heroic".into(),
            wine_prefix: String::new(),
            wine_version: WineInstallation::default(),
            wrapper_options: Vec::new(),
            saves_path: String::new(),
            saves_path_found: None,
            gog_saves: Vec::new(),
            before_launch_script_path: String::new(),
            before_launch_script_alongside: false,
            after_launch_script_path: String::new(),
            disable_umu: false,
            no_wine: false,
            verbose_logs: true,
            advertise_avx_for_rosetta: false,
            dxvk_version: String::new(),
            vkd3d_version: String::new(),
            descriptor_heap: false,
            shader_cache_size: DEFAULT_SHADER_CACHE_GIB,
            dlss_upgrade: false,
            dlss_indicator: false,
            fsr4_upgrade: false,
            xess_upgrade: false,
            gpu: String::new(),
            windows_gpu: String::new(),
            hide_gpu: String::new(),
            smooth_motion: false,
            nvidia_reflex: false,
            lsfg: LsfgSettings::default(),
            enable_quick_saves_menu: false,
            retina_mode: false,
            rosetta: false,
            metal_fx_dlss: false,
            engine: String::new(),
            engine_version: String::new(),
            engine_options: BTreeMap::new(),
            engine_own_settings: false,
            lutris_script: None,
            sandbox: SandboxSettings::default(),
            launch_programs: Vec::new(),
            actions: Vec::new(),
            ludusavi_auto_backup: false,
            ludusavi_interval: 0,
            ludusavi_title: String::new(),
            hltb_id: String::new(),
            igdb_id: String::new(),
            vndb_id: String::new(),
            steam_app_id: String::new(),
            umu_id: String::new(),
            replay: ReplaySettings::default(),
            game_hud: crate::hud::GameHudSettings::default(),
            optiscaler: super::OptiscalerDefaults::default(),
        }
    }
}
