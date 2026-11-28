//! Declarative layout of the application settings views: sidebar grouping,
//! horizontal sub-tabs for long pages, progressive-disclosure tiers,
//! dependent-field hiding, group descriptions, and side-by-side field pairs.
//!
//! Per-game settings (`FormTarget::Game`) and global settings search bypass
//! sub-tabs, parent hiding, and collapsed "More options" blocks so every
//! setting remains directly searchable and per-game behavior stays unchanged.

use serde_json::Value;

use crate::registry::{FieldSection, FieldSpec};
use crate::value::{bool_at, get_path};
use crate::view::SettingsSection;

/// One labeled group of sections in the settings sidebar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SidebarGroup {
    pub label_key: &'static str,
    pub sections: &'static [SettingsSection],
}

/// Sidebar sections grouped for quick scanning without changing route IDs.
pub(crate) const SIDEBAR_GROUPS: &[SidebarGroup] = &[
    SidebarGroup {
        label_key: "settings.sidebar.setup",
        sections: &[
            SettingsSection::Accounts,
            SettingsSection::WineManager,
            SettingsSection::Emulators,
        ],
    },
    SidebarGroup {
        label_key: "settings.sidebar.preferences",
        sections: &[
            SettingsSection::General,
            SettingsSection::Controller,
            SettingsSection::GameDefaults,
            SettingsSection::PerformanceHud,
            SettingsSection::Appearance,
        ],
    },
    SidebarGroup {
        label_key: "settings.sidebar.data",
        sections: &[
            SettingsSection::Storage,
            SettingsSection::Saves,
            SettingsSection::Backup,
        ],
    },
    SidebarGroup {
        label_key: "settings.sidebar.system",
        sections: &[
            SettingsSection::Advanced,
            SettingsSection::Logs,
            SettingsSection::SystemInfo,
            SettingsSection::About,
        ],
    },
];

/// One horizontal sub-tab inside a settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubTab {
    pub id: &'static str,
    pub label_key: &'static str,
}

/// Sub-tabs of Settings -> General.
pub(crate) const GENERAL_TABS: &[SubTab] = &[
    SubTab {
        id: "basics",
        label_key: "settings.subtab.general.basics",
    },
    SubTab {
        id: "window",
        label_key: "settings.subtab.general.window",
    },
    SubTab {
        id: "library",
        label_key: "settings.subtab.general.library",
    },
    SubTab {
        id: "integrations",
        label_key: "settings.subtab.general.integrations",
    },
];

/// Sub-tabs of Settings -> Game defaults.
pub(crate) const DEFAULTS_TABS: &[SubTab] = &[
    SubTab {
        id: "wine",
        label_key: "settings.subtab.defaults.wine",
    },
    SubTab {
        id: "performance",
        label_key: "settings.subtab.defaults.performance",
    },
    SubTab {
        id: "replay",
        label_key: "settings.subtab.defaults.replay",
    },
    SubTab {
        id: "gpu",
        label_key: "settings.subtab.defaults.gpu",
    },
    SubTab {
        id: "launch",
        label_key: "settings.subtab.defaults.launch",
    },
    SubTab {
        id: "saves",
        label_key: "settings.subtab.defaults.saves",
    },
];

/// Horizontal sub-tabs inside a single game's Performance section.
pub(crate) const GAME_PERFORMANCE_TABS: &[SubTab] = &[
    SubTab {
        id: "performance",
        label_key: "settings.subtab.defaults.performance",
    },
    SubTab {
        id: "replay",
        label_key: "settings.subtab.defaults.replay",
    },
];

/// Sub-tabs of Settings -> Backup & sync.
pub(crate) const BACKUP_TABS: &[SubTab] = &[
    SubTab {
        id: "backup_restore",
        label_key: "settings.subtab.backup.backup_restore",
    },
    SubTab {
        id: "sync",
        label_key: "settings.subtab.backup.sync",
    },
    SubTab {
        id: "export",
        label_key: "settings.subtab.backup.export",
    },
];

