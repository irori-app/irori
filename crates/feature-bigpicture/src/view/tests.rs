//! Big Picture driven end to end in a test window over an in-memory store:
//! keyboard and gamepad actions through Home, the Library tab's chips and
//! search, a game's page, the quick menu and the uninstall question, and
//! mouse clicks on covers.

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

use super::tile::CoverSlot;
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
    let computes = cx.read(|cx| view.read(cx).show_option_computes());
    cx.simulate_input(handle, "alm");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_overlay_open(), "M typed into the field");
        assert!(v.library.games.is_empty());
        // Typing filters the list but doesn't read the library again for
        // the Show choices.
        assert_eq!(v.show_option_computes(), computes);
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

    // Enter opens the game's page; Right on Favourite (below Play and Play
    // with logs) toggles it.
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
    cx.dispatch_action(handle, gamepad::Down);
    cx.dispatch_action(handle, gamepad::Right);
    let alpha = GameId::new(Runner::Gog, "a");
    wait_for(cx, |cx| models(cx).library.read(cx).is_favourite(&alpha));

    // Uninstall asks first; Back answers the question, not the page.
    cx.simulate_keystrokes(handle, "down down down down enter");
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
        let rows = crate::model::page_rows(state, 1, false);
        assert_eq!(rows[0], PageRow::Action(GameAction::Install));
        assert!(!rows.contains(&PageRow::Action(GameAction::Uninstall)));
        assert!(!v.hints(cx).is_empty());
    });
    // Guide leaves the page and Big Picture's overlays alike.
    cx.dispatch_action(handle, gamepad::Menu);
    cx.dispatch_action(handle, gamepad::Guide);
    cx.read(|cx| assert!(!view.read(cx).is_overlay_open()));
}

#[gpui_kit::test]
fn show_log_puts_the_game_log_beside_the_rows(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    };
    let charlie = GameId::new(Runner::Legendary, "c");
    let path = Paths::get().game_log_file(&charlie);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Starting\nerr: missing dll\n").unwrap();

    // Charlie's page: Play, Play with logs, Favourite, Settings, Show log.
    // Opened directly: tests share a data folder, so Home's shelves depend on
    // what the others favourited.
    view.update(cx, |v, cx| v.open_page(charlie.clone(), cx));
    for _ in 0..4 {
        cx.dispatch_action(handle, gamepad::Down);
    }
    cx.read(|cx| {
        let v = view.read(cx);
        let game = v.page_game(cx).expect("page game");
        let rows = crate::model::page_rows(BigPictureView::game_state(&game, cx), 1, false);
        assert_eq!(rows[1], PageRow::Action(GameAction::PlayWithLog));
        assert_eq!(rows[v.page.as_ref().unwrap().ix], PageRow::Log);
        assert!(v.log.is_none());
    });
    cx.dispatch_action(handle, gamepad::Right);
    draw(cx);
    wait_for(cx, |cx| view.read(cx).log_lines() == Some(2));

    // Leaving the page drops the panel; the next page shows its log again.
    cx.dispatch_action(handle, gamepad::Back);
    draw(cx);
    cx.read(|cx| assert!(view.read(cx).log.is_none()));
    view.update(cx, |v, cx| v.open_page(charlie.clone(), cx));
    draw(cx);
    cx.read(|cx| assert!(view.read(cx).log.is_some()));
    // Turning it off brings the details back.
    view.update(cx, |v, cx| {
        v.show_log = false;
        cx.notify();
    });
    draw(cx);
    cx.read(|cx| assert!(view.read(cx).log.is_none()));
}

