//! UI integration tests: the real `LibraryView` in a headless window over a
//! real backend with an in-memory store.

use std::path::Path;
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use gpui_kit::{
    AppContext as _, Bounds, Focusable as _, TestAppContext, WindowBounds, WindowOptions, px, size,
};
use irori_core::{
    DynStore, ExtraInfo, GameId, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform,
    InstallRequest, LaunchPlan, LaunchRequest, LoginFlow, OpContext, Outcome, Paths, RemoveArgs,
    Runner, Store, UpdateRequest, UserInfo, async_trait,
};
use irori_services::Backend;
use irori_sideload::{SideloadApp, SideloadPlatform, SideloadStore};
use irori_ui::models;

use crate::dialogs::sideload::SideloadDialog;
use crate::filter::InstallFilter;
use crate::view::LibraryView;

struct MemoryStore {
    games: Vec<Arc<GameInfo>>,
}

impl MemoryStore {
    fn new() -> Arc<Self> {
        let game = |app: &str, title: &str, installed: bool| {
            // Hades is installed in a folder that does not exist.
            let install = (app == "3").then(|| irori_core::InstalledInfo {
                install_path: "/nonexistent/irori-library-test/hades".into(),
                ..Default::default()
            });
            Arc::new(
                GameInfo {
                    runner: Some(Runner::Gog),
                    app_name: app.into(),
                    title: title.into(),
                    is_installed: installed,
                    installable: true,
                    install,
                    game_modes: if app == "3" {
                        vec![irori_core::GameMode::Single, irori_core::GameMode::Coop]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                }
                .finalize(),
            )
        };
        Arc::new(Self {
            games: vec![
                game("1", "Alan Wake", false),
                game("2", "Celeste", true),
                game("3", "Hades", true),
                game("4", "Inside", false),
                game("5", "Outer Wilds", true),
            ],
        })
    }
}

#[async_trait]
impl Store for MemoryStore {
    fn runner(&self) -> Runner {
        Runner::Gog
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        Some(UserInfo {
            account_id: "1".into(),
            display_name: "Tester".into(),
        })
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("not supported")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        self.games.clone()
    }
    fn game(&self, app_name: &str) -> Option<Arc<GameInfo>> {
        self.games.iter().find(|g| g.app_name == app_name).cloned()
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("not supported")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!("not supported")
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("not supported")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("not supported")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("not supported")
    }
}

/// Keep every file the backend writes in a private temporary directory.
fn isolate_paths() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = std::env::temp_dir().join(format!("irori-library-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Paths::init(Paths::under(&root));
    });
}

/// Start a backend over the in-memory store, install the UI layer and wait
/// until the library model holds the store's games.
fn start(cx: &mut TestAppContext) -> Backend {
    isolate_paths();
    cx.executor().allow_parking();
    let store: DynStore = MemoryStore::new();
    let backend = Backend::start(move |_, _| vec![store]).expect("backend starts");
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend.clone(), cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let len = cx.read(|cx| models(cx).library.read(cx).snapshot.len());
        if len == 5 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "library never loaded ({len} games)"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    backend
}