/// Map a Game defaults sub-tab ID to its [`FieldSection`].
pub(crate) fn defaults_section_for_tab(tab: &str) -> FieldSection {
    match tab {
        "wine" => FieldSection::Wine,
        "performance" | "replay" => FieldSection::Performance,
        "gpu" => FieldSection::Gpu,
        "launch" => FieldSection::Launch,
        "saves" => FieldSection::CloudSaves,
        _ => FieldSection::Wine,
    }
}

/// Sub-tabs of `Settings -> Game defaults` that have at least one supported
/// shared field on the current host.
pub(crate) fn supported_defaults_tabs() -> Vec<SubTab> {
    DEFAULTS_TABS
        .iter()
        .copied()
        .filter(|tab| {
            let section = defaults_section_for_tab(tab.id);
            crate::registry::FIELDS.iter().any(|f| {
                f.section == section
                    && f.scope == crate::registry::Scope::Shared
                    && f.supported_on_host()
                    && subtab_of(f) == Some(tab.id)
            })
        })
        .collect()
}

/// Horizontal sub-tabs of a game's Performance section supported on the
/// current host (`Replay` is Linux-only).
pub(crate) fn supported_game_performance_tabs() -> Vec<SubTab> {
    GAME_PERFORMANCE_TABS
        .iter()
        .copied()
        .filter(|tab| {
            crate::registry::FIELDS.iter().any(|f| {
                f.section == FieldSection::Performance
                    && f.supported_on_host()
                    && subtab_of(f) == Some(tab.id)
            })
        })
        .collect()
}

/// Which sub-tab a registry field belongs to in compact App Settings mode
/// and in a game's Performance sub-tabs.
pub(crate) fn subtab_of(spec: &FieldSpec) -> Option<&'static str> {
    match spec.section {
        FieldSection::Wine => Some("wine"),
        FieldSection::Performance => {
            if spec.path == "panel.replay" || spec.path.starts_with("iroriReplay.") {
                Some("replay")
            } else {
                Some("performance")
            }
        }
        FieldSection::Gpu => Some("gpu"),
        FieldSection::Launch => Some("launch"),
        FieldSection::CloudSaves => Some("saves"),
        FieldSection::General => general_subtab(spec.path),
        // Appearance, Controller, Advanced, and Saves are compact enough to stay flat.
        FieldSection::Appearance
        | FieldSection::Advanced
        | FieldSection::Saves
        | FieldSection::Artwork
        | FieldSection::Controller => None,
    }
}

/// Short choice fields rendered as one-click segmented buttons instead of
/// dropdown selects.
pub(crate) fn is_segmented_choice(path: &str) -> bool {
    matches!(
        path,
        "iroriUi.theme"
            | "iroriUi.translucency"
            | "iroriUi.density"
            | "iroriUi.libraryBackground"
            | "iroriUi.coverFit"
            | "iroriUi.coverHover"
            | "iroriUi.libraryFocusColumns"
    )
}

