use super::*;
use crate::value::get_path;
use irori_core::{AppSettings, GameSettings};
use irori_ui::i18n::LANGUAGES as UI_LANGUAGES;
use serde_json::Value;
use std::collections::HashSet;

#[test]
fn paths_are_unique() {
    let mut seen = HashSet::new();
    for f in FIELDS {
        assert!(seen.insert(f.path), "duplicate field {}", f.path);
    }
}

#[test]
fn every_value_field_exists_in_the_schema() {
    let app = serde_json::to_value(AppSettings::default()).unwrap();
    let game = serde_json::to_value(GameSettings::default()).unwrap();
    for f in FIELDS {
        if matches!(f.kind, FieldKind::Action(_)) {
            assert!(f.path.starts_with("action."), "{}", f.path);
            continue;
        }
        if matches!(f.kind, FieldKind::Panel(_)) {
            assert!(f.path.starts_with("panel."), "{}", f.path);
            continue;
        }
        assert!(!f.path.starts_with("panel.") && !f.path.starts_with("action."));
        let root = if f.scope == Scope::App { &app } else { &game };
        assert!(
            get_path(root, f.path).is_some(),
            "{} is not a settings key",
            f.path
        );
        // Game fields must also resolve in the flattened app settings
        // (game defaults are edited there).
        if f.scope != Scope::App {
            assert!(get_path(&app, f.path).is_some(), "{}", f.path);
        }
    }
}

#[test]
fn conditions_point_at_booleans() {
    let app = serde_json::to_value(AppSettings::default()).unwrap();
    for f in FIELDS {
        for c in f.requires {
            // Gamescope's switch stays unset until first used (`bool_at`).
            assert!(
                c.path == "gamescope.iroriEnabled"
                    || matches!(get_path(&app, c.path), Some(Value::Bool(_))),
                "{} requires non-boolean {}",
                f.path,
                c.path
            );
        }
    }
}

#[test]
fn game_sections_hold_only_game_fields() {
    for f in FIELDS {
        assert_eq!(f.section.is_game(), f.scope != Scope::App, "{}", f.path);
    }
}

#[test]
fn search_matches_labels_keywords_and_tags() {
    crate::i18n::register();
    let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
    assert_eq!(
        paths("fsync"),
        if cfg!(target_os = "linux") {
            vec!["enableFsync"]
        } else {
            vec![]
        }
    );
    assert!(paths("TRAY").contains(&"exitToTray"));
    assert!(paths("epic").contains(&"downloadNoHttps"));
    assert!(paths("epic").contains(&"offlineMode"));
    // Every token must match.
    let fps = paths("gamescope fps");
    assert_eq!(
        fps.contains(&"gamescope.fpsLimiter"),
        cfg!(target_os = "linux")
    );
    // On Linux the frame limit goes through gamescope too, which its
    // help says.
    assert_eq!(
        fps.contains(&"enableDXVKFpsLimit"),
        cfg!(target_os = "linux")
    );
    assert!(paths("   ").is_empty());
    assert!(paths("no such setting").is_empty());
}

#[test]
fn search_finds_actions() {
    crate::i18n::register();
    assert!(search("heroic").any(|f| f.path == "action.import-heroic"));
    assert!(search("cache").any(|f| f.path == "action.clear-caches"));
}

#[test]
fn scope_filters_per_game_items() {
    let target = find("targetExe").unwrap();
    assert!(target.applies_to_game(true));
    assert!(!target.applies_to_game(false));
    let esync = find("enableEsync").unwrap();
    assert!(esync.applies_to_game(false) && esync.applies_to_game(true));
    assert!(!find("exitToTray").unwrap().applies_to_game(true));
    assert_eq!(
        find("gamescope.fpsLimiter").unwrap().override_key(),
        "gamescope"
    );
}