#[gpui_kit::test]
fn keyboard_navigation_filters_and_search(cx: &mut TestAppContext) {
    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    draw(cx);

    let titles = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            view.read(cx)
                .visible_games()
                .iter()
                .map(|g| g.title.clone())
                .collect()
        })
    };
    assert_eq!(
        titles(cx),
        vec!["Alan Wake", "Celeste", "Hades", "Inside", "Outer Wilds"]
    );

    // Focus the grid and move with the arrow keys: the first key focuses the
    // first game, Right moves along the row, End jumps to the last game.
    cx.update_window(window, |_, window, cx| {
        let grid = view.read(cx).grid_focus_handle();
        grid.focus(window, cx);
    })
    .unwrap();
    draw(cx);
    cx.simulate_keystrokes(window, "down right");
    draw(cx);
    let focused = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).focused_game().cloned());
    assert_eq!(focused(cx), Some(GameId::new(Runner::Gog, "2")));
    cx.simulate_keystrokes(window, "end");
    assert_eq!(focused(cx), Some(GameId::new(Runner::Gog, "5")));

    // Typing a letter in the grid starts a search with it, in the field.
    cx.simulate_keystrokes(window, "c");
    draw(cx);
    let search_focused = cx
        .update_window(window, |_, window, cx| {
            view.read(cx).search_focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(search_focused, "typing moves to the search field");
    cx.simulate_input(window, "eleste");
    draw(cx);
    assert_eq!(titles(cx), vec!["Celeste"]);
    cx.simulate_keystrokes(window, "escape");
    draw(cx);
    assert_eq!(titles(cx).len(), 5);
    cx.update_window(window, |_, window, cx| {
        view.read(cx).grid_focus_handle().focus(window, cx);
    })
    .unwrap();
    draw(cx);

    // "/" moves focus to the search field; typing filters instantly.
    cx.simulate_keystrokes(window, "/");
    draw(cx);
    let search_focused = cx
        .update_window(window, |_, window, cx| {
            view.read(cx).search_focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(search_focused, "search field has focus after /");
    cx.simulate_input(window, "hades");
    draw(cx);
    assert_eq!(titles(cx), vec!["Hades"]);

    // Escape clears the search; filters still apply and are counted.
    cx.simulate_keystrokes(window, "escape");
    draw(cx);
    assert_eq!(titles(cx).len(), 5);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.install = InstallFilter::Installed, cx)
        })
    });
    draw(cx);
    assert_eq!(titles(cx), vec!["Celeste", "Hades", "Outer Wilds"]);
    assert_eq!(cx.read(|cx| view.read(cx).filter(cx).active_count()), 1);

    // Hiding games with missing files drops Hades once the background
    // availability check finished.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.hide_unavailable = true, cx)
        })
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while titles(cx).len() != 2 {
        assert!(Instant::now() < deadline, "availability never resolved");
        std::thread::sleep(Duration::from_millis(10));
        draw(cx);
    }
    assert_eq!(titles(cx), vec!["Celeste", "Outer Wilds"]);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.hide_unavailable = false, cx)
        })
    });
    draw(cx);

    // Enter in the search field moves into the results.
    cx.simulate_keystrokes(window, "ctrl-f");
    cx.simulate_input(window, "wilds");
    cx.simulate_keystrokes(window, "enter");
    draw(cx);
    assert_eq!(focused(cx), Some(GameId::new(Runner::Gog, "5")));
    let grid_focused = cx
        .update_window(window, |_, window, cx| {
            view.read(cx).grid_focus_handle().is_focused(window)
        })
        .unwrap();
    assert!(grid_focused);

    // The view's root is focusable for the shell.
    let _ = cx.read(|cx| view.read(cx).focus_handle(cx));
}

#[gpui_kit::test]
fn picking_games_for_bulk_actions(cx: &mut TestAppContext) {
    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    draw(cx);
    let state = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).selection_state());
    let game = |app: &str| GameId::new(Runner::Gog, app);
    let ctrl = gpui_kit::Modifiers {
        control: !cfg!(target_os = "macos"),
        platform: cfg!(target_os = "macos"),
        ..Default::default()
    };
    let shift = gpui_kit::Modifiers {
        shift: true,
        ..Default::default()
    };
    let click = |cx: &mut TestAppContext, app: &str, m: gpui_kit::Modifiers| {
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |v, cx| v.click_for_test(&game(app), m, 1, window, cx))
        })
        .unwrap();
    };

    // Ctrl-click starts selection mode; Shift-click picks the range.
    click(cx, "2", ctrl);
    click(cx, "4", shift);
    draw(cx);
    assert_eq!(
        state(cx),
        (
            vec!["Celeste".into(), "Hades".into(), "Inside".into()],
            true
        )
    );
    // In selection mode a plain click toggles instead of opening the game.
    click(cx, "3", Default::default());
    assert_eq!(state(cx).0, ["Celeste", "Inside"]);

    // Keyboard (focus follows the last click): Home moves to the first game,
    // Space picks it, Ctrl+A picks all, Escape ends.
    cx.update_window(window, |_, window, cx| {
        view.read(cx).grid_focus_handle().focus(window, cx);
    })
    .unwrap();
    draw(cx);
    cx.simulate_keystrokes(window, "home space");
    draw(cx);
    assert_eq!(state(cx).0, ["Alan Wake", "Celeste", "Inside"]);
    cx.simulate_keystrokes(window, "ctrl-a");
    draw(cx);
    assert_eq!(state(cx).0.len(), 5);
    // Redrawing with everything picked doesn't work out the bar again.
    let computes = cx.read(|cx| view.read(cx).bulk_computes());
    draw(cx);
    draw(cx);
    assert_eq!(cx.read(|cx| view.read(cx).bulk_computes()), computes);

    // A filter hiding picked games drops them from the selection.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.install = InstallFilter::Installed, cx)
        })
    });
    draw(cx);
    assert_eq!(state(cx).0, ["Celeste", "Hades", "Outer Wilds"]);

    cx.simulate_keystrokes(window, "escape");
    draw(cx);
    assert_eq!(state(cx), (Vec::new(), false));
}

