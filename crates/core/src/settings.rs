//! Settings schema. Field names follow Heroic's JSON keys (including its
//! `enviromentOptions` spelling) so Heroic's `config.json` and `GamesConfig/*.json`
//! deserialize directly during import.
//!
//! Global defaults live in [`AppSettings::game`]. Per-game settings are stored as
//! a sparse override map ([`GameSettingsOverrides`]) layered on top of the
//! defaults, so changing a global default reaches every game that has not
//! overridden that field.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::Runner;
use crate::game::{CloudSaveLocation, LaunchOption, SavesPathFound};
use crate::merged_lists;

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
            steam_app_id: String::new(),
            umu_id: String::new(),
            replay: ReplaySettings::default(),
            game_hud: crate::hud::GameHudSettings::default(),
        }
    }
}

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

/// Sparse per-game settings: only the keys a user changed for that game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Map<String, Value>", into = "Map<String, Value>")]
pub struct GameSettingsOverrides(pub Map<String, Value>);

impl From<Map<String, Value>> for GameSettingsOverrides {
    /// Irori was called Ember before its first release; its own keys were
    /// `ember…`, so files written then keep their overrides.
    fn from(map: Map<String, Value>) -> Self {
        Self(
            map.into_iter()
                .map(|(k, v)| match k.strip_prefix("ember") {
                    Some(rest) if rest.starts_with(char::is_uppercase) => {
                        (format!("irori{rest}"), v)
                    }
                    _ => (k, v),
                })
                .collect(),
        )
    }
}

impl From<GameSettingsOverrides> for Map<String, Value> {
    fn from(overrides: GameSettingsOverrides) -> Self {
        overrides.0
    }
}

impl GameSettingsOverrides {
    /// Effective settings: `defaults` with these overrides applied.
    pub fn apply(&self, defaults: &GameSettings) -> GameSettings {
        if self.0.is_empty() {
            return defaults.clone();
        }
        let mut base = serde_json::to_value(defaults).expect("settings serialize");
        if let Value::Object(map) = &mut base {
            for (k, v) in &self.0 {
                // Emulator options merge per emulator and option, so a game
                // that sets one option keeps the defaults of the others.
                if k == "iroriEngineOptions"
                    && let (Some(Value::Object(base)), Value::Object(set)) = (map.get_mut(k), v)
                {
                    for (engine, options) in set {
                        match (base.get_mut(engine), options) {
                            (Some(Value::Object(b)), Value::Object(o)) => {
                                b.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())))
                            }
                            _ => {
                                base.insert(engine.clone(), options.clone());
                            }
                        }
                    }
                    continue;
                }
                // Variables, wrappers and programs hold the game's own
                // entries, laid over the defaults' (`merged_lists`).
                if let Some(field) = merged_lists::key_field(k) {
                    let merged = merged_lists::merge(map.get(k), v, field);
                    map.insert(k.clone(), merged);
                    continue;
                }
                map.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).unwrap_or_else(|_| defaults.clone())
    }

    /// Overrides that turn `defaults` into `effective`: every key whose value
    /// differs, lists that merge by key as the game's own entries.
    pub fn diff(defaults: &GameSettings, effective: &GameSettings) -> Self {
        let (Value::Object(d), Value::Object(e)) = (
            serde_json::to_value(defaults).expect("settings serialize"),
            serde_json::to_value(effective).expect("settings serialize"),
        ) else {
            return Self::default();
        };
        Self(
            e.into_iter()
                .filter_map(|(k, v)| {
                    let value = override_of(&d, &k, v)?;
                    Some((k, value))
                })
                .collect(),
        )
    }

    /// What to store for `key` so the game reads `value` over `defaults`:
    /// `None` when that is the default, the game's own entries for a list
    /// that merges by key, else `value` itself.
    pub fn override_value(defaults: &GameSettings, key: &str, value: Value) -> Option<Value> {
        let Value::Object(d) = serde_json::to_value(defaults).expect("settings serialize") else {
            return Some(value);
        };
        override_of(&d, key, value)
    }

    pub fn is_overridden(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_string(), value);
    }

    pub fn reset(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn override_of(defaults: &Map<String, Value>, key: &str, value: Value) -> Option<Value> {
    match merged_lists::key_field(key) {
        Some(field) => merged_lists::own_entries(defaults.get(key), &value, field),
        None => (defaults.get(key) != Some(&value)).then_some(value),
    }
}

/// What the library shows above the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryTopSection {
    Disabled,
    #[default]
    RecentlyPlayed,
    RecentlyPlayedInstalled,
    Favourites,
}

/// Snapshots of each game's saves kept by default (`iroriSaveSnapshots`).
pub const DEFAULT_SAVE_SNAPSHOTS: u32 = 5;

/// Other launchers Irori keeps in sync with (`iroriLauncherImports`): read
/// again at start and every quarter of an hour, and only the games not seen
/// before are imported. Their own files are only read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherImports {
    pub lutris: bool,
    pub bottles: bool,
    pub faugus: bool,
}

impl LauncherImports {
    pub fn any(&self) -> bool {
        self.lutris || self.bottles || self.faugus
    }
}

