//! UI integration tests: real views in headless windows against a backend
//! whose files live in a temporary directory.

use std::sync::OnceLock;

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Focusable as _, TestAppContext, WindowOptions,
};
use irori_core::{GameId, Paths, Runner};
use irori_services::Backend;
use serde_json::Value;

use crate::form::{FormTarget, SettingsForm};
use crate::view::{SettingsSection, SettingsView};

/// A fresh backend per test (its event bus wakes only that test's tasks);
/// all of them share one temporary settings directory.
fn test_backend() -> Backend {
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("temp dir").keep();
        Paths::init(Paths::under(&root));
        root
    });
    Backend::start(|_, _| Vec::new()).expect("backend")
}

fn open<V: gpui_kit::Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::Context<V>) -> V + 'static,
) -> (AnyWindowHandle, Entity<V>) {
    // This integration test receives events from the real Tokio backend.
    cx.executor().allow_parking();
    let backend = test_backend();
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
        crate::init(cx);
    });
    let opened = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| build(window, cx))
        })
        .expect("open window")
    });
    cx.run_until_parked();
    opened
}

#[gpui_kit::test]
fn typing_in_search_filters_fields_across_sections(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, |window, cx| {
        SettingsView::new("general".into(), window, cx)
    });
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.section(), SettingsSection::General);
        assert!(v.app_form().read(cx).visible_count() > 10);
    });

    // Focus the search field and type through the window's input path.
    cx.update_window(handle, |_, window, cx| {
        let search = view.read(cx).search_input();
        search.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    cx.simulate_input(handle, "fsync");
    cx.run_until_parked();

    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.query(), "fsync");
        assert_eq!(v.app_form().read(cx).visible_count(), 0);
        // Fsync lives in the game defaults form, not the current section.
        assert_eq!(
            v.defaults_form().read(cx).visible_count(),
            usize::from(cfg!(target_os = "linux"))
        );
    });

    // Opening a section clears the query and restores the section filter.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |v, cx| v.set_section("appearance", window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.query(), "");
        assert_eq!(v.section(), SettingsSection::Appearance);
        let form = v.app_form().read(cx);
        assert!(form.visible_count() >= 5);
    });
}