#[gpui_kit::test]
fn the_log_pages_with_the_shoulders_and_filters_from_the_page(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    };
    // Alpha's own log: the other log test writes Charlie's.
    let alpha = GameId::new(Runner::Gog, "a");
    let lines: Vec<String> = (0..150)
        .map(|i| {
            if i % 10 == 0 {
                format!("err: failure {i}")
            } else {
                format!("frame {i}")
            }
        })
        .collect();
    let path = Paths::get().game_log_file(&alpha);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, lines.join("\n")).unwrap();

    // Alpha's page: Play, Play with logs, Favourite, Settings, Show log.
    view.update(cx, |v, cx| v.open_page(alpha.clone(), cx));
    for _ in 0..4 {
        cx.dispatch_action(handle, gamepad::Down);
    }
    cx.dispatch_action(handle, gamepad::Right);
    draw(cx);
    wait_for(cx, |cx| view.read(cx).log_lines() == Some(150));
    // The log follows its newest line; drawing again applies the scroll.
    draw(cx);
    draw(cx);
    let offset = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).log_offset().unwrap());
    let bottom = offset(cx);
    assert!(bottom > 0.0, "the log starts at its newest line");

    // The shoulders (and Page Up / Page Down) page through it, not the tabs.
    cx.dispatch_action(handle, gamepad::PrevPage);
    let up = offset(cx);
    assert!(up < bottom, "a page up: {up} < {bottom}");
    cx.simulate_keystrokes(handle, "pageup");
    assert!(offset(cx) < up);
    cx.dispatch_action(handle, gamepad::NextPage);
    cx.dispatch_action(handle, gamepad::NextPage);
    cx.dispatch_action(handle, gamepad::NextPage);
    assert_eq!(offset(cx), bottom, "paging stops at the newest line");
    cx.read(|cx| assert!(view.read(cx).is_page_open()));

    // Under Show log: errors and warnings only, then the text filter.
    cx.read(|cx| {
        let v = view.read(cx);
        let game = v.page_game(cx).expect("page game");
        let rows = crate::model::page_rows(BigPictureView::game_state(&game, cx), 1, true);
        assert_eq!(rows[5], PageRow::LogErrors);
        assert_eq!(rows[6], PageRow::LogFilter);
        assert!(
            v.hints(cx)
                .iter()
                .any(|(inputs, _)| inputs.contains(&gamepad::GamepadInput::NextPage)),
            "a hint says the shoulders scroll the log"
        );
    });
    cx.dispatch_action(handle, gamepad::Down);
    cx.dispatch_action(handle, gamepad::Right);
    draw(cx);
    cx.read(|cx| assert_eq!(view.read(cx).log_lines(), Some(15)));

    // The filter field takes letters (M doesn't open the menu); Enter
    // returns to the page with the filter kept.
    cx.dispatch_action(handle, gamepad::Down);
    cx.dispatch_action(handle, gamepad::Confirm);
    cx.read(|cx| assert!(view.read(cx).log_filter.editing));
    cx.simulate_input(handle, "failure 14");
    draw(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_overlay_open());
        assert_eq!(v.log_lines(), Some(1));
    });
    cx.simulate_keystrokes(handle, "enter");
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.log_filter.editing);
        assert!(v.is_page_open());
    });
    // Errors off again: the text alone filters.
    cx.dispatch_action(handle, gamepad::Up);
    cx.dispatch_action(handle, gamepad::Left);
    draw(cx);
    cx.read(|cx| assert_eq!(view.read(cx).log_lines(), Some(1)));
}

#[gpui_kit::test]
fn a_click_opens_a_cover_and_buttons_act_on_their_own_game(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let click = |slot: CoverSlot, count: usize, cx: &mut TestAppContext| {
        view.update(cx, |v, cx| v.click_cover(slot, count, cx));
    };
    let page =
        |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).page.as_ref().map(|p| p.id.clone()));
    // Tests share a data folder, so another may have added a Favourites
    // shelf: find the covers on the Installed one.
    let shelf = |title: &str, cx: &mut TestAppContext| {
        cx.read(|cx| {
            let rows = &view.read(cx).rows;
            let row = rows
                .iter()
                .position(|r| r.kind == RowKind::Installed)
                .expect("Installed shelf");
            let col = rows[row]
                .games
                .iter()
                .position(|g| g.title == title)
                .expect("game on the shelf");
            CoverSlot::Shelf { row, col }
        })
    };
    let charlie = GameId::new(Runner::Legendary, "c");

    // One click focuses Charlie (the backdrop follows the focus) and opens
    // its page.
    let shelf_charlie = shelf("Charlie", cx);
    click(shelf_charlie, 1, cx);
    assert_eq!(focused_title(&view, cx), "Charlie");
    assert_eq!(page(cx), Some(charlie));
    cx.dispatch_action(handle, gamepad::Back);
    assert_eq!(page(cx), None);

    // The second click of a double click does nothing more.
    let shelf_alpha = shelf("Alpha", cx);
    click(shelf_alpha, 2, cx);
    assert_eq!(page(cx), None);
    click(shelf_alpha, 1, cx);
    assert_eq!(page(cx), Some(GameId::new(Runner::Gog, "a")));
    cx.dispatch_action(handle, gamepad::Back);

    // Library (Alpha, Bravo, Charlie; Alpha focused): after clicking Charlie
    // then Bravo, Bravo's button acts on Bravo, and a button drawn for a game
    // no longer in its place acts on nothing.
    cx.dispatch_action(handle, gamepad::NextPage);
    assert_eq!(focused_title(&view, cx), "Alpha");
    click(CoverSlot::Grid(2), 1, cx);
    assert_eq!(focused_title(&view, cx), "Charlie");
    cx.dispatch_action(handle, gamepad::Back);
    click(CoverSlot::Grid(1), 1, cx);
    assert_eq!(focused_title(&view, cx), "Bravo");
    cx.dispatch_action(handle, gamepad::Back);
    assert_eq!(page(cx), None);
    let bravo = GameId::new(Runner::Gog, "b");
    let target = view.update(cx, |v, cx| {
        v.select_cover_game(CoverSlot::Grid(1), &bravo, cx)
    });
    assert_eq!(target.map(|g| g.id()), Some(bravo.clone()));
    let alpha = GameId::new(Runner::Gog, "a");
    let stale = view.update(cx, |v, cx| {
        v.select_cover_game(CoverSlot::Grid(1), &alpha, cx)
    });
    assert!(stale.is_none());
    // The button also selects its cover, so Alt and the hints follow it.
    let target = view.update(cx, |v, cx| {
        v.select_cover_game(CoverSlot::Grid(0), &alpha, cx)
    });
    assert_eq!(target.map(|g| g.id()), Some(alpha));
    assert_eq!(focused_title(&view, cx), "Alpha");
}

