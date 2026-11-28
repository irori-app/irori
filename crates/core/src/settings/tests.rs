use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::Runner;

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
    assert!(ui.program_icon_art);
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
    let mako: LsfgSettings = serde_json::from_str(r#"{"enabled": true, "layer": "mako"}"#).unwrap();
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
    let app: AppSettings = serde_json::from_str(r#"{"iroriCheckForAppUpdates": false}"#).unwrap();
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
    assert_eq!(app.deals_region, "");
    assert_eq!(app.deals_layout, DealsLayout::Grid);
    assert!(app.deals_hide_owned);
    let app: AppSettings = serde_json::from_str(
        r#"{"iroriDeals": false, "iroriDealsRegion": "BR", "iroriDealsLayout": "list", "iroriDealsHideOwned": false}"#,
    )
    .unwrap();
    assert!(!app.deals);
    assert_eq!(app.deals_region, "BR");
    assert_eq!(app.deals_layout, DealsLayout::List);
    assert!(!app.deals_hide_owned);
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
    let ui: UiPrefs = serde_json::from_str(r#"{"coverShape":"hexagon","tileSize":9.0}"#).unwrap();
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
        serde_json::from_str(r#"{"storeBadgesAlways":true,"storeBadgesOneStore":true}"#).unwrap();
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
        serde_json::from_str(r#"{"bigPictureHoldButton":"select","bigPictureHoldMs":20}"#).unwrap();
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
    let newer: OverlayPrefs = serde_json::from_str(r#"{"side":"top","volumeStep":90}"#).unwrap();
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

#[test]
fn web_download_prefs_default_round_trip_and_folder_fallbacks() {
    let app: AppSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(app.web_downloads, WebDownloadPrefs::default());
    assert_eq!(app.web_downloads.default_folder, "");
    assert_eq!(app.web_downloads.max_concurrent, 3);
    assert_eq!(app.web_downloads.clamped_max_concurrent(), 3);
    assert!(app.web_downloads.auto_extract);
    assert!(!app.web_downloads.delete_archive_after_extract);
    assert!(app.web_downloads.auto_add_to_library);
    assert!(!app.web_downloads.bittorrent, "BitTorrent must be opt-in");
    assert_eq!(app.web_downloads.aria2_path, "");

    let custom: AppSettings = serde_json::from_str(
        r#"{
            "iroriWebDownloads": {
                "defaultFolder": "/downloads/custom",
                "maxConcurrent": 99,
                "autoExtract": false,
                "deleteArchiveAfterExtract": true,
                "autoAddToLibrary": false,
                "bittorrent": true,
                "aria2Path": "/usr/local/bin/aria2c"
            }
        }"#,
    )
    .unwrap();
    assert_eq!(custom.web_downloads.default_folder, "/downloads/custom");
    assert_eq!(custom.web_downloads.max_concurrent, 99);
    assert_eq!(custom.web_downloads.clamped_max_concurrent(), 5);
    assert!(!custom.web_downloads.auto_extract);
    assert!(custom.web_downloads.delete_archive_after_extract);
    assert!(!custom.web_downloads.auto_add_to_library);
    assert!(custom.web_downloads.bittorrent);
    assert_eq!(custom.web_downloads.aria2_path, "/usr/local/bin/aria2c");

    let zero: WebDownloadPrefs = serde_json::from_str(r#"{"maxConcurrent": 0}"#).unwrap();
    assert_eq!(zero.clamped_max_concurrent(), 1);

    let json = serde_json::to_value(&custom).unwrap();
    assert_eq!(
        json["iroriWebDownloads"]["defaultFolder"],
        "/downloads/custom"
    );
    assert_eq!(json["iroriWebDownloads"]["maxConcurrent"], 99);
    assert_eq!(json["iroriWebDownloads"]["autoExtract"], false);
    assert_eq!(json["iroriWebDownloads"]["deleteArchiveAfterExtract"], true);
    assert_eq!(json["iroriWebDownloads"]["autoAddToLibrary"], false);
    assert_eq!(json["iroriWebDownloads"]["bittorrent"], true);
    assert_eq!(
        json["iroriWebDownloads"]["aria2Path"],
        "/usr/local/bin/aria2c"
    );
    let back: AppSettings = serde_json::from_value(json).unwrap();
    assert_eq!(back.web_downloads, custom.web_downloads);

    // Default folder resolution precedence:
    // configured -> portable/Downloads -> os_downloads/Irori -> home/Downloads/Irori -> default_install_path.
    assert_eq!(
        resolve_web_download_folder_with(
            "/explicit",
            Some(PathBuf::from("/portable")),
            Some(PathBuf::from("/os-dl")),
            Some(PathBuf::from("/home/u")),
            "/games",
        ),
        PathBuf::from("/explicit")
    );
    assert_eq!(
        resolve_web_download_folder_with(
            "",
            Some(PathBuf::from("/portable")),
            Some(PathBuf::from("/os-dl")),
            Some(PathBuf::from("/home/u")),
            "/games",
        ),
        PathBuf::from("/portable/Downloads")
    );
    assert_eq!(
        resolve_web_download_folder_with(
            "",
            None,
            Some(PathBuf::from("/os-dl")),
            Some(PathBuf::from("/home/u")),
            "/games",
        ),
        PathBuf::from("/os-dl/Irori")
    );
    assert_eq!(
        resolve_web_download_folder_with("", None, None, Some(PathBuf::from("/home/u")), "/games",),
        PathBuf::from("/home/u/Downloads/Irori")
    );
    assert_eq!(
        resolve_web_download_folder_with("", None, None, None, "/games"),
        PathBuf::from("/games")
    );
}

#[test]
fn list_columns_defaults_and_round_trip() {
    let cols = ListColumns::default();
    assert!(cols.store);
    assert!(!cols.size);
    assert!(cols.playtime);
    assert!(cols.last_played);
    assert!(cols.status);
    assert_eq!(cols.compat, cfg!(target_os = "linux"));
    let ui: UiPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(ui.library_list_columns, cols);
    assert_eq!(ui.cover_compat_badges, cfg!(target_os = "linux"));
    let custom = UiPrefs {
        cover_compat_badges: !cfg!(target_os = "linux"),
        library_list_columns: ListColumns {
            store: false,
            size: true,
            playtime: false,
            last_played: true,
            status: false,
            compat: true,
        },
        ..UiPrefs::default()
    };
    let value = serde_json::to_value(&custom).unwrap();
    assert_eq!(value["libraryListColumns"]["size"], Value::Bool(true));
    assert_eq!(value["libraryListColumns"]["store"], Value::Bool(false));
    assert_eq!(value["libraryListColumns"]["compat"], Value::Bool(true));
    assert_eq!(
        value["coverCompatBadges"],
        Value::Bool(!cfg!(target_os = "linux"))
    );
    let back: UiPrefs = serde_json::from_value(value).unwrap();
    assert_eq!(back.library_list_columns, custom.library_list_columns);
    assert_eq!(back.cover_compat_badges, custom.cover_compat_badges);
}

#[test]
fn list_row_size_defaults_round_trips_and_falls_back_on_unknown() {
    let ui: UiPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(ui.library_list_row_size, ListRowSize::Medium);
    let ui: UiPrefs = serde_json::from_str(r#"{"libraryListRowSize":"compact"}"#).unwrap();
    assert_eq!(ui.library_list_row_size, ListRowSize::Compact);
    let ui: UiPrefs = serde_json::from_str(r#"{"libraryListRowSize":"large"}"#).unwrap();
    assert_eq!(ui.library_list_row_size, ListRowSize::Large);
    let value = serde_json::to_value(&ui).unwrap();
    assert_eq!(value["libraryListRowSize"], Value::String("large".into()));
    let unknown: UiPrefs = serde_json::from_str(r#"{"libraryListRowSize":"giant"}"#).unwrap();
    assert_eq!(unknown.library_list_row_size, ListRowSize::Medium);
}

#[test]
fn greeting_and_hover_caption_defaults_and_round_trip() {
    let ui: UiPrefs = serde_json::from_str("{}").unwrap();
    assert!(ui.library_greeting);
    assert_eq!(ui.greeting_name, None);
    assert_eq!(ui.library_focus_cover_caption, None);
    assert_eq!(ui.focus_cover_caption(), CoverCaption::Full);

    let custom = UiPrefs {
        cover_caption: CoverCaption::Hover,
        library_focus_cover_caption: Some(CoverCaption::Title),
        library_greeting: false,
        greeting_name: Some("Alex".into()),
        ..UiPrefs::default()
    };
    assert_eq!(custom.focus_cover_caption(), CoverCaption::Title);
    let value = serde_json::to_value(&custom).unwrap();
    assert_eq!(value["coverCaption"], Value::String("hover".into()));
    assert_eq!(
        value["libraryFocusCoverCaption"],
        Value::String("title".into())
    );
    assert_eq!(value["libraryGreeting"], Value::Bool(false));
    assert_eq!(value["greetingName"], Value::String("Alex".into()));
    let back: UiPrefs = serde_json::from_value(value).unwrap();
    assert_eq!(back.cover_caption, CoverCaption::Hover);
    assert_eq!(back.library_focus_cover_caption, Some(CoverCaption::Title));
    assert!(!back.library_greeting);
    assert_eq!(back.greeting_name.as_deref(), Some("Alex"));
}