/// Application-wide settings. Heroic stores these next to the default game
/// settings in one object; `game` is flattened to keep that shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(flatten)]
    pub game: GameSettings,
    pub add_desktop_shortcuts: bool,
    pub add_start_menu_shortcuts: bool,
    pub add_steam_shortcuts: bool,
    pub auto_update_games: bool,
    pub check_for_updates_on_startup: bool,
    /// Minutes between background update checks.
    pub check_updates_interval: u32,
    pub custom_wine_paths: Vec<String>,
    pub dark_tray_icon: bool,
    pub default_install_path: String,
    /// Install each store's games in a folder of the store's own inside
    /// `defaultInstallPath` (`Games/Irori/GOG/<game>`). Irori-only.
    #[serde(rename = "iroriInstallPerStore")]
    pub install_per_store: bool,
    /// More folders games install into beside `defaultInstallPath`, each
    /// with a name (another drive, an SD card). Irori-only.
    #[serde(rename = "iroriLibraryFolders")]
    pub library_folders: Vec<LibraryFolder>,
    pub default_steam_path: String,
    pub default_wine_prefix_dir: String,
    pub disable_controller: bool,
    /// How controller input behaves (`iroriController`). Irori-only.
    #[serde(rename = "iroriController")]
    pub controller: crate::controller_prefs::ControllerPrefs,
    /// Big Picture's sounds and idle screen (`iroriBigPicture`). Irori-only.
    #[serde(rename = "iroriBigPicture")]
    pub big_picture: crate::big_picture_prefs::BigPicturePrefs,
    pub disable_playtime_sync: bool,
    pub disable_logs: bool,
    pub disable_animations: bool,
    #[serde(rename = "discordRPC")]
    pub discord_rpc: bool,
    /// A Discord application ID of the user's own for Rich Presence,
    /// replacing [`DISCORD_APPLICATION_ID`]. Empty uses Irori's.
    pub discord_client_id: String,
    #[serde(rename = "disableGOGPresence")]
    pub disable_gog_presence: bool,
    pub download_no_https: bool,
    pub download_proton_to_steam: bool,
    /// Proton-CachyOS and Proton-Wineland builds for x86-64-v3 processors.
    /// Linux only; Irori-only.
    #[serde(rename = "iroriProtonCpuLevel")]
    pub proton_cpu_level: ProtonCpuLevel,
    pub egs_linked_path: String,
    pub exit_to_tray: bool,
    pub gamepad_repeat_delay: u32,
    pub gamepad_initial_repeat_delay: u32,
    pub no_tray_icon: bool,
    pub hide_changelogs_on_startup: bool,
    pub hide_window_on_protocol_launch: bool,
    pub library_top_section: LibraryTopSection,
    pub max_recent_games: u32,
    /// Download workers (0 = store default).
    pub max_workers: u32,
    /// Download bandwidth limit in KiB/s (0 = unlimited).
    pub max_download_speed: u32,
    pub minimize_on_launch: bool,
    pub start_in_console_mode: bool,
    pub start_in_tray: bool,
    /// Start Irori when the user logs in. Irori-only.
    #[serde(rename = "iroriStartWithSystem")]
    pub start_with_system: bool,
    /// When started with the system, start in the tray. Irori-only.
    #[serde(rename = "iroriStartWithSystemMinimized")]
    pub start_with_system_minimized: bool,
    pub allow_installation_broken_anticheat: bool,
    pub show_valve_proton: bool,
    pub steam_grid_db_api_key: String,
    pub comet_support: bool,
    pub zoom_platform: bool,
    /// The itch.io store (off until turned on). Irori-only.
    #[serde(rename = "iroriItchIo", alias = "emberItchIo")]
    pub itch_io: bool,
    /// The Xbox store: Microsoft Store and PC Game Pass games, Linux only
    /// (off until turned on). Irori-only.
    #[serde(rename = "iroriXbox")]
    pub xbox: bool,
    /// The Microsoft Store market (two-letter region) the Xbox store's
    /// library, catalog and licences use; empty follows the interface
    /// language's region, else `US`. Irori-only.
    #[serde(rename = "iroriXboxMarket")]
    pub xbox_market: String,
    /// UI language code ("en", "de", …).
    #[serde(rename = "uiLanguage")]
    pub ui_language: String,
    #[serde(rename = "iroriUi", alias = "emberUi")]
    pub ui: UiPrefs,
    /// Where offline installers and goodies are saved (each game in its own
    /// folder inside).
    #[serde(rename = "iroriExtrasPath", alias = "emberExtrasPath")]
    pub extras_path: String,
    /// macOS: where CrossOver keeps its bottles, when moved from
    /// `~/Library/Application Support/CrossOver/Bottles` ("" = there).
    #[serde(
        rename = "iroriCrossoverBottlesPath",
        alias = "emberCrossoverBottlesPath"
    )]
    pub crossover_bottles_path: String,
    /// Where the Wine manager installs the Wine and Proton builds it
    /// downloads, in `wine` and `proton` folders inside ("" = Irori's tools
    /// folder). Builds already installed stay where they are. Irori-only.
    #[serde(rename = "iroriCompatToolsPath")]
    pub compat_tools_path: String,
    /// The default emulator of each system (`psx` → `duckstation`); systems
    /// not listed use the first installed recommended emulator.
    #[serde(rename = "iroriSystemEngines", alias = "emberSystemEngines")]
    pub system_engines: BTreeMap<String, String>,
    /// Folders scanned for console and retro games.
    #[serde(rename = "iroriRomFolders", alias = "emberRomFolders")]
    pub rom_folders: Vec<String>,
    /// Where the user keeps BIOS files ("" = not chosen).
    #[serde(rename = "iroriBiosFolder", alias = "emberBiosFolder")]
    pub bios_folder: String,
    /// List games of Battle.net, EA app, Ubisoft Connect and Rockstar
    /// (off until turned on). Irori-only.
    #[serde(
        rename = "iroriThirdPartyLaunchers",
        alias = "emberThirdPartyLaunchers"
    )]
    pub third_party_launchers: bool,
    /// Minutes between rescans of the third-party launchers' games, so
    /// games installed in a launcher show up without a refresh; 0 rescans
    /// only on a refresh. Irori-only.
    #[serde(rename = "iroriLaunchersRescan")]
    pub launchers_rescan: u32,
    /// Install new builds of components (DXVK, Wine, emulators) as soon as
    /// the daily check finds them. Irori-only.
    #[serde(rename = "iroriAutoUpdateComponents")]
    pub auto_update_components: bool,
    /// Hour automatic updates may start, `0`..`23`; empty for any. Irori-only.
    #[serde(rename = "iroriAutoUpdateFrom")]
    pub auto_update_from: String,
    /// Hour automatic updates stop starting, `0`..`23`; empty for any.
    /// Earlier than `auto_update_from` means past midnight. Irori-only.
    #[serde(rename = "iroriAutoUpdateUntil")]
    pub auto_update_until: String,
    /// Hold automatic updates on a metered connection. Irori-only.
    #[serde(rename = "iroriAutoUpdateUnmetered")]
    pub auto_update_unmetered: bool,
    /// Look for new versions of Irori once a day. Irori-only; about Irori
    /// itself, where `checkForUpdatesOnStartup` is about games.
    #[serde(rename = "iroriCheckForAppUpdates")]
    pub check_for_app_updates: bool,
    /// Show deals from GOG and other stores on the Stores page; off, they
    /// are neither shown nor asked for. Irori-only.
    #[serde(rename = "iroriDeals")]
    pub deals: bool,
    /// Whether Irori probes the connection, takes it as working, or works
    /// offline and never reaches the internet. Irori-only.
    #[serde(rename = "iroriNetworkMode")]
    pub network_mode: crate::net::NetworkMode,
    /// Seconds a network request may take (10–600), for slow or unreliable
    /// connections; 0 keeps Irori's own timeouts. Irori-only.
    #[serde(rename = "iroriNetworkTimeout")]
    pub network_timeout: u32,
    /// Snapshots of a game's saves kept from before cloud syncs that could
    /// replace them (0–50); 0 takes none. Irori-only.
    #[serde(rename = "iroriSaveSnapshots")]
    pub save_snapshots: u32,
    /// Open a game's log when it starts. Irori-only.
    #[serde(rename = "iroriShowLogOnLaunch")]
    pub show_log_on_launch: bool,
    /// Announce the achievements a session unlocked once the game exits.
    /// Irori-only.
    #[serde(rename = "iroriAchievementNotices")]
    pub achievement_notices: bool,
    /// How unlocks show while the game runs: over the game, and with which
    /// lines. Irori-only.
    #[serde(rename = "iroriAchievementToast")]
    pub achievement_toast: AchievementToastPrefs,
    /// Read the achievements that Steam API emulators (Goldberg, gbe_fork,
    /// CODEX-style) keep in a game's own files, for games Irori starts from
    /// their own executable. Irori-only.
    #[serde(rename = "iroriFileAchievements")]
    pub file_achievements: bool,
    /// Fill what store pages leave out (descriptions, genres, release
    /// dates, screenshots) from IGDB, and offer its art in the art picker.
    /// Needs the user's own Twitch application, kept with `irori_secrets`;
    /// off by default. Irori-only.
    #[serde(rename = "iroriIgdb")]
    pub igdb: bool,
    /// Box art for the ROM library from the LaunchBox Games Database: its
    /// game list (about 110 MB) is downloaded, and again once a week while
    /// the ROM library has games. Off by default; the art picker's and the
    /// palette's download turn it on. Irori-only.
    #[serde(rename = "iroriLaunchBox", default)]
    pub launchbox: bool,
    /// Link games of other stores to the Steam app of their exact title
    /// (edition words aside, and a release year apart at most), for
    /// Steam's tags, ProtonDB and screenshots, when no source names their
    /// Steam app. Off by default. Irori-only.
    #[serde(rename = "iroriSteamTitleMatch")]
    pub steam_title_match: bool,
    /// Steam tags each game keeps, strongest first (5–20); tags far fewer
    /// players gave than the strongest are left out too. Irori-only.
    #[serde(rename = "iroriSteamTagLimit")]
    pub steam_tag_limit: u32,
    /// The user's own launch presets. Irori-only.
    #[serde(rename = "iroriLaunchPresets")]
    pub launch_presets: Vec<crate::launch_extras::LaunchPreset>,
    /// The Ludusavi program to run for save backups; empty finds it on
    /// `PATH`, in the usual install folders or on Flathub. Irori-only.
    #[serde(rename = "iroriLudusaviPath")]
    pub ludusavi_path: String,
    /// Where Ludusavi keeps the backups Irori makes; empty for Ludusavi's
    /// own backup folder. Irori-only.
    #[serde(rename = "iroriLudusaviBackupPath")]
    pub ludusavi_backup_path: String,
    /// Full backups Ludusavi keeps of each game when Irori backs it up
    /// (1–255); 0 keeps Ludusavi's own setting. Irori-only.
    #[serde(rename = "iroriLudusaviFullLimit")]
    pub ludusavi_full_limit: u32,
    /// Differential backups Ludusavi keeps per full one ("0"–"255"); empty
    /// keeps Ludusavi's own setting. Irori-only.
    #[serde(rename = "iroriLudusaviDifferentialLimit")]
    pub ludusavi_differential_limit: String,
    /// Largest size of the art cache on disk in MiB (downloaded art and the
    /// thumbnails cut from it); the least recently used art goes first.
    /// 0 = no cap. Irori-only.
    #[serde(rename = "iroriArtCacheLimit")]
    pub art_cache_limit: u32,
    /// Other launchers whose new games Irori keeps importing. Irori-only.
    #[serde(rename = "iroriLauncherImports")]
    pub launcher_imports: LauncherImports,
    /// Shortcuts of a Windows program take the program's own icon, before
    /// the store's art. Irori-only.
    #[serde(rename = "iroriShortcutIconFromExe")]
    pub shortcut_icon_from_exe: bool,
    /// Scheduled backups. Irori-only; stays on this computer (backups and
    /// restores leave it alone).
    #[serde(rename = "iroriBackups")]
    pub backups: BackupPrefs,
    /// Library sync between computers. Irori-only; stays on this computer.
    #[serde(rename = "iroriSync")]
    pub sync: SyncPrefs,
    /// Irori's games in Sunshine, the game-streaming host. Irori-only; stays
    /// on this computer.
    #[serde(rename = "iroriSunshine")]
    pub sunshine: SunshinePrefs,
    /// The in-game overlay on the controller's Guide button. Irori-only;
    /// Linux only for now.
    #[serde(rename = "iroriOverlay")]
    pub overlay: OverlayPrefs,
    /// Global Performance HUD settings (MangoHud on Linux, Metal HUD on macOS).
    /// Irori-only.
    #[serde(rename = "iroriHud", default)]
    pub hud: crate::hud::GlobalHudSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let games = home.join("Games").join("Irori");
        let extras = dirs::download_dir()
            .unwrap_or_else(|| home.join("Downloads"))
            .join("Irori");
        Self {
            extras_path: extras.to_string_lossy().into_owned(),
            crossover_bottles_path: String::new(),
            compat_tools_path: String::new(),
            system_engines: BTreeMap::new(),
            rom_folders: Vec::new(),
            bios_folder: String::new(),
            third_party_launchers: false,
            launchers_rescan: 5,
            auto_update_components: false,
            auto_update_from: String::new(),
            auto_update_until: String::new(),
            auto_update_unmetered: false,
            check_for_app_updates: true,
            deals: true,
            network_mode: crate::net::NetworkMode::Auto,
            network_timeout: 0,
            save_snapshots: DEFAULT_SAVE_SNAPSHOTS,
            show_log_on_launch: false,
            achievement_notices: false,
            achievement_toast: AchievementToastPrefs::default(),
            file_achievements: false,
            igdb: false,
            launchbox: false,
            steam_title_match: false,
            steam_tag_limit: DEFAULT_STEAM_TAG_LIMIT,
            launch_presets: Vec::new(),
            ludusavi_path: String::new(),
            ludusavi_backup_path: String::new(),
            art_cache_limit: DEFAULT_ART_CACHE_LIMIT,
            ludusavi_full_limit: 0,
            ludusavi_differential_limit: String::new(),
            backups: BackupPrefs::default(),
            sync: SyncPrefs::default(),
            sunshine: SunshinePrefs::default(),
            overlay: OverlayPrefs::default(),
            hud: crate::hud::GlobalHudSettings::default(),
            game: GameSettings::default(),
            add_desktop_shortcuts: false,
            add_start_menu_shortcuts: false,
            add_steam_shortcuts: false,
            launcher_imports: LauncherImports::default(),
            shortcut_icon_from_exe: false,
            auto_update_games: false,
            check_for_updates_on_startup: true,
            check_updates_interval: 10,
            custom_wine_paths: Vec::new(),
            dark_tray_icon: false,
            default_install_path: games.to_string_lossy().into_owned(),
            install_per_store: false,
            library_folders: Vec::new(),
            default_steam_path: crate::platform::steam_dir().to_string_lossy().into_owned(),
            default_wine_prefix_dir: games.join("Prefixes").to_string_lossy().into_owned(),
            disable_controller: false,
            controller: Default::default(),
            big_picture: Default::default(),
            disable_playtime_sync: false,
            disable_logs: false,
            disable_animations: false,
            discord_rpc: false,
            discord_client_id: String::new(),
            disable_gog_presence: false,
            download_no_https: false,
            download_proton_to_steam: false,
            proton_cpu_level: ProtonCpuLevel::Auto,
            egs_linked_path: String::new(),
            exit_to_tray: false,
            gamepad_repeat_delay: 50,
            gamepad_initial_repeat_delay: 300,
            no_tray_icon: false,
            hide_changelogs_on_startup: false,
            hide_window_on_protocol_launch: false,
            library_top_section: LibraryTopSection::RecentlyPlayed,
            max_recent_games: 8,
            max_workers: 0,
            max_download_speed: 0,
            minimize_on_launch: false,
            start_in_console_mode: false,
            start_in_tray: false,
            start_with_system: false,
            start_with_system_minimized: true,
            allow_installation_broken_anticheat: false,
            show_valve_proton: false,
            steam_grid_db_api_key: String::new(),
            comet_support: true,
            zoom_platform: true,
            itch_io: false,
            xbox: false,
            xbox_market: String::new(),
            ui_language: "en".into(),
            ui: UiPrefs::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryLayout {
    #[default]
    Grid,
    List,
    /// A column of covers beside the selected game's page.
    Focus,
}

/// How much of the desktop shows through Irori's window chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Translucency {
    /// On where the system blurs what is behind the window (macOS,
    /// Windows 11, Linux compositors that offer blur), off elsewhere.
    #[default]
    Auto,
    Off,
    /// Translucent title bar and sidebar; pages stay solid.
    Subtle,
    /// Clearer chrome, and pages let a little of the desktop through too.
    Strong,
}