#[gpui_kit::test]
fn every_layout_draws_and_moves_and_the_editor_changes_them_live(cx: &mut TestAppContext) {
    use crate::layout::HomeLayout;
    use crate::theme::store::{self, ThemeRef};

    let (handle, view) = open(cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    };

    // Every layout draws, and the arrows walk the shelf in each (Compact's
    // list runs down).
    for layout in HomeLayout::ALL {
        view.update(cx, |v, cx| {
            v.look.layout = layout;
            cx.notify();
        });
        draw(cx);
        assert_eq!(focused_title(&view, cx), "Alpha", "{layout:?}");
        if layout == HomeLayout::Compact {
            cx.dispatch_action(handle, gamepad::Down);
        } else {
            cx.dispatch_action(handle, gamepad::Right);
        }
        draw(cx);
        assert_eq!(focused_title(&view, cx), "Charlie", "{layout:?}");
        cx.simulate_keystrokes(
            handle,
            if layout == HomeLayout::Compact {
                "up"
            } else {
                "left"
            },
        );
    }
    view.update(cx, |v, _| v.look.layout = HomeLayout::Shelves);

    // The quick menu opens the editor (after Interface size and Layout).
    cx.simulate_keystrokes(handle, "m down down enter");
    cx.read(|cx| assert!(view.read(cx).look.editor.is_some()));
    draw(cx);

    // Layout changes live, without touching the setting yet.
    cx.simulate_keystrokes(handle, "down right");
    cx.read(|cx| {
        let look = &view.read(cx).look;
        assert_eq!(look.layout(), HomeLayout::Spotlight);
        assert_eq!(look.layout, HomeLayout::Shelves);
    });
    draw(cx);

    // The next gallery theme brings its own layout; a new accent changes it.
    cx.simulate_keystrokes(handle, "up right");
    cx.read(|cx| {
        let look = &view.read(cx).look;
        assert_eq!(look.theme().name, "Hearth");
        assert_eq!(look.layout(), HomeLayout::Spotlight);
    });
    cx.simulate_keystrokes(handle, "down down right");
    draw(cx);

    // Closing keeps the change as the user's own copy, and uses it.
    cx.simulate_keystrokes(handle, "escape");
    let copy = ThemeRef::User("hearth-mine".into());
    cx.read(|cx| {
        let look = &view.read(cx).look;
        assert!(look.editor.is_none());
        assert_eq!(look.active, copy);
        assert_eq!(look.theme.name, "Hearth (mine)");
    });
    wait_for(cx, |cx| {
        models(cx).settings.read(cx).app.ui.big_picture_theme == "user:hearth-mine"
    });
    let file = store::themes_dir().join("hearth-mine.irori-theme.json");
    wait_for(cx, |_| file.is_file());
    draw(cx);

    // Deleting it asks first, then goes back to Irori's own theme.
    cx.simulate_keystrokes(handle, "m down down enter");
    for _ in 0..30 {
        cx.simulate_keystrokes(handle, "down");
    }
    cx.simulate_keystrokes(handle, "enter");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).overlay,
            Some(Overlay::DeleteTheme {
                id: "hearth-mine".into(),
                choice: 0
            })
        );
    });
    draw(cx);
    cx.simulate_keystrokes(handle, "right enter");
    wait_for(cx, |_| !file.exists());
    cx.read(|cx| {
        let look = &view.read(cx).look;
        assert!(look.user.is_empty());
        assert_eq!(look.active.setting(), "");
    });
    cx.simulate_keystrokes(handle, "escape");
    cx.read(|cx| assert!(view.read(cx).look.editor.is_none()));
}