#[gpui_kit::test]
fn grid_drawer_focus_view_and_switching(cx: &mut TestAppContext) {
    use irori_core::settings::LibraryLayout;
    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1600.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    draw(cx);
    let game = |app: &str| GameId::new(Runner::Gog, app);
    let focused = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).focused_game().cloned());
    let state = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| view.read(cx).view_state(window))
            .unwrap()
    };
    let click = |cx: &mut TestAppContext, app: &str, count: usize| {
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.click_for_test(&game(app), Default::default(), count, window, cx)
            })
        })
        .unwrap();
        draw(cx);
    };
    let route = |cx: &mut TestAppContext| {
        cx.read(|cx| irori_ui::nav::navigator(cx).read(cx).current().clone())
    };

    // Grid: a click selects the game and opens the drawer within its limits.
    assert_eq!(state(cx).0, LibraryLayout::Grid);
    let before = route(cx);
    click(cx, "2", 1);
    let (layout, open, width) = state(cx);
    assert_eq!((layout, open), (LibraryLayout::Grid, true));
    assert!(
        (crate::layouts::DRAWER_MIN_REM..=crate::layouts::DRAWER_MAX_REM).contains(&width),
        "{width}"
    );
    assert_eq!(focused(cx), Some(game("2")));
    assert_eq!(route(cx), before, "a single click stays in the library");

    // Arrow keys move the selection, and the drawer with it; Escape closes it.
    cx.simulate_keystrokes(window, "right");
    draw(cx);
    assert_eq!(focused(cx), Some(game("3")));
    assert!(state(cx).1);
    cx.simulate_keystrokes(window, "escape");
    draw(cx);
    assert!(!state(cx).1);
    // I opens the drawer from the keyboard, and closes it again.
    cx.simulate_keystrokes(window, "i");
    draw(cx);
    assert!(state(cx).1);
    cx.simulate_keystrokes(window, "i");
    draw(cx);
    assert!(!state(cx).1);

    // Focus keeps the selected game and shows it beside the covers; the
    // arrow keys step through the column.
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::Focus, cx)));
    draw(cx);
    assert_eq!(state(cx).0, LibraryLayout::Focus);
    assert_eq!(focused(cx), Some(game("3")));
    cx.simulate_keystrokes(window, "right");
    draw(cx);
    assert_eq!(focused(cx), Some(game("4")));
    click(cx, "1", 1);
    assert_eq!(focused(cx), Some(game("1")));

    // A search hiding the selected game moves Focus to a shown one; the page
    // is never left empty.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.query = "wilds".into(), cx)
        })
    });
    draw(cx);
    assert_eq!(focused(cx), Some(game("5")));
    cx.update(|cx| view.update(cx, |v, cx| v.update_filter(|f| f.query.clear(), cx)));
    draw(cx);
    assert_eq!(focused(cx), Some(game("5")));

    // Alt+1 goes back to the grid with the game still selected; the drawer
    // stays closed until a click.
    cx.simulate_keystrokes(window, "alt-1");
    draw(cx);
    assert_eq!(state(cx).0, LibraryLayout::Grid);
    assert_eq!(focused(cx), Some(game("5")));
    assert!(!state(cx).1);

    // A double click opens the game's page.
    click(cx, "2", 2);
    assert_eq!(route(cx), irori_ui::Route::Game(game("2")));
}

