//! Big Picture driven end to end in a test window over an in-memory store:
//! keyboard and gamepad actions through Home, the Library tab's chips and
//! search, a game's page, the quick menu and the uninstall question.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};
use irori_core::{
    DynStore, ExtraInfo, GameId, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform,
    InstallRequest, LaunchPlan, LaunchRequest, LoginFlow, OpContext, Outcome, Paths, RemoveArgs,
    Runner, Store, UpdateRequest, UserInfo, async_trait,
};
use irori_services::Backend;
use irori_ui::models;

use super::{BigPictureView, LibraryFocus, Overlay, Tab};
use crate::gamepad;
use crate::model::{GameAction, PageRow, RowKind, Sort, StatusFilter};

struct FakeStore {
    runner: Runner,
    games: Vec<Arc<GameInfo>>,
}

#[async_trait]
impl Store for FakeStore {
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
        bail!("unsupported")
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
        Ok(InstallInfo::default())
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("unsupported")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("unsupported")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("unsupported")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!("unsupported")
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("unsupported")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        Ok(())
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("unsupported")
    }
}

fn game(runner: Runner, app: &str, title: &str, installed: bool) -> Arc<GameInfo> {
    Arc::new(
        GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            is_installed: installed,
            installable: true,
            ..Default::default()
        }
        .finalize(),
    )
}

fn library() -> Vec<Arc<GameInfo>> {
    vec![
        game(Runner::Gog, "a", "Alpha", true),
        game(Runner::Gog, "b", "Bravo", false),
        game(Runner::Legendary, "c", "Charlie", true),
    ]
}

fn start_backend() -> Backend {
    let root = std::env::temp_dir().join(format!("irori-bigpicture-test-{}", std::process::id()));
    Paths::init(Paths::under(&root));
    let backend = Backend::start_headless(|_, _| {
        let games = library();
        let store = |runner: Runner| -> DynStore {
            Arc::new(FakeStore {
                runner,
                games: games
                    .iter()
                    .filter(|g| g.runner() == runner)
                    .cloned()
                    .collect(),
            })
        };
        vec![store(Runner::Gog), store(Runner::Legendary)]
    })
    .expect("backend starts");
    let deadline = Instant::now() + Duration::from_secs(15);
    while backend.library().len() != 3 {
        assert!(Instant::now() < deadline, "test library did not load");
        std::thread::sleep(Duration::from_millis(10));
    }
    backend
}

fn focused_title(view: &Entity<BigPictureView>, cx: &TestAppContext) -> String {
    cx.read(|cx| {
        view.read(cx)
            .focused_game()
            .map(|g| g.title.clone())
            .unwrap_or_default()
    })
}

/// Let the backend's events reach the models until `done` holds.
fn wait_for(cx: &mut TestAppContext, done: impl Fn(&gpui_kit::App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if cx.read(|cx| done(cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<BigPictureView>) {
    // This integration test receives events from the real Tokio backend.
    cx.executor().allow_parking();
    let backend = start_backend();
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
        crate::register_catalogs();
        crate::bind_keys(cx);
    });
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1280.), px(800.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| BigPictureView::new(window, cx)),
        )
        .expect("open test window")
    });
    cx.run_until_parked();
    (handle, view)
}

