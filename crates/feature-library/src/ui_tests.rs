//! UI integration tests: the real `LibraryView` in a headless window over a
//! real backend with an in-memory store.

use std::path::Path;
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Focusable as _, TestAppContext, WindowBounds, WindowOptions,
    px, size,
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

/// Serialises UI tests that share the process-wide `Paths` root so concurrent
/// `persist_ui` / `SettingsService` writes cannot race on `settings.json`.
static UI_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Holds the [`Backend`] together with [`UI_TEST_LOCK`] for the lifetime of a
/// UI test so the shared temp directory stays isolated until the test exits.
struct TestBackend {
    backend: Backend,
    _guard: parking_lot::MutexGuard<'static, ()>,
}

impl std::ops::Deref for TestBackend {
    type Target = Backend;

    fn deref(&self) -> &Self::Target {
        &self.backend
    }
}

/// Keep every file the backend writes in a private temporary directory.
fn isolate_paths() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("irori-library-ui-{}", std::process::id()));
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::fs::remove_dir_all(&root);
        Paths::init(Paths::under(&root));
    });
    root
}

/// Start a backend over the in-memory store, install the UI layer and wait
/// until the library model holds the store's games.
fn start(cx: &mut TestAppContext) -> TestBackend {
    let guard = UI_TEST_LOCK.lock();
    let root = isolate_paths();
    let _ = std::fs::remove_dir_all(&root);
    cx.executor().allow_parking();
    let store: DynStore = MemoryStore::new();
    let backend = Backend::start_for_tests(move |_, _| vec![store]).expect("backend starts");
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
    TestBackend {
        backend,
        _guard: guard,
    }
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

/// A controller's directions are these arrow keys: at the grid's edges they
/// must be left unhandled, so the controller moves on to the controls
/// around the grid instead of getting stuck in it.
#[gpui_kit::test]
fn arrows_at_the_grid_edges_are_left_to_the_window(cx: &mut TestAppContext) {
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
    cx.update_window(window, |_, window, cx| {
        view.read(cx).grid_focus_handle().focus(window, cx);
    })
    .unwrap();
    draw(cx);
    let press = |cx: &mut TestAppContext, key: &str| -> bool {
        let handled = cx
            .update_window(window, |_, window, cx| {
                window.dispatch_keystroke(gpui_kit::Keystroke::parse(key).unwrap(), cx)
            })
            .unwrap();
        draw(cx);
        handled
    };
    let focused = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).focused_game().cloned());
    let first = GameId::new(Runner::Gog, "1");
    let last = GameId::new(Runner::Gog, "5");

    assert!(press(cx, "down"), "the first key focuses the first game");
    assert_eq!(focused(cx), Some(first.clone()));
    assert!(!press(cx, "left"), "nothing left of the first game");
    assert!(!press(cx, "up"), "nothing above the first row");
    assert!(!press(cx, "pageup"));
    assert_eq!(focused(cx), Some(first));
    assert!(press(cx, "end"));
    assert_eq!(focused(cx), Some(last.clone()));
    assert!(!press(cx, "right"), "nothing right of the last game");
    assert!(!press(cx, "down"), "nothing below the last row");
    assert_eq!(focused(cx), Some(last));
    assert!(press(cx, "left"), "inside the grid the keys still move");
    assert_eq!(focused(cx), Some(GameId::new(Runner::Gog, "4")));
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
    // Every game shown: tests share one state folder, so another test's
    // added game may be among them.
    let shown = cx.read(|cx| view.read(cx).visible_games().len());
    assert!(shown >= 5);
    assert_eq!(state(cx).0.len(), shown);
    // The bar offers to take the picked games out of Steam too.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("bulk-steam-remove").is_some());
    }
    // Redrawing with everything picked doesn't work out the bar again,
    // unless one of its inputs changed meanwhile: the library or the shown
    // list (another test's game arriving in the state folder they share,
    // a filter's data arriving in the background) or a game's status.
    let seen = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let model = models(cx).library.read(cx);
            let shown = view.read(cx).visible_games();
            let statuses = models(cx).statuses.read(cx).all().count();
            (
                model.snapshot.revision,
                Arc::as_ptr(&model.user) as usize,
                Arc::as_ptr(&shown) as *const () as usize,
                statuses,
            )
        })
    };
    let (computes, before) = (cx.read(|cx| view.read(cx).bulk_computes()), seen(cx));
    draw(cx);
    draw(cx);
    if seen(cx) == before {
        assert_eq!(cx.read(|cx| view.read(cx).bulk_computes()), computes);
    }

    // A filter hiding picked games drops them from the selection.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.update_filter(|f| f.install = InstallFilter::Installed, cx)
        })
    });
    draw(cx);
    // Of this test's own games (another test's may be installed too).
    let ours = ["Alan Wake", "Celeste", "Hades", "Inside", "Outer Wilds"];
    let picked: Vec<String> = state(cx)
        .0
        .into_iter()
        .filter(|t| ours.contains(&t.as_str()))
        .collect();
    assert_eq!(picked, ["Celeste", "Hades", "Outer Wilds"]);

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

    // With details on click off, a click only selects; I still opens the
    // drawer, which then follows the clicks.
    cx.update(|cx| view.update(cx, |v, cx| v.set_open_details_for_test(false, cx)));
    click(cx, "1", 1);
    assert_eq!(focused(cx), Some(game("1")));
    assert!(!state(cx).1);
    cx.simulate_keystrokes(window, "i");
    draw(cx);
    click(cx, "3", 1);
    assert!(state(cx).1);
    assert_eq!(focused(cx), Some(game("3")));
    cx.update(|cx| view.update(cx, |v, cx| v.set_open_details_for_test(true, cx)));
    cx.simulate_keystrokes(window, "escape");
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

/// An added-games or ROM store whose removal of `fails` is refused, and
/// which records every removal with whether its files were to go.
struct RemovableStore {
    runner: Runner,
    games: Vec<Arc<GameInfo>>,
    fails: &'static str,
    removed: parking_lot::Mutex<Vec<(String, bool)>>,
}

impl RemovableStore {
    fn new(runner: Runner, apps: &[&str], fails: &'static str) -> Arc<Self> {
        let games = apps
            .iter()
            .map(|app| {
                Arc::new(
                    GameInfo {
                        runner: Some(runner),
                        app_name: (*app).into(),
                        title: (*app).into(),
                        is_installed: true,
                        ..Default::default()
                    }
                    .finalize(),
                )
            })
            .collect();
        Arc::new(Self {
            runner,
            games,
            fails,
            removed: Default::default(),
        })
    }
}

#[async_trait]
impl Store for RemovableStore {
    fn runner(&self) -> Runner {
        self.runner
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        None
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("no accounts")
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
        bail!("not installable")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("not installable")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("not installable")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("not installable")
    }
    async fn uninstall(&self, app_name: &str, args: RemoveArgs) -> Result<()> {
        if app_name == self.fails {
            bail!("{app_name} is in use");
        }
        self.removed
            .lock()
            .push((app_name.to_string(), args.delete_files));
        Ok(())
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("not installable")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("not installable")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("not playable")
    }
}

