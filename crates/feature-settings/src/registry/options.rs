use super::types::{
    ChoiceOption, ChoiceValue, Condition,
    Label::{Key, Verbatim},
    opt, when,
};
use irori_ui::i18n::LANGUAGES as UI_LANGUAGES;

/// Interface languages by native name (never translated).
pub(super) const LANGUAGES: [ChoiceOption; UI_LANGUAGES.len()] = {
    let mut out = [opt(ChoiceValue::Str("en"), Verbatim("English")); UI_LANGUAGES.len()];
    let mut i = 0;
    while i < UI_LANGUAGES.len() {
        let lang = UI_LANGUAGES[i];
        out[i] = opt(ChoiceValue::Str(lang.code), Verbatim(lang.native_name));
        i += 1;
    }
    out
};

/// `iroriWindowsGpu`: Windows' graphics preference for the game's program.
pub(super) const WINDOWS_GPU: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str(""),
        Key("settings.option.windows_gpu.unset"),
    ),
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.windows_gpu.auto"),
    ),
    opt(
        ChoiceValue::Str("powerSaving"),
        Key("settings.option.windows_gpu.power_saving"),
    ),
    opt(
        ChoiceValue::Str("highPerformance"),
        Key("settings.option.windows_gpu.high_performance"),
    ),
];

pub(super) const TOP_SECTION: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("recently_played"),
        Key("settings.option.top_section.recently_played"),
    ),
    opt(
        ChoiceValue::Str("recently_played_installed"),
        Key("settings.option.top_section.recently_played_installed"),
    ),
    opt(
        ChoiceValue::Str("favourites"),
        Key("settings.option.top_section.favourites"),
    ),
    opt(
        ChoiceValue::Str("disabled"),
        Key("settings.option.top_section.nothing"),
    ),
];

pub(super) const NETWORK_MODE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.network_mode.auto"),
    ),
    opt(
        ChoiceValue::Str("online"),
        Key("settings.option.network_mode.online"),
    ),
    opt(
        ChoiceValue::Str("offline"),
        Key("settings.option.network_mode.offline"),
    ),
];

pub(super) const THEME_MODE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("system"),
        Key("settings.option.theme_mode.system"),
    ),
    opt(
        ChoiceValue::Str("dark"),
        Key("settings.option.theme_mode.dark"),
    ),
    opt(
        ChoiceValue::Str("light"),
        Key("settings.option.theme_mode.light"),
    ),
];

pub(super) const TRANSLUCENCY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.translucency.auto"),
    ),
    opt(
        ChoiceValue::Str("off"),
        Key("settings.option.translucency.off"),
    ),
    opt(
        ChoiceValue::Str("subtle"),
        Key("settings.option.translucency.subtle"),
    ),
    opt(
        ChoiceValue::Str("strong"),
        Key("settings.option.translucency.strong"),
    ),
];

pub(super) const BIG_PICTURE_SCALE: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(100), Verbatim("100%")),
    opt(ChoiceValue::Num(125), Verbatim("125%")),
    opt(ChoiceValue::Num(150), Verbatim("150%")),
    opt(ChoiceValue::Num(175), Verbatim("175%")),
    opt(ChoiceValue::Num(200), Verbatim("200%")),
];

pub(super) const NAV_SOUNDS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("off"),
        Key("settings.option.nav_sounds.off"),
    ),
    opt(
        ChoiceValue::Str("soft"),
        Key("settings.option.nav_sounds.soft"),
    ),
    opt(
        ChoiceValue::Str("classic"),
        Key("settings.option.nav_sounds.classic"),
    ),
];

pub(super) const IDLE_SCREEN: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("slideshow"),
        Key("settings.option.idle_screen.slideshow"),
    ),
    opt(
        ChoiceValue::Str("dim"),
        Key("settings.option.idle_screen.dim"),
    ),
];