fn general_subtab(path: &str) -> Option<&'static str> {
    match path {
        "uiLanguage"
        | "defaultInstallPath"
        | "iroriInstallPerStore"
        | "panel.libraryFolders"
        | "defaultWinePrefixDir"
        | "defaultSteamPath"
        | "iroriCrossoverBottlesPath"
        | "iroriCheckForAppUpdates"
        | "checkForUpdatesOnStartup"
        | "checkUpdatesInterval"
        | "iroriLaunchersRescan"
        | "autoUpdateGames"
        | "iroriAutoUpdateComponents"
        | "iroriAutoUpdateFrom"
        | "iroriAutoUpdateUntil"
        | "iroriAutoUpdateUnmetered" => Some("basics"),
        "noTrayIcon"
        | "exitToTray"
        | "startInTray"
        | "iroriUi.startMaximized"
        | "iroriStartWithSystem"
        | "iroriStartWithSystemMinimized"
        | "minimizeOnLaunch"
        | "iroriShowLogOnLaunch"
        | "darkTrayIcon"
        | "hideWindowOnProtocolLaunch"
        | "startInConsoleMode"
        | "iroriUi.bigPictureInGameMode"
        | "iroriUi.bigPictureOnController"
        | "iroriUi.bigPictureFullscreen"
        | "iroriUi.quickLaunchShortcut"
        | "panel.quick_launch"
        | "iroriUi.middleClickScroll" => Some("window"),
        "libraryTopSection"
        | "maxRecentGames"
        | "iroriUi.groupCopies"
        | "iroriUi.showAgeRatings"
        | "iroriUi.ageRatingBoard"
        | "iroriUi.trailerAutoplay"
        | "iroriUi.libraryOpenDetailsOnSelect"
        | "iroriAchievementNotices"
        | "iroriAchievementToast.overGame"
        | "iroriAchievementToast.showGame"
        | "iroriAchievementToast.showDescription"
        | "iroriAchievementToast.showRarity"
        | "iroriAchievementToast.showProgress"
        | "iroriAchievementToast.showPoints"
        | "iroriFileAchievements"
        | "iroriUi.stores.epic"
        | "iroriUi.stores.gog"
        | "iroriUi.stores.amazon"
        | "iroriDeals"
        | "zoomPlatform"
        | "iroriItchIo"
        | "iroriXbox"
        | "iroriXboxMarket"
        | "addDesktopShortcuts"
        | "addStartMenuShortcuts"
        | "addSteamShortcuts"
        | "iroriShortcutIconFromExe" => Some("library"),
        "iroriNetworkMode"
        | "iroriNetworkTimeout"
        | "maxWorkers"
        | "maxDownloadSpeed"
        | "downloadNoHttps"
        | "discordRPC"
        | "discordClientId"
        | "iroriSunshine.enabled"
        | "iroriSunshine.games"
        | "iroriSunshine.bigPicture"
        | "iroriSunshine.address"
        | "iroriSunshine.matchClient"
        | "panel.sunshine"
        | "cometSupport"
        | "disablePlaytimeSync"
        | "disableGOGPresence"
        | "action.open-windows-files" => Some("integrations"),
        _ => None,
    }
}

/// Whether `spec` lives inside the collapsible "More options" block of its
/// page or sub-tab. Pages without disclosure groups (`Advanced`, `Saves`) stay flat.
pub(crate) fn is_advanced(spec: &FieldSpec) -> bool {
    matches!(
        spec.path,
        // General -> Basics
        "checkUpdatesInterval"
            | "iroriLaunchersRescan"
            // General -> Window & tray
            | "iroriShowLogOnLaunch"
            | "hideWindowOnProtocolLaunch"
            | "startInConsoleMode"
            | "iroriUi.bigPictureInGameMode"
            | "iroriUi.bigPictureOnController"
            | "iroriUi.bigPictureFullscreen"
            // General -> Library & stores
            | "iroriUi.ageRatingBoard"
            | "iroriUi.libraryOpenDetailsOnSelect"
            | "iroriAchievementToast.showGame"
            | "iroriAchievementToast.showDescription"
            | "iroriAchievementToast.showRarity"
            | "iroriAchievementToast.showProgress"
            | "iroriAchievementToast.showPoints"
            | "iroriFileAchievements"
            | "iroriShortcutIconFromExe"
            // General -> Integrations & downloads
            | "iroriNetworkTimeout"
            | "downloadNoHttps"
            | "discordClientId"
            | "action.open-windows-files"
            // Controller
            | "gamepadInitialRepeatDelay"
            | "gamepadRepeatDelay"
            | "iroriUi.bigPictureHoldMs"
            // Appearance
            | "iroriUi.wallpaperColors"
            | "iroriUi.wallpaperColorsFile"
            | "iroriUi.libraryQuickControls"
            | "iroriUi.coverFit"
            | "iroriUi.coverHover"
            | "iroriUi.storeBadgesOneStore"
            | "iroriUi.libraryFocusColumns"
            | "iroriUi.libraryAutoLayout"
            | "iroriUi.programIconArt"
            // Game defaults -> Wine & Proton
            | "disableUMU"
            | "iroriProtonPreferSdl"
            | "iroriDllOverrides"
            | "enableEsync"
            | "enableFsync"
            | "iroriNtsync"
            | "enableMsync"
            | "iroriRetinaMode"
            | "enableWineWayland"
            | "enableHDR"
            | "enableFSR"
            | "maxSharpness"
            | "enableWoW64"
            | "autoInstallDxvk"
            | "autoInstallVkd3d"
            | "autoInstallDxvkNvapi"
            | "iroriDxvkVersion"
            | "iroriVkd3dVersion"
            | "iroriDescriptorHeap"
            | "preferSystemLibs"
            // Game defaults -> Performance
            | "gamescope.additionalOptions"
            // Game defaults -> Graphics
            | "iroriDlssIndicator"
            | "iroriNvidiaReflex"
            | "iroriShaderCacheSize"
            // Game defaults -> Launch
            | "wrapperOptions"
            | "enviromentOptions"
            | "beforeLaunchScriptPath"
            | "iroriBeforeLaunchScriptAlongside"
            | "afterLaunchScriptPath"
            | "panel.launchPrograms"
            | "iroriSandbox.noNetwork"
            | "iroriSandbox.noHome"
            | "verboseLogs"
    )
}