#[test]
fn removing_several_games_reports_the_ones_that_failed() {
    use crate::view::remove::{plan_removal, remove_games};
    let _guard = UI_TEST_LOCK.lock();
    let root = isolate_paths();
    let _ = std::fs::remove_dir_all(&root);
    let added = RemovableStore::new(Runner::Sideload, &["notes", "tool"], "");
    let roms = RemovableStore::new(Runner::Emulation, &["zelda", "mario"], "mario");
    let stores: Vec<DynStore> = vec![MemoryStore::new(), added.clone(), roms.clone()];
    let b = Backend::start_for_tests(move |_, _| stores).expect("backend starts");
    let picked: Vec<Arc<GameInfo>> = [
        b.game(&GameId::new(Runner::Gog, "2")),
        b.game(&GameId::new(Runner::Sideload, "notes")),
        b.game(&GameId::new(Runner::Emulation, "zelda")),
        b.game(&GameId::new(Runner::Emulation, "mario")),
        b.game(&GameId::new(Runner::Sideload, "tool")),
    ]
    .into_iter()
    .map(|g| g.expect("the game is known"))
    .collect();
    let plan = plan_removal(&picked, |_| false);
    assert_eq!((plan.remove.len(), plan.stores), (4, 1));

    let outcome = b
        .runtime()
        .block_on(remove_games(&b, plan.remove.clone(), true));
    assert_eq!(outcome.removed, 3);
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].0, "mario");
    assert!(
        outcome.failed[0].1.contains("in use"),
        "{:?}",
        outcome.failed
    );
    // Added games' files go only because the box was ticked; ROMs never.
    assert_eq!(
        *added.removed.lock(),
        [("notes".to_string(), true), ("tool".to_string(), true)]
    );
    assert_eq!(*roms.removed.lock(), [("zelda".to_string(), false)]);

    let kept = b
        .runtime()
        .block_on(remove_games(&b, vec![picked[1].clone()], false));
    assert_eq!(kept.removed, 1);
    assert_eq!(
        added.removed.lock().last(),
        Some(&("notes".to_string(), false))
    );
}

#[gpui_kit::test]
fn the_list_opens_at_its_top_and_its_scrollbar_knows_every_row(cx: &mut TestAppContext) {
    use irori_core::settings::LibraryLayout;
    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(820.), px(380.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let draw = |cx: &mut TestAppContext| {
        for _ in 0..2 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    let scroll = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).scroll_state());
    draw(cx);
    // Scrolled down the grid with nothing selected…
    cx.update(|cx| view.update(cx, |v, cx| v.scroll_for_test(px(100.), cx)));
    draw(cx);
    assert!(scroll(cx).0 > px(0.), "the grid scrolled: {:?}", scroll(cx));
    cx.update(|cx| view.update(cx, |v, cx| v.clear_focus_for_test(cx)));
    draw(cx);
    // …the list opens at its top, and its scrollbar spans every row.
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::List, cx)));
    draw(cx);
    let (offset, content, items, viewport) = scroll(cx);
    assert!(
        items > viewport,
        "the list scrolls: {items:?} in {viewport:?}"
    );
    assert_eq!(offset, px(0.), "the list opens at its top");
    assert!(
        (content - items).abs() < px(1.),
        "the scrollbar knows {content:?} of {items:?}"
    );
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

    // A tag chip gives the filters way to that tag alone.
    cx.update(|cx| crate::show_tag("Roguelike".into(), cx));
    let (tags, modes) = cx.read(|cx| {
        let filters = crate::filters_panel::active_filters(cx).expect("the view's filters");
        let filter = filters.read(cx).filter();
        (filter.tags.clone(), filter.modes.clone())
    });
    assert_eq!(tags, ["Roguelike".to_string()].into());
    assert!(modes.is_empty());
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
fn installer_detection_fills_single_executable_and_asks_for_multiple(cx: &mut TestAppContext) {
    use crate::dialogs::sideload::InstallerRun;
    use irori_sideload::scan::{DetectionOutcome, InstalledCandidate, PrefixSnapshot};

    let _backend = start(cx);
    let prefill = SideloadApp {
        title: "Skyrim".into(),
        platform: SideloadPlatform::Windows,
        ..Default::default()
    };
    let (window, dialog) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| SideloadDialog::with_app(None, Some(prefill), window, cx))
            })
        })
        .expect("window opens");
    cx.run_until_parked();

    let run = InstallerRun {
        prefix: "/pfx".into(),
        snapshot: Arc::new(PrefixSnapshot::default()),
        installer: "/tmp/setup.exe".into(),
        extra_dir: None,
    };

    // 1. Single detected executable fills Executable, Install folder and Arguments automatically.
    let single = InstalledCandidate {
        executable: "/pfx/drive_c/Games/Skyrim/SkyrimSE.exe".into(),
        args: "-fullscreen".into(),
        install_folder: Some("/pfx/drive_c/Games/Skyrim".into()),
        label: None,
        from_shortcut: false,
    };
    cx.update_window(window, |_, window, cx| {
        dialog.update(cx, |d, cx| {
            d.apply_detection(
                run.clone(),
                DetectionOutcome::Picked(single.clone()),
                window,
                cx,
            );
        });
    })
    .unwrap();
    let app = cx.read(|cx| dialog.read(cx).collect(cx));
    assert_eq!(
        app.executable.as_deref(),
        Some(Path::new("/pfx/drive_c/Games/Skyrim/SkyrimSE.exe"))
    );
    assert_eq!(
        app.install_folder.as_deref(),
        Some(Path::new("/pfx/drive_c/Games/Skyrim"))
    );
    assert_eq!(app.launcher_args, "-fullscreen");

    // 2. Multiple detected executables wait for the user to choose one.
    let launcher = InstalledCandidate {
        executable: "/pfx/drive_c/Games/Skyrim/SkyrimSELauncher.exe".into(),
        args: String::new(),
        install_folder: Some("/pfx/drive_c/Games/Skyrim".into()),
        label: None,
        from_shortcut: false,
    };
    cx.update_window(window, |_, window, cx| {
        dialog.update(cx, |d, cx| {
            d.apply_detection(
                run.clone(),
                DetectionOutcome::Ambiguous(vec![single.clone(), launcher.clone()]),
                window,
                cx,
            );
            d.choose_installer_candidate(1, window, cx);
        });
    })
    .unwrap();
    let chosen = cx.read(|cx| dialog.read(cx).collect(cx));
    assert_eq!(
        chosen.executable.as_deref(),
        Some(Path::new("/pfx/drive_c/Games/Skyrim/SkyrimSELauncher.exe"))
    );
}