pub(super) const BIG_PICTURE_HOLD: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("start"),
        Key("settings.option.big_picture_hold.start"),
    ),
    opt(
        ChoiceValue::Str("select"),
        Key("settings.option.big_picture_hold.select"),
    ),
    opt(
        ChoiceValue::Str("off"),
        Key("settings.option.big_picture_hold.off"),
    ),
];

/// The board shown first (`ageRatingBoard`): the region's, or one by name.
pub(super) const AGE_RATING_BOARDS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str(""),
        Key("settings.option.age_rating_board.auto"),
    ),
    opt(ChoiceValue::Str("PEGI"), Verbatim("PEGI")),
    opt(ChoiceValue::Str("ESRB"), Verbatim("ESRB")),
    opt(ChoiceValue::Str("USK"), Verbatim("USK")),
    opt(ChoiceValue::Str("ACB"), Verbatim("ACB")),
    opt(ChoiceValue::Str("CERO"), Verbatim("CERO")),
    opt(ChoiceValue::Str("ClassInd"), Verbatim("ClassInd")),
    opt(ChoiceValue::Str("GRAC"), Verbatim("GRAC")),
    opt(ChoiceValue::Str("CSRR"), Verbatim("CSRR")),
    opt(ChoiceValue::Str("GOG"), Verbatim("GOG")),
];

pub(super) const COVER_FIT: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("crop"),
        Key("settings.option.cover_fit.crop"),
    ),
    opt(
        ChoiceValue::Str("whole"),
        Key("settings.option.cover_fit.whole"),
    ),
];

pub(super) const COVER_HOVER: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("lift"),
        Key("settings.option.cover_hover.lift"),
    ),
    opt(
        ChoiceValue::Str("outline"),
        Key("settings.option.cover_hover.outline"),
    ),
];

pub(super) const FOCUS_COLUMNS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(0),
        Key("settings.option.focus_columns.auto"),
    ),
    opt(ChoiceValue::Num(1), Verbatim("1")),
    opt(ChoiceValue::Num(2), Verbatim("2")),
    opt(ChoiceValue::Num(3), Verbatim("3")),
];

pub(super) const DENSITY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("compact"),
        Key("settings.option.density.compact"),
    ),
    opt(
        ChoiceValue::Str("comfortable"),
        Key("settings.option.density.comfortable"),
    ),
    opt(
        ChoiceValue::Str("spacious"),
        Key("settings.option.density.spacious"),
    ),
];

pub(super) const LIBRARY_BACKGROUND: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("none"),
        Key("settings.option.library_background.none"),
    ),
    opt(
        ChoiceValue::Str("game_art"),
        Key("settings.option.library_background.game_art"),
    ),
    opt(
        ChoiceValue::Str("picture"),
        Key("settings.option.library_background.picture"),
    ),
];

pub(super) const STEAM_RUNTIME: &[ChoiceOption] = &[
    opt(
        ChoiceValue::False,
        Key("settings.option.steam_runtime.none"),
    ),
    opt(
        ChoiceValue::Str("umu-scout"),
        Verbatim("Steam Linux Runtime 1.0 (scout)"),
    ),
    opt(
        ChoiceValue::Str("umu-soldier"),
        Verbatim("Steam Linux Runtime 2.0 (soldier)"),
    ),
    opt(
        ChoiceValue::Str("umu-sniper"),
        Verbatim("Steam Linux Runtime 3.0 (sniper)"),
    ),
    opt(
        ChoiceValue::Str("umu-steamrt4"),
        Verbatim("Steam Linux Runtime 4.0"),
    ),
];

pub(super) const SHARPNESS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(1),
        Key("settings.option.sharpness.softest"),
    ),
    opt(ChoiceValue::Num(2), Verbatim("2")),
    opt(ChoiceValue::Num(3), Verbatim("3")),
    opt(ChoiceValue::Num(4), Verbatim("4")),
    opt(
        ChoiceValue::Num(5),
        Key("settings.option.sharpness.sharpest"),
    ),
];