#[gpui_kit::test]
fn a_mode_chosen_before_the_library_opens_filters_it(cx: &mut TestAppContext) {
    use irori_core::GameMode;
    start(cx);
    // A game page opened first (from a link): no library view exists yet.
    assert!(cx.read(crate::filters_panel::active_filters).is_none());
    cx.update(|cx| crate::show_game_mode(GameMode::Coop, cx));
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    cx.update_window(window, |_, window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let modes = cx.read(|cx| {
        let model = crate::filters_panel::active_filters(cx).expect("the view's filters");
        model.read(cx).filter().modes.clone()
    });
    assert_eq!(modes, [GameMode::Coop].into());
    let shown: Vec<String> = cx.read(|cx| {
        view.read(cx)
            .visible_games()
            .iter()
            .map(|g| g.title.clone())
            .collect()
    });
    assert_eq!(shown, ["Hades"]);

    // With the view open, the chip changes it at once.
    cx.update(|cx| crate::show_game_mode(GameMode::Single, cx));
    let modes = cx.read(|cx| {
        crate::filters_panel::active_filters(cx)
            .expect("the view's filters")
            .read(cx)
            .filter()
            .modes
            .clone()
    });
    assert_eq!(modes, [GameMode::Single].into());
}

#[gpui_kit::test]
fn grouping_by_play_status(cx: &mut TestAppContext) {
    use irori_services::{EffectiveStatus, PlayStatus};
    let backend = start(cx);
    let game = |app: &str| GameId::new(Runner::Gog, app);
    for (app, status) in [
        ("3", PlayStatus::Playing),
        ("4", PlayStatus::Playing),
        ("2", PlayStatus::Beaten),
    ] {
        backend.userdata.set_play_status(&game(app), Some(status));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while cx.read(|cx| models(cx).library.read(cx).user.play_status.len()) < 3 {
        assert!(Instant::now() < deadline, "play statuses never arrived");
        std::thread::sleep(Duration::from_millis(10));
        cx.run_until_parked();
    }
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1280.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    let titles = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            view.read(cx)
                .visible_games()
                .iter()
                .map(|g| g.title.clone())
                .collect()
        })
    };
    draw(cx);
    cx.update_window(window, |_, window, cx| {
        view.read(cx).grid_focus_handle().focus(window, cx);
    })
    .unwrap();
    draw(cx);

    // Ctrl+G groups: playing games first, untouched ones, then beaten.
    cx.simulate_keystrokes(window, "ctrl-g");
    draw(cx);
    assert_eq!(cx.read(crate::groups_by_status), Some(true));
    assert_eq!(
        titles(cx),
        ["Hades", "Inside", "Alan Wake", "Outer Wilds", "Celeste"]
    );
    let sections = cx.read(|cx| view.read(cx).status_groups());
    assert_eq!(
        sections,
        [
            EffectiveStatus::Playing,
            EffectiveStatus::NotPlayed,
            EffectiveStatus::Beaten
        ]
    );

    // Down moves section to section, keeping the column where it can.
    let focused = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).focused_game().cloned());
    cx.simulate_keystrokes(window, "down down");
    assert_eq!(focused(cx), Some(game("1")));
    cx.simulate_keystrokes(window, "right down");
    assert_eq!(focused(cx), Some(game("2")));
    cx.simulate_keystrokes(window, "up");
    assert_eq!(focused(cx), Some(game("1")));

    // A status change moves the game to a section of its own.
    backend
        .userdata
        .set_play_status(&game("1"), Some(PlayStatus::OnHold));
    let deadline = Instant::now() + Duration::from_secs(10);
    while cx.read(|cx| view.read(cx).status_groups()).len() != 4 {
        assert!(Instant::now() < deadline, "regrouping never happened");
        std::thread::sleep(Duration::from_millis(10));
        draw(cx);
    }
    assert_eq!(
        cx.read(|cx| view.read(cx).status_groups())[1],
        EffectiveStatus::OnHold
    );

    // Ctrl+G again returns to one A–Z list.
    cx.simulate_keystrokes(window, "ctrl-g");
    draw(cx);
    assert_eq!(
        titles(cx),
        ["Alan Wake", "Celeste", "Hades", "Inside", "Outer Wilds"]
    );
    assert!(cx.read(|cx| view.read(cx).status_groups()).is_empty());
}