#[gpui_kit::test]
fn view_tabs_apply_presets_and_follow_quick_controls_setting(cx: &mut TestAppContext) {
    use crate::chips::PresetChip;
    use crate::filter::InstallFilter;

    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    // Preset counts match the in-memory store (5 games, 3 installed, 0 favourites).
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(v.view_tabs_shown());
        let counts = v.chip_counts();
        assert_eq!(counts.preset(PresetChip::All), Some(5));
        assert_eq!(counts.preset(PresetChip::Installed), Some(3));
        assert_eq!(counts.preset(PresetChip::Favourites), Some(0));
        assert_eq!(counts.preset(PresetChip::RecentlyAdded), None);
    });

    // Clicking the Installed tab narrows the filter and visible games.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let bounds = vcx
            .debug_bounds("chip-installed")
            .expect("Installed tab is rendered");
        vcx.simulate_click(bounds.center(), Default::default());
    }
    settle(cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).filter(cx).install, InstallFilter::Installed);
        assert_eq!(view.read(cx).visible_games().len(), 3);
    });

    // Clicking the All tab restores all games.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let bounds = vcx.debug_bounds("chip-all").expect("All tab is rendered");
        vcx.simulate_click(bounds.center(), Default::default());
    }
    settle(cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).filter(cx).install, InstallFilter::Any);
        assert_eq!(view.read(cx).visible_games().len(), 5);
    });

    // Turning `libraryQuickControls` off hides the view tabs strip.
    let mut off = false;
    for _ in 0..50 {
        cx.update(|cx| view.update(cx, |v, cx| v.set_quick_controls_for_test(false, cx)));
        settle(cx);
        if !cx.read(|cx| view.read(cx).view_tabs_shown()) {
            off = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(off, "the setting turns the view tabs off");
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("chip-all").is_none(),
            "tabs disappear when quick controls are off"
        );
    }
}

#[gpui_kit::test]
fn smart_categories_filter_the_library_and_reopen_as_saved(cx: &mut TestAppContext) {
    use irori_services::{CategoryRef, Match, Rule, SmartCategory};

    let backend = start(cx);
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
    let wait_for = |cx: &mut TestAppContext, done: &dyn Fn(&mut TestAppContext) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(cx) {
            assert!(Instant::now() < deadline, "the library never caught up");
            std::thread::sleep(Duration::from_millis(10));
            draw(cx);
        }
    };
    draw(cx);

    // "Any" with no rules and a "Leave out" row: every game but Hades.
    let rest = SmartCategory::new(
        "Not Hades",
        Match::Any,
        Vec::new(),
        vec![Rule::Query {
            query: "hades".into(),
        }],
    );
    let id = backend.userdata.save_smart_category(None, rest.clone());
    let smart = CategoryRef::Smart(id.clone());
    wait_for(cx, &|cx| {
        cx.read(|cx| models(cx).library.read(cx).categories.count(&smart) == 4)
    });
    cx.update(|cx| view.update(cx, |v, cx| v.update_filter(|f| f.toggle(&smart), cx)));
    draw(cx);
    assert_eq!(
        titles(cx),
        vec!["Alan Wake", "Celeste", "Inside", "Outer Wilds"]
    );

    // The editor shows it as it was saved: "any", the row left out.
    let editor = cx
        .update_window(window, |_, window, cx| {
            cx.new(|cx| {
                crate::dialogs::smart::SmartEditor::new(Some(id.clone()), rest.clone(), window, cx)
            })
        })
        .unwrap();
    cx.read(|cx| {
        let editor = editor.read(cx);
        assert_eq!(editor.matching(), &Match::Any);
        assert_eq!(editor.draft(cx), rest);
    });

    // Deleted, it leaves the filter: every game again.
    backend.userdata.delete_smart_category(&id);
    wait_for(cx, &|cx| titles(cx).len() == 5);
    assert!(cx.read(|cx| view.read(cx).filter(cx).smart_categories.is_empty()));
}

#[gpui_kit::test]
fn the_art_picker_offers_igdb_only_while_it_is_on(cx: &mut TestAppContext) {
    use crate::dialogs::steamgrid::{ArtKind, ArtPicker, ArtSource};

    let _backend = start(cx);
    let open = |igdb_on: bool, cx: &mut TestAppContext| {
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| ArtPicker::with_sources(ArtKind::Cover, "", None, igdb_on, window, cx))
            })
        })
        .expect("window opens")
        .1
    };
    let source = |picker: &Entity<ArtPicker>, cx: &mut TestAppContext| {
        cx.read(|cx| picker.read(cx).source())
    };

    // Off: covers default to keyless Steam (no key needed), while switching to
    // SteamGridDB (or "All" with only one keyless source) still asks for its key.
    let picker = open(false, cx);
    cx.run_until_parked();
    assert_eq!(source(&picker, cx), (ArtSource::Steam, false));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::SteamGridDb, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::SteamGridDb, true));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::All, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::All, true));

    // Logos are not provided by Steam, so without a key they still open on SteamGridDB.
    let logo_picker = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| ArtPicker::with_sources(ArtKind::Logo, "", None, false, window, cx))
            })
        })
        .expect("window opens")
        .1;
    cx.run_until_parked();
    assert_eq!(source(&logo_picker, cx), (ArtSource::SteamGridDb, true));

    // On without a SteamGridDB key: keyless Steam + IGDB search together via All;
    // picking IGDB alone works without a key and SteamGridDB still asks.
    let picker = open(true, cx);
    cx.run_until_parked();
    assert_eq!(source(&picker, cx), (ArtSource::All, false));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::Igdb, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::Igdb, false));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::SteamGridDb, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::SteamGridDb, true));

    // On with a key: all ready sources together from the start.
    let both = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let key = irori_extras::steamgrid::ApiKey::new("key");
                cx.new(|cx| ArtPicker::with_sources(ArtKind::Cover, "", key, true, window, cx))
            })
        })
        .expect("window opens")
        .1;
    cx.run_until_parked();
    assert_eq!(source(&both, cx), (ArtSource::All, false));
}

#[gpui_kit::test]
fn the_art_picker_moves_through_pictures_with_the_arrows(cx: &mut TestAppContext) {
    use crate::dialogs::steamgrid::{ArtKind, ArtPicker, ArtPickerEvent, ArtSource, Found};
    use irori_extras::steamgrid::SgdbImage;

    let _backend = start(cx);
    // The shell binds the library's keys at start.
    cx.update(crate::init);
    let (window, picker) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| ArtPicker::with_sources(ArtKind::Cover, "", None, true, window, cx))
            })
        })
        .expect("window opens");
    let picked = Arc::new(parking_lot::Mutex::new(None::<String>));
    let sink = picked.clone();
    cx.update(|cx| {
        cx.subscribe(&picker, move |_, event: &ArtPickerEvent, _| {
            if let ArtPickerEvent::Picked(url) = event {
                *sink.lock() = Some(url.clone());
            }
        })
        .detach()
    });
    let found = (0..5)
        .map(|id| Found {
            source: ArtSource::Igdb,
            image: SgdbImage {
                id,
                url: format!("https://example.invalid/{id}.png"),
                thumb: String::new(),
                width: Some(600),
                height: Some(900),
            },
        })
        .collect();
    cx.update(|cx| picker.update(cx, |p, cx| p.show_for_test(found, cx)));
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    draw(cx);
    let cursor = |cx: &mut TestAppContext| cx.read(|cx| picker.read(cx).cursor());

    // From the search field, Down reaches the first picture; Right moves on.
    cx.update_window(window, |_, window, cx| {
        picker.read(cx).query_focus(cx).focus(window, cx)
    })
    .unwrap();
    cx.simulate_keystrokes(window, "down");
    draw(cx);
    assert_eq!(cursor(cx), Some(0));
    let holds = cx
        .update_window(window, |_, window, cx| {
            picker.read(cx).holds_keyboard(window)
        })
        .unwrap();
    assert!(holds, "the pictures hold the keyboard");
    cx.simulate_keystrokes(window, "right");
    draw(cx);
    assert_eq!(cursor(cx), Some(1));

    // Enter uses the picture the cursor is on.
    cx.simulate_keystrokes(window, "enter");
    assert_eq!(
        picked.lock().as_deref(),
        Some("https://example.invalid/1.png")
    );

    // Up from the first row goes back to the search field.
    cx.simulate_keystrokes(window, "up");
    assert_eq!(cursor(cx), None);
    let in_search = cx
        .update_window(window, |_, window, cx| {
            picker.read(cx).query_focus(cx).is_focused(window)
        })
        .unwrap();
    assert!(in_search);
}