pub(super) const UPDATE_HOURS: &[ChoiceOption] = &[
    opt(ChoiceValue::Str(""), Key("settings.option.update_hour.any")),
    opt(ChoiceValue::Str("0"), Verbatim("00:00")),
    opt(ChoiceValue::Str("1"), Verbatim("01:00")),
    opt(ChoiceValue::Str("2"), Verbatim("02:00")),
    opt(ChoiceValue::Str("3"), Verbatim("03:00")),
    opt(ChoiceValue::Str("4"), Verbatim("04:00")),
    opt(ChoiceValue::Str("5"), Verbatim("05:00")),
    opt(ChoiceValue::Str("6"), Verbatim("06:00")),
    opt(ChoiceValue::Str("7"), Verbatim("07:00")),
    opt(ChoiceValue::Str("8"), Verbatim("08:00")),
    opt(ChoiceValue::Str("9"), Verbatim("09:00")),
    opt(ChoiceValue::Str("10"), Verbatim("10:00")),
    opt(ChoiceValue::Str("11"), Verbatim("11:00")),
    opt(ChoiceValue::Str("12"), Verbatim("12:00")),
    opt(ChoiceValue::Str("13"), Verbatim("13:00")),
    opt(ChoiceValue::Str("14"), Verbatim("14:00")),
    opt(ChoiceValue::Str("15"), Verbatim("15:00")),
    opt(ChoiceValue::Str("16"), Verbatim("16:00")),
    opt(ChoiceValue::Str("17"), Verbatim("17:00")),
    opt(ChoiceValue::Str("18"), Verbatim("18:00")),
    opt(ChoiceValue::Str("19"), Verbatim("19:00")),
    opt(ChoiceValue::Str("20"), Verbatim("20:00")),
    opt(ChoiceValue::Str("21"), Verbatim("21:00")),
    opt(ChoiceValue::Str("22"), Verbatim("22:00")),
    opt(ChoiceValue::Str("23"), Verbatim("23:00")),
];

pub(super) const UPSCALE_METHOD: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("fsr"),
        Verbatim("AMD FidelityFX Super Resolution 1.0 (FSR)"),
    ),
    opt(
        ChoiceValue::Str("nis"),
        Verbatim("NVIDIA Image Scaling (NIS)"),
    ),
    opt(
        ChoiceValue::Str("integer"),
        Key("settings.option.upscale_method.integer"),
    ),
    opt(
        ChoiceValue::Str("stretch"),
        Key("settings.option.upscale_method.stretch"),
    ),
];

pub(super) const WINDOW_TYPE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("fullscreen"),
        Key("settings.option.window_type.fullscreen"),
    ),
    opt(
        ChoiceValue::Str("borderless"),
        Key("settings.option.window_type.borderless"),
    ),
    opt(
        ChoiceValue::Str("windowed"),
        Key("settings.option.window_type.windowed"),
    ),
];

pub(super) const LSFG_MULTIPLIER: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(2), Verbatim("2×")),
    opt(ChoiceValue::Num(3), Verbatim("3×")),
    opt(ChoiceValue::Num(4), Verbatim("4×")),
    opt(ChoiceValue::Num(6), Verbatim("6×")),
    opt(ChoiceValue::Num(8), Verbatim("8×")),
];

pub(super) const LSFG_LAYER: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str(irori_core::LAYER_LSFG_VK),
        Verbatim("lsfg-vk"),
    ),
    opt(ChoiceValue::Str(irori_core::LAYER_MAKO), Verbatim("MAKO")),
];

pub(super) const LSFG_FLOW_SCALE: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(25), Verbatim("25%")),
    opt(ChoiceValue::Num(50), Verbatim("50%")),
    opt(ChoiceValue::Num(75), Verbatim("75%")),
    opt(ChoiceValue::Num(85), Verbatim("85%")),
    opt(ChoiceValue::Num(100), Verbatim("100%")),
];