/// The controller button held to open Big Picture (`bigPictureHoldButton`).
/// Values a newer Irori writes read as [`HoldButton::Start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldButton {
    /// Holding a button does nothing.
    Off,
    /// Start (Menu, Options, +).
    #[default]
    Start,
    /// Select (View, Share, Create, −).
    Select,
}

impl<'de> Deserialize<'de> for HoldButton {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "off" => HoldButton::Off,
            "select" => HoldButton::Select,
            _ => HoldButton::Start,
        })
    }
}

/// How tightly the library packs its covers and rows. Separate from the
/// interface size, which scales everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

impl Density {
    /// Factor on the cover size the user picked.
    pub fn cover_scale(self) -> f32 {
        match self {
            Density::Compact => 0.85,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.15,
        }
    }

    /// Factor on the space between covers and the height of list rows' padding.
    pub fn spacing_scale(self) -> f32 {
        match self {
            Density::Compact => 0.55,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.5,
        }
    }

    /// Factor on the height of a row in the list layout.
    pub fn row_scale(self) -> f32 {
        match self {
            Density::Compact => 0.8,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.2,
        }
    }
}

/// The corners of the library's covers (`coverCorners`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverCorners {
    Square,
    /// The theme's large radius.
    #[default]
    Rounded,
    /// Twice the theme's large radius.
    Round,
}

/// What the library writes under each cover (`coverCaption`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverCaption {
    /// The title and a line about the game (install state, size, playtime).
    #[default]
    Full,
    /// Only the title.
    Title,
    /// Nothing: the covers alone.
    None,
}

/// How the library draws box art (`coverFit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverFit {
    /// Every cover filled to 2:3, cut where its shape differs.
    #[default]
    Crop,
    /// Each picture whole in its own shape (tall, square, wide), standing
    /// on its row; tall boxes of one row share a height.
    Whole,
}

/// The shape of the library grid's covers (`coverShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverShape {
    /// Square art: the store's square capsule, else the box art cropped.
    Square,
    /// A wide banner of the game's hero art, 16:9.
    Banner,
    /// Box art, 2:3. Last, so a shape a newer Irori writes reads as this.
    #[default]
    #[serde(other)]
    Portrait,
}

impl CoverShape {
    /// Height over width.
    pub fn ratio(self) -> f32 {
        match self {
            CoverShape::Portrait => 1.5,
            CoverShape::Square => 1.0,
            CoverShape::Banner => 9.0 / 16.0,
        }
    }

    /// How much wider than a box a cover of this shape is drawn at one
    /// cover size, so the size slider keeps covers of about one weight.
    pub fn width_scale(self) -> f32 {
        match self {
            CoverShape::Portrait => 1.0,
            CoverShape::Square => 1.2,
            CoverShape::Banner => 1.75,
        }
    }
}

/// How large the covers in the Focus view's column are
/// (`libraryFocusCoverSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusCoverSize {
    Small,
    Large,
    /// Last, so a size a newer Irori writes reads as this one.
    #[default]
    #[serde(other)]
    Medium,
}

/// How a cover answers the pointer and the keyboard focus (`coverHover`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverHover {
    /// It rises a little, with a shadow and a ring.
    #[default]
    Lift,
    /// Only the ring.
    Outline,
}

/// The art cache's default cap in MiB.
pub const DEFAULT_ART_CACHE_LIMIT: u32 = 2048;

/// [`AppSettings::steam_tag_limit`] by default: every tag Steam names.
pub const DEFAULT_STEAM_TAG_LIMIT: u32 = 20;

/// How often Irori backs up on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupSchedule {
    Off,
    Daily,
    #[default]
    Weekly,
}

/// Scheduled backups: when, where, how many and what goes in. Store
/// sign-ins never do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupPrefs {
    pub schedule: BackupSchedule,
    /// Where backups go ("" = the `backups` folder in Irori's data folder).
    pub folder: String,
    /// How many scheduled backups to keep; older ones are removed.
    pub keep: u32,
    /// Back up before a new version of Irori first runs.
    pub before_updates: bool,
    pub settings: bool,
    pub game_settings: bool,
    pub library: bool,
    pub playtime: bool,
    /// Where store games are installed, so a restore attaches them again.
    pub installs: bool,
    /// Copy the downloaded game art too (large; it can be downloaded again).
    pub art: bool,
}

impl Default for BackupPrefs {
    fn default() -> Self {
        Self {
            schedule: BackupSchedule::Weekly,
            folder: String::new(),
            keep: 10,
            before_updates: true,
            settings: true,
            game_settings: true,
            library: true,
            playtime: true,
            installs: true,
            art: false,
        }
    }
}

/// What the library shows behind its covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryBackground {
    #[default]
    None,
    /// The focused game's art, else the last played game's, else the first
    /// game shown, blurred.
    GameArt,
    /// A picture the user picked (`libraryBackgroundPicture`), blurred.
    Picture,
}

/// Where the library syncs with Irori on other computers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[default]
    Off,
    /// A folder something else keeps in sync (Syncthing, Dropbox, a share).
    Folder,
    /// A WebDAV server; the login is kept with `irori_secrets`.
    Webdav,
    /// A remote of the user's own rclone (`rcloneRemote`); rclone keeps the
    /// account.
    Rclone,
}

/// Library sync between computers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPrefs {
    pub mode: SyncMode,
    pub folder: String,
    /// The WebDAV folder's address, without the login.
    pub webdav_url: String,
    /// What other computers call this one ("" = its host name).
    pub device_name: String,
    /// The rclone remote, with an optional folder (`gdrive:Games`).
    pub rclone_remote: String,
    /// The rclone program; empty finds it on `PATH`.
    pub rclone_path: String,
    /// Cloud copies of backups, Ludusavi's saves and screenshots at the
    /// sync target.
    pub copies: CopyPrefs,
    /// Exchange only playtime and sessions: this computer's favourites,
    /// categories and statuses stay here, and the others' don't change it.
    pub playtime_only: bool,
}