#[gpui_kit::test]
fn a_roms_cover_picker_opens_on_its_launchbox_boxes(cx: &mut TestAppContext) {
    use irori_extras::launchbox::{BoxArt, IndexState, LaunchBoxGame, SystemIndex, table_key};

    use crate::dialogs::steamgrid::{ArtKind, ArtPicker, ArtSource};

    let backend = start(cx);
    let dir = Paths::get().cache.join("ui-launchbox");
    let rom = dir.join("roms/Super Mario Bros. 3 (Europe).nes");
    std::fs::create_dir_all(rom.parent().unwrap()).unwrap();
    std::fs::write(&rom, b"NES\x1a").unwrap();
    let settings = backend.settings.clone();
    let make = |cache: &str| {
        let settings = settings.clone();
        irori_emulation::EmulationStore::with_dirs(
            backend.bus.clone(),
            Arc::new(move || settings.app()),
            dir.join("config"),
            dir.join(cache),
        )
    };

    // LaunchBox's index as a download leaves it.
    let index = dir.join("cache/launchbox");
    let art = |file: &str, region: &str| BoxArt {
        file: file.into(),
        region: Some(region.into()),
        reconstructed: false,
    };
    irori_core::json_store::write(
        &index.join("systems/nes.json"),
        &SystemIndex {
            games: vec![LaunchBoxGame {
                id: 10,
                name: "Super Mario Bros. 3".into(),
                alternate: Vec::new(),
                boxes: vec![art("us.jpg", "North America"), art("eu.jpg", "Europe")],
            }],
        },
    )
    .unwrap();
    irori_core::json_store::write(
        &index.join("state.json"),
        &IndexState {
            updated_at: irori_extras::launchbox::now(),
            systems: vec!["nes".into()],
            table: table_key(&irori_emulation::launchbox::platforms()),
            ..Default::default()
        },
    )
    .unwrap();

    let open = |store: Arc<irori_emulation::EmulationStore>, cx: &mut TestAppContext| {
        let rt = backend.runtime();
        rt.block_on(store.init()).unwrap();
        let id = rt.block_on(store.add_game(&rom, None, None)).unwrap();
        cx.update(|cx| crate::install_emulation_store(store, cx));
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    ArtPicker::for_game(
                        ArtKind::Cover,
                        Some(&id),
                        "Super Mario Bros. 3",
                        window,
                        cx,
                    )
                })
            })
        })
        .expect("window opens")
        .1
    };
    let wait = |picker: &Entity<ArtPicker>, cx: &mut TestAppContext| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let (captions, offers) = cx.read(|cx| picker.read(cx).launchbox_view());
            if !captions.is_empty() || offers || Instant::now() > deadline {
                return (captions, offers);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    // The ROM's own region comes first, each box labelled with its region.
    let picker = open(make("cache"), cx);
    let (captions, offers) = wait(&picker, cx);
    assert_eq!(
        cx.read(|cx| picker.read(cx).source().0),
        ArtSource::LaunchBox
    );
    assert!(!offers);
    let captions: Vec<String> = captions.iter().map(|c| c.to_string()).collect();
    assert_eq!(captions, ["Europe", "North America"]);

    // Without the game list yet, it offers to download it, and its button
    // turns LaunchBox box art on (this store has nowhere to download from).
    let picker = open(make("cache-empty"), cx);
    let (captions, offers) = wait(&picker, cx);
    assert!(captions.is_empty());
    assert!(offers);
    assert!(!backend.settings.app().launchbox);
    cx.update(|cx| picker.update(cx, |p, cx| p.download_launchbox(cx)));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !backend.settings.app().launchbox {
        assert!(Instant::now() < deadline, "the setting never turned on");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn the_filters_popover_finds_a_value_by_name(cx: &mut TestAppContext) {
    use crate::filters_panel::{FilterModel, FiltersPanel};

    let _backend = start(cx);
    let model = cx.update(|cx| cx.new(|_| FilterModel::new(crate::filter::ViewFilter::default())));
    let (window, panel) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(800.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|cx| FiltersPanel::new(model.clone(), cx)),
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
    // Nothing searched: the sections, and no values worked out.
    assert_eq!(cx.read(|cx| panel.read(cx).found_values(cx)), None);

    // The play status is found by name, with its count. Tests share one
    // state folder, so another test may have given some games a status.
    cx.update_window(window, |_, window, cx| {
        panel.update(cx, |p, cx| p.search_for("not pla", window, cx))
    })
    .unwrap();
    draw(cx);
    let found = cx
        .read(|cx| panel.read(cx).found_values(cx))
        .expect("searched");
    assert!(
        found.len() == 1 && found.iter().all(|(_, count)| *count > 0),
        "{found:?}"
    );
}

#[gpui_kit::test]
fn picking_a_range_with_the_keyboard(cx: &mut TestAppContext) {
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
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.show_layout_for_test(irori_core::settings::LibraryLayout::List, cx)
        })
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    draw(cx);
    cx.update_window(window, |_, window, cx| {
        view.read(cx).grid_focus_handle().focus(window, cx);
    })
    .unwrap();
    draw(cx);
    let state = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).selection_state());

    // Shift+Down from the first game picks it and the ones stepped onto.
    cx.simulate_keystrokes(window, "home shift-down shift-down");
    draw(cx);
    assert_eq!(
        state(cx),
        (
            vec!["Alan Wake".into(), "Celeste".into(), "Hades".into()],
            true
        )
    );
    // Stepping back unpicks what the range leaves.
    cx.simulate_keystrokes(window, "shift-up");
    draw(cx);
    assert_eq!(state(cx).0, ["Alan Wake", "Celeste"]);
    // A plain move ends the range; Shift+Space ticks one game alone.
    cx.simulate_keystrokes(window, "down down down shift-space");
    draw(cx);
    assert_eq!(state(cx).0, ["Alan Wake", "Celeste", "Outer Wilds"]);
    // Shift+End picks from there to the last game.
    cx.simulate_keystrokes(window, "up shift-end");
    draw(cx);
    assert_eq!(
        state(cx).0,
        ["Alan Wake", "Celeste", "Inside", "Outer Wilds"]
    );
}

#[gpui_kit::test]
fn the_view_follows_the_window_width_when_asked(cx: &mut TestAppContext) {
    use irori_core::settings::LibraryLayout;
    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1600.), px(800.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    let layout = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| view.read(cx).view_state(window).0)
            .unwrap()
    };
    let resize = |cx: &mut TestAppContext, width: f32| {
        cx.simulate_window_resize(window, size(px(width), px(800.)));
        settle(cx);
    };
    settle(cx);
    assert_eq!(layout(cx), LibraryLayout::Grid, "off: nothing switches");
    cx.update(|cx| view.update(cx, |v, cx| v.set_auto_layout_for_test(true, cx)));
    settle(cx);
    assert_eq!(layout(cx), LibraryLayout::Focus, "wide: Focus");
    resize(cx, 700.);
    assert_eq!(layout(cx), LibraryLayout::Grid, "narrow: the grid");
    // A view picked by hand holds while the window stays on its side.
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::Focus, cx)));
    resize(cx, 720.);
    assert_eq!(layout(cx), LibraryLayout::Focus);
    resize(cx, 1600.);
    assert_eq!(layout(cx), LibraryLayout::Focus);
    cx.update(|cx| view.update(cx, |v, cx| v.set_auto_layout_for_test(false, cx)));
}