/// Whether `spec` should stay hidden in compact App Settings mode because its
/// parent feature is currently turned off.
pub(crate) fn is_hidden_by_parent(spec: &FieldSpec, snapshot: &Value) -> bool {
    if spec
        .requires
        .iter()
        .any(|c| bool_at(snapshot, c.path) != c.equals)
    {
        return true;
    }
    match spec.path {
        "iroriAutoUpdateFrom" | "iroriAutoUpdateUntil" | "iroriAutoUpdateUnmetered" => {
            !bool_at(snapshot, "autoUpdateGames") && !bool_at(snapshot, "iroriAutoUpdateComponents")
        }
        "panel.sunshine" => !bool_at(snapshot, "iroriSunshine.enabled"),
        "panel.igdb" => !bool_at(snapshot, "iroriIgdb"),
        "iroriUi.libraryBackgroundPicture" => {
            get_path(snapshot, "iroriUi.libraryBackground").and_then(Value::as_str)
                != Some("picture")
        }
        "iroriUi.libraryBackgroundStrength" | "iroriUi.libraryBackgroundDim" => matches!(
            get_path(snapshot, "iroriUi.libraryBackground").and_then(Value::as_str),
            None | Some("" | "none")
        ),
        "iroriUi.coverFit" => !matches!(
            get_path(snapshot, "iroriUi.coverShape").and_then(Value::as_str),
            None | Some("" | "portrait")
        ),
        _ => false,
    }
}

/// Shared one-line group description shown under selected group headers so
/// sibling rows do not have to repeat the same explanation.
pub(crate) fn group_description_key(group_key: &str) -> Option<&'static str> {
    match group_key {
        "settings.group.stores" => Some("settings.group_desc.stores"),
        "settings.group.upscalers" => Some("settings.group_desc.upscalers"),
        "settings.group.wine_sync" => Some("settings.group_desc.wine_sync"),
        _ => None,
    }
}

/// Two related short fields laid out side by side on a single row in compact
/// App Settings mode.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FieldPair {
    pub first: &'static str,
    pub second: &'static str,
    pub label_key: &'static str,
    pub help_key: &'static str,
    pub tooltip_key: &'static str,
    pub first_sublabel_key: &'static str,
    pub second_sublabel_key: &'static str,
    pub separator: Option<&'static str>,
}

pub(crate) const FIELD_PAIRS: &[FieldPair] = &[
    FieldPair {
        first: "iroriAutoUpdateFrom",
        second: "iroriAutoUpdateUntil",
        label_key: "settings.pair.update_hours",
        help_key: "settings.pair.update_hours_help",
        tooltip_key: "settings.pair.update_hours.tip",
        first_sublabel_key: "settings.pair.from",
        second_sublabel_key: "settings.pair.until",
        separator: None,
    },
    FieldPair {
        first: "gamescope.gameWidth",
        second: "gamescope.gameHeight",
        label_key: "settings.pair.game_resolution",
        help_key: "",
        tooltip_key: "settings.pair.game_resolution.tip",
        first_sublabel_key: "settings.pair.width",
        second_sublabel_key: "settings.pair.height",
        separator: Some("×"),
    },
    FieldPair {
        first: "gamescope.upscaleWidth",
        second: "gamescope.upscaleHeight",
        label_key: "settings.pair.output_resolution",
        help_key: "",
        tooltip_key: "settings.pair.output_resolution.tip",
        first_sublabel_key: "settings.pair.width",
        second_sublabel_key: "settings.pair.height",
        separator: Some("×"),
    },
    FieldPair {
        first: "gamescope.fpsLimiter",
        second: "gamescope.fpsLimiterNoFocus",
        label_key: "settings.pair.fps_limits",
        help_key: "",
        tooltip_key: "settings.pair.fps_limits.tip",
        first_sublabel_key: "settings.pair.active",
        second_sublabel_key: "settings.pair.unfocused",
        separator: None,
    },
    FieldPair {
        first: "gamepadInitialRepeatDelay",
        second: "gamepadRepeatDelay",
        label_key: "settings.pair.controller_repeat",
        help_key: "settings.pair.controller_repeat_help",
        tooltip_key: "settings.pair.controller_repeat.tip",
        first_sublabel_key: "settings.pair.delay",
        second_sublabel_key: "settings.pair.interval",
        separator: None,
    },
];

