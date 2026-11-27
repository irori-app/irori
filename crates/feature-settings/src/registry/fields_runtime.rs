use super::options::{
    FPS_LIMIT_HELP, FPS_LIMITER, LSFG_FLOW_SCALE, LSFG_LAYER, LSFG_MULTIPLIER, NEEDS_DXVK,
    NEEDS_FPS_LIMIT, NEEDS_FSR, NEEDS_GAMESCOPE, NEEDS_LIMITER, NEEDS_LSFG, NEEDS_MANGOHUD,
    NEEDS_NVAPI, NEEDS_REPLAY, NEEDS_UPSCALING, NEEDS_VKD3D, NEEDS_WAYLAND, REPLAY_AUDIO,
    REPLAY_CAPTURE, REPLAY_CODEC, REPLAY_FPS, REPLAY_QUALITY, REPLAY_RESOLUTION, REPLAY_SECONDS,
    SHARPNESS, UPSCALE_METHOD, WINDOW_TYPE, WINDOWS_GPU,
};
use super::types::{
    CustomKind,
    FieldKind::{Choice, Custom, Panel, Text, Toggle},
    FieldSection::{Gpu, Performance, Wine},
    FieldSpec,
    Label::{Key, Verbatim},
    Need, PanelKind, TextKind, game, game_only,
};
use irori_runtime::DxTool;