/// When a kind of cloud copy is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyMode {
    #[default]
    Off,
    /// Only when the user asks ("Send now").
    Manual,
    /// On its own, every so often, never while a game runs.
    Automatic,
}

/// What goes to the sync target besides the library.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CopyPrefs {
    /// Irori's scheduled backups (never the store sign-ins).
    pub backups: CopyMode,
    /// Ludusavi's backup folder.
    pub saves: CopyMode,
    /// The screenshots Irori finds (not clips).
    pub screenshots: CopyMode,
}

/// Which of Irori's games Sunshine lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SunshineGames {
    /// Every installed game the library shows.
    #[default]
    Installed,
    /// Installed favourites only.
    Favourites,
}

/// How unlocks show while a game runs (`iroriAchievementToast`), with
/// `iroriAchievementNotices` on: a card in a corner above the game, with
/// the achievement's name and the lines chosen here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AchievementToastPrefs {
    /// Show the card above the game. Off, or where nothing can show above
    /// it, unlocks are announced as a notice (on the desktop while Irori is
    /// in the background).
    pub over_game: bool,
    /// The game's title.
    pub show_game: bool,
    pub show_description: bool,
    /// The share of players who have it.
    pub show_rarity: bool,
    /// How many of the game's achievements are unlocked now.
    pub show_progress: bool,
    /// What it is worth, for stores whose achievements have points.
    pub show_points: bool,
}

impl Default for AchievementToastPrefs {
    fn default() -> Self {
        Self {
            over_game: true,
            show_game: false,
            show_description: true,
            show_rarity: true,
            show_progress: true,
            show_points: false,
        }
    }
}

/// The in-game overlay (`iroriOverlay`): Guide over a game Irori started
/// opens it instead of Big Picture. Off by default while it is in beta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OverlayPrefs {
    pub enabled: bool,
    /// Where the panel sits on the screen.
    pub side: OverlaySide,
    /// Guide + RB (R1) takes a screenshot.
    pub screenshot_chord: bool,
    /// Windows and macOS: Ctrl+Shift+S (⌃⇧S) takes a screenshot while a game
    /// Irori started runs, with or without the overlay.
    pub screenshot_key: bool,
    /// Guide + Up/Down change the system's volume.
    pub volume_chords: bool,
    /// Guide + Y (Triangle) marks the moment: a screenshot and a journal
    /// note to write later (`momentChord`).
    pub moment_chord: bool,
    /// How far one press of a volume control moves a volume, in percent
    /// (see [`Self::volume_step`]).
    pub volume_step: u32,
    /// After Guide + Up/Down the new level shows over the game for a moment.
    pub level_notice: bool,
    /// Back + Start (View + Menu) together open and close the overlay where
    /// Guide can't be read while a game has the focus (Windows' XInput).
    pub back_start: bool,
}

impl Default for OverlayPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            side: OverlaySide::Center,
            screenshot_chord: true,
            screenshot_key: true,
            volume_chords: true,
            moment_chord: true,
            volume_step: 5,
            level_notice: true,
            back_start: true,
        }
    }
}

impl OverlayPrefs {
    /// [`Self::volume_step`] within the offered range, 1 to 25 percent.
    pub fn volume_step(&self) -> i32 {
        self.volume_step.clamp(1, 25) as i32
    }
}

/// What applies a game's frame limit (`iroriFpsLimiter`). Values a newer
/// Irori writes read as [`FpsLimiter::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsLimiter {
    /// gamescope, else a shown MangoHud, else DXVK, else a hidden MangoHud.
    #[default]
    Auto,
    Gamescope,
    #[serde(rename = "mangohud")]
    MangoHud,
    Dxvk,
}

impl<'de> Deserialize<'de> for FpsLimiter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "gamescope" => FpsLimiter::Gamescope,
            "mangohud" => FpsLimiter::MangoHud,
            "dxvk" => FpsLimiter::Dxvk,
            _ => FpsLimiter::Auto,
        })
    }
}

/// Which build the Wine manager installs where a source publishes one for
/// x86-64-v3 processors beside the baseline one (`iroriProtonCpuLevel`).
/// Values a newer Irori writes read as [`ProtonCpuLevel::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonCpuLevel {
    /// The x86-64-v3 build when this processor runs it.
    #[default]
    Auto,
    /// Always the baseline x86-64 build.
    Baseline,
    /// Always the x86-64-v3 build.
    V3,
}

impl<'de> Deserialize<'de> for ProtonCpuLevel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "baseline" => ProtonCpuLevel::Baseline,
            "v3" => ProtonCpuLevel::V3,
            _ => ProtonCpuLevel::Auto,
        })
    }
}

/// Where the in-game overlay's panel sits. Values a newer Irori writes read
/// as [`OverlaySide::Center`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlaySide {
    #[default]
    Center,
    /// Along the left edge, the height of the screen.
    Left,
    /// Along the right edge, the height of the screen.
    Right,
}

impl<'de> Deserialize<'de> for OverlaySide {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "left" => OverlaySide::Left,
            "right" => OverlaySide::Right,
            _ => OverlaySide::Center,
        })
    }
}

/// Irori's games in Sunshine (`iroriSunshine`). Sunshine's own login is
/// kept with `irori_secrets`, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SunshinePrefs {
    /// Keep Irori's games among Sunshine's applications.
    pub enabled: bool,
    pub games: SunshineGames,
    /// Add an application that opens Big Picture.
    pub big_picture: bool,
    /// Sunshine's web UI port; 0 reads it from Sunshine's configuration.
    pub port: u16,
    /// Sunshine on another computer: its web UI's address (`htpc.local`,
    /// `192.168.1.20:47990`); empty for this computer.
    pub address: String,
    /// Games in gamescope take the Moonlight client's resolution, frame rate
    /// and HDR when a stream starts them, unless the game sets its own.
    pub match_client: bool,
}

impl Default for SunshinePrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            games: SunshineGames::Installed,
            big_picture: true,
            port: 0,
            address: String::new(),
            match_client: true,
        }
    }
}