#[gpui_kit::test]
fn the_art_picker_offers_vndb_while_it_is_on(cx: &mut TestAppContext) {
    use crate::dialogs::steamgrid::{ArtKind, ArtPicker, ArtSource};

    let _backend = start(cx);
    let open = |igdb_on: bool, vndb_on: bool, cx: &mut TestAppContext| {
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    ArtPicker::with_vndb(ArtKind::Cover, "", None, igdb_on, vndb_on, window, cx)
                })
            })
        })
        .expect("window opens")
        .1
    };
    let source = |picker: &Entity<ArtPicker>, cx: &mut TestAppContext| {
        cx.read(|cx| picker.read(cx).source())
    };

    // VNDB alone, without a SteamGridDB key: VNDB first.
    let picker = open(false, true, cx);
    cx.run_until_parked();
    assert_eq!(source(&picker, cx), (ArtSource::Vndb, false));
    // With IGDB on too: Steam + IGDB search together via All, and both IGDB
    // and VNDB remain selectable as sources of their own without a key.
    let picker = open(true, true, cx);
    cx.run_until_parked();
    assert_eq!(source(&picker, cx), (ArtSource::All, false));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::Igdb, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::Igdb, false));
    cx.update(|cx| picker.update(cx, |p, cx| p.set_source(ArtSource::Vndb, cx)));
    assert_eq!(source(&picker, cx), (ArtSource::Vndb, false));
}

#[gpui_kit::test]
fn the_filters_popover_opens_below_trigger_and_closes_on_list_scroll(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    use irori_core::LibraryTopSection;

    let _backend = start(cx);
    cx.update(|cx| {
        let m = models(cx).clone();
        m.settings.update(cx, |s, cx| {
            Arc::make_mut(&mut s.app).library_top_section = LibraryTopSection::RecentlyPlayed;
            cx.notify();
        });
        m.library.update(cx, |l, cx| {
            let mut u = (*l.user).clone();
            let key = GameId::new(Runner::Gog, "2").key();
            if !u.recent.contains(&key) {
                u.recent.insert(0, key);
            }
            l.user = Arc::new(u);
            cx.notify();
        });
    });

    let win_h = px(1032.);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1920.), win_h),
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
    draw(cx);

    let trigger = {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.debug_bounds("library-filters-trigger")
            .expect("filters trigger is rendered")
    };
    assert!(
        trigger.bottom() > px(250.),
        "expected Continue playing row to push toolbar down, got trigger={trigger:?}"
    );

    // Click the Filters button and check its placement on the very first draw.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.simulate_click(trigger.center(), Default::default());
    }
    draw(cx);
    let first = {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.debug_bounds("library-filters-panel")
            .expect("filters popover opened")
    };
    assert!(
        first.top() >= trigger.bottom(),
        "popover overlapped trigger/Continue playing on first frame: trigger={trigger:?} panel={first:?}"
    );
    assert!(
        first.bottom() <= win_h,
        "popover extended past bottom of 1080p window: panel={first:?}"
    );

    // Redrawing must not jump or resize the popover (no feedback loop).
    draw(cx);
    let second = {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.debug_bounds("library-filters-panel")
            .expect("filters popover stayed open")
    };
    assert!(
        (second.top() - first.top()).abs() <= px(0.5)
            && (second.size.height - first.size.height).abs() <= px(0.5),
        "popover jumped between frames: first={first:?} second={second:?}"
    );

    // Scrolling inside the Filters popover must keep it open.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.simulate_event(ScrollWheelEvent {
            position: second.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-60.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
    }
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-panel").is_some(),
            "scrolling inside the popover closed it"
        );
    }

    // Scrolling the library list outside the popover must dismiss it cleanly.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.simulate_event(ScrollWheelEvent {
            position: point(px(1500.), px(700.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
    }
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-panel").is_none(),
            "scrolling the library list left the popover open"
        );
    }
    let _ = view;
}

#[gpui_kit::test]
fn slim_toolbar_look_popover_and_background_fits_unclipped(cx: &mut TestAppContext) {
    use irori_core::settings::LibraryLayout;

    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    // Clicking Look opens the layout-aware popover tailored to Grid, and the
    // entire Grid Look body—including Background—fits unclipped inside 1400×900.
    let look_trigger = {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let b = vcx
            .debug_bounds("library-look-trigger")
            .expect("Look trigger is rendered");
        vcx.simulate_click(b.center(), Default::default());
        b
    };
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let panel = vcx
            .debug_bounds("library-look-panel")
            .expect("Look panel rendered");
        let body = vcx
            .debug_bounds("library-look-body")
            .expect("Look body rendered");
        let bg = vcx
            .debug_bounds("look-section-background")
            .expect("Background section rendered in Grid Look");
        assert!(vcx.debug_bounds("look-section-tile-size").is_some());
        assert!(vcx.debug_bounds("look-section-focus-size").is_none());
        assert!(vcx.debug_bounds("look-section-columns").is_none());
        assert!(
            bg.bottom() <= body.bottom() + px(1.),
            "Background section must not clip past Look body: bg={bg:?} body={body:?}"
        );
        assert!(
            panel.bottom() <= px(900.),
            "Grid Look popover must fit within 1400×900 window: panel={panel:?}"
        );
    }

    // Switching to Focus adapts the Look popover to Focus controls.
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::Focus, cx)));
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("look-section-focus-size").is_some());
        assert!(vcx.debug_bounds("look-section-tile-size").is_none());
        assert!(vcx.debug_bounds("look-section-columns").is_none());
    }

    // Switching to List adapts the Look popover to List columns.
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::List, cx)));
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("look-section-columns").is_some());
        assert!(vcx.debug_bounds("look-section-tile-size").is_none());
        assert!(vcx.debug_bounds("look-section-focus-size").is_none());
    }

    // Close Look popover and return to Grid.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        vcx.simulate_click(look_trigger.center(), Default::default());
    }
    cx.update(|cx| view.update(cx, |v, cx| v.show_layout_for_test(LibraryLayout::Grid, cx)));
    settle(cx);
}