#[test]
fn mac_options_show_on_macos_only() {
    let mac = cfg!(target_os = "macos");
    for path in [
        "iroriRetinaMode",
        "wineCrossoverBottle",
        "iroriCrossoverBottlesPath",
    ] {
        assert_eq!(find(path).unwrap().supported_on_host(), mac, "{path}");
    }
    assert!(
        !find("iroriCrossoverBottlesPath")
            .unwrap()
            .applies_to_game(true)
    );
    assert!(find("iroriRetinaMode").unwrap().applies_to_game(true));
}

#[test]
fn gpu_options_live_in_the_graphics_section() {
    for path in [
        "iroriGpu",
        "nvidiaPrime",
        "iroriSmoothMotion",
        "iroriLsfg.enabled",
        "iroriLsfg.layer",
        "iroriLsfg.multiplier",
        "iroriLsfg.flowScale",
        "iroriLsfg.performanceMode",
        "iroriNvidiaReflex",
        "iroriDlssUpgrade",
        "iroriFsr4Upgrade",
        "iroriXessUpgrade",
        "iroriShaderCacheSize",
    ] {
        let f = find(path).unwrap_or_else(|| panic!("{path}"));
        assert_eq!(f.section, FieldSection::Gpu, "{path}");
        assert_eq!(f.scope, Scope::Shared, "{path}");
    }
    assert_eq!(find("panel.artwork").unwrap().scope, Scope::GameOnly);
    assert_eq!(
        find("panel.artwork").unwrap().section,
        FieldSection::Artwork
    );
    let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
    assert!(paths("frame generation").contains(&"iroriLsfg.enabled"));
    assert!(paths("frame generation").contains(&"iroriSmoothMotion"));
    assert!(paths("lossless").contains(&"iroriLsfg.multiplier"));
    // Each system shows one of the two card choosers.
    assert!(find("iroriGpu").unwrap().matches("graphics card"));
    let windows = find("iroriWindowsGpu").unwrap();
    assert!(windows.matches("graphics card") && windows.matches("high performance"));
    assert_eq!(
        paths("graphics card").contains(&"iroriWindowsGpu"),
        cfg!(windows)
    );
    assert_eq!(windows.section, FieldSection::Gpu);
    assert_eq!(windows.scope, Scope::Shared);
    assert_eq!(windows.supported_on_host(), cfg!(windows));
    assert_eq!(
        find("iroriGpu").unwrap().supported_on_host(),
        !cfg!(windows)
    );
    assert!(!windows.applies_to_steam());
    assert!(paths("steamgriddb").contains(&"panel.artwork"));
    assert!(paths("cover").contains(&"panel.artwork"));
}