/// Irori-only presentation preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiPrefs {
    pub theme: ThemePreference,
    /// Theme name from the bundled/user theme registry ("" = default: the
    /// wallpaper's colours with `wallpaperColors` on, else the desktop's
    /// theme on COSMIC, KDE Plasma and Omarchy, Irori's own elsewhere, in the
    /// system's accent colour where it has one).
    pub theme_name: String,
    /// Tint the default theme with the system's accent colour (`systemAccent`).
    pub system_accent: bool,
    /// Make the default theme of the colours pywal, wallust or matugen made
    /// from the wallpaper (`wallpaperColors`), and follow them as they change.
    pub wallpaper_colors: bool,
    /// The colours file to follow (`wallpaperColorsFile`): pywal's
    /// `colors.json`, or JSON matugen rendered. "" = pywal's, in
    /// `$PYWAL_CACHE_DIR` or `~/.cache/wal`, which wallust writes too.
    pub wallpaper_colors_file: String,
    pub translucency: Translucency,
    /// Base font size in px; drives the rem scale (application zoom).
    pub font_size: f32,
    pub library_layout: LibraryLayout,
    /// Cover tile width in rem for the grid.
    pub tile_size: f32,
    /// Width in rem of the game drawer beside the grid (`libraryDrawerWidth`).
    pub library_drawer_width: f32,
    pub reduce_motion: bool,
    /// Hold the middle mouse button and move the mouse to scroll what is
    /// under it (`middleClickScroll`, off unless turned on).
    pub middle_click_scroll: bool,
    pub show_store_badges: bool,
    /// The game's categories on its cover in the grid (`coverCategories`,
    /// off unless turned on).
    pub cover_categories: bool,
    /// Copies of one game from several stores are one library entry
    /// (`groupCopies`, on unless turned off).
    pub group_copies: bool,
    /// A game page's first trailer starts on its own, muted
    /// (`trailerAutoplay`, off unless turned on).
    pub trailer_autoplay: bool,
    /// Quick sort, install, favourites and grouping controls in the
    /// library's header, shown as far as its width allows
    /// (`libraryQuickControls`, on unless turned off).
    pub library_quick_controls: bool,
    pub sidebar_collapsed: bool,
    pub window_width: f32,
    pub window_height: f32,
    pub window_maximized: bool,
    /// Open the main window maximized, whatever state it was left in
    /// (`startMaximized`).
    pub start_maximized: bool,
    /// The main window's place on its display, in global logical pixels,
    /// where the system lets Irori place windows (not Wayland).
    pub window_x: Option<f32>,
    pub window_y: Option<f32>,
    /// The UUID of the display the main window was on ("" = unknown).
    pub window_display: String,
    /// Stores the library, accounts and store pages show.
    pub stores: ShownStores,
    /// Open Big Picture at startup in Steam's Game Mode or Big Picture.
    pub big_picture_in_game_mode: bool,
    /// Size of Big Picture's interface in percent, for TVs.
    pub big_picture_scale: u32,
    /// How Big Picture's Home arranges games (`shelves`, `spotlight`,
    /// `cinema`, `wall`, `carousel`, `compact`); unknown values read as
    /// `shelves`.
    pub big_picture_layout: String,
    /// Big Picture's theme: `builtin:<id>`, `user:<id>` (a file in the
    /// config folder's `bigpicture/themes`), or "" for Irori's own.
    pub big_picture_theme: String,
    /// Open Big Picture when a controller connects while Irori runs, or
    /// is connected when it starts.
    pub big_picture_on_controller: bool,
    /// Ask the desktop for a shortcut that opens the quick launcher (the
    /// command palette in a window of its own) from anywhere
    /// (`quickLaunchShortcut`); off, `irori --quick-launch` still opens it.
    pub quick_launch_shortcut: bool,
    /// The controller button held to open Big Picture from anywhere in
    /// Irori (`bigPictureHoldButton`).
    pub big_picture_hold_button: HoldButton,
    /// How long that button is held, in milliseconds (`bigPictureHoldMs`).
    pub big_picture_hold_ms: u32,
    /// The accent colour the user picked, `#rrggbb` ("" = the theme's own,
    /// or the system's where Irori follows it).
    pub accent_color: String,
    pub density: Density,
    /// Interface typeface ("" = Manrope, Irori's own).
    pub font_family: String,
    pub library_background: LibraryBackground,
    /// The picture behind the library when `libraryBackground` is `picture`.
    pub library_background_picture: String,
    /// How strongly the background shows, in percent of the most text keeps
    /// legible over it.
    pub library_background_strength: u32,
    /// How much of the theme's background is laid over it, in percent.
    pub library_background_dim: u32,
    /// Big Picture fills the screen (`bigPictureFullscreen`, on unless
    /// turned off); off, it shows in the window as it is.
    pub big_picture_fullscreen: bool,
    pub cover_corners: CoverCorners,
    pub cover_caption: CoverCaption,
    pub cover_hover: CoverHover,
    pub cover_fit: CoverFit,
    /// The shape of the grid's covers (`coverShape`).
    pub cover_shape: CoverShape,
    /// The star on favourites' covers (`coverFavourite`, on unless turned
    /// off).
    pub cover_favourite: bool,
    /// Covers to a row in the Focus view's column (`libraryFocusColumns`,
    /// 1 to 3); 0 fits as many as the window allows.
    pub library_focus_columns: u32,
    /// How large the Focus view's covers are (`libraryFocusCoverSize`).
    pub library_focus_cover_size: FocusCoverSize,
    /// The library switches to Focus when the window grows wide enough for
    /// its page beside the covers, and to the grid when it shrinks below
    /// that (`libraryAutoLayout`, off unless turned on).
    pub library_auto_layout: bool,
    /// A click on a cover in the grid opens the drawer about the game
    /// (`libraryOpenDetailsOnSelect`, on unless turned off); off, it only
    /// selects it, and `I`, Enter or a double click go further.
    pub library_open_details_on_select: bool,
    /// Store badges stay on every cover (`storeBadgesAlways`); off, they
    /// show on the cover pointed at or focused.
    pub store_badges_always: bool,
    /// Store badges show while the library lists one store too
    /// (`storeBadgesOneStore`, off unless turned on), where they all say the
    /// same.
    pub store_badges_one_store: bool,
    /// Game pages show the age-rating card (`showAgeRatings`, on unless
    /// turned off). The library's age filter works either way.
    pub show_age_ratings: bool,
    /// The board whose rating shows first (`ageRatingBoard`): its name
    /// ("PEGI", as [`crate::RatingSystem::from_key`] reads it), or empty for
    /// the board of the user's region.
    pub age_rating_board: String,
    /// Kinds of rows the game page's Details leave out (`hiddenDetailRows`,
    /// ids such as `publisher`); ids a later Irori adds stay as written.
    pub hidden_detail_rows: Vec<String>,
}

/// Which stores Irori shows. A hidden store keeps its sign-in and installed
/// games on disk; its games, account card and store page are left out.
/// ZOOM Platform follows `zoomPlatform`, itch.io follows `iroriItchIo`, and
/// apps the user added are always shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShownStores {
    pub epic: bool,
    pub gog: bool,
    pub amazon: bool,
    pub steam: bool,
}

impl Default for ShownStores {
    fn default() -> Self {
        Self {
            epic: true,
            gog: true,
            amazon: true,
            steam: true,
        }
    }
}

impl ShownStores {
    /// Whether the store's games and pages are shown.
    pub fn shows(&self, runner: Runner) -> bool {
        match runner {
            Runner::Legendary => self.epic,
            Runner::Gog => self.gog,
            Runner::Nile => self.amazon,
            Runner::Steam => self.steam,
            Runner::Zoom
            | Runner::Itch
            | Runner::Xbox
            | Runner::Sideload
            | Runner::Emulation
            | Runner::Launchers => true,
        }
    }

    /// Show or hide a store. ZOOM Platform, itch.io and added apps have no
    /// switch here, so they are left as they are.
    pub fn set(&mut self, runner: Runner, shown: bool) {
        match runner {
            Runner::Legendary => self.epic = shown,
            Runner::Gog => self.gog = shown,
            Runner::Nile => self.amazon = shown,
            Runner::Steam => self.steam = shown,
            Runner::Zoom
            | Runner::Itch
            | Runner::Xbox
            | Runner::Sideload
            | Runner::Emulation
            | Runner::Launchers => {}
        }
    }

    /// Whether the store can be hidden with [`ShownStores::set`].
    pub fn can_hide(runner: Runner) -> bool {
        matches!(
            runner,
            Runner::Legendary | Runner::Gog | Runner::Nile | Runner::Steam
        )
    }
}

/// Irori's Discord application (client) ID, which Rich Presence shows as
/// "Irori". It is public, not a secret: the project owner created the
/// Discord application "Irori" at discord.com/developers/applications and
/// copied its Application ID from General Information. Rich Presence's
/// small image is that application's art asset named `irori` (Rich
/// Presence → Art Assets). Left empty, Rich Presence stays off unless the
/// user enters an application ID of their own (`discordClientId`).
pub const DISCORD_APPLICATION_ID: &str = "1568789394926870640";

impl AppSettings {
    /// The Discord application Rich Presence speaks for: the user's own, else
    /// Irori's. `None` while Rich Presence is off or there is no ID at all.
    pub fn discord_application(&self) -> Option<String> {
        if !self.discord_rpc {
            return None;
        }
        let own = self.discord_client_id.trim();
        let id = if own.is_empty() {
            DISCORD_APPLICATION_ID
        } else {
            own
        };
        (!id.is_empty()).then(|| id.to_string())
    }

    /// The folder new installs of a store's games are offered, as the user
    /// wrote it (`~` unexpanded): `defaultInstallPath`, and with
    /// `iroriInstallPerStore` the store's own folder inside it, named as the
    /// store is (`Epic Games`, `GOG`). Stores whose games Irori doesn't
    /// install, and added games, keep `defaultInstallPath`.
    pub fn install_folder(&self, runner: Runner) -> String {
        self.install_folder_in(&self.default_install_path, runner)
    }

    /// The folder new installs of a store's games go to inside the library
    /// folder `base`: `base` itself, or with `iroriInstallPerStore` the
    /// store's own folder inside it, as [`Self::install_folder`] does for the
    /// default one.
    pub fn install_folder_in(&self, base: &str, runner: Runner) -> String {
        if !self.install_per_store || base.trim().is_empty() || !installs_per_store(runner) {
            return base.to_string();
        }
        let separator = if base.contains('\\') && !base.contains('/') {
            '\\'
        } else {
            '/'
        };
        let base = base.trim_end_matches(['/', '\\']);
        format!("{base}{separator}{}", runner.store_name())
    }

    /// Every library folder: the default install folder first (unnamed),
    /// then the user's named ones with a folder, each folder once.
    pub fn library_folders(&self) -> Vec<LibraryFolder> {
        let mut out = vec![LibraryFolder {
            name: String::new(),
            path: self.default_install_path.clone(),
        }];
        for folder in &self.library_folders {
            if folder.path.trim().is_empty()
                || out.iter().any(|f| same_folder(&f.path, &folder.path))
            {
                continue;
            }
            let mut folder = folder.clone();
            if folder.name.trim().is_empty() {
                // An unnamed folder would read as the default one.
                folder.name = folder_name(&folder.path);
            }
            out.push(folder);
        }
        out
    }
}

/// A named folder games can be installed into (`iroriLibraryFolders`)
/// beside the default install folder: another drive, an SD card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LibraryFolder {
    /// What the user calls it; empty only for the default install folder.
    pub name: String,
    /// As the user chose it (`~` unexpanded).
    pub path: String,
}

impl LibraryFolder {
    /// Whether this is the default install folder (`defaultInstallPath`).
    pub fn is_default(&self) -> bool {
        self.name.is_empty()
    }
}

/// Whether two folders as the user wrote them are one folder.
pub fn same_folder(a: &str, b: &str) -> bool {
    let normal = |p: &str| {
        crate::paths::expand_home(p.trim())
            .components()
            .collect::<PathBuf>()
    };
    normal(a) == normal(b)
}