#[gpui_kit::test]
fn home_library_page_and_overlays_follow_keyboard_and_gamepad(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);

    let fullscreen = cx
        .update_window(handle, |_, window, _| window.is_fullscreen())
        .unwrap();
    assert!(fullscreen, "Big Picture enters fullscreen");
    cx.read(|cx| {
        let kinds: Vec<RowKind> = view.read(cx).rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Installed]);
    });
    assert_eq!(focused_title(&view, cx), "Alpha");

    // Home: gamepad actions and keys drive the same focus; shelves don't wrap.
    cx.dispatch_action(handle, gamepad::Right);
    assert_eq!(focused_title(&view, cx), "Charlie");
    cx.simulate_keystrokes(handle, "right");
    assert_eq!(focused_title(&view, cx), "Charlie", "no wrap at the end");
    cx.simulate_keystrokes(handle, "down");
    assert_eq!(focused_title(&view, cx), "Charlie", "nothing below");

    // The shoulders switch to the Library, which lists every game A–Z.
    cx.dispatch_action(handle, gamepad::NextPage);
    cx.read(|cx| assert_eq!(view.read(cx).tab, Tab::Library));
    assert_eq!(focused_title(&view, cx), "Alpha");
    cx.simulate_keystrokes(handle, "right");
    assert_eq!(focused_title(&view, cx), "Bravo");

    // Up from the first row reaches the chips; Status filters to installed.
    cx.simulate_keystrokes(handle, "up");
    cx.read(|cx| assert_eq!(view.read(cx).library.focus, LibraryFocus::Chips(0)));
    cx.simulate_keystrokes(handle, "right right enter");
    cx.read(|cx| {
        let v = view.read(cx);
        assert_eq!(v.library.filter.status, StatusFilter::Installed);
        let titles: Vec<&str> = v.library.games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, vec!["Alpha", "Charlie"]);
    });
    cx.simulate_keystrokes(handle, "down right");
    assert_eq!(focused_title(&view, cx), "Charlie");

    // Search: letters go to the field (M doesn't open the menu), Enter
    // returns to the results.
    cx.simulate_keystrokes(handle, "up right right right enter");
    cx.read(|cx| assert!(view.read(cx).library.searching));
    cx.simulate_input(handle, "alm");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_overlay_open(), "M typed into the field");
        assert!(v.library.games.is_empty());
    });
    cx.simulate_keystrokes(handle, "backspace backspace");
    cx.simulate_keystrokes(handle, "enter");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.library.searching);
        assert_eq!(v.library.focus, LibraryFocus::Grid);
        assert_eq!(v.library.filter.query, "a");
    });
    assert_eq!(focused_title(&view, cx), "Alpha");

    // Enter opens the game's page; Down and Right toggle Favourite.
    cx.simulate_keystrokes(handle, "enter");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(v.is_page_open());
        assert_eq!(
            v.page.as_ref().map(|p| &p.id),
            Some(&GameId::new(Runner::Gog, "a"))
        );
    });
    cx.dispatch_action(handle, gamepad::Down);
    cx.dispatch_action(handle, gamepad::Right);
    let alpha = GameId::new(Runner::Gog, "a");
    wait_for(cx, |cx| models(cx).library.read(cx).is_favourite(&alpha));

    // Uninstall asks first; Back answers the question, not the page.
    cx.simulate_keystrokes(handle, "down down down enter");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).overlay,
            Some(Overlay::Uninstall {
                id: alpha.clone(),
                choice: 0
            })
        );
    });
    cx.dispatch_action(handle, gamepad::Back);
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_overlay_open());
        assert!(v.is_page_open());
    });

    // Menu opens the quick menu over the page; Back closes it, then the page.
    cx.simulate_keystrokes(handle, "m");
    cx.read(|cx| assert_eq!(view.read(cx).overlay, Some(Overlay::Menu { ix: 0 })));
    cx.dispatch_action(handle, gamepad::Back);
    cx.simulate_keystrokes(handle, "escape");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_page_open());
        assert_eq!(v.tab, Tab::Library);
    });

    // Back on the Library returns Home, where Favourites now has Alpha.
    cx.dispatch_action(handle, gamepad::Back);
    cx.read(|cx| {
        let v = view.read(cx);
        assert_eq!(v.tab, Tab::Home);
        let kinds: Vec<RowKind> = v.rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Favourites, RowKind::Installed]);
    });
}

#[gpui_kit::test]
fn page_rows_and_quick_actions(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // Bravo isn't installed: its page leads with Install and has no
    // Uninstall.
    cx.dispatch_action(handle, gamepad::NextPage);
    cx.dispatch_action(handle, gamepad::Right);
    assert_eq!(focused_title(&view, cx), "Bravo");
    // Another order keeps the focused game.
    cx.simulate_keystrokes(handle, "up right enter");
    cx.read(|cx| assert_eq!(view.read(cx).library.filter.sort, Sort::RecentlyPlayed));
    assert_eq!(focused_title(&view, cx), "Bravo");
    cx.simulate_keystrokes(handle, "down");
    assert_eq!(focused_title(&view, cx), "Bravo");
    cx.dispatch_action(handle, gamepad::Confirm);
    cx.read(|cx| {
        let v = view.read(cx);
        let game = v.page_game(cx).expect("page game");
        let state = BigPictureView::game_state(&game, cx);
        let rows = crate::model::page_rows(state, 1);
        assert_eq!(rows[0], PageRow::Action(GameAction::Install));
        assert!(!rows.contains(&PageRow::Action(GameAction::Uninstall)));
        assert!(!v.hints(cx).is_empty());
    });
    // Guide leaves the page and Big Picture's overlays alike.
    cx.dispatch_action(handle, gamepad::Menu);
    cx.dispatch_action(handle, gamepad::Guide);
    cx.read(|cx| assert!(!view.read(cx).is_overlay_open()));
}