#[gpui_kit::test]
fn saved_views_stay_in_palette_and_tabs_auto_save_visualization_per_tab(cx: &mut TestAppContext) {
    use crate::filter::Grouping;
    use crate::views::TabKey;
    use irori_core::settings::{CoverShape, LibraryLayout, ListRowSize};
    use irori_services::{LibrarySort, Match, Rule, SmartCategory};

    let backend = start(cx);
    let col_id = backend.userdata.save_smart_category(
        Some("s-souls"),
        SmartCategory::new(
            "Souls-like",
            Match::Any,
            vec![Rule::Tags {
                tags: vec!["Souls-like".into()],
            }],
            vec![],
        ),
    );
    assert_eq!(col_id, "s-souls");
    cx.run_until_parked();

    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    // Legacy saved views do NOT appear in the top tab strip (only presets +
    // collections + direct "+ New collection" button), yet apply_view still
    // works from the command palette.
    let (grid_id, list_id) = cx.update(|cx| {
        let model = crate::views::saved_views(cx);
        model.update(cx, |m, cx| {
            m.update(
                |v| {
                    let g = v
                        .add("All Grid", &Default::default(), Some(LibraryLayout::Grid))
                        .unwrap();
                    let l = v
                        .add("All List", &Default::default(), Some(LibraryLayout::List))
                        .unwrap();
                    (g, l)
                },
                cx,
            )
        })
    });
    assert_eq!((grid_id, list_id), (2, 3));
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("library-new-collection-tab").is_some());
        assert!(vcx.debug_bounds("chip-recently-added").is_some());
        assert!(vcx.debug_bounds("view-tab:2").is_none());
        assert!(vcx.debug_bounds("view-tab:3").is_none());
    }

    // Command palette apply_view still switches layouts cleanly.
    cx.update(|cx| crate::views::apply_view(list_id, cx));
    settle(cx);
    assert_eq!(
        cx.update_window(window, |_, window, cx| view.read(cx).view_state(window).0)
            .unwrap(),
        LibraryLayout::List
    );
    cx.update(|cx| crate::views::apply_view(grid_id, cx));
    settle(cx);
    assert_eq!(
        cx.update_window(window, |_, window, cx| view.read(cx).view_state(window).0)
            .unwrap(),
        LibraryLayout::Grid
    );

    // 1. Click the dynamic collection tab and customize it: Grid + Banner + 13.5rem + Score.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let tab = vcx
            .debug_bounds("collection-tab:s-souls")
            .expect("collection tab rendered");
        vcx.simulate_click(tab.center(), Default::default());
    }
    settle(cx);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.set_layout(LibraryLayout::Grid, cx);
            v.set_cover_shape(CoverShape::Banner, cx);
            v.set_tile_size(13.5, cx);
            v.update_filter(|f| f.sort = LibrarySort::Score, cx);
        });
    });
    settle(cx);

    // 2. Switch to Installed and customize it independently: List + Large + LastPlayed + Status group.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let tab = vcx
            .debug_bounds("chip-installed")
            .expect("Installed tab rendered");
        vcx.simulate_click(tab.center(), Default::default());
    }
    settle(cx);
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.set_layout(LibraryLayout::List, cx);
            v.set_list_row_size(ListRowSize::Large, cx);
            v.update_filter(
                |f| {
                    f.sort = LibrarySort::LastPlayed;
                    f.group = Grouping::Status;
                },
                cx,
            );
        });
    });
    settle(cx);

    // 3. Return to the collection: exact Grid + Banner + 13.5rem + Score restored!
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let tab = vcx
            .debug_bounds("collection-tab:s-souls")
            .expect("collection tab rendered");
        vcx.simulate_click(tab.center(), Default::default());
    }
    settle(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        let f = v.filter(cx);
        assert_eq!(v.current_layout(), LibraryLayout::Grid);
        assert_eq!(v.current_cover_shape(), CoverShape::Banner);
        assert!((v.current_tile_rem() - 13.5).abs() < 1e-4);
        assert_eq!(f.sort, LibrarySort::Score);
        assert_eq!(f.group, Grouping::None);
    });

    // 4. Return to Installed: exact List + Large + LastPlayed + Status restored!
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let tab = vcx
            .debug_bounds("chip-installed")
            .expect("Installed tab rendered");
        vcx.simulate_click(tab.center(), Default::default());
    }
    settle(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        let f = v.filter(cx);
        assert_eq!(v.current_layout(), LibraryLayout::List);
        assert_eq!(v.current_list_row_size(), ListRowSize::Large);
        assert_eq!(f.sort, LibrarySort::LastPlayed);
        assert_eq!(f.group, Grouping::Status);
    });

    // 5. Switch to Favorites (untouched): inherits clean global defaults, zero cross-tab leak!
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let tab = vcx
            .debug_bounds("chip-favourites")
            .expect("Favorites tab rendered");
        vcx.simulate_click(tab.center(), Default::default());
    }
    settle(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        let f = v.filter(cx);
        assert_eq!(v.current_layout(), LibraryLayout::Grid);
        assert_eq!(v.current_cover_shape(), CoverShape::Portrait);
        assert_eq!(f.sort, LibrarySort::Title);
        assert_eq!(f.group, Grouping::None);
    });
    assert!(cx.update(|cx| {
        crate::views::saved_views(cx)
            .read(cx)
            .views()
            .prefs_for(&TabKey::Collection(col_id))
            .is_some()
    }));
}

#[gpui_kit::test]
fn filters_more_section_collapses_by_default_and_auto_expands_when_active(cx: &mut TestAppContext) {
    use crate::filters_panel::{FilterModel, FiltersPanel};

    let _backend = start(cx);
    let model = cx.update(|cx| cx.new(|_| FilterModel::new(crate::filter::ViewFilter::default())));
    let (window, panel) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(800.), px(1200.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|cx| {
                        let panel = FiltersPanel::new(model.clone(), cx);
                        panel.anchor_bottom().set(Some(px(40.)));
                        panel
                    })
                },
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

    // Collapsed by default when no secondary filters are active.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("library-filters-more-toggle").is_some());
        assert!(vcx.debug_bounds("library-filters-more-body").is_none());
    }

    // Setting a secondary filter with a very long Developer name auto-expands
    // More filters and truncates the chip cleanly inside the popover width.
    const LONG_DEV: &str =
        "Super Extremely Long Independent Game Development Studio International Entertainment Corp";
    const LONG_DEV_SELECTOR: &str = "developer-super extremely long independent game development studio international entertainment corp";
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.update(
                |f| {
                    f.developers.insert(LONG_DEV.into());
                },
                cx,
            );
        });
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let panel_bounds = vcx
            .debug_bounds("library-filters-panel")
            .expect("filters panel rendered");
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_some(),
            "active secondary filter must auto-expand More filters"
        );
        let chip_bounds = vcx
            .debug_bounds(LONG_DEV_SELECTOR)
            .expect("long developer chip rendered");
        assert!(
            chip_bounds.right() <= panel_bounds.right(),
            "long developer chip must not overflow popover: chip={chip_bounds:?} panel={panel_bounds:?}"
        );
    }

    // Unchecking the last secondary filter keeps More filters open so it does
    // not slam shut under the user's cursor while editing.
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.update(|f| f.developers.clear(), cx);
        });
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_some(),
            "More filters must stay open while the user edits secondary filters"
        );
        let toggle = vcx
            .debug_bounds("library-filters-more-toggle")
            .expect("More filters toggle rendered");
        vcx.simulate_click(toggle.center(), Default::default());
    }
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_none(),
            "clicking More filters toggle collapses it"
        );
    }

    // Tags is a primary filter now, so activating it must NOT expand More filters.
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.update(
                |f| {
                    f.tags.insert("Roguelike".into());
                },
                cx,
            );
        });
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_none(),
            "primary Tags filter must leave More filters collapsed"
        );
    }

    // Activating a new secondary filter (Show only: Offline) auto-expands
    // More filters again even after a manual collapse.
    cx.update(|cx| {
        model.update(cx, |m, cx| {
            m.update(
                |f| {
                    f.offline_only = true;
                },
                cx,
            );
        });
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_some(),
            "newly activated secondary filter must re-expand More filters"
        );
    }

    // Resetting state after clearing returns More filters to collapsed.
    cx.update(|cx| {
        panel.update(cx, |p, _| p.reset_more_open());
        model.update(cx, |m, cx| {
            m.update(crate::filter::ViewFilter::reset_filters, cx)
        });
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-filters-more-body").is_none(),
            "clearing and resetting collapses More filters"
        );
    }
}