/// Return the [`FieldPair`] that starts with `first` followed immediately by
/// `second`.
pub(crate) fn match_pair(first: &str, second: &str) -> Option<&'static FieldPair> {
    FIELD_PAIRS
        .iter()
        .find(|p| p.first == first && p.second == second)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::registry::{FIELDS, Scope, find};

    use super::*;

    #[test]
    fn every_app_and_shared_field_on_subtabbed_pages_has_a_subtab() {
        for f in FIELDS {
            let needs_tab = match f.section {
                FieldSection::General => f.scope == Scope::App,
                FieldSection::Wine
                | FieldSection::Performance
                | FieldSection::Gpu
                | FieldSection::Launch
                | FieldSection::CloudSaves => f.scope == Scope::Shared,
                _ => false,
            };
            if !needs_tab {
                continue;
            }
            let tab = subtab_of(f).unwrap_or_else(|| panic!("{} lacks a sub-tab", f.path));
            let valid = match f.section {
                FieldSection::General => GENERAL_TABS.iter().any(|t| t.id == tab),
                _ => DEFAULTS_TABS
                    .iter()
                    .any(|t| t.id == tab && defaults_section_for_tab(t.id) == f.section),
            };
            assert!(valid, "{} mapped to invalid sub-tab {tab}", f.path);
        }
    }

    #[test]
    fn sidebar_groups_cover_every_settings_section_once() {
        let mut seen = HashSet::new();
        for group in SIDEBAR_GROUPS {
            for &s in group.sections {
                assert!(seen.insert(s), "duplicate sidebar section {s:?}");
            }
        }
        assert_eq!(seen.len(), SettingsSection::ALL.len());
    }

    #[test]
    fn field_pairs_share_section_subtab_and_tier() {
        for pair in FIELD_PAIRS {
            let a = find(pair.first).unwrap_or_else(|| panic!("missing {}", pair.first));
            let b = find(pair.second).unwrap_or_else(|| panic!("missing {}", pair.second));
            assert_eq!(a.section, b.section, "{} vs {}", a.path, b.path);
            assert_eq!(a.group, b.group, "{} vs {}", a.path, b.group);
            assert_eq!(subtab_of(a), subtab_of(b), "{} vs {}", a.path, b.path);
            assert_eq!(
                is_advanced(a),
                is_advanced(b),
                "pair tier mismatch: {} vs {}",
                a.path,
                b.path
            );
        }
    }

    #[test]
    fn hidden_by_parent_follows_parent_state() {
        let off = serde_json::json!({
            "autoUpdateGames": false,
            "iroriAutoUpdateComponents": false,
            "iroriSunshine": { "enabled": false },
            "iroriIgdb": false,
            "iroriUi": { "libraryBackground": "none" },
            "gamescope": { "iroriEnabled": false }
        });
        assert!(is_hidden_by_parent(
            find("iroriAutoUpdateFrom").unwrap(),
            &off
        ));
        assert!(is_hidden_by_parent(find("panel.sunshine").unwrap(), &off));
        assert!(is_hidden_by_parent(find("panel.igdb").unwrap(), &off));
        assert!(is_hidden_by_parent(
            find("iroriUi.libraryBackgroundPicture").unwrap(),
            &off
        ));
        assert!(is_hidden_by_parent(
            find("iroriUi.libraryBackgroundStrength").unwrap(),
            &off
        ));
        assert!(is_hidden_by_parent(
            find("gamescope.gameWidth").unwrap(),
            &off
        ));

        let game_art = serde_json::json!({
            "iroriUi": { "libraryBackground": "game_art" }
        });
        assert!(is_hidden_by_parent(
            find("iroriUi.libraryBackgroundPicture").unwrap(),
            &game_art
        ));
        assert!(!is_hidden_by_parent(
            find("iroriUi.libraryBackgroundStrength").unwrap(),
            &game_art
        ));

        let picture = serde_json::json!({
            "iroriUi": { "libraryBackground": "picture" },
            "autoUpdateGames": true,
            "iroriSunshine": { "enabled": true },
            "iroriIgdb": true,
            "gamescope": { "iroriEnabled": true }
        });
        assert!(!is_hidden_by_parent(
            find("iroriUi.libraryBackgroundPicture").unwrap(),
            &picture
        ));
        assert!(!is_hidden_by_parent(
            find("iroriAutoUpdateFrom").unwrap(),
            &picture
        ));
        assert!(!is_hidden_by_parent(
            find("panel.sunshine").unwrap(),
            &picture
        ));
        assert!(!is_hidden_by_parent(find("panel.igdb").unwrap(), &picture));
        assert!(!is_hidden_by_parent(
            find("gamescope.gameWidth").unwrap(),
            &picture
        ));

        let square_covers = serde_json::json!({
            "iroriUi": { "coverShape": "square" }
        });
        let portrait_covers = serde_json::json!({
            "iroriUi": { "coverShape": "portrait" }
        });
        assert!(is_hidden_by_parent(
            find("iroriUi.coverFit").unwrap(),
            &square_covers
        ));
        assert!(!is_hidden_by_parent(
            find("iroriUi.coverFit").unwrap(),
            &portrait_covers
        ));
    }

    #[test]
    fn layout_keys_exist_in_english_catalog() {
        crate::i18n::register();
        for g in SIDEBAR_GROUPS {
            assert!(
                !crate::i18n::english(g.label_key).is_empty(),
                "{}",
                g.label_key
            );
        }
        for t in GENERAL_TABS
            .iter()
            .chain(DEFAULTS_TABS)
            .chain(GAME_PERFORMANCE_TABS)
            .chain(BACKUP_TABS)
        {
            assert!(
                !crate::i18n::english(t.label_key).is_empty(),
                "{}",
                t.label_key
            );
        }
        for p in FIELD_PAIRS {
            assert!(
                !crate::i18n::english(p.label_key).is_empty(),
                "{}",
                p.label_key
            );
            if !p.help_key.is_empty() {
                assert!(
                    !crate::i18n::english(p.help_key).is_empty(),
                    "{}",
                    p.help_key
                );
            }
            if !p.tooltip_key.is_empty() {
                assert!(
                    !crate::i18n::english(p.tooltip_key).is_empty(),
                    "{}",
                    p.tooltip_key
                );
            }
            assert!(
                !crate::i18n::english(p.first_sublabel_key).is_empty(),
                "{}",
                p.first_sublabel_key
            );
            assert!(
                !crate::i18n::english(p.second_sublabel_key).is_empty(),
                "{}",
                p.second_sublabel_key
            );
        }
    }

    #[test]
    fn performance_and_replay_fields_split_into_separate_subtabs() {
        assert_eq!(
            subtab_of(find("panel.performance-hud").unwrap()),
            Some("performance")
        );
        assert_eq!(
            subtab_of(find("panel.optiscaler").unwrap()),
            Some("performance")
        );
        assert_eq!(subtab_of(find("panel.replay").unwrap()), Some("replay"));
        assert_eq!(
            subtab_of(find("iroriReplay.enabled").unwrap()),
            Some("replay")
        );
        let tabs: Vec<_> = supported_game_performance_tabs()
            .into_iter()
            .map(|t| t.id)
            .collect();
        if cfg!(target_os = "linux") {
            assert_eq!(tabs, ["performance", "replay"]);
        } else {
            assert_eq!(tabs, ["performance"]);
        }
    }
}