#[gpui_kit::test]
fn per_game_override_marks_and_resets(cx: &mut TestAppContext) {
    let id = GameId::new(Runner::Sideload, "ui-test-game");
    let target = FormTarget::Game(id.clone());
    let (handle, form) = open(cx, move |window, cx| SettingsForm::new(target, window, cx));
    let backend = cx.update(|cx| irori_ui::backend(cx));
    cx.update(|cx| assert_eq!(form.read(cx).overridden_count(), 0));

    cx.update_window(handle, |_, window, cx| {
        form.update(cx, |f, cx| {
            f.write_for_test("showFps", Value::Bool(true), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(backend.settings.overrides(&id).is_overridden("showFps"));
    assert!(!backend.settings.app().game.show_fps);
    cx.update(|cx| assert_eq!(form.read(cx).overridden_count(), 1));

    // Nested gamescope field overrides the parent key only while it differs.
    cx.update_window(handle, |_, window, cx| {
        form.update(cx, |f, cx| {
            f.write_for_test("gamescope.fpsLimiter", Value::from("60"), window, cx)
        });
    })
    .unwrap();
    assert!(backend.settings.overrides(&id).is_overridden("gamescope"));
    assert_eq!(backend.settings.game(&id).gamescope.fps_limiter, "60");

    cx.update_window(handle, |_, window, cx| {
        form.update(cx, |f, cx| {
            f.reset_for_test("gamescope.fpsLimiter", window, cx);
            f.reset_for_test("showFps", window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(backend.settings.overrides(&id).0.is_empty());
    cx.update(|cx| assert_eq!(form.read(cx).overridden_count(), 0));
}

#[gpui_kit::test]
fn store_and_platform_specific_rows_follow_their_context(cx: &mut TestAppContext) {
    use crate::registry::FieldSection;

    // A game that isn't installed natively: no Steam runtime choice, but the
    // manual save sync row is there.
    let id = GameId::new(Runner::Sideload, "ui-test-visibility");
    let target = FormTarget::Game(id);
    let (_, form) = open(cx, move |window, cx| SettingsForm::new(target, window, cx));
    cx.update(|cx| {
        form.update(cx, |f, cx| f.set_query("steam linux runtime", cx));
        assert_eq!(form.read(cx).visible_count(), 0);
        form.update(cx, |f, cx| f.set_query("sync saves now", cx));
        assert_eq!(form.read(cx).visible_count(), 1);
    });

    // Without the Epic store handle the Epic group stays hidden, while the
    // controller rows are always offered.
    let (_, app_form) = open(cx, |window, cx| {
        SettingsForm::new(FormTarget::App, window, cx)
    });
    cx.update(|cx| {
        app_form.update(cx, |f, cx| f.set_query("eos overlay", cx));
        assert_eq!(app_form.read(cx).visible_count(), 0);
        app_form.update(cx, |f, cx| f.set_sections(vec![FieldSection::General], cx));
        let general = app_form.read(cx).visible_count();
        app_form.update(cx, |f, cx| f.set_query("controller", cx));
        assert_eq!(app_form.read(cx).visible_count(), 5);
        assert!(general > 20);
    });
}

#[gpui_kit::test]
fn about_says_who_updates_irori(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, |window, cx| {
        SettingsView::new("about".into(), window, cx)
    });
    cx.update_window(handle, |_, window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(view.read(cx).section(), SettingsSection::About);
        let status = irori_ui::models(cx).app_update.read(cx).status.clone();
        // Tests run from Cargo's target folder: a build from source never
        // looks for new versions, so nothing here reaches GitHub.
        assert_eq!(status.kind, irori_services::InstallKind::Source);
        assert!(!status.announce && status.available.is_none());
    });
}

#[gpui_kit::test]
fn graphics_and_artwork_tabs_draw_and_store_nested_values(cx: &mut TestAppContext) {
    use crate::registry::FieldSection;

    let id = GameId::new(Runner::Sideload, "ui-test-graphics");
    let target = FormTarget::Game(id.clone());
    let (handle, form) = open(cx, move |window, cx| SettingsForm::new(target, window, cx));
    let backend = cx.update(|cx| irori_ui::backend(cx));
    cx.update(|cx| {
        form.update(cx, |f, cx| f.set_sections(vec![FieldSection::Gpu], cx));
        // The new options show on every system, greyed out where they can't apply.
        assert!(form.read(cx).visible_count() >= 7);
        form.update(cx, |f, cx| f.set_query("cover background logo", cx));
        assert_eq!(form.read(cx).visible_count(), 1);
        form.update(cx, |f, cx| {
            f.set_sections(vec![FieldSection::Gpu, FieldSection::Artwork], cx)
        });
    });
    cx.update_window(handle, |_, window, cx| {
        form.update(cx, |f, cx| {
            f.write_for_test("iroriLsfg.multiplier", Value::from(3), window, cx)
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(backend.settings.overrides(&id).is_overridden("iroriLsfg"));
    let lsfg = backend.settings.game(&id).lsfg;
    assert_eq!(
        (lsfg.multiplier, lsfg.flow_scale, lsfg.enabled),
        (3, 100, false)
    );
    cx.update_window(handle, |_, window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

/// Runs the app until `done` holds, letting real time pass for the backend's
/// events and file watches, and the app's clock for its debounces.
fn wait_for(cx: &mut TestAppContext, mut done: impl FnMut(&mut gpui_kit::App) -> bool) -> bool {
    for _ in 0..100 {
        cx.run_until_parked();
        if cx.update(&mut done) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
    }
    false
}

#[gpui_kit::test]
fn the_wallpapers_colours_follow_the_switch_and_the_file(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    use irori_ui::theme::{Desktop, followed_desktop};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let file = dir.join("colors.json");
    let pywal = |background: &str| {
        format!(
            r##"{{"special": {{"background": "{background}", "foreground": "#c3c5c6"}},
                "colors": {{"color4": "#5e7f8c", "color8": "#5f6a72"}}}}"##
        )
    };
    std::fs::write(&file, pywal("#0f1418")).unwrap();

    let (handle, form) = open(cx, |window, cx| {
        SettingsForm::new(FormTarget::App, window, cx)
    });
    let write = |cx: &mut TestAppContext, path: &'static str, value: Value| {
        cx.update_window(handle, |_, window, cx| {
            form.update(cx, |f, cx| f.write_for_test(path, value, window, cx))
        })
        .unwrap();
    };
    write(
        cx,
        "iroriUi.wallpaperColorsFile",
        Value::from(file.to_string_lossy().into_owned()),
    );
    // The file alone changes nothing while the switch is off.
    cx.run_until_parked();
    cx.update(|cx| assert_ne!(followed_desktop(cx), Some(Desktop::Wallpaper)));

    write(cx, "iroriUi.wallpaperColors", Value::Bool(true));
    // GPUI keeps colours as HSL, which can move a channel by one.
    let shows = |cx: &mut gpui_kit::App, hex: u32| {
        let (shown, wanted) = (cx.theme().background.to_rgb(), gpui_kit::rgb(hex));
        [shown.r - wanted.r, shown.g - wanted.g, shown.b - wanted.b]
            .iter()
            .all(|d| d.abs() < 1.5 / 255.0)
    };
    assert!(
        wait_for(cx, |cx| followed_desktop(cx) == Some(Desktop::Wallpaper)),
        "the wallpaper's colours never showed"
    );
    cx.update(|cx| {
        assert_eq!(cx.theme().theme_name().as_ref(), "Wallpaper Dark");
        assert!(shows(cx, 0x0f1418));
    });

    // pywal writes a new file and renames it over the old one.
    let partial = dir.join("colors.json.tmp");
    std::fs::write(&partial, pywal("#201010")).unwrap();
    std::fs::rename(&partial, &file).unwrap();
    assert!(
        wait_for(cx, |cx| shows(cx, 0x201010)),
        "the new wallpaper's colours never showed"
    );

    write(cx, "iroriUi.wallpaperColors", Value::Bool(false));
    write(cx, "iroriUi.wallpaperColorsFile", Value::from(""));
    assert!(
        wait_for(cx, |cx| followed_desktop(cx) != Some(Desktop::Wallpaper)),
        "turning the switch off kept the wallpaper's colours"
    );
}

#[gpui_kit::test]
fn steam_compat_panel_shows_the_tools_and_the_games_choice(cx: &mut TestAppContext) {
    let steam = tempfile::tempdir().expect("temp dir").keep();
    std::fs::create_dir_all(steam.join("steamapps")).unwrap();
    std::fs::write(
        steam.join("steamapps/appmanifest_2805730.acf"),
        "\"AppState\" { \"appid\" \"2805730\" \"name\" \"Proton 9.0\" \"StateFlags\" \"4\" \"installdir\" \"Proton 9.0\" }",
    )
    .unwrap();
    let ge = steam.join("compatibilitytools.d/GE-Proton10-4");
    std::fs::create_dir_all(&ge).unwrap();
    std::fs::write(
        ge.join("compatibilitytool.vdf"),
        "\"compatibilitytools\" { \"compat_tools\" { \"GE-Proton10-4\" { \"display_name\" \"GE-Proton10-4\" } } }",
    )
    .unwrap();
    std::fs::create_dir_all(steam.join("config")).unwrap();
    let config = "\"InstallConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { \"CompatToolMapping\" { \"620\" { \"name\" \"GE-Proton10-4\" \"config\" \"\" \"priority\" \"250\" } } } } } }";
    std::fs::write(steam.join("config/config.vdf"), config).unwrap();

    let id = GameId::new(Runner::Steam, "620");
    let data = steam.join("irori");
    let (handle, panel) = open(cx, move |window, cx| {
        let backend = irori_ui::backend(cx);
        let settings = backend.settings.clone();
        let store = irori_steam::SteamStore::with_layout(
            backend.bus.clone(),
            std::sync::Arc::new(move || settings.app()),
            steam,
            &data,
            irori_steam_api::Endpoints::default(),
        );
        crate::steam_compat::SteamCompatPanel::new(id, store, window, cx)
    });
    // The tools are read on the backend's runtime, outside the test executor.
    for _ in 0..200 {
        cx.run_until_parked();
        if cx.update(|cx| panel.read(cx).state_for_test().is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    cx.update(|cx| {
        let p = panel.read(cx);
        let state = p.state_for_test().expect("tools read");
        let names: Vec<&str> = state.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"proton_9") && names.contains(&"GE-Proton10-4"));
        assert_eq!(state.current.as_deref(), Some("GE-Proton10-4"));
        assert_eq!(p.selected_for_test(cx).as_deref(), Some("GE-Proton10-4"));
    });
    cx.update_window(handle, |_, window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn steam_game_settings_show_only_what_applies(cx: &mut TestAppContext) {
    use crate::game_view::{GameSection, GameSettingsView};
    let id = GameId::new(Runner::Steam, "620");
    let (handle, view) = open(cx, move |window, cx| {
        GameSettingsView::new(id, "".into(), window, cx)
    });
    // Steam Play needs the Steam store (Linux); here no handle is installed.
    cx.update(|cx| assert_eq!(view.read(cx).section(), GameSection::Launch));
    for hidden in ["gpu", "tools", "log", "wine"] {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |v, cx| v.set_section(hidden, window, cx));
        })
        .unwrap();
        cx.update(|cx| assert_eq!(view.read(cx).section(), GameSection::Launch, "{hidden}"));
    }
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |v, cx| v.set_section("art", window, cx));
    })
    .unwrap();
    cx.update(|cx| assert_eq!(view.read(cx).section(), GameSection::Artwork));
}
