use super::options::{
    BIG_PICTURE_HOLD, IDLE_SCREEN, LANGUAGES, NAV_SOUNDS, NEEDS_ACHIEVEMENT_NOTICES,
    NEEDS_CONTROLLER, NEEDS_EXIT_TO_TRAY, NEEDS_OVERLAY, NEEDS_START_WITH_SYSTEM, NEEDS_SUNSHINE,
    NEEDS_TRAY, NETWORK_MODE, OVERLAY_HELP, OVERLAY_SIDE, OVERLAY_TIP, SUNSHINE_GAMES, TOP_SECTION,
    UPDATE_HOURS,
};
use super::types::{
    ActionKind,
    FieldKind::{Action, Choice, Panel, Text, Toggle},
    FieldSection::{Controller, General},
    FieldSpec,
    Label::{Key, Verbatim},
    Need, PanelKind, TextKind, app, when,
};

/// A line an unlock notice may show under the achievement's name; the
/// overlay's corner notices show them too, so they don't need the notices.
const fn achievement_line(path: &'static str, label: &'static str) -> FieldSpec {
    app(path, label, General, "settings.group.library", Toggle)
        .help("settings.field.achievement_toast_line.help")
        .keywords(&["achievements", "unlocked", "notification", "toast", "lines"])
}