/// A folder's own name (`SD card` for `/run/media/me/SD card`), for a
/// library folder the user gave no name.
pub fn folder_name(path: &str) -> String {
    let path = path.trim().trim_end_matches(['/', '\\']);
    path.rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// The library folder holding `install` (an installed game's folder): the
/// one whose folder is the longest start of it.
pub fn library_folder_of(folders: &[LibraryFolder], install: &std::path::Path) -> Option<usize> {
    folders
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.path.trim().is_empty())
        .map(|(i, f)| (i, crate::paths::expand_home(f.path.trim())))
        .filter(|(_, root)| install.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(i, _)| i)
}

/// Whether a store's games get a folder of the store's own with
/// `iroriInstallPerStore`: the stores Irori downloads games from.
fn installs_per_store(runner: Runner) -> bool {
    matches!(
        runner,
        Runner::Legendary | Runner::Gog | Runner::Nile | Runner::Zoom | Runner::Itch | Runner::Xbox
    )
}

/// Shader cache room games get unless the user changes it, in GiB.
pub const DEFAULT_SHADER_CACHE_GIB: u32 = 10;

/// Sizes offered for Big Picture's interface, in percent.
pub const BIG_PICTURE_SCALES: [u32; 5] = [100, 125, 150, 175, 200];

impl UiPrefs {
    /// Big Picture's scale factor, limited to the offered range.
    pub fn big_picture_zoom(&self) -> f32 {
        self.big_picture_scale.clamp(100, 200) as f32 / 100.0
    }

    /// How long [`Self::big_picture_hold_button`] is held to open Big
    /// Picture, limited to the offered range.
    pub fn big_picture_hold(&self) -> std::time::Duration {
        std::time::Duration::from_millis(u64::from(self.big_picture_hold_ms.clamp(300, 3000)))
    }

    /// The accent colour the user picked, if it is a valid `#rrggbb`.
    pub fn accent_rgb(&self) -> Option<u32> {
        let hex = self.accent_color.trim().strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    }
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            theme_name: String::new(),
            system_accent: true,
            wallpaper_colors: false,
            wallpaper_colors_file: String::new(),
            translucency: Translucency::Auto,
            font_size: 16.0,
            library_layout: LibraryLayout::Grid,
            tile_size: 11.0,
            library_drawer_width: 26.0,
            reduce_motion: false,
            middle_click_scroll: false,
            show_store_badges: true,
            cover_categories: false,
            group_copies: true,
            trailer_autoplay: false,
            library_quick_controls: true,
            sidebar_collapsed: false,
            window_width: 1440.0,
            window_height: 900.0,
            window_maximized: false,
            start_maximized: false,
            window_x: None,
            window_y: None,
            window_display: String::new(),
            stores: ShownStores::default(),
            big_picture_in_game_mode: true,
            big_picture_scale: 100,
            big_picture_layout: String::new(),
            big_picture_theme: String::new(),
            big_picture_on_controller: false,
            quick_launch_shortcut: false,
            big_picture_hold_button: HoldButton::Start,
            big_picture_hold_ms: 1000,
            accent_color: String::new(),
            density: Density::Comfortable,
            font_family: String::new(),
            library_background: LibraryBackground::None,
            library_background_picture: String::new(),
            library_background_strength: 70,
            library_background_dim: 20,
            big_picture_fullscreen: true,
            cover_corners: CoverCorners::Rounded,
            cover_caption: CoverCaption::Full,
            cover_hover: CoverHover::Lift,
            cover_fit: CoverFit::Crop,
            cover_shape: CoverShape::Portrait,
            cover_favourite: true,
            library_focus_columns: 0,
            library_focus_cover_size: FocusCoverSize::Medium,
            library_auto_layout: false,
            library_open_details_on_select: true,
            store_badges_always: false,
            store_badges_one_store: false,
            show_age_ratings: true,
            age_rating_board: String::new(),
            hidden_detail_rows: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_presence_uses_the_users_application_else_irori() {
        let mut app = AppSettings::default();
        assert!(!app.discord_rpc, "Rich Presence is opt-in");
        assert_eq!(app.discord_application(), None);
        app.discord_rpc = true;
        assert_eq!(
            app.discord_application().as_deref(),
            Some(DISCORD_APPLICATION_ID).filter(|id| !id.is_empty())
        );
        app.discord_client_id = " 123456789012345678 ".into();
        assert_eq!(
            app.discord_application().as_deref(),
            Some("123456789012345678")
        );
        let game: GameSettings = serde_json::from_str("{}").unwrap();
        assert!(game.discord_presence);
        let json = serde_json::to_value(GameSettings {
            discord_presence: false,
            ..GameSettings::default()
        })
        .unwrap();
        assert_eq!(json["iroriDiscordPresence"], false);
    }

    #[test]
    fn appearance_keys_default_and_round_trip() {
        let ui: UiPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(ui.density, Density::Comfortable);
        assert_eq!(ui.library_background, LibraryBackground::None);
        assert_eq!(ui.accent_rgb(), None);
        assert!(ui.library_quick_controls);
        assert!(!ui.cover_categories);
        assert!(ui.group_copies);
        assert!(!ui.wallpaper_colors);
        assert!(ui.wallpaper_colors_file.is_empty());
        let json = serde_json::to_value(UiPrefs {
            wallpaper_colors: true,
            wallpaper_colors_file: "~/.cache/matugen/irori.json".into(),
            ..UiPrefs::default()
        })
        .unwrap();
        assert_eq!(json["wallpaperColors"], true);
        assert_eq!(json["wallpaperColorsFile"], "~/.cache/matugen/irori.json");
        let ui: UiPrefs = serde_json::from_str(
            r##"{"accentColor":"#3a7BFF","density":"compact","libraryBackground":"game_art","libraryQuickControls":false}"##,
        )
        .unwrap();
        assert!(!ui.library_quick_controls);
        assert!(
            serde_json::from_str::<UiPrefs>(r#"{"coverCategories":true}"#)
                .unwrap()
                .cover_categories
        );
        assert_eq!(ui.accent_rgb(), Some(0x3a7bff));
        assert_eq!(ui.density, Density::Compact);
        assert_eq!(ui.library_background, LibraryBackground::GameArt);
        for bad in ["3a7bff", "#3a7bf", "#3a7bfg", "#3a7bff00"] {
            let ui = UiPrefs {
                accent_color: bad.into(),
                ..UiPrefs::default()
            };
            assert_eq!(ui.accent_rgb(), None, "{bad}");
        }
    }

    #[test]
    fn the_replay_buffer_is_off_and_one_game_may_turn_it_on() {
        let defaults = GameSettings::default();
        assert!(!defaults.replay.enabled);
        let json = serde_json::to_value(&defaults).unwrap();
        assert_eq!(json["iroriReplay"]["seconds"], 60);
        assert_eq!(json["iroriReplay"]["quality"], "very_high");
        // A game's override holds the whole object; missing fields take
        // their defaults.
        let mut overrides = GameSettingsOverrides::default();
        overrides.set(
            "iroriReplay",
            serde_json::json!({"enabled": true, "seconds": 30}),
        );
        let game = overrides.apply(&defaults);
        assert!(game.replay.enabled);
        assert_eq!(game.replay.seconds, 30);
        assert_eq!(game.replay.audio, "desktop");
        let old: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.game.replay, ReplaySettings::default());
    }