pub(super) const LUDUSAVI_INTERVAL: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(0),
        Key("settings.option.ludusavi_interval.0"),
    ),
    opt(
        ChoiceValue::Num(15),
        Key("settings.option.ludusavi_interval.15"),
    ),
    opt(
        ChoiceValue::Num(30),
        Key("settings.option.ludusavi_interval.30"),
    ),
    opt(
        ChoiceValue::Num(60),
        Key("settings.option.ludusavi_interval.60"),
    ),
    opt(
        ChoiceValue::Num(120),
        Key("settings.option.ludusavi_interval.120"),
    ),
];

pub(super) const LUDUSAVI_FULL_LIMIT: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(0),
        Key("settings.option.ludusavi_limit.ludusavi"),
    ),
    opt(ChoiceValue::Num(1), Verbatim("1")),
    opt(ChoiceValue::Num(2), Verbatim("2")),
    opt(ChoiceValue::Num(3), Verbatim("3")),
    opt(ChoiceValue::Num(5), Verbatim("5")),
    opt(ChoiceValue::Num(10), Verbatim("10")),
];

pub(super) const LUDUSAVI_DIFFERENTIAL_LIMIT: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str(""),
        Key("settings.option.ludusavi_limit.ludusavi"),
    ),
    opt(
        ChoiceValue::Str("0"),
        Key("settings.option.ludusavi_limit.none"),
    ),
    opt(ChoiceValue::Str("1"), Verbatim("1")),
    opt(ChoiceValue::Str("3"), Verbatim("3")),
    opt(ChoiceValue::Str("5"), Verbatim("5")),
    opt(ChoiceValue::Str("10"), Verbatim("10")),
];

pub(super) const ART_CACHE_LIMIT: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(0),
        Key("settings.option.art_cache_limit.0"),
    ),
    opt(
        ChoiceValue::Num(512),
        Key("settings.option.art_cache_limit.512"),
    ),
    opt(
        ChoiceValue::Num(1024),
        Key("settings.option.art_cache_limit.1024"),
    ),
    opt(
        ChoiceValue::Num(2048),
        Key("settings.option.art_cache_limit.2048"),
    ),
    opt(
        ChoiceValue::Num(5120),
        Key("settings.option.art_cache_limit.5120"),
    ),
    opt(
        ChoiceValue::Num(10240),
        Key("settings.option.art_cache_limit.10240"),
    ),
];

pub(super) const REPLAY_SECONDS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(15),
        Key("settings.option.replay_seconds.15"),
    ),
    opt(
        ChoiceValue::Num(30),
        Key("settings.option.replay_seconds.30"),
    ),
    opt(
        ChoiceValue::Num(60),
        Key("settings.option.replay_seconds.60"),
    ),
    opt(
        ChoiceValue::Num(120),
        Key("settings.option.replay_seconds.120"),
    ),
    opt(
        ChoiceValue::Num(300),
        Key("settings.option.replay_seconds.300"),
    ),
    opt(
        ChoiceValue::Num(600),
        Key("settings.option.replay_seconds.600"),
    ),
];

pub(super) const REPLAY_QUALITY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("medium"),
        Key("settings.option.replay_quality.medium"),
    ),
    opt(
        ChoiceValue::Str("high"),
        Key("settings.option.replay_quality.high"),
    ),
    opt(
        ChoiceValue::Str("very_high"),
        Key("settings.option.replay_quality.very_high"),
    ),
    opt(
        ChoiceValue::Str("ultra"),
        Key("settings.option.replay_quality.ultra"),
    ),
];

pub(super) const REPLAY_AUDIO: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("desktop"),
        Key("settings.option.replay_audio.desktop"),
    ),
    opt(
        ChoiceValue::Str("desktop_and_mic"),
        Key("settings.option.replay_audio.desktop_and_mic"),
    ),
    opt(
        ChoiceValue::Str("game"),
        Key("settings.option.replay_audio.game"),
    ),
    opt(
        ChoiceValue::Str("none"),
        Key("settings.option.replay_audio.none"),
    ),
];