pub(super) const FIELDS: &[FieldSpec] = &[
    // ---- General ----
    app(
        "uiLanguage",
        "settings.field.ui_language.label",
        General,
        "settings.group.language",
        Choice(&LANGUAGES),
    )
    .help("settings.field.ui_language.help")
    .tooltip("settings.field.ui_language.tip")
    .keywords(&["language", "locale", "translation", "interface"]),
    app(
        "defaultInstallPath",
        "settings.field.default_install_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_install_path.help")
    .tooltip("settings.field.default_install_path.tip")
    .keywords(&["path", "directory", "games", "default install folder"]),
    app(
        "iroriInstallPerStore",
        "settings.field.install_per_store.label",
        General,
        "settings.group.folders",
        Toggle,
    )
    .help("settings.field.install_per_store.help")
    .tooltip("settings.field.install_per_store.tip")
    .keywords(&["path", "directory", "folder", "store", "install"]),
    app(
        "panel.libraryFolders",
        "settings.field.panel_library_folders.label",
        General,
        "settings.group.folders",
        Panel(PanelKind::LibraryFolders),
    )
    .help("settings.field.panel_library_folders.help")
    .keywords(&["library", "folders", "drive", "sd card", "move", "install"]),
    app(
        "defaultWinePrefixDir",
        "settings.field.default_wine_prefix_dir.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_wine_prefix_dir.help")
    .tooltip("settings.field.default_wine_prefix_dir.tip")
    .keywords(&["prefix", "path", "wineprefix"]),
    app(
        "defaultSteamPath",
        "settings.field.default_steam_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_steam_path.help")
    .tooltip("settings.field.default_steam_path.tip")
    .keywords(&["steam", "proton", "path"]),
    app(
        "iroriCrossoverBottlesPath",
        "settings.field.crossover_bottles_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .tag(Verbatim("CrossOver"))
    .help("settings.field.crossover_bottles_path.help")
    .tooltip("settings.field.crossover_bottles_path.tip")
    .placeholder(Verbatim("~/Library/Application Support/CrossOver/Bottles"))
    .keywords(&["crossover", "bottle", "bottles", "path", "mac"]),
    app(
        "iroriNetworkMode",
        "settings.field.network_mode.label",
        General,
        "settings.group.network",
        Choice(NETWORK_MODE),
    )
    .help("settings.field.network_mode.help")
    .keywords(&[
        "offline",
        "online",
        "network",
        "internet",
        "connection",
        "airplane",
    ]),
    app(
        "iroriNetworkTimeout",
        "settings.field.network_timeout.label",
        General,
        "settings.group.network",
        Text(TextKind::Number { min: 0, max: 600 }),
    )
    .help("settings.field.network_timeout.help")
    .keywords(&["timeout", "slow", "network", "connection", "seconds"]),
    app(
        "iroriCheckForAppUpdates",
        "settings.field.check_for_app_updates.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.check_for_app_updates.help")
    .tooltip("settings.field.check_for_app_updates.tip")
    .keywords(&["irori", "version", "update", "release", "new"]),
    app(
        "checkForUpdatesOnStartup",
        "settings.field.check_for_updates_on_startup.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .tooltip("settings.field.check_for_updates_on_startup.tip")
    .keywords(&["update", "startup"]),
    app(
        "checkUpdatesInterval",
        "settings.field.check_updates_interval.label",
        General,
        "settings.group.updates",
        Text(TextKind::Number { min: 5, max: 1440 }),
    )
    .help("settings.field.check_updates_interval.help")
    .tooltip("settings.field.check_updates_interval.tip")
    .keywords(&["minutes", "update", "frequency"]),
    app(
        "iroriLaunchersRescan",
        "settings.field.launchers_rescan.label",
        General,
        "settings.group.updates",
        Text(TextKind::Number { min: 0, max: 1440 }),
    )
    .help("settings.field.launchers_rescan.help")
    .tooltip("settings.field.launchers_rescan.tip")
    .keywords(&[
        "battle.net",
        "ea",
        "ubisoft",
        "rockstar",
        "xbox",
        "launcher",
        "rescan",
        "minutes",
    ]),
    app(
        "autoUpdateGames",
        "settings.field.auto_update_games.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_games.help")
    .tooltip("settings.field.auto_update_games.tip")
    .keywords(&["update", "automatic"]),
    app(
        "iroriAutoUpdateComponents",
        "settings.field.auto_update_components.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_components.help")
    .tooltip("settings.field.auto_update_components.tip")
    .keywords(&["update", "automatic", "wine", "proton", "dxvk", "emulator"]),
    app(
        "iroriAutoUpdateFrom",
        "settings.field.auto_update_from.label",
        General,
        "settings.group.updates",
        Choice(UPDATE_HOURS),
    )
    .help("settings.field.auto_update_window.help")
    .tooltip("settings.field.auto_update_from.tip")
    .keywords(&["update", "schedule", "night", "window", "time"]),
    app(
        "iroriAutoUpdateUntil",
        "settings.field.auto_update_until.label",
        General,
        "settings.group.updates",
        Choice(UPDATE_HOURS),
    )
    .tooltip("settings.field.auto_update_until.tip")
    .keywords(&["update", "schedule", "night", "window", "time"]),
    app(
        "iroriAutoUpdateUnmetered",
        "settings.field.auto_update_unmetered.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_unmetered.help")
    .tooltip("settings.field.auto_update_unmetered.tip")
    .keywords(&["update", "metered", "data limit", "mobile", "hotspot"]),
    app(
        "noTrayIcon",
        "settings.field.no_tray_icon.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .tooltip("settings.field.no_tray_icon.tip")
    .tag(Key("settings.tag.restart_required"))
    .keywords(&["tray", "systray", "icon", "restart required"]),
    app(
        "exitToTray",
        "settings.field.exit_to_tray.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.exit_to_tray.help")
    .tooltip("settings.field.exit_to_tray.tip")
    .keywords(&["tray", "exit", "close", "background"])
    .requires(NEEDS_TRAY),
    app(
        "startInTray",
        "settings.field.start_in_tray.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_in_tray.help")
    .tooltip("settings.field.start_in_tray.tip")
    .keywords(&["tray", "minimized", "startup"])
    .requires(NEEDS_EXIT_TO_TRAY),
    app(
        "iroriUi.startMaximized",
        "settings.field.ui_start_maximized.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.ui_start_maximized.help")
    .tooltip("settings.field.ui_start_maximized.tip")
    .keywords(&["maximize", "maximized", "window", "startup", "full size"]),
    app(
        "iroriStartWithSystem",
        "settings.field.start_with_system.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_with_system.help")
    .tooltip("settings.field.start_with_system.tip")
    .keywords(&["autostart", "login", "startup", "boot", "log in"]),
    app(
        "iroriStartWithSystemMinimized",
        "settings.field.start_with_system_minimized.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_with_system_minimized.help")
    .tooltip("settings.field.start_with_system_minimized.tip")
    .keywords(&["autostart", "tray", "minimized", "startup"])
    .requires(NEEDS_START_WITH_SYSTEM),
    app(
        "minimizeOnLaunch",
        "settings.field.minimize_on_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.minimize_on_launch.help")
    .tooltip("settings.field.minimize_on_launch.tip")
    .keywords(&["minimize", "hide", "launch"])
    .requires(NEEDS_TRAY),
    app(
        "iroriShowLogOnLaunch",
        "settings.field.show_log_on_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.show_log_on_launch.help")
    .tooltip("settings.field.show_log_on_launch.tip")
    .keywords(&["log", "console", "output", "launch"]),
    app(
        "darkTrayIcon",
        "settings.field.dark_tray_icon.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .tooltip("settings.field.dark_tray_icon.tip")
    .tag(Key("settings.tag.restart_required"))
    .keywords(&["tray", "icon", "dark"])
    .requires(NEEDS_TRAY),
    app(
        "hideWindowOnProtocolLaunch",
        "settings.field.hide_window_on_protocol_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.hide_window_on_protocol_launch.help")
    .tooltip("settings.field.hide_window_on_protocol_launch.tip")
    .keywords(&["protocol", "url", "heroic://", "link", "shortcut"]),
    app(
        "startInConsoleMode",
        "settings.field.start_in_console_mode.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .tooltip("settings.field.start_in_console_mode.tip")
    .keywords(&["console", "gamepad", "fullscreen", "big picture"]),
    app(
        "iroriUi.bigPictureInGameMode",
        "settings.field.big_picture_in_game_mode.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.big_picture_in_game_mode.help")
    .tooltip("settings.field.big_picture_in_game_mode.tip")
    .keywords(&[
        "steam deck",
        "game mode",
        "gamescope",
        "console",
        "big picture",
    ]),
    app(
        "iroriUi.bigPictureOnController",
        "settings.field.big_picture_on_controller.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.big_picture_on_controller.help")
    .tooltip("settings.field.big_picture_on_controller.tip")
    .keywords(&["controller", "gamepad", "console", "big picture", "connect"]),
    app(
        "iroriUi.quickLaunchShortcut",
        "settings.field.quick_launch_shortcut.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.quick_launch_shortcut.help")
    .keywords(&[
        "quick launch",
        "launcher",
        "palette",
        "shortcut",
        "hotkey",
        "keyboard",
    ]),
    app(
        "panel.quick_launch",
        "settings.field.panel_quick_launch.label",
        General,
        "settings.group.tray_and_window",
        Panel(PanelKind::QuickLaunch),
    )
    .keywords(&["quick launch", "launcher", "shortcut", "hotkey", "command"]),
    app(
        "iroriUi.bigPictureFullscreen",
        "settings.field.big_picture_fullscreen.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.big_picture_fullscreen.help")
    .keywords(&["fullscreen", "window", "console", "big picture"]),
    app(
        "addDesktopShortcuts",
        "settings.field.add_desktop_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .help("settings.field.add_desktop_shortcuts.help")
    .tooltip("settings.field.add_desktop_shortcuts.tip")
    .keywords(&["shortcut", "desktop"]),
    app(
        "addStartMenuShortcuts",
        "settings.field.add_start_menu_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .tooltip("settings.field.add_start_menu_shortcuts.tip")
    .keywords(&["shortcut", "start menu", "applications"]),
    app(
        "addSteamShortcuts",
        "settings.field.add_steam_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .help("settings.field.add_steam_shortcuts.help")
    .tooltip("settings.field.add_steam_shortcuts.tip")
    .keywords(&["steam", "shortcut", "deck"]),
    app(
        "iroriShortcutIconFromExe",
        "settings.field.shortcut_icon_from_exe.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .help("settings.field.shortcut_icon_from_exe.help")
    .keywords(&["shortcut", "icon", "exe", "program"]),
    app(
        "libraryTopSection",
        "settings.field.library_top_section.label",
        General,
        "settings.group.library",
        Choice(TOP_SECTION),
    )
    .tooltip("settings.field.library_top_section.tip")
    .keywords(&["recent", "favourites", "top"]),
    app(
        "maxRecentGames",
        "settings.field.max_recent_games.label",
        General,
        "settings.group.library",
        Text(TextKind::Number { min: 1, max: 20 }),
    )
    .tooltip("settings.field.max_recent_games.tip")
    .keywords(&["recent", "tray"]),
    app(
        "iroriAchievementNotices",
        "settings.field.achievement_notices.label",
        General,
        "settings.group.library",
        Toggle,
    )
    .help("settings.field.achievement_notices.help")
    .tooltip("settings.field.achievement_notices.tip")
    .keywords(&[
        "achievements",
        "trophies",
        "unlocked",
        "notification",
        "retroachievements",
    ]),
    app(
        "iroriAchievementToast.overGame",
        "settings.field.achievement_toast_over_game.label",
        General,
        "settings.group.library",
        Toggle,
    )
    .help("settings.field.achievement_toast_over_game.help")
    .keywords(&[
        "achievements",
        "unlocked",
        "overlay",
        "in-game",
        "toast",
        "popup",
    ])
    .requires(NEEDS_ACHIEVEMENT_NOTICES),
    achievement_line(
        "iroriAchievementToast.showGame",
        "settings.field.achievement_toast_game.label",
    ),
    achievement_line(
        "iroriAchievementToast.showDescription",
        "settings.field.achievement_toast_description.label",
    ),
    achievement_line(
        "iroriAchievementToast.showRarity",
        "settings.field.achievement_toast_rarity.label",
    ),
    achievement_line(
        "iroriAchievementToast.showProgress",
        "settings.field.achievement_toast_progress.label",
    ),
    achievement_line(
        "iroriAchievementToast.showPoints",
        "settings.field.achievement_toast_points.label",
    ),
    app(
        "iroriFileAchievements",
        "settings.field.file_achievements.label",
        General,
        "settings.group.library",
        Toggle,
    )
    .help("settings.field.file_achievements.help")
    .tooltip("settings.field.file_achievements.tip")
    .keywords(&[
        "achievements",
        "goldberg",
        "gbe",
        "emulator",
        "steam_api",
        "codex",
        "drm-free",
        "offline",
    ]),
    app(
        "iroriUi.stores.epic",
        "settings.field.show_store_epic.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .keywords(&["store", "hide", "epic", "legendary"]),
    app(
        "iroriUi.stores.gog",
        "settings.field.show_store_gog.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .keywords(&["store", "hide", "gog"]),
    app(
        "iroriUi.stores.amazon",
        "settings.field.show_store_amazon.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .keywords(&["store", "hide", "amazon", "prime", "nile"]),
    app(
        "iroriDeals",
        "settings.field.deals.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.deals.help")
    .keywords(&["deals", "sale", "discount", "price", "offers"]),
    app(
        "zoomPlatform",
        "settings.field.zoom_platform.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .tooltip("settings.field.zoom_platform.tip")
    .keywords(&["zoom", "store"]),
    app(
        "iroriItchIo",
        "settings.field.itch_io.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.itch_io.help")
    .tooltip("settings.field.itch_io.tip")
    .keywords(&["itch", "itch.io", "store", "indie"]),
    app(
        "iroriXbox",
        "settings.field.xbox.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.xbox.help")
    .tooltip("settings.field.xbox.tip")
    .needs(&[Need::Linux])
    .keywords(&["xbox", "microsoft", "store", "game pass", "gdk"]),
    app(
        "iroriXboxMarket",
        "settings.field.xbox_market.label",
        General,
        "settings.group.stores",
        Text(TextKind::Plain),
    )
    .help("settings.field.xbox_market.help")
    .tooltip("settings.field.xbox_market.tip")
    .placeholder(Verbatim("US"))
    .requires(&[when("iroriXbox", true, "settings.field.xbox_market.off")])
    .needs(&[Need::Linux])
    .keywords(&["xbox", "market", "region", "country", "microsoft"]),
    app(
        "discordRPC",
        "settings.field.discord_rpc.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.discord_rpc.help")
    .tooltip("settings.field.discord_rpc.tip")
    .keywords(&["discord", "presence", "rpc"]),
    app(
        "discordClientId",
        "settings.field.discord_client_id.label",
        General,
        "settings.group.integrations",
        Text(TextKind::Digits { max_len: 20 }),
    )
    .help("settings.field.discord_client_id.help")
    .tooltip("settings.field.discord_client_id.tip")
    .placeholder(Key("settings.field.discord_client_id.placeholder"))
    .keywords(&[
        "discord",
        "presence",
        "rpc",
        "client id",
        "application",
        "developers",
    ]),
    app(
        "iroriSunshine.enabled",
        "settings.field.sunshine_enabled.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_enabled.help")
    .tooltip("settings.field.sunshine_enabled.tip")
    .keywords(&[
        "sunshine",
        "moonlight",
        "stream",
        "streaming",
        "remote play",
    ]),
    app(
        "iroriSunshine.games",
        "settings.field.sunshine_games.label",
        General,
        "settings.group.integrations",
        Choice(SUNSHINE_GAMES),
    )
    .tooltip("settings.field.sunshine_games.tip")
    .keywords(&["sunshine", "moonlight", "stream", "favourites"])
    .requires(NEEDS_SUNSHINE),
    app(
        "iroriSunshine.bigPicture",
        "settings.field.sunshine_big_picture.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_big_picture.help")
    .tooltip("settings.field.sunshine_big_picture.tip")
    .keywords(&["sunshine", "moonlight", "big picture", "couch"])
    .requires(NEEDS_SUNSHINE),
    app(
        "iroriSunshine.address",
        "settings.field.sunshine_address.label",
        General,
        "settings.group.integrations",
        Text(TextKind::Plain),
    )
    .help("settings.field.sunshine_address.help")
    .tooltip("settings.field.sunshine_address.tip")
    .placeholder(Key("settings.field.sunshine_address.placeholder"))
    .keywords(&[
        "sunshine",
        "moonlight",
        "address",
        "another computer",
        "remote",
        "certificate",
        "fingerprint",
    ])
    .requires(NEEDS_SUNSHINE),
    app(
        "iroriSunshine.matchClient",
        "settings.field.sunshine_match_client.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_match_client.help")
    .tooltip("settings.field.sunshine_match_client.tip")
    .keywords(&[
        "sunshine",
        "moonlight",
        "resolution",
        "refresh rate",
        "fps",
        "hdr",
        "gamescope",
    ])
    .requires(NEEDS_SUNSHINE),
    app(
        "panel.sunshine",
        "settings.field.panel_sunshine.label",
        General,
        "settings.group.integrations",
        Panel(PanelKind::Sunshine),
    )
    .tooltip("settings.field.panel_sunshine.tip")
    .keywords(&[
        "sunshine",
        "moonlight",
        "stream",
        "password",
        "login",
        "certificate",
    ]),
    app(
        "disablePlaytimeSync",
        "settings.field.disable_playtime_sync.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tooltip("settings.field.disable_playtime_sync.tip")
    .tag(Verbatim("GOG · Epic"))
    .keywords(&["playtime", "sync", "gog", "epic"]),
    app(
        "disableGOGPresence",
        "settings.field.disable_gog_presence.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tooltip("settings.field.disable_gog_presence.tip")
    .tag(Verbatim("GOG"))
    .keywords(&["presence", "gog", "galaxy", "online"]),
    app(
        "cometSupport",
        "settings.field.comet_support.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tag(Verbatim("GOG"))
    .help("settings.field.comet_support.help")
    .tooltip("settings.field.comet_support.tip")
    .keywords(&["comet", "galaxy", "achievements", "gog"]),
    app(
        "action.open-windows-files",
        "settings.field.action_open_windows_files.label",
        General,
        "settings.group.integrations",
        Action(ActionKind::OpenWindowsFiles),
    )
    .help("settings.field.action_open_windows_files.help")
    .tooltip("settings.field.action_open_windows_files.tip")
    .keywords(&[
        "exe",
        "msi",
        "bat",
        "reg",
        "double click",
        "open with",
        "file",
        "wine",
        "default",
    ]),
    app(
        "maxWorkers",
        "settings.field.max_workers.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number { min: 0, max: 64 }),
    )
    .help("settings.field.max_workers.help")
    .tooltip("settings.field.max_workers.tip")
    .keywords(&["workers", "threads", "parallel", "download"]),
    app(
        "maxDownloadSpeed",
        "settings.field.max_download_speed.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number {
            min: 0,
            max: 10_000_000,
        }),
    )
    .help("settings.field.max_download_speed.help")
    .tooltip("settings.field.max_download_speed.tip")
    .keywords(&["bandwidth", "speed", "limit", "throttle", "download", "kib"]),
    app(
        "downloadNoHttps",
        "settings.field.download_no_https.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.download_no_https.help")
    .tooltip("settings.field.download_no_https.tip")
    .keywords(&["https", "lancache", "cdn", "cache"]),
    app(
        "panel.button-map",
        "settings.field.panel_button_map.label",
        Controller,
        "settings.group.buttons",
        Panel(PanelKind::ButtonMap),
    )
    .keywords(&[
        "gamepad",
        "controller",
        "buttons",
        "bindings",
        "remap",
        "rebind",
        "confirm",
        "nintendo",
        "glyphs",
        "layout",
    ]),
    app(
        "panel.controllers",
        "settings.field.panel_controllers.label",
        Controller,
        "settings.group.controller",
        Panel(PanelKind::Controllers),
    )
    .tooltip("settings.field.panel_controllers.tip")
    .keywords(&[
        "gamepad",
        "controller",
        "joystick",
        "xbox",
        "playstation",
        "dualsense",
        "steam deck",
        "battery",
    ]),
    app(
        "disableController",
        "settings.field.disable_controller.label",
        Controller,
        "settings.group.controller",
        Toggle,
    )
    .help("settings.field.disable_controller.help")
    .tooltip("settings.field.disable_controller.tip")
    .keywords(&["gamepad", "controller", "joystick"]),
    app(
        "iroriController.hidePointer",
        "settings.field.hide_pointer.label",
        Controller,
        "settings.group.controller",
        Toggle,
    )
    .help("settings.field.hide_pointer.help")
    .keywords(&[
        "mouse",
        "pointer",
        "cursor",
        "hide",
        "gamepad",
        "controller",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "gamepadInitialRepeatDelay",
        "settings.field.gamepad_initial_repeat_delay.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number { min: 50, max: 500 }),
    )
    .help("settings.field.gamepad_initial_repeat_delay.help")
    .tooltip("settings.field.gamepad_initial_repeat_delay.tip")
    .keywords(&["gamepad", "controller", "repeat", "delay"])
    .requires(NEEDS_CONTROLLER),
    app(
        "gamepadRepeatDelay",
        "settings.field.gamepad_repeat_delay.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number { min: 5, max: 150 }),
    )
    .help("settings.field.gamepad_repeat_delay.help")
    .tooltip("settings.field.gamepad_repeat_delay.tip")
    .keywords(&["gamepad", "controller", "repeat", "frequency"])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriUi.bigPictureHoldButton",
        "settings.field.big_picture_hold_button.label",
        Controller,
        "settings.group.controller",
        Choice(BIG_PICTURE_HOLD),
    )
    .help("settings.field.big_picture_hold_button.help")
    .tooltip("settings.field.big_picture_hold_button.tip")
    .keywords(&[
        "gamepad",
        "controller",
        "hold",
        "start",
        "select",
        "console",
        "big picture",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriUi.bigPictureHoldMs",
        "settings.field.big_picture_hold_ms.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number {
            min: 300,
            max: 3000,
        }),
    )
    .help("settings.field.big_picture_hold_ms.help")
    .tooltip("settings.field.big_picture_hold_ms.tip")
    .keywords(&["gamepad", "controller", "hold", "delay", "big picture"])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriOverlay.enabled",
        "settings.field.overlay.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help(OVERLAY_HELP)
    .tooltip(OVERLAY_TIP)
    .keywords(&[
        "overlay",
        "in-game",
        "guide",
        "screenshot",
        "volume",
        "gamepad",
        "controller",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriBigPicture.sounds",
        "settings.field.nav_sounds.label",
        Controller,
        "settings.group.big_picture",
        Choice(NAV_SOUNDS),
    )
    .help("settings.field.nav_sounds.help")
    .keywords(&[
        "sound",
        "sounds",
        "audio",
        "beep",
        "navigation",
        "big picture",
    ]),
    app(
        "iroriBigPicture.soundVolume",
        "settings.field.nav_sound_volume.label",
        Controller,
        "settings.group.big_picture",
        Text(TextKind::Number { min: 1, max: 100 }),
    )
    .help("settings.field.nav_sound_volume.help")
    .keywords(&["sound", "volume", "loudness", "big picture"]),
    app(
        "iroriBigPicture.idleMinutes",
        "settings.field.idle_minutes.label",
        Controller,
        "settings.group.big_picture",
        Text(TextKind::Number { min: 0, max: 1440 }),
    )
    .help("settings.field.idle_minutes.help")
    .keywords(&[
        "idle",
        "screensaver",
        "attract",
        "slideshow",
        "dim",
        "big picture",
    ]),
    app(
        "iroriBigPicture.idleScreen",
        "settings.field.idle_screen.label",
        Controller,
        "settings.group.big_picture",
        Choice(IDLE_SCREEN),
    )
    .keywords(&["idle", "screensaver", "slideshow", "dim", "big picture"]),
    app(
        "panel.overlay",
        "settings.field.panel_overlay.label",
        Controller,
        "settings.group.overlay",
        Panel(PanelKind::Overlay),
    )
    .keywords(&["overlay", "in-game", "layer-shell", "wayland", "controller"]),
    app(
        "iroriOverlay.side",
        "settings.field.overlay_side.label",
        Controller,
        "settings.group.overlay",
        Choice(OVERLAY_SIDE),
    )
    .keywords(&["overlay", "position", "left", "right", "centre", "center"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.screenshotChord",
        "settings.field.overlay_screenshot_chord.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_screenshot_chord.help")
    .keywords(&["overlay", "guide", "rb", "r1", "screenshot", "shortcut"])
    .requires(NEEDS_OVERLAY),
    // Its own key, so it needs no overlay: Windows and macOS (Linux binds
    // `irori --screenshot` through the replay buffer's shortcuts).
    app(
        "iroriOverlay.screenshotKey",
        "settings.field.overlay_screenshot_key.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_screenshot_key.help")
    .keywords(&["screenshot", "shortcut", "hotkey", "key", "capture"]),
    app(
        "iroriOverlay.volumeChords",
        "settings.field.overlay_volume_chords.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_volume_chords.help")
    .keywords(&["overlay", "guide", "volume", "shortcut"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.momentChord",
        "settings.field.overlay_moment_chord.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_moment_chord.help")
    .keywords(&[
        "overlay",
        "guide",
        "y",
        "triangle",
        "journal",
        "moment",
        "screenshot",
        "shortcut",
    ])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.volumeStep",
        "settings.field.overlay_volume_step.label",
        Controller,
        "settings.group.overlay",
        Text(TextKind::Number { min: 1, max: 25 }),
    )
    .help("settings.field.overlay_volume_step.help")
    .keywords(&["overlay", "volume", "step", "percent"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.levelNotice",
        "settings.field.overlay_level_notice.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_level_notice.help")
    .keywords(&["overlay", "volume", "level", "osd", "notice"])
    .requires(NEEDS_OVERLAY),
    // Windows only (`supported_on_host`): Guide reaches Irori elsewhere.
    app(
        "iroriOverlay.backStart",
        "settings.field.overlay_back_start.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_back_start.help")
    .keywords(&[
        "overlay",
        "back",
        "start",
        "view",
        "menu",
        "select",
        "xinput",
        "controller",
    ])
    .requires(NEEDS_OVERLAY),
];