    #[test]
    fn ember_keys_load_under_irori_names() {
        let overrides: GameSettingsOverrides = serde_json::from_str(
            r#"{"emberEngine": "duckstation", "embers": 1, "wineVersion": {"name": "w"}}"#,
        )
        .unwrap();
        assert_eq!(overrides.0["iroriEngine"], "duckstation");
        assert!(overrides.0.contains_key("embers"));
        assert!(overrides.0.contains_key("wineVersion"));
        let json = serde_json::to_value(&overrides).unwrap();
        assert!(json.get("iroriEngine").is_some() && json.get("emberEngine").is_none());

        let app: AppSettings =
            serde_json::from_str(r#"{"emberItchIo": true, "emberEngine": "mgba"}"#).unwrap();
        assert!(app.itch_io);
        assert_eq!(serde_json::to_value(&app).unwrap()["iroriEngine"], "mgba");
    }

    #[test]
    fn library_folders_list_the_default_first_and_each_folder_once() {
        let mut app: AppSettings = serde_json::from_str(
            r#"{"defaultInstallPath": "/games", "iroriLibraryFolders": [
                {"name": "SD card", "path": "/run/media/me/SD"},
                {"name": "Again", "path": "/games/"},
                {"name": "", "path": "/mnt/Big Disk/"},
                {"name": "Nowhere", "path": " "}
            ]}"#,
        )
        .unwrap();
        let folders = app.library_folders();
        let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["", "SD card", "Big Disk"]);
        assert!(folders[0].is_default() && !folders[2].is_default());
        assert_eq!(folders[0].path, "/games");

        // A game belongs to the deepest folder holding it.
        app.library_folders.push(LibraryFolder {
            name: "Inner".into(),
            path: "/games/inner".into(),
        });
        let folders = app.library_folders();
        let of = |p: &str| library_folder_of(&folders, std::path::Path::new(p));
        assert_eq!(of("/games/Hades"), Some(0));
        assert_eq!(of("/games/inner/GOG/Hades"), Some(3));
        assert_eq!(of("/run/media/me/SD/Celeste"), Some(1));
        assert_eq!(of("/elsewhere/Celeste"), None);
        // `/games2` is not inside `/games`.
        assert_eq!(of("/games2/Hades"), None);

        // Each folder gets the stores' own folders like the default one.
        app.install_per_store = true;
        assert_eq!(
            app.install_folder_in("/run/media/me/SD", Runner::Gog),
            "/run/media/me/SD/GOG"
        );
        assert_eq!(
            app.install_folder_in("/run/media/me/SD", Runner::Sideload),
            "/run/media/me/SD"
        );
        // Older settings without the list read as before.
        let old: AppSettings = serde_json::from_str(r#"{"defaultInstallPath": "/g"}"#).unwrap();
        assert_eq!(old.library_folders().len(), 1);
    }

    #[test]
    fn stores_can_install_in_folders_of_their_own() {
        let mut app: AppSettings =
            serde_json::from_str(r#"{"defaultInstallPath": "~/Games/Irori/"}"#).unwrap();
        assert!(!app.install_per_store, "off unless the user turns it on");
        assert_eq!(app.install_folder(Runner::Gog), "~/Games/Irori/");
        app.install_per_store = true;
        assert_eq!(app.install_folder(Runner::Gog), "~/Games/Irori/GOG");
        assert_eq!(
            app.install_folder(Runner::Legendary),
            "~/Games/Irori/Epic Games"
        );
        assert_eq!(app.install_folder(Runner::Itch), "~/Games/Irori/itch.io");
        // Games Irori doesn't download keep the folder itself.
        assert_eq!(app.install_folder(Runner::Sideload), "~/Games/Irori/");
        assert_eq!(app.install_folder(Runner::Steam), "~/Games/Irori/");
        app.default_install_path = r"D:\Games".into();
        assert_eq!(app.install_folder(Runner::Nile), r"D:\Games\Amazon Games");
        app.default_install_path = String::new();
        assert_eq!(app.install_folder(Runner::Gog), "");
        let json = serde_json::to_value(&app).unwrap();
        assert_eq!(json["iroriInstallPerStore"], true);
    }

    #[test]
    fn gpu_and_frame_generation_keys() {
        let gs: GameSettings = serde_json::from_str(
            r#"{"iroriGpu": "0000:03:00.0", "iroriSmoothMotion": true,
                "iroriLsfg": {"enabled": true, "multiplier": 3}}"#,
        )
        .unwrap();
        assert_eq!(gs.gpu, "0000:03:00.0");
        assert!(gs.smooth_motion && !gs.nvidia_reflex);
        assert_eq!(gs.lsfg.multiplier, 3);
        assert_eq!(gs.lsfg.flow_scale, 100, "missing keys keep their defaults");
        assert_eq!(
            gs.lsfg.active_layer(),
            Some(FrameGenLayer::LsfgVk),
            "settings from before MAKO keep lsfg-vk"
        );
        let mako: LsfgSettings =
            serde_json::from_str(r#"{"enabled": true, "layer": "mako"}"#).unwrap();
        assert_eq!(mako.active_layer(), Some(FrameGenLayer::Mako));
        let newer: LsfgSettings =
            serde_json::from_str(r#"{"enabled": true, "layer": "something-new"}"#).unwrap();
        assert_eq!(newer.active_layer(), None);
        assert_eq!(LsfgSettings::default().active_layer(), None);
        let json = serde_json::to_value(GameSettings::default()).unwrap();
        assert_eq!(json["iroriGpu"], "");
        assert_eq!(json["iroriWindowsGpu"], "");
        let gs: GameSettings =
            serde_json::from_str(r#"{"iroriWindowsGpu": "highPerformance"}"#).unwrap();
        assert_eq!(gs.windows_gpu, "highPerformance");
        assert_eq!(json["iroriHideGpu"], "");
        let gs: GameSettings =
            serde_json::from_str(r#"{"iroriHideGpu": "PCI\\VEN_10DE&DEV_2484\\4&1"}"#).unwrap();
        assert_eq!(gs.hide_gpu, r"PCI\VEN_10DE&DEV_2484\4&1");
        assert_eq!(json["iroriLsfg"]["multiplier"], 2);
        assert_eq!(json["iroriLsfg"]["performanceMode"], false);
    }

    #[test]
    fn app_update_check_is_on_unless_turned_off() {
        let app: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(app.check_for_app_updates);
        let app: AppSettings =
            serde_json::from_str(r#"{"iroriCheckForAppUpdates": false}"#).unwrap();
        assert!(!app.check_for_app_updates);
    }

    #[test]
    fn network_policy_reads_and_defaults() {
        let app: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(app.network_mode, crate::net::NetworkMode::Auto);
        assert_eq!(app.network_timeout, 0);
        let app: AppSettings =
            serde_json::from_str(r#"{"iroriNetworkMode": "offline", "iroriNetworkTimeout": 180}"#)
                .unwrap();
        assert_eq!(app.network_mode, crate::net::NetworkMode::Offline);
        assert_eq!(app.network_timeout, 180);
    }

    #[test]
    fn save_snapshots_are_kept_unless_turned_off() {
        let app: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(app.save_snapshots, DEFAULT_SAVE_SNAPSHOTS);
        let app: AppSettings = serde_json::from_str(r#"{"iroriSaveSnapshots": 0}"#).unwrap();
        assert_eq!(app.save_snapshots, 0);
    }

    #[test]
    fn deals_show_unless_turned_off() {
        let app: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(app.deals);
        let app: AppSettings = serde_json::from_str(r#"{"iroriDeals": false}"#).unwrap();
        assert!(!app.deals);
    }

    #[test]
    fn ui_prefs_from_older_files_keep_defaults() {
        let ui: UiPrefs = serde_json::from_str(r#"{"fontSize":18}"#).unwrap();
        assert_eq!(ui.stores, ShownStores::default());
        assert!(ui.big_picture_in_game_mode);
        assert_eq!(ui.big_picture_zoom(), 1.0);
        let ui: UiPrefs = serde_json::from_str(
            r#"{"stores":{"epic":false},"bigPictureScale":400,"bigPictureInGameMode":false}"#,
        )
        .unwrap();
        assert!(!ui.stores.shows(Runner::Legendary));
        assert!(ui.stores.shows(Runner::Gog));
        assert!(ui.stores.shows(Runner::Sideload));
        let mut stores = ui.stores;
        stores.set(Runner::Legendary, true);
        stores.set(Runner::Sideload, false);
        assert_eq!(stores, ShownStores::default());
        assert_eq!(ui.big_picture_zoom(), 2.0);
        assert!(!ui.big_picture_in_game_mode);
        let ui: UiPrefs = serde_json::from_str(
            r#"{"bigPictureLayout":"carousel","bigPictureTheme":"user:x","bigPictureOnController":true}"#,
        )
        .unwrap();
        assert_eq!(ui.big_picture_layout, "carousel");
        assert_eq!(ui.big_picture_theme, "user:x");
        assert!(ui.big_picture_on_controller);
        assert!(!UiPrefs::default().big_picture_on_controller);
        assert!(ui.big_picture_fullscreen, "fullscreen unless turned off");
        let ui: UiPrefs = serde_json::from_str(r#"{"bigPictureFullscreen":false}"#).unwrap();
        assert!(!ui.big_picture_fullscreen);
    }

    #[test]
    fn a_click_opens_the_drawer_unless_turned_off() {
        assert!(UiPrefs::default().library_open_details_on_select);
        let ui: UiPrefs = serde_json::from_str(r#"{"libraryOpenDetailsOnSelect":false}"#).unwrap();
        assert!(!ui.library_open_details_on_select);
    }

    #[test]
    fn focus_covers_are_medium_unless_chosen() {
        assert_eq!(
            UiPrefs::default().library_focus_cover_size,
            FocusCoverSize::Medium
        );
        let ui: UiPrefs = serde_json::from_str(r#"{"libraryFocusCoverSize":"large"}"#).unwrap();
        assert_eq!(ui.library_focus_cover_size, FocusCoverSize::Large);
        // A size a newer Irori writes doesn't lose the other settings.
        let ui: UiPrefs =
            serde_json::from_str(r#"{"libraryFocusCoverSize":"huge","tileSize":9.0}"#).unwrap();
        assert_eq!(ui.library_focus_cover_size, FocusCoverSize::Medium);
        assert_eq!(ui.tile_size, 9.0);
    }

    #[test]
    fn the_layout_follows_the_window_only_when_asked() {
        assert!(!UiPrefs::default().library_auto_layout);
        let ui: UiPrefs = serde_json::from_str(r#"{"libraryAutoLayout":true}"#).unwrap();
        assert!(ui.library_auto_layout);
    }

    #[test]
    fn covers_are_boxes_unless_another_shape_is_chosen() {
        assert_eq!(UiPrefs::default().cover_shape, CoverShape::Portrait);
        let ui: UiPrefs = serde_json::from_str(r#"{"coverShape":"banner"}"#).unwrap();
        assert_eq!(ui.cover_shape, CoverShape::Banner);
        assert!(CoverShape::Banner.ratio() < 1.0 && CoverShape::Square.ratio() == 1.0);
        let ui: UiPrefs =
            serde_json::from_str(r#"{"coverShape":"hexagon","tileSize":9.0}"#).unwrap();
        assert_eq!(ui.cover_shape, CoverShape::Portrait);
        assert_eq!(ui.tile_size, 9.0);
    }

    #[test]
    fn covers_are_cropped_unless_drawn_whole() {
        assert_eq!(UiPrefs::default().cover_fit, CoverFit::Crop);
        let ui: UiPrefs = serde_json::from_str(r#"{"coverFit":"whole"}"#).unwrap();
        assert_eq!(ui.cover_fit, CoverFit::Whole);
    }

    #[test]
    fn store_badges_show_on_hover_and_not_for_one_store_by_default() {
        let ui = UiPrefs::default();
        assert!(ui.show_store_badges && !ui.store_badges_always && !ui.store_badges_one_store);
        let ui: UiPrefs =
            serde_json::from_str(r#"{"storeBadgesAlways":true,"storeBadgesOneStore":true}"#)
                .unwrap();
        assert!(ui.store_badges_always && ui.store_badges_one_store);
    }

    #[test]
    fn cover_options_read_their_keys_and_default_to_todays_look() {
        let ui = UiPrefs::default();
        assert_eq!(ui.cover_corners, CoverCorners::Rounded);
        assert_eq!(ui.cover_caption, CoverCaption::Full);
        assert_eq!(ui.cover_hover, CoverHover::Lift);
        assert!(ui.cover_favourite);
        let ui: UiPrefs = serde_json::from_str(
            r#"{"coverCorners":"square","coverCaption":"title","coverHover":"outline","coverFavourite":false}"#,
        )
        .unwrap();
        assert_eq!(ui.cover_corners, CoverCorners::Square);
        assert_eq!(ui.cover_caption, CoverCaption::Title);
        assert_eq!(ui.cover_hover, CoverHover::Outline);
        assert!(!ui.cover_favourite);
    }

    #[test]
    fn holding_a_button_opens_big_picture_after_a_second_by_default() {
        let ui = UiPrefs::default();
        assert_eq!(ui.big_picture_hold_button, HoldButton::Start);
        assert_eq!(ui.big_picture_hold(), std::time::Duration::from_secs(1));
        let ui: UiPrefs =
            serde_json::from_str(r#"{"bigPictureHoldButton":"select","bigPictureHoldMs":20}"#)
                .unwrap();
        assert_eq!(ui.big_picture_hold_button, HoldButton::Select);
        assert_eq!(ui.big_picture_hold(), std::time::Duration::from_millis(300));
        let ui: UiPrefs = serde_json::from_str(
            r#"{"bigPictureHoldButton":"guide-and-start","bigPictureHoldMs":90000}"#,
        )
        .unwrap();
        assert_eq!(ui.big_picture_hold_button, HoldButton::Start, "unknown");
        assert_eq!(ui.big_picture_hold(), std::time::Duration::from_secs(3));
        let json = serde_json::to_value(UiPrefs {
            big_picture_hold_button: HoldButton::Off,
            ..UiPrefs::default()
        })
        .unwrap();
        assert_eq!(json["bigPictureHoldButton"], "off");
        assert_eq!(json["bigPictureHoldMs"], 1000);
        let back: UiPrefs = serde_json::from_value(json).unwrap();
        assert_eq!(back.big_picture_hold_button, HoldButton::Off);
    }

    #[test]
    fn the_frame_limiter_reads_leniently() {
        let gs: GameSettings =
            serde_json::from_str(r#"{"iroriFpsLimiter":"mangohud","iroriMangohudHidden":true}"#)
                .unwrap();
        assert_eq!(gs.fps_limiter, FpsLimiter::MangoHud);
        assert!(gs.mangohud_hidden);
        let gs: GameSettings = serde_json::from_str(r#"{"iroriFpsLimiter":"vsync"}"#).unwrap();
        assert_eq!(gs.fps_limiter, FpsLimiter::Auto, "unknown");
        let json = serde_json::to_value(GameSettings::default()).unwrap();
        assert_eq!(json["iroriFpsLimiter"], "auto");
        let back = serde_json::to_value(GameSettings {
            fps_limiter: FpsLimiter::MangoHud,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(back["iroriFpsLimiter"], "mangohud");
    }

    #[test]
    fn overlay_options_default_on_and_read_leniently() {
        // Settings written before the options existed keep the overlay as it
        // was: on or off, chords and the notice on, a 5% step, centred.
        let old: OverlayPrefs = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(
            old,
            OverlayPrefs {
                enabled: true,
                ..OverlayPrefs::default()
            }
        );
        assert!(old.screenshot_chord && old.volume_chords && old.level_notice && old.back_start);
        assert_eq!(old.volume_step(), 5);
        let set: OverlayPrefs = serde_json::from_str(
            r#"{"side":"right","volumeStep":0,"volumeChords":false,"levelNotice":false}"#,
        )
        .unwrap();
        assert_eq!(set.side, OverlaySide::Right);
        assert_eq!(set.volume_step(), 1, "at least 1%");
        assert!(!set.volume_chords && !set.level_notice);
        let newer: OverlayPrefs =
            serde_json::from_str(r#"{"side":"top","volumeStep":90}"#).unwrap();
        assert_eq!(newer.side, OverlaySide::Center, "unknown side");
        assert_eq!(newer.volume_step(), 25);
        let json = serde_json::to_value(OverlayPrefs::default()).unwrap();
        assert_eq!(json["side"], "center");
        assert_eq!(json["volumeStep"], 5);
        assert_eq!(json["backStart"], true);
    }

    #[test]
    fn overrides_layer_on_defaults() {
        let defaults = GameSettings::default();
        let mut changed = defaults.clone();
        changed.show_fps = true;
        changed.launcher_args = "-dx11".into();
        let o = GameSettingsOverrides::diff(&defaults, &changed);
        assert_eq!(o.0.len(), 2);
        assert!(o.is_overridden("showFps"));
        assert_eq!(o.apply(&defaults), changed);
    }

    #[test]
    fn game_variables_merge_over_the_defaults() {
        let var = |k: &str, v: &str| EnvVar {
            key: k.into(),
            value: v.into(),
        };
        let mut defaults = GameSettings {
            environment_options: vec![var("FOO", "1"), var("BAR", "2")],
            ..Default::default()
        };
        let mut game = GameSettingsOverrides::default();
        game.set(
            "enviromentOptions",
            serde_json::json!([{"key": "PROTON_USE_XALIA", "value": "0"}]),
        );
        assert_eq!(
            game.apply(&defaults).environment_options,
            vec![
                var("FOO", "1"),
                var("BAR", "2"),
                var("PROTON_USE_XALIA", "0")
            ]
        );

        // A game that copied the whole list before reads the same, and a
        // variable added to the defaults since reaches it too.
        game.set(
            "enviromentOptions",
            serde_json::json!([
                {"key": "FOO", "value": "1"},
                {"key": "BAR", "value": "9"},
            ]),
        );
        defaults.environment_options.push(var("NEW", "x"));
        assert_eq!(
            game.apply(&defaults).environment_options,
            vec![var("FOO", "1"), var("BAR", "9"), var("NEW", "x")]
        );

        // Taking a default variable out for one game stores a removal.
        let mut wanted = defaults.clone();
        wanted.environment_options = vec![var("BAR", "2"), var("OWN", "y")];
        let o = GameSettingsOverrides::diff(&defaults, &wanted);
        assert_eq!(
            o.0["enviromentOptions"],
            serde_json::json!([
                {"key": "FOO", "iroriRemoved": true},
                {"key": "NEW", "iroriRemoved": true},
                {"key": "OWN", "value": "y"},
            ])
        );
        assert_eq!(o.apply(&defaults), wanted);
    }

    #[test]
    fn engine_options_merge_option_by_option() {
        let mut defaults = GameSettings::default();
        let options = |pairs: &[(&str, Value)]| -> Map<String, Value> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };
        defaults.engine_options.insert(
            "retroarch".into(),
            options(&[
                ("vsync", Value::Bool(true)),
                ("show_fps", Value::Bool(true)),
            ]),
        );
        let mut game = GameSettingsOverrides::default();
        game.set(
            "iroriEngineOptions",
            serde_json::json!({"retroarch": {"vsync": false}, "dolphin": {"fullscreen": false}}),
        );
        let effective = game.apply(&defaults);
        let ra = &effective.engine_options["retroarch"];
        assert_eq!(ra["vsync"], Value::Bool(false));
        assert_eq!(ra["show_fps"], Value::Bool(true));
        assert_eq!(
            effective.engine_options["dolphin"]["fullscreen"],
            Value::Bool(false)
        );
    }

    #[test]
    fn heroic_global_config_parses() {
        let json = r#"{
            "autoInstallDxvk": true, "enviromentOptions": [{"key":"A","value":"1"}],
            "wineVersion": {"bin":"/usr/bin/wine","name":"wine","type":"wine","wineserver":"/usr/bin/wineserver"},
            "steamRuntime": false, "DXVKFpsCap": "60", "disableUMU": false,
            "defaultInstallPath": "/games", "libraryTopSection": "disabled", "maxWorkers": 0
        }"#;
        let s: AppSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.game.environment_options[0].key, "A");
        assert_eq!(s.game.steam_runtime, None);
        assert_eq!(s.game.dxvk_fps_cap, "60");
        assert_eq!(s.default_install_path, "/games");
        assert_eq!(s.library_top_section, LibraryTopSection::Disabled);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["steamRuntime"], Value::Bool(false));
        assert_eq!(back["enviromentOptions"][0]["value"], "1");
    }
}