/// Stands in for the settings feature's view: one control to reach.
struct FakeSettings {
    control: gpui_kit::FocusHandle,
}

impl gpui_kit::Render for FakeSettings {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{InteractiveElement as _, ParentElement as _};
        gpui_kit::div().child(
            gpui_kit::div()
                .id("fake-control")
                .track_focus(&self.control)
                .child("Wine version"),
        )
    }
}

/// The settings the hook made last, and how they close.
struct OpenedSettings(irori_ui::hooks::CloseFn, Entity<FakeSettings>);

impl gpui_kit::Global for OpenedSettings {}

#[gpui_kit::test]
fn game_settings_open_inside_big_picture(cx: &mut TestAppContext) {
    use irori_ui::hooks::Hooks;
    use irori_ui::{Route, nav};

    let (handle, view) = open(cx);
    cx.update(|cx| {
        irori_ui::hooks::install(
            Hooks {
                game_settings: Some(Arc::new(|_, close, _, cx| {
                    let fake = cx.new(|cx| FakeSettings {
                        control: cx.focus_handle().tab_stop(true),
                    });
                    cx.set_global(OpenedSettings(close, fake.clone()));
                    fake.into()
                })),
                ..Default::default()
            },
            cx,
        )
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    };
    let route =
        |cx: &mut TestAppContext| cx.read(|cx| nav::navigator(cx).read(cx).current().clone());
    let page_ix = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).page.as_ref().unwrap().ix);
    let before = route(cx);

    // Charlie's page: Play, Play with logs, Favourite, Settings, Show log.
    let charlie = GameId::new(Runner::Legendary, "c");
    view.update(cx, |v, cx| v.open_page(charlie.clone(), cx));
    for _ in 0..3 {
        cx.dispatch_action(handle, gamepad::Down);
    }
    cx.dispatch_action(handle, gamepad::Confirm);
    draw(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(v.is_settings_open(), "the settings open in Big Picture");
        assert!(v.is_page_open());
    });
    assert_eq!(route(cx), before, "Big Picture stays");
    assert!(!matches!(route(cx), Route::GameSettings(..)));

    // A controller's directions move into the settings, not along the page.
    let route_input = |input: gamepad::GamepadInput, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            gamepad::route_in_window(input, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
    };
    route_input(gamepad::GamepadInput::Down, cx);
    let fake = cx.read(|cx| cx.global::<OpenedSettings>().1.clone());
    let in_settings = cx
        .update_window(handle, |_, window, cx| {
            fake.read(cx).control.is_focused(window)
        })
        .unwrap();
    assert!(in_settings, "Down reaches the settings' first control");
    assert_eq!(page_ix(cx), 3, "the page behind keeps its row");

    // Back returns to the page, which has the keys again.
    route_input(gamepad::GamepadInput::Back, cx);
    draw(cx);
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_settings_open());
        assert!(v.is_page_open(), "Back closes only the settings");
    });
    cx.dispatch_action(handle, gamepad::Down);
    assert_eq!(page_ix(cx), 4);

    // The settings' own Back button closes them the same way.
    cx.dispatch_action(handle, gamepad::Up);
    cx.dispatch_action(handle, gamepad::Confirm);
    cx.read(|cx| assert!(view.read(cx).is_settings_open()));
    let close = cx.read(|cx| cx.global::<OpenedSettings>().0.clone());
    cx.update_window(handle, |_, window, cx| close(window, cx))
        .unwrap();
    cx.read(|cx| {
        let v = view.read(cx);
        assert!(!v.is_settings_open());
        assert!(v.is_page_open());
    });
    assert_eq!(route(cx), before);
}