pub(super) const FIELDS: &[FieldSpec] = &[
    // ---- Wine & Proton ----
    game_only(
        "iroriNoWine",
        "settings.field.no_wine.label",
        Wine,
        "settings.group.compatibility_layer",
        Toggle,
    )
    .help("settings.field.no_wine.help")
    .keywords(&["wine", "proton", "native", "openjk", "port", "replacement"]),
    game(
        "wineVersion",
        "settings.field.wine_version.label",
        Wine,
        "settings.group.compatibility_layer",
        Custom(CustomKind::WineVersion),
    )
    .tooltip("settings.field.wine_version.tip")
    .keywords(&["wine", "proton", "ge", "version", "runner"]),
    game(
        "winePrefix",
        "settings.field.wine_prefix.label",
        Wine,
        "settings.group.compatibility_layer",
        Text(TextKind::Directory),
    )
    .help("settings.field.wine_prefix.help")
    .tooltip("settings.field.wine_prefix.tip")
    .placeholder(Key("settings.field.wine_prefix.placeholder"))
    .keywords(&["prefix", "wineprefix", "path", "bottle"]),
    game(
        "wineCrossoverBottle",
        "settings.field.wine_crossover_bottle.label",
        Wine,
        "settings.group.compatibility_layer",
        Text(TextKind::Plain),
    )
    .tag(Verbatim("CrossOver"))
    .help("settings.field.wine_crossover_bottle.help")
    .tooltip("settings.field.wine_crossover_bottle.tip")
    .keywords(&["crossover", "bottle", "prefix", "mac"]),
    game(
        "disableUMU",
        "settings.field.disable_umu.label",
        Wine,
        "settings.group.compatibility_layer",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.disable_umu.help")
    .tooltip("settings.field.disable_umu.tip")
    .keywords(&["umu", "proton"]),
    game_only(
        "iroriUmuId",
        "settings.field.umu_id.label",
        Wine,
        "settings.group.compatibility_layer",
        Text(TextKind::Plain),
    )
    .tag(Verbatim("umu"))
    .help("settings.field.umu_id.help")
    .tooltip("settings.field.umu_id.tip")
    .placeholder(Verbatim("umu-1091500"))
    .keywords(&["umu", "gameid", "protonfixes", "proton", "fixes"]),
    game_only(
        "panel.umu",
        "settings.field.panel_umu.label",
        Wine,
        "settings.group.compatibility_layer",
        Panel(PanelKind::Umu),
    )
    .tooltip("settings.field.panel_umu.tip")
    .keywords(&["umu", "gameid", "protonfixes", "database"]),
    game(
        "iroriProtonPreferSdl",
        "settings.field.proton_prefer_sdl.label",
        Wine,
        "settings.group.compatibility_layer",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.proton_prefer_sdl.help")
    .tooltip("settings.field.proton_prefer_sdl.tip")
    .keywords(&[
        "sdl",
        "controller",
        "gamepad",
        "joystick",
        "hidraw",
        "evdev",
        "proton_prefer_sdl",
    ]),
    game(
        "iroriDllOverrides",
        "settings.field.dll_overrides.label",
        Wine,
        "settings.group.compatibility_layer",
        Custom(CustomKind::DllOverrides),
    )
    .help("settings.field.dll_overrides.help")
    .tooltip("settings.field.dll_overrides.tip")
    .keywords(&[
        "dll",
        "overrides",
        "winedlloverrides",
        "native",
        "builtin",
        "disabled",
        "lutris",
    ]),
    game(
        "enableEsync",
        "settings.field.enable_esync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .help("settings.field.enable_esync.help")
    .tooltip("settings.field.enable_esync.tip")
    .keywords(&["esync", "eventfd", "sync", "performance"]),
    game(
        "enableFsync",
        "settings.field.enable_fsync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .help("settings.field.enable_fsync.help")
    .tooltip("settings.field.enable_fsync.tip")
    .keywords(&["fsync", "futex", "sync", "performance"]),
    game(
        "iroriNtsync",
        "settings.field.ntsync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .help("settings.field.ntsync.help")
    .tooltip("settings.field.ntsync.tip")
    .keywords(&["ntsync", "kernel", "sync", "performance"]),
    game(
        "enableMsync",
        "settings.field.enable_msync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .tooltip("settings.field.enable_msync.tip")
    .tag(Verbatim("macOS"))
    .keywords(&["msync", "sync", "mac"]),
    game(
        "iroriRetinaMode",
        "settings.field.retina_mode.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Verbatim("macOS"))
    .help("settings.field.retina_mode.help")
    .tooltip("settings.field.retina_mode.tip")
    .keywords(&["retina", "hidpi", "resolution", "scaling", "mac"]),
    game(
        "iroriMetalFxDlss",
        "settings.field.metal_fx_dlss.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Verbatim("macOS"))
    .help("settings.field.metal_fx_dlss.help")
    .keywords(&[
        "dlss",
        "metalfx",
        "upscaling",
        "nvidia",
        "d3dmetal",
        "dxmt",
        "game porting toolkit",
        "gptk",
        "crossover",
        "mac",
    ]),
    game(
        "enableWineWayland",
        "settings.field.enable_wine_wayland.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.enable_wine_wayland.help")
    .tooltip("settings.field.enable_wine_wayland.tip")
    .keywords(&["wayland", "xwayland", "display"]),
    game(
        "enableHDR",
        "settings.field.enable_hdr.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tooltip("settings.field.enable_hdr.tip")
    .keywords(&["hdr", "high dynamic range", "wayland"])
    .requires(NEEDS_WAYLAND),
    game(
        "enableFSR",
        "settings.field.enable_fsr.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tooltip("settings.field.enable_fsr.tip")
    .keywords(&["fsr", "upscale", "amd", "fidelityfx"]),
    game(
        "maxSharpness",
        "settings.field.max_sharpness.label",
        Wine,
        "settings.group.display",
        Choice(SHARPNESS),
    )
    .tooltip("settings.field.max_sharpness.tip")
    .keywords(&["fsr", "sharpness", "strength"])
    .requires(NEEDS_FSR),
    game(
        "enableWoW64",
        "settings.field.enable_wow64.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.enable_wow64.help")
    .tooltip("settings.field.enable_wow64.tip")
    .keywords(&["wow64", "32-bit", "multilib"]),
    game(
        "autoInstallDxvk",
        "settings.field.auto_install_dxvk.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_dxvk.help")
    .tooltip("settings.field.auto_install_dxvk.tip")
    .keywords(&["dxvk", "directx", "d3d11", "vulkan"]),
    game(
        "autoInstallVkd3d",
        "settings.field.auto_install_vkd3d.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_vkd3d.help")
    .tooltip("settings.field.auto_install_vkd3d.tip")
    .keywords(&["vkd3d", "directx 12", "d3d12", "vulkan"])
    .requires(NEEDS_DXVK),
    game(
        "autoInstallDxvkNvapi",
        "settings.field.auto_install_dxvk_nvapi.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_dxvk_nvapi.help")
    .tooltip("settings.field.auto_install_dxvk_nvapi.tip")
    .keywords(&["nvapi", "dlss", "nvidia"])
    .requires(NEEDS_DXVK),
    game(
        "iroriDxvkVersion",
        "settings.field.dxvk_version.label",
        Wine,
        "settings.group.graphics_translation",
        Custom(CustomKind::ToolVersion(DxTool::Dxvk)),
    )
    .help("settings.field.tool_version.help")
    .tooltip("settings.field.dxvk_version.tip")
    .keywords(&["dxvk", "version", "release"])
    .requires(NEEDS_DXVK),
    game(
        "iroriVkd3dVersion",
        "settings.field.vkd3d_version.label",
        Wine,
        "settings.group.graphics_translation",
        Custom(CustomKind::ToolVersion(DxTool::Vkd3d)),
    )
    .help("settings.field.tool_version.help")
    .tooltip("settings.field.vkd3d_version.tip")
    .keywords(&["vkd3d", "version", "release"])
    .requires(NEEDS_VKD3D),
    game(
        "iroriDescriptorHeap",
        "settings.field.descriptor_heap.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.descriptor_heap.help")
    .tooltip("settings.field.descriptor_heap.tip")
    .keywords(&[
        "descriptor heap",
        "descriptor_heap",
        "vkd3d_config",
        "proton_vkd3d_heap",
        "d3d12",
    ]),
    game(
        "preferSystemLibs",
        "settings.field.prefer_system_libs.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.prefer_system_libs.help")
    .tooltip("settings.field.prefer_system_libs.tip")
    .keywords(&["libraries", "ld_library_path", "system"]),
    game(
        "eacRuntime",
        "settings.field.eac_runtime.label",
        Wine,
        "settings.group.anti_cheat",
        Toggle,
    )
    .tooltip("settings.field.eac_runtime.tip")
    .keywords(&["eac", "easyanticheat", "anticheat", "runtime"]),
    game(
        "battlEyeRuntime",
        "settings.field.battleye_runtime.label",
        Wine,
        "settings.group.anti_cheat",
        Toggle,
    )
    .tooltip("settings.field.battleye_runtime.tip")
    .keywords(&["battleye", "anticheat", "runtime"]),
    // ---- Performance ----
    game_only(
        "panel.performance-hud",
        "settings.field.panel_performance_hud.label",
        Performance,
        "settings.group.performance_hud",
        Panel(PanelKind::PerformanceHud),
    )
    .tooltip("settings.field.panel_performance_hud.tip")
    .keywords(&[
        "hud",
        "mangohud",
        "metal",
        "overlay",
        "fps",
        "frametime",
        "performance",
        "in-game stats",
        "stats",
    ]),
    game(
        "panel.optiscaler",
        "settings.field.panel_optiscaler.label",
        Performance,
        "settings.group.optiscaler",
        Panel(PanelKind::Optiscaler),
    )
    .tooltip("settings.field.panel_optiscaler.tip")
    .keywords(&[
        "optiscaler",
        "upscaling",
        "dlss",
        "xess",
        "fsr",
        "fakenvapi",
        "optipatcher",
        "proxy",
        "shortcut",
    ]),
    game(
        "useGameMode",
        "settings.field.use_game_mode.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .help("settings.field.use_game_mode.help")
    .tooltip("settings.field.use_game_mode.tip")
    .keywords(&["gamemode", "feral", "performance", "cpu governor"]),
    game(
        "showMangohud",
        "settings.field.show_mangohud.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .help("settings.field.show_mangohud.help")
    .tooltip("settings.field.show_mangohud.tip")
    .keywords(&["mangohud", "overlay", "fps", "hud"]),
    game(
        "iroriMangohudHidden",
        "settings.field.mangohud_hidden.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .help("settings.field.mangohud_hidden.help")
    .keywords(&["mangohud", "hidden", "overlay", "no_display", "hud"])
    .requires(NEEDS_MANGOHUD),
    game(
        "showFps",
        "settings.field.show_fps.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .tooltip("settings.field.show_fps.tip")
    .keywords(&["fps", "hud", "dxvk", "counter"]),
    game(
        "panel.replay",
        "settings.field.panel_replay.label",
        Performance,
        "settings.group.replay_buffer",
        Panel(PanelKind::Replay),
    )
    .tooltip("settings.field.panel_replay.tip")
    .keywords(&[
        "replay",
        "clip",
        "record",
        "video",
        "capture",
        "gpu-screen-recorder",
        "shadowplay",
    ]),
    game(
        "iroriReplay.enabled",
        "settings.field.replay_enabled.label",
        Performance,
        "settings.group.replay_buffer",
        Toggle,
    )
    .help("settings.field.replay_enabled.help")
    .tooltip("settings.field.replay_enabled.tip")
    .keywords(&["replay", "clip", "record", "video", "instant replay"]),
    game(
        "iroriReplay.seconds",
        "settings.field.replay_seconds.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_SECONDS),
    )
    .tooltip("settings.field.replay_seconds.tip")
    .keywords(&["replay", "clip", "length", "duration", "seconds"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.quality",
        "settings.field.replay_quality.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_QUALITY),
    )
    .help("settings.field.replay_quality.help")
    .tooltip("settings.field.replay_quality.tip")
    .keywords(&["replay", "clip", "quality", "bitrate"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.audio",
        "settings.field.replay_audio.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_AUDIO),
    )
    .tooltip("settings.field.replay_audio.tip")
    .keywords(&["replay", "clip", "audio", "sound", "microphone"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.capture",
        "settings.field.replay_capture.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_CAPTURE),
    )
    .help("settings.field.replay_capture.help")
    .tooltip("settings.field.replay_capture.tip")
    .keywords(&["replay", "clip", "monitor", "screen", "portal", "wayland"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.folder",
        "settings.field.replay_folder.label",
        Performance,
        "settings.group.replay_buffer",
        Text(TextKind::Directory),
    )
    .tooltip("settings.field.replay_folder.tip")
    .placeholder(Key("settings.field.replay_folder.placeholder"))
    .keywords(&["replay", "clip", "folder", "videos", "output"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.fps",
        "settings.field.replay_fps.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_FPS),
    )
    .keywords(&["replay", "clip", "fps", "frame rate", "framerate"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.codec",
        "settings.field.replay_codec.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_CODEC),
    )
    .help("settings.field.replay_codec.help")
    .keywords(&["replay", "clip", "codec", "h264", "hevc", "av1", "encoder"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.resolution",
        "settings.field.replay_resolution.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_RESOLUTION),
    )
    .help("settings.field.replay_resolution.help")
    .keywords(&["replay", "clip", "resolution", "size", "1080p", "scale"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.monitor",
        "settings.field.replay_monitor.label",
        Performance,
        "settings.group.replay_buffer",
        Text(TextKind::Plain),
    )
    .placeholder(Key("settings.field.replay_monitor.placeholder"))
    .help("settings.field.replay_monitor.help")
    .keywords(&[
        "replay",
        "clip",
        "monitor",
        "display",
        "screen",
        "connector",
    ])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.restart",
        "settings.field.replay_restart.label",
        Performance,
        "settings.group.replay_buffer",
        Toggle,
    )
    .help("settings.field.replay_restart.help")
    .keywords(&["replay", "clip", "restart", "crash", "recorder"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.journal",
        "settings.field.replay_journal.label",
        Performance,
        "settings.group.replay_buffer",
        Toggle,
    )
    .help("settings.field.replay_journal.help")
    .keywords(&["replay", "clip", "journal", "note", "diary"])
    .requires(NEEDS_REPLAY),
    game(
        "enableDXVKFpsLimit",
        "settings.field.enable_dxvk_fps_limit.label",
        Performance,
        "settings.group.frame_rate",
        Toggle,
    )
    .help(FPS_LIMIT_HELP)
    .tooltip("settings.field.enable_dxvk_fps_limit.tip")
    .keywords(&[
        "fps",
        "limit",
        "cap",
        "dxvk",
        "frame rate",
        "mangohud",
        "fps_limit",
    ]),
    game(
        "DXVKFpsCap",
        "settings.field.dxvk_fps_cap.label",
        Performance,
        "settings.group.frame_rate",
        Text(TextKind::Digits { max_len: 4 }),
    )
    .tooltip("settings.field.dxvk_fps_cap.tip")
    .placeholder(Verbatim("60"))
    .keywords(&["fps", "limit", "cap"])
    .requires(NEEDS_FPS_LIMIT),
    // Linux only (`supported_on_host`): DXVK is the only limiter elsewhere.
    game(
        "iroriFpsLimiter",
        "settings.field.fps_limiter.label",
        Performance,
        "settings.group.frame_rate",
        Choice(FPS_LIMITER),
    )
    .help("settings.field.fps_limiter.help")
    .keywords(&["fps", "limit", "limiter", "gamescope", "mangohud", "dxvk"])
    .requires(NEEDS_FPS_LIMIT),
    game(
        "gamescope.iroriEnabled",
        "settings.field.gamescope_enabled.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .help("settings.field.gamescope_enabled.help")
    .tooltip("settings.field.gamescope_enabled.tip")
    .keywords(&["gamescope", "compositor", "presenter", "window"]),
    game(
        "gamescope.enableUpscaling",
        "settings.field.gamescope_enable_upscaling.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .help("settings.field.gamescope_enable_upscaling.help")
    .tooltip("settings.field.gamescope_enable_upscaling.tip")
    .keywords(&["gamescope", "upscale", "resolution"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleMethod",
        "settings.field.gamescope_upscale_method.label",
        Performance,
        "settings.group.gamescope",
        Choice(UPSCALE_METHOD),
    )
    .tooltip("settings.field.gamescope_upscale_method.tip")
    .keywords(&["gamescope", "fsr", "nis", "integer"])
    .requires(NEEDS_UPSCALING),
    game(
        "gamescope.windowType",
        "settings.field.gamescope_window_type.label",
        Performance,
        "settings.group.gamescope",
        Choice(WINDOW_TYPE),
    )
    .tooltip("settings.field.gamescope_window_type.tip")
    .keywords(&["gamescope", "fullscreen", "borderless", "windowed"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.gameWidth",
        "settings.field.gamescope_game_width.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .tooltip("settings.field.gamescope_game_width.tip")
    .placeholder(Verbatim("1280"))
    .keywords(&["gamescope", "resolution", "width"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.gameHeight",
        "settings.field.gamescope_game_height.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .tooltip("settings.field.gamescope_game_height.tip")
    .placeholder(Verbatim("720"))
    .keywords(&["gamescope", "resolution", "height"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleWidth",
        "settings.field.gamescope_upscale_width.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .tooltip("settings.field.gamescope_upscale_width.tip")
    .placeholder(Verbatim("2560"))
    .keywords(&["gamescope", "resolution", "width"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleHeight",
        "settings.field.gamescope_upscale_height.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .tooltip("settings.field.gamescope_upscale_height.tip")
    .placeholder(Verbatim("1440"))
    .keywords(&["gamescope", "resolution", "height"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.enableLimiter",
        "settings.field.gamescope_enable_limiter.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .tooltip("settings.field.gamescope_enable_limiter.tip")
    .keywords(&["gamescope", "fps", "limit"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.fpsLimiter",
        "settings.field.gamescope_fps_limiter.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 3 }),
    )
    .tooltip("settings.field.gamescope_fps_limiter.tip")
    .placeholder(Verbatim("60"))
    .keywords(&["gamescope", "fps", "limit"])
    .requires(NEEDS_LIMITER),
    game(
        "gamescope.fpsLimiterNoFocus",
        "settings.field.gamescope_fps_limiter_no_focus.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 3 }),
    )
    .tooltip("settings.field.gamescope_fps_limiter_no_focus.tip")
    .placeholder(Verbatim("30"))
    .keywords(&["gamescope", "fps", "limit", "focus", "background"])
    .requires(NEEDS_LIMITER),
    game(
        "gamescope.enableForceGrabCursor",
        "settings.field.gamescope_enable_force_grab_cursor.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .tooltip("settings.field.gamescope_enable_force_grab_cursor.tip")
    .keywords(&["gamescope", "cursor", "mouse", "grab"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.additionalOptions",
        "settings.field.gamescope_additional_options.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Plain),
    )
    .tooltip("settings.field.gamescope_additional_options.tip")
    .placeholder(Verbatim("--hdr-enabled"))
    .keywords(&["gamescope", "options", "arguments"])
    .requires(NEEDS_GAMESCOPE),
    // ---- GPU ----
    game(
        "iroriGpu",
        "settings.field.gpu.label",
        Gpu,
        "settings.group.graphics_card",
        Custom(CustomKind::GpuCard),
    )
    .help("settings.field.gpu.help")
    .tooltip("settings.field.gpu.tip")
    .keywords(&[
        "gpu",
        "graphics card",
        "dri_prime",
        "mesa_vk_device_select",
        "prime",
        "multi gpu",
        "integrated",
        "discrete",
    ])
    .needs(&[Need::Linux, Need::SeveralGpus]),
    // Windows' own per-program choice, in place of `iroriGpu` there.
    game(
        "iroriWindowsGpu",
        "settings.field.windows_gpu.label",
        Gpu,
        "settings.group.graphics_card",
        Choice(WINDOWS_GPU),
    )
    .help("settings.field.windows_gpu.help")
    .tooltip("settings.field.windows_gpu.tip")
    .keywords(&[
        "gpu",
        "graphics card",
        "graphics preference",
        "high performance",
        "power saving",
        "integrated",
        "discrete",
        "UserGpuPreferences",
    ]),
    // Windows has no per-program way to hide a card: it is disabled for the
    // session (`irori_services::gpu_hide`), one game at a time.
    game_only(
        "iroriHideGpu",
        "settings.field.hide_gpu.label",
        Gpu,
        "settings.group.graphics_card",
        Custom(CustomKind::HiddenGpu),
    )
    .help("settings.field.hide_gpu.help")
    .keywords(&[
        "gpu",
        "graphics card",
        "hide",
        "disable",
        "device manager",
        "integrated",
        "discrete",
        "wrong gpu",
    ]),
    game(
        "nvidiaPrime",
        "settings.field.nvidia_prime.label",
        Gpu,
        "settings.group.graphics_card",
        Toggle,
    )
    .help("settings.field.nvidia_prime.help")
    .tooltip("settings.field.nvidia_prime.tip")
    .keywords(&["prime", "optimus", "dgpu", "nvidia", "hybrid"])
    .needs(&[Need::NoChosenGpu]),
    game(
        "iroriSmoothMotion",
        "settings.field.smooth_motion.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .tag(Verbatim("NVIDIA"))
    .help("settings.field.smooth_motion.help")
    .tooltip("settings.field.smooth_motion.tip")
    .keywords(&[
        "smooth motion",
        "frame generation",
        "nvpresent",
        "nvidia",
        "fps",
    ])
    .needs(&[Need::Linux, Need::SmoothMotion]),
    game(
        "iroriLsfg.enabled",
        "settings.field.lsfg_enabled.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .tag(Verbatim("lsfg-vk · MAKO"))
    .help("settings.field.lsfg_enabled.help")
    .tooltip("settings.field.lsfg_enabled.tip")
    .keywords(&[
        "lossless scaling",
        "lsfg",
        "lsfg-vk",
        "mako",
        "frame generation",
        "fps",
    ])
    .needs(&[Need::Linux, Need::Lsfg]),
    game(
        "iroriLsfg.layer",
        "settings.field.lsfg_layer.label",
        Gpu,
        "settings.group.frame_generation",
        Choice(LSFG_LAYER),
    )
    .help("settings.field.lsfg_layer.help")
    .tooltip("settings.field.lsfg_layer.tip")
    .keywords(&["lsfg-vk", "mako", "frame generation", "layer"])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriLsfg.multiplier",
        "settings.field.lsfg_multiplier.label",
        Gpu,
        "settings.group.frame_generation",
        Choice(LSFG_MULTIPLIER),
    )
    .help("settings.field.lsfg_multiplier.help")
    .tooltip("settings.field.lsfg_multiplier.tip")
    .keywords(&["lossless scaling", "lsfg", "multiplier", "frame generation"])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriLsfg.flowScale",
        "settings.field.lsfg_flow_scale.label",
        Gpu,
        "settings.group.frame_generation",
        Choice(LSFG_FLOW_SCALE),
    )
    .help("settings.field.lsfg_flow_scale.help")
    .tooltip("settings.field.lsfg_flow_scale.tip")
    .keywords(&["lossless scaling", "lsfg", "flow scale", "frame generation"])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriLsfg.performanceMode",
        "settings.field.lsfg_performance_mode.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .help("settings.field.lsfg_performance_mode.help")
    .tooltip("settings.field.lsfg_performance_mode.tip")
    .keywords(&[
        "lossless scaling",
        "lsfg",
        "performance",
        "frame generation",
    ])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriNvidiaReflex",
        "settings.field.nvidia_reflex.label",
        Gpu,
        "settings.group.latency",
        Toggle,
    )
    .tag(Verbatim("NVIDIA"))
    .help("settings.field.nvidia_reflex.help")
    .tooltip("settings.field.nvidia_reflex.tip")
    .keywords(&[
        "reflex",
        "latency",
        "dxvk-nvapi",
        "vkreflex",
        "nvidia",
        "input lag",
    ])
    .needs(&[Need::Linux, Need::NvidiaDriver, Need::WindowsGame])
    .requires(NEEDS_NVAPI),
    game(
        "iroriDlssUpgrade",
        "settings.field.dlss_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tooltip("settings.field.dlss_upgrade.tip")
    .tag(Verbatim("Proton"))
    .keywords(&["dlss", "proton_dlss_upgrade", "nvidia"]),
    game(
        "iroriDlssIndicator",
        "settings.field.dlss_indicator.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tooltip("settings.field.dlss_indicator.tip")
    .tag(Verbatim("Proton"))
    .keywords(&["dlss", "proton_dlss_indicator", "overlay"]),
    game(
        "iroriFsr4Upgrade",
        "settings.field.fsr4_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tooltip("settings.field.fsr4_upgrade.tip")
    .tag(Verbatim("Proton"))
    .keywords(&["fsr", "fsr4", "proton_fsr4_upgrade", "amd"]),
    game(
        "iroriXessUpgrade",
        "settings.field.xess_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tooltip("settings.field.xess_upgrade.tip")
    .tag(Verbatim("Proton"))
    .keywords(&["xess", "proton_xess_upgrade", "intel"]),
    game(
        "iroriShaderCacheSize",
        "settings.field.shader_cache_size.label",
        Gpu,
        "settings.group.shader_cache",
        Text(TextKind::Number { min: 0, max: 1024 }),
    )
    .help("settings.field.shader_cache_size.help")
    .tooltip("settings.field.shader_cache_size.tip")
    .keywords(&["shader", "cache", "stutter", "nvidia", "mesa", "gl"]),
];