#[test]
fn needs_say_what_is_missing() {
    use irori_core::HostPlatform::{Linux, Mac, Windows};
    use irori_runtime::graphics::{GpuAdapter, HostGraphics, LsfgVk};
    let amd = GpuAdapter {
        slot: "0000:03:00.0".into(),
        vendor: 0x1002,
        device: 0x744c,
        driver: "amdgpu".into(),
        boot_vga: true,
        model: None,
    };
    let one = HostGraphics {
        adapters: vec![amd.clone()],
        ..Default::default()
    };
    let cx = |host, graphics: Option<&'static HostGraphics>| NeedContext {
        host,
        graphics,
        chosen_gpu: false,
        native_game: false,
    };
    let one: &'static HostGraphics = Box::leak(Box::new(one));
    assert_eq!(Need::Linux.blocker(&cx(Linux, Some(one))), None);
    assert_eq!(
        Need::Linux.blocker(&cx(Mac, None)),
        Some("settings.needs.linux_mac")
    );
    assert_eq!(
        Need::Linux.blocker(&cx(Windows, None)),
        Some("settings.needs.linux_windows")
    );
    assert_eq!(
        Need::SeveralGpus.blocker(&cx(Linux, None)),
        Some("settings.needs.probing")
    );
    assert_eq!(
        Need::SeveralGpus.blocker(&cx(Linux, Some(one))),
        Some("settings.needs.several_gpus")
    );
    assert_eq!(
        Need::SmoothMotion.blocker(&cx(Linux, Some(one))),
        Some("settings.needs.nvidia")
    );
    assert_eq!(
        Need::NvidiaDriver.blocker(&cx(Linux, Some(one))),
        Some("settings.needs.nvidia")
    );
    assert_eq!(
        Need::Lsfg.blocker(&cx(Linux, Some(one))),
        Some("settings.needs.lsfg")
    );

    let mut two = one.clone();
    two.adapters.push(GpuAdapter {
        slot: "0000:01:00.0".into(),
        vendor: 0x10de,
        device: 0x2b85,
        driver: "nvidia".into(),
        boot_vga: false,
        model: None,
    });
    two.lsfg = Some(LsfgVk {
        manifest: "/m.json".into(),
        current: false,
        dll: None,
    });
    let two: &'static HostGraphics = Box::leak(Box::new(two));
    assert_eq!(Need::SeveralGpus.blocker(&cx(Linux, Some(two))), None);
    assert_eq!(Need::NvidiaDriver.blocker(&cx(Linux, Some(two))), None);
    assert_eq!(
        Need::SmoothMotion.blocker(&cx(Linux, Some(two))),
        Some("settings.needs.nvidia_575")
    );
    assert_eq!(
        Need::Lsfg.blocker(&cx(Linux, Some(two))),
        Some("settings.needs.lsfg_update")
    );
    // MAKO alone is enough: it needs no Lossless Scaling.
    let mako: &'static HostGraphics = Box::leak(Box::new(HostGraphics {
        mako: Some("/home/u/.local/bin/mako-launch".into()),
        ..one.clone()
    }));
    assert_eq!(Need::Lsfg.blocker(&cx(Linux, Some(mako))), None);

    let chosen = NeedContext {
        chosen_gpu: true,
        native_game: true,
        ..cx(Linux, Some(two))
    };
    assert_eq!(
        Need::NoChosenGpu.blocker(&chosen),
        Some("settings.needs.no_chosen_gpu")
    );
    assert_eq!(
        Need::WindowsGame.blocker(&chosen),
        Some("settings.needs.windows_game")
    );
    assert_eq!(Need::NoChosenGpu.blocker(&cx(Linux, None)), None);
    let gone = NeedContext {
        chosen_gpu: true,
        ..cx(Linux, Some(one))
    };
    assert_eq!(Need::SeveralGpus.blocker(&gone), None);
}

#[test]
fn native_build_choices_are_per_game() {
    let natives: Vec<_> = FIELDS.iter().filter(|f| f.native_only).collect();
    let paths: Vec<_> = natives.iter().map(|f| f.path).collect();
    assert_eq!(paths, ["steamRuntime", "iroriRosetta"]);
    for field in &natives {
        assert!(field.applies_to_game(true));
        assert_ne!(field.section, FieldSection::Wine);
    }
    // The game defaults offer the Steam runtime, so every native build
    // can start in it without setting each game; Rosetta is per game.
    assert!(natives[0].applies_to_game(false));
    assert!(!natives[1].applies_to_game(false));
    // Rosetta is a Mac's, the Steam runtime Linux's.
    let rosetta = FIELDS.iter().find(|f| f.path == "iroriRosetta").unwrap();
    assert_eq!(
        rosetta.supported_on_host(),
        irori_core::HostPlatform::current() == irori_core::HostPlatform::Mac
    );
}

#[test]
fn controller_and_epic_rows_are_findable() {
    crate::i18n::register();
    let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
    let pads = paths("controller");
    for p in [
        "panel.controllers",
        "disableController",
        "gamepadInitialRepeatDelay",
        "gamepadRepeatDelay",
        "iroriUi.bigPictureHoldButton",
        "iroriUi.bigPictureHoldMs",
        "iroriController.hidePointer",
        "panel.button-map",
    ] {
        assert!(pads.contains(&p), "{p}");
        assert_eq!(find(p).unwrap().section, FieldSection::Controller);
    }
    assert!(paths("rebind").contains(&"panel.button-map"));
    assert!(paths("egl").contains(&"panel.egl-sync"));
    assert!(paths("eos overlay").contains(&"panel.eos-overlay"));
    assert!(paths("discord").contains(&"discordClientId"));
    assert!(paths("sync saves").contains(&"panel.sync-saves"));
    assert_eq!(find("panel.sync-saves").unwrap().scope, Scope::GameOnly);
}