#[gpui_kit::test]
fn editing_an_app_keeps_and_changes_its_description(cx: &mut TestAppContext) {
    let backend = start(cx);
    let settings = backend.settings.clone();
    let store = SideloadStore::new(backend.bus.clone(), Arc::new(move || settings.app()));
    cx.update(|cx| crate::install_sideload_store(store.clone(), cx));
    let id = backend
        .runtime()
        .block_on(store.add_app(SideloadApp {
            title: "Visual novel".into(),
            platform: SideloadPlatform::Browser,
            browser_url: Some("https://example.com/".into()),
            description: Some("A quiet story.".into()),
            ..Default::default()
        }))
        .expect("app added");

    let app_name = id.app_name.to_string();
    let (window, dialog) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| SideloadDialog::with_app(Some(app_name), None, window, cx))
            })
        })
        .expect("window opens");
    cx.run_until_parked();

    let kept = cx.read(|cx| dialog.read(cx).collect(cx));
    assert_eq!(kept.description.as_deref(), Some("A quiet story."));

    cx.update_window(window, |_, window, cx| {
        let description = dialog.read(cx).description.clone();
        description.update(cx, |state, cx| {
            state.set_value("A louder story.", window, cx)
        });
    })
    .unwrap();
    let changed = cx.read(|cx| dialog.read(cx).collect(cx));
    assert_eq!(changed.description.as_deref(), Some("A louder story."));
}

#[gpui_kit::test]
fn a_dropped_file_opens_add_game_filled_in(cx: &mut TestAppContext) {
    let _backend = start(cx);
    let prefill = SideloadApp {
        title: "Hollow Knight".into(),
        executable: Some("/games/hk/Hollow_Knight.exe".into()),
        platform: SideloadPlatform::Windows,
        launcher_args: "-windowed".into(),
        ..Default::default()
    };
    let (_window, dialog) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| SideloadDialog::with_app(None, Some(prefill), window, cx))
            })
        })
        .expect("window opens");
    cx.run_until_parked();
    let app = cx.read(|cx| dialog.read(cx).collect(cx));
    assert_eq!(app.title, "Hollow Knight");
    assert_eq!(
        app.executable.as_deref(),
        Some(Path::new("/games/hk/Hollow_Knight.exe"))
    );
    assert_eq!(app.platform, SideloadPlatform::Windows);
    assert_eq!(app.launcher_args, "-windowed");
}

#[gpui_kit::test]
fn quick_controls_follow_the_measured_width(cx: &mut TestAppContext) {
    use crate::quick::QuickControl;

    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(2400.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    // Measuring takes a frame or two: the room first, then the controls.
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..4 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    let resize = |cx: &mut TestAppContext, width: f32| {
        cx.simulate_window_resize(window, size(px(width), px(800.)));
    };
    let shown = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).quick_controls_shown());
    settle(cx);
    assert_eq!(shown(cx), &QuickControl::ORDER, "a wide window has room");

    // Narrower windows drop controls from the end, never the title.
    resize(cx, 760.);
    settle(cx);
    let narrow = shown(cx);
    assert!(narrow.len() < QuickControl::ORDER.len(), "{narrow:?}");
    assert_eq!(narrow, &QuickControl::ORDER[..narrow.len()]);
    resize(cx, 500.);
    settle(cx);
    assert!(shown(cx).len() <= narrow.len());

    resize(cx, 2400.);
    settle(cx);
    assert_eq!(shown(cx), &QuickControl::ORDER, "they come back with room");

    // The settings Irori loaded at start can still reach the view and put
    // the saved value back; turn the controls off until the view keeps it.
    let mut off = false;
    for _ in 0..50 {
        cx.update(|cx| view.update(cx, |v, cx| v.set_quick_controls_for_test(false, cx)));
        settle(cx);
        if shown(cx).is_empty() {
            off = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(off, "the setting turns them off");
}