#[gpui_kit::test]
fn look_popover_slider_clearance_and_single_line_segments(cx: &mut TestAppContext) {
    let _backend = start(cx);
    let (window, _view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let trigger = vcx
            .debug_bounds("library-look-trigger")
            .expect("Look trigger rendered");
        vcx.simulate_click(trigger.center(), Default::default());
    }
    settle(cx);

    let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
    let panel = vcx
        .debug_bounds("library-look-panel")
        .expect("Look panel rendered");
    let body = vcx
        .debug_bounds("library-look-body")
        .expect("Look body rendered");
    let slider_wrap = vcx
        .debug_bounds("look-tile-slider-wrap")
        .expect("Tile size slider wrap rendered");
    assert!(
        slider_wrap.left() - body.left() >= px(12.),
        "slider must leave clearance for left thumb + hover ring: wrap={slider_wrap:?} body={body:?}"
    );
    assert!(
        body.right() - slider_wrap.right() >= px(12.),
        "slider must leave clearance on right edge too: wrap={slider_wrap:?} body={body:?}"
    );

    // Top of page (4 segments) stays on one horizontal line and uses proportional
    // width so longer labels ("Favourites") get more room than short ones ("Off").
    let top_ids = [
        "look-top-recent",
        "look-top-installed",
        "look-top-favourites",
        "look-top-none",
    ];
    let top_bounds: Vec<_> = top_ids
        .iter()
        .map(|id| vcx.debug_bounds(id).expect(id))
        .collect();
    for pair in top_bounds.windows(2) {
        assert!(
            (pair[0].origin.y - pair[1].origin.y).abs() <= px(1.),
            "Top of page segments must share one line: {top_bounds:?}"
        );
        assert!(
            pair[1].left() >= pair[0].right(),
            "Top of page segments must not overlap: {top_bounds:?}"
        );
    }
    assert!(
        top_bounds.last().unwrap().right() <= panel.right(),
        "Top of page segments must stay within Look panel"
    );
    assert!(
        top_bounds[2].size.width > top_bounds[3].size.width,
        "Favourites should receive more width than Off with flex_auto: {top_bounds:?}"
    );
}

#[gpui_kit::test]
fn sideload_dialog_renders_quick_bar_and_two_detail_tabs(cx: &mut TestAppContext) {
    use crate::dialogs::sideload::DetailsTab;

    let _backend = start(cx);
    let (window, dialog) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(900.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| SideloadDialog::with_app(None, None, window, cx)),
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

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("sideload-quick-bar").is_some());
        assert!(vcx.debug_bounds("sideload-panel-artwork").is_none());
        assert!(vcx.debug_bounds("sideload-panel-details").is_none());
    }

    // Expanding More details defaults to the Artwork tab.
    cx.update(|cx| dialog.update(cx, |d, cx| d.toggle_more_for_test(true, cx)));
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("sideload-panel-artwork").is_some());
        assert!(vcx.debug_bounds("sideload-panel-details").is_none());
    }

    // Switching to Details & Launch swaps cleanly to the details form.
    cx.update(|cx| {
        dialog.update(cx, |d, cx| {
            d.set_details_tab_for_test(DetailsTab::Details, cx)
        })
    });
    draw(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("sideload-panel-artwork").is_none());
        assert!(vcx.debug_bounds("sideload-panel-details").is_some());
        assert!(
            vcx.debug_bounds("sideload-fetch-art").is_some(),
            "Add game dialog must expose the explicit Fetch art button"
        );
    }
}

#[gpui_kit::test]
fn sort_popover_collapses_more_and_auto_expands_for_secondary(cx: &mut TestAppContext) {
    use irori_services::LibrarySort;

    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    // Click Sort & Group trigger; More options starts collapsed.
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let trigger = vcx
            .debug_bounds("library-sort-trigger")
            .expect("Sort trigger rendered");
        vcx.simulate_click(trigger.center(), Default::default());
    }
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("library-sort-panel").is_some());
        assert!(vcx.debug_bounds("library-sort-more-toggle").is_some());
        assert!(vcx.debug_bounds("library-sort-more-body").is_none());
        let toggle = vcx
            .debug_bounds("library-sort-more-toggle")
            .expect("More options toggle");
        vcx.simulate_click(toggle.center(), Default::default());
    }
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-sort-more-body").is_some(),
            "clicking More options expands secondary sorts and groupings"
        );
    }

    // Selecting a secondary sort and reopening the popover auto-expands More options.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.set_sort_for_test(LibrarySort::Rating, cx);
            v.close_sort_popover_for_test(cx);
        });
    });
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let trigger = vcx
            .debug_bounds("library-sort-trigger")
            .expect("Sort trigger rendered");
        vcx.simulate_click(trigger.center(), Default::default());
    }
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("library-sort-more-body").is_some(),
            "secondary sort must auto-expand More options when reopening popover"
        );
        // Switching to Playtime (naturally descending) resets descending=false
        // so Desc is active; clicking Asc flips descending=true (lowest first).
        let playtime = vcx
            .debug_bounds("sort-opt-Playtime")
            .expect("Playtime sort pill");
        vcx.simulate_click(playtime.center(), Default::default());
    }
    settle(cx);
    assert_eq!(
        cx.read(|cx| {
            let f = view.read(cx).filter(cx);
            (f.sort, f.descending)
        }),
        (LibrarySort::Playtime, false)
    );
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let asc = vcx.debug_bounds("sort-dir-asc").expect("Asc button");
        vcx.simulate_click(asc.center(), Default::default());
    }
    settle(cx);
    assert_eq!(
        cx.read(|cx| {
            let f = view.read(cx).filter(cx);
            (f.sort, f.descending)
        }),
        (LibrarySort::Playtime, true)
    );
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let title = vcx.debug_bounds("sort-opt-Title").expect("Title sort pill");
        vcx.simulate_click(title.center(), Default::default());
    }
    settle(cx);
    assert_eq!(
        cx.read(|cx| {
            let f = view.read(cx).filter(cx);
            (f.sort, f.descending)
        }),
        (LibrarySort::Title, false)
    );
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let desc = vcx.debug_bounds("sort-dir-desc").expect("Desc button");
        vcx.simulate_click(desc.center(), Default::default());
    }
    settle(cx);
    assert_eq!(
        cx.read(|cx| {
            let f = view.read(cx).filter(cx);
            (f.sort, f.descending)
        }),
        (LibrarySort::Title, true)
    );
}