#[test]
fn language_choice_lists_every_interface_language_by_native_name() {
    let FieldKind::Choice(options) = find("uiLanguage").unwrap().kind else {
        panic!("uiLanguage is not a choice");
    };
    assert_eq!(options.len(), UI_LANGUAGES.len());
    for (o, lang) in options.iter().zip(UI_LANGUAGES) {
        assert_eq!(o.value, ChoiceValue::Str(lang.code));
        assert_eq!(o.label, Label::Verbatim(lang.native_name));
    }
    assert_eq!(find("uiLanguage").unwrap().section, FieldSection::General);
}

#[test]
fn search_matches_english_text() {
    crate::i18n::register();
    let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
    assert!(paths("default install folder").contains(&"defaultInstallPath"));
    assert!(paths("language").contains(&"uiLanguage"));
    assert!(paths("restart required").contains(&"noTrayIcon"));
}

#[test]
fn choice_values_match_stored_json() {
    assert!(ChoiceValue::False.matches(&Value::Bool(false)));
    assert!(ChoiceValue::False.matches(&Value::Null));
    assert!(ChoiceValue::Str("fsr").matches(&Value::from("fsr")));
    assert!(ChoiceValue::Num(3).matches(&Value::from(3)));
    assert!(!ChoiceValue::Num(3).matches(&Value::from("3")));
}

#[test]
fn steam_games_keep_only_what_irori_does_around_the_launch() {
    let applies = |path: &str| find(path).expect(path).applies_to_steam();
    for path in [
        "launcherArgs",
        "beforeLaunchScriptPath",
        "iroriBeforeLaunchScriptAlongside",
        "panel.launchPrograms",
        "panel.performance-hud",
        "iroriReplay.enabled",
        "panel.save-backups",
        "panel.artwork",
        "iroriInSunshine",
        "iroriDiscordPresence",
    ] {
        assert!(applies(path), "{path}");
    }
    for path in [
        "wineVersion",
        "useGameMode",
        "enviromentOptions",
        "wrapperOptions",
        "steamRuntime",
        "iroriGpu",
        "autoSyncSaves",
        "panel.launchPresets",
    ] {
        assert!(!applies(path), "{path}");
    }
}

#[test]
fn performance_hud_and_legacy_overlay_visibility() {
    let hud = find("panel.performance-hud").expect("panel.performance-hud");
    assert_eq!(hud.section, FieldSection::Performance);
    assert_eq!(hud.scope, Scope::GameOnly);
    assert!(hud.applies_to_steam());
    assert_eq!(
        hud.supported_on_host(),
        cfg!(any(target_os = "linux", target_os = "macos"))
    );

    let legacy_mango = find("showMangohud").expect("showMangohud");
    assert!(!legacy_mango.supported_on_host());

    let show_fps = find("showFps").expect("showFps");
    assert_eq!(show_fps.supported_on_host(), cfg!(target_os = "linux"));
}

#[test]
fn search_matches_tooltip_copy() {
    crate::i18n::register();
    let speed = find("maxDownloadSpeed").expect("maxDownloadSpeed");
    assert!(!speed.tooltip_text().is_empty());
    assert!(search("voice calls").any(|f| f.path == "maxDownloadSpeed"));
    assert!(search("stale metadata").any(|f| f.path == "action.clear-caches"));
    assert_eq!(
        search("laptop fans").any(|f| f.path == "enableDXVKFpsLimit"),
        !cfg!(windows)
    );
}