pub(super) const REPLAY_FPS: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(30), Key("settings.option.replay_fps.30")),
    opt(ChoiceValue::Num(60), Key("settings.option.replay_fps.60")),
    opt(ChoiceValue::Num(120), Key("settings.option.replay_fps.120")),
    opt(ChoiceValue::Num(144), Key("settings.option.replay_fps.144")),
];

pub(super) const REPLAY_CODEC: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.replay_codec.auto"),
    ),
    opt(
        ChoiceValue::Str("h264"),
        Key("settings.option.replay_codec.h264"),
    ),
    opt(
        ChoiceValue::Str("hevc"),
        Key("settings.option.replay_codec.hevc"),
    ),
    opt(
        ChoiceValue::Str("av1"),
        Key("settings.option.replay_codec.av1"),
    ),
];

pub(super) const REPLAY_RESOLUTION: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("native"),
        Key("settings.option.replay_resolution.native"),
    ),
    opt(
        ChoiceValue::Str("3840x2160"),
        Key("settings.option.replay_resolution.2160"),
    ),
    opt(
        ChoiceValue::Str("2560x1440"),
        Key("settings.option.replay_resolution.1440"),
    ),
    opt(
        ChoiceValue::Str("1920x1080"),
        Key("settings.option.replay_resolution.1080"),
    ),
    opt(
        ChoiceValue::Str("1280x720"),
        Key("settings.option.replay_resolution.720"),
    ),
];

pub(super) const REPLAY_CAPTURE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("screen"),
        Key("settings.option.replay_capture.screen"),
    ),
    opt(
        ChoiceValue::Str("portal"),
        Key("settings.option.replay_capture.portal"),
    ),
];

pub(super) const NEEDS_STORE_BADGES: &[Condition] = &[when(
    "iroriUi.showStoreBadges",
    true,
    "settings.requires.store_badges",
)];
pub(super) const NEEDS_AGE_RATINGS: &[Condition] = &[when(
    "iroriUi.showAgeRatings",
    true,
    "settings.requires.age_ratings",
)];
pub(super) const NEEDS_TRAY: &[Condition] =
    &[when("noTrayIcon", false, "settings.requires.tray_icon")];
pub(super) const NEEDS_EXIT_TO_TRAY: &[Condition] = &[
    when("noTrayIcon", false, "settings.requires.tray_icon"),
    when("exitToTray", true, "settings.requires.close_to_tray"),
];
pub(super) const NEEDS_START_WITH_SYSTEM: &[Condition] = &[
    when("noTrayIcon", false, "settings.requires.tray_icon"),
    when(
        "iroriStartWithSystem",
        true,
        "settings.requires.start_with_system",
    ),
];
pub(super) const NEEDS_WAYLAND: &[Condition] = &[when(
    "enableWineWayland",
    true,
    "settings.requires.wine_wayland",
)];
pub(super) const NEEDS_FSR: &[Condition] = &[when("enableFSR", true, "settings.requires.fsr")];
pub(super) const NEEDS_DXVK: &[Condition] =
    &[when("autoInstallDxvk", true, "settings.requires.dxvk")];
pub(super) const NEEDS_FPS_LIMIT: &[Condition] = &[when(
    "enableDXVKFpsLimit",
    true,
    "settings.requires.fps_limit",
)];
pub(super) const NEEDS_MANGOHUD: &[Condition] =
    &[when("showMangohud", true, "settings.requires.mangohud")];

/// On Linux the frame limit goes through the limiter chosen below it
/// (`irori_runtime::frame_limit`); elsewhere through DXVK.
pub(super) const FPS_LIMIT_HELP: &str = if cfg!(target_os = "linux") {
    "settings.field.enable_dxvk_fps_limit.help_linux"
} else {
    "settings.field.enable_dxvk_fps_limit.help"
};