#[gpui_kit::test]
fn edit_tabs_popover_reorders_hides_and_protects_locked_tabs(cx: &mut TestAppContext) {
    use crate::views::{TabKey, saved_views};

    let _backend = start(cx);
    let (window, _view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let trigger = vcx
            .debug_bounds("library-edit-tabs-trigger")
            .expect("Edit tabs trigger rendered");
        vcx.simulate_click(trigger.center(), Default::default());
    }
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("library-edit-tabs-panel").is_some());
        assert!(vcx.debug_bounds("tab-mgr-row-all").is_some());
        assert!(vcx.debug_bounds("tab-mgr-row-installed").is_some());
        assert!(vcx.debug_bounds("tab-mgr-row-favorites").is_some());
        assert!(vcx.debug_bounds("tab-mgr-row-recently-added").is_some());
    }

    // All and Installed stay locked; Favorites and Recently added can be hidden.
    cx.update(|cx| {
        saved_views(cx).update(cx, |m, cx| {
            m.update(
                |v| {
                    v.set_tab_hidden(&TabKey::All, true);
                    v.set_tab_hidden(&TabKey::Installed, true);
                    v.set_tab_hidden(&TabKey::Favorites, true);
                    v.set_tab_hidden(&TabKey::RecentlyAdded, true);
                    v.move_tab(&TabKey::Installed, &TabKey::All, &[]);
                },
                cx,
            );
        });
    });
    settle(cx);
    let visible = cx.update(|cx| saved_views(cx).read(cx).views().visible_tabs(&[]));
    assert_eq!(visible, vec![TabKey::Installed, TabKey::All]);
}

#[gpui_kit::test]
fn list_look_popover_switches_row_size(cx: &mut TestAppContext) {
    use crate::views::{TabKey, saved_views};
    use irori_core::{LibraryLayout, ListRowSize};

    let _backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    cx.update(|cx| {
        view.update(cx, |v, cx| v.set_layout_for_test(LibraryLayout::List, cx));
    });
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let trigger = vcx
            .debug_bounds("library-look-trigger")
            .expect("Look trigger rendered");
        vcx.simulate_click(trigger.center(), Default::default());
    }
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(vcx.debug_bounds("look-section-row-size").is_some());
        let large = vcx
            .debug_bounds("look-row-large")
            .expect("Large row size segment");
        vcx.simulate_click(large.center(), Default::default());
    }
    settle(cx);

    assert_eq!(
        cx.read(|cx| view.read(cx).current_list_row_size()),
        ListRowSize::Large
    );
    assert_eq!(
        cx.update(|cx| {
            saved_views(cx)
                .read(cx)
                .views()
                .prefs_for(&TabKey::All)
                .and_then(|p| p.list_row_size)
        }),
        Some(ListRowSize::Large)
    );
}

#[gpui_kit::test]
fn deleting_active_collection_clears_filter_and_routes_advanced_rules(cx: &mut TestAppContext) {
    use crate::dialogs::collection::{delete_collection_with_undo, open_collection_dialog};
    use crate::views::{TabKey, saved_views};
    use gpui_kit::component::WindowExt as _;
    use irori_core::settings::CoverShape;
    use irori_services::{Match, Rule, SmartCategory};

    let backend = start(cx);
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    // 1. Create, activate and customize a dynamic collection, then delete it:
    // active filters and per-tab prefs must drop the deleted smart ID.
    let col_id = backend.userdata.save_smart_category(
        None,
        SmartCategory::new(
            "Souls-like",
            Match::Any,
            vec![Rule::Tags {
                tags: vec!["Souls-like".into()],
            }],
            vec![],
        ),
    );
    cx.run_until_parked();
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.apply_collection_tab(col_id.clone(), cx);
            v.set_cover_shape(CoverShape::Banner, cx);
        });
    });
    settle(cx);
    assert!(cx.read(|cx| view.read(cx).filter(cx).smart_categories.contains(&col_id)));

    cx.update_window(window, |_, window, cx| {
        delete_collection_with_undo(col_id.clone(), window, cx);
    })
    .unwrap();
    settle(cx);
    let after = cx.read(|cx| view.read(cx).filter(cx));
    assert!(
        !after.smart_categories.contains(&col_id),
        "deleting active collection must clear its ID from ViewFilter"
    );
    assert_eq!(after.active_count(), 0);
    assert!(cx.update(|cx| {
        saved_views(cx)
            .read(cx)
            .views()
            .prefs_for(&TabKey::Collection(col_id.clone()))
            .is_none()
    }));

    // 2. Editing a rule-based (non-tag) smart category via open_collection_dialog
    // must route to the full SmartEditor rather than CollectionDialog so rules
    // are never silently stripped.
    let adv_id = backend.userdata.save_smart_category(
        None,
        SmartCategory::new(
            "Installed Only",
            Match::All,
            vec![Rule::Installed { installed: true }],
            vec![],
        ),
    );
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        open_collection_dialog(Some(adv_id.clone()), window, cx);
    })
    .unwrap();
    settle(cx);
    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        assert!(
            vcx.debug_bounds("collection-dialog").is_none(),
            "advanced smart categories must not open in tag-only CollectionDialog"
        );
    }
    cx.update_window(window, |_, window, cx| window.close_dialog(cx))
        .unwrap();
    settle(cx);
}

#[gpui_kit::test]
fn collection_dialog_scrolls_all_tags_and_supports_icon_and_manual_games(cx: &mut TestAppContext) {
    use crate::dialogs::collection::open_collection_dialog_with_games;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, point};

    let _backend = start(cx);
    let (window, _view) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1400.), px(900.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| LibraryView::new(window, cx)),
            )
        })
        .expect("window opens");
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);

    cx.update_window(window, |_, window, cx| {
        open_collection_dialog_with_games(None, vec!["steam:10".into()], window, cx);
    })
    .unwrap();
    settle(cx);

    {
        let mut vcx = gpui_kit::VisualTestContext::from_window(window, cx);
        let dialog_bounds = vcx
            .debug_bounds("collection-dialog")
            .expect("collection dialog must render");
        assert!(dialog_bounds.size.width > px(500.));
        assert!(
            vcx.debug_bounds("collection-icon-picker").is_some(),
            "icon picker must render"
        );
        assert!(
            vcx.debug_bounds("collection-icon-flame").is_some(),
            "curated flame icon must render"
        );
        assert!(
            vcx.debug_bounds("collection-games-picker").is_some(),
            "manual games column must render"
        );

        let viewport_bounds = vcx
            .debug_bounds("collection-tag-viewport")
            .expect("tag viewport must render");
        assert!(
            viewport_bounds.size.height <= px(280.),
            "tag viewport must be bounded (~15.5rem), got {:?}",
            viewport_bounds.size.height
        );
        let last_tag_before = vcx
            .debug_bounds("collection-tag-vr")
            .expect("late alphabetical tag 'VR' must be laid out in picker")
            .origin
            .y;
        assert!(
            last_tag_before > viewport_bounds.bottom(),
            "80+ tags must overflow the 15.5rem viewport before scrolling"
        );

        // Scroll downward inside the visible tag viewport and confirm the late tag moves up.
        vcx.simulate_event(ScrollWheelEvent {
            position: viewport_bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-240.))),
            ..Default::default()
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let last_tag_after = vcx
            .debug_bounds("collection-tag-vr")
            .expect("tag 'VR' still present after scrolling")
            .origin
            .y;
        assert!(
            last_tag_after < last_tag_before,
            "collection-tag-picker must actually scroll (before={last_tag_before:?}, after={last_tag_after:?})"
        );
    }

    cx.update_window(window, |_, window, cx| window.close_dialog(cx))
        .unwrap();
    settle(cx);
}