pub(super) const FPS_LIMITER: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.fps_limiter.auto"),
    ),
    opt(ChoiceValue::Str("gamescope"), Verbatim("gamescope")),
    opt(ChoiceValue::Str("mangohud"), Verbatim("MangoHud")),
    opt(ChoiceValue::Str("dxvk"), Verbatim("DXVK")),
];
pub(super) const PROTON_CPU_LEVEL: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.proton_cpu_level.auto"),
    ),
    opt(
        ChoiceValue::Str("baseline"),
        Key("settings.option.proton_cpu_level.baseline"),
    ),
    opt(
        ChoiceValue::Str("v3"),
        Key("settings.option.proton_cpu_level.v3"),
    ),
];
pub(super) const NEEDS_NVAPI: &[Condition] = &[when(
    "autoInstallDxvkNvapi",
    true,
    "settings.requires.dxvk_nvapi",
)];
pub(super) const NEEDS_ACHIEVEMENT_NOTICES: &[Condition] = &[when(
    "iroriAchievementNotices",
    true,
    "settings.requires.achievement_notices",
)];

pub(super) const NEEDS_LSFG: &[Condition] =
    &[when("iroriLsfg.enabled", true, "settings.requires.lsfg")];
pub(super) const NEEDS_VKD3D: &[Condition] =
    &[when("autoInstallVkd3d", true, "settings.requires.vkd3d")];
pub(super) const NEEDS_SUNSHINE: &[Condition] = &[when(
    "iroriSunshine.enabled",
    true,
    "settings.requires.sunshine",
)];
pub(super) const SUNSHINE_GAMES: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("installed"),
        Key("settings.option.sunshine_games.installed"),
    ),
    opt(
        ChoiceValue::Str("favourites"),
        Key("settings.option.sunshine_games.favourites"),
    ),
];
pub(super) const NEEDS_REPLAY: &[Condition] = &[when(
    "iroriReplay.enabled",
    true,
    "settings.requires.replay",
)];
pub(super) const NEEDS_GAMESCOPE: &[Condition] = &[when(
    "gamescope.iroriEnabled",
    true,
    "settings.requires.gamescope",
)];
pub(super) const NEEDS_UPSCALING: &[Condition] = &[
    when(
        "gamescope.iroriEnabled",
        true,
        "settings.requires.gamescope",
    ),
    when(
        "gamescope.enableUpscaling",
        true,
        "settings.requires.gamescope_upscaling",
    ),
];
pub(super) const NEEDS_LIMITER: &[Condition] = &[
    when(
        "gamescope.iroriEnabled",
        true,
        "settings.requires.gamescope",
    ),
    when(
        "gamescope.enableLimiter",
        true,
        "settings.requires.gamescope_limiter",
    ),
];

pub(super) const NEEDS_WALLPAPER_COLORS: &[Condition] = &[when(
    "iroriUi.wallpaperColors",
    true,
    "settings.requires.wallpaper_colors",
)];

/// The in-game overlay works differently per system (its Windows and macOS
/// shortcut, the controller grab only Linux has, macOS's system-only volume).
pub(super) const OVERLAY_HELP: &str = if cfg!(windows) {
    "settings.field.overlay.help_windows"
} else if cfg!(target_os = "macos") {
    "settings.field.overlay.help_macos"
} else {
    "settings.field.overlay.help"
};

pub(super) const OVERLAY_TIP: &str = if cfg!(windows) {
    "settings.field.overlay.tip_windows"
} else if cfg!(target_os = "macos") {
    "settings.field.overlay.tip_macos"
} else {
    "settings.field.overlay.tip"
};

pub(super) const NEEDS_OVERLAY: &[Condition] = &[when(
    "iroriOverlay.enabled",
    true,
    "settings.requires.overlay",
)];

pub(super) const OVERLAY_SIDE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("center"),
        Key("settings.option.overlay_side.center"),
    ),
    opt(
        ChoiceValue::Str("left"),
        Key("settings.option.overlay_side.left"),
    ),
    opt(
        ChoiceValue::Str("right"),
        Key("settings.option.overlay_side.right"),
    ),
];

pub(super) const NEEDS_CONTROLLER: &[Condition] = &[when(
    "disableController",
    false,
    "settings.requires.controller_navigation",
)];
