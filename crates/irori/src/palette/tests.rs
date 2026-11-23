use super::*;

fn targets(install: Option<InstallFilter>, favourites: Option<bool>, quick: bool) -> Vec<String> {
    let mut targets = Vec::new();
    let library = LibraryCommands {
        grouped: install.map(|_| false),
        grouped_by_mode: install.map(|_| true),
        grouped_by_genre: None,
        grouped_by_category: None,
        categories: &[],
        views: &[],
        layout: LibraryLayout::Grid,
        install,
        favourites,
        quick_controls: quick,
        hidden_pin: None,
    };
    Palette::items(
        None,
        None,
        library,
        false,
        &[],
        (false, false),
        &mut targets,
    );
    targets
        .into_iter()
        .filter_map(|t| match t {
            PaletteTarget::Install(f) => Some(format!("install {f:?}")),
            PaletteTarget::FavouritesOnly(on) => Some(format!("favourites {on}")),
            PaletteTarget::QuickControls(on) => Some(format!("quick {on}")),
            PaletteTarget::GroupByMode(on) => Some(format!("modes {on}")),
            _ => None,
        })
        .collect()
}

#[test]
fn every_category_of_both_kinds_is_offered() {
    use irori_services::CategoryRef;
    let entry = |category: CategoryRef, name: &str| irori_ui::categories::CategoryEntry {
        category,
        name: name.into(),
        count: 1,
        pinned: false,
        colour: None,
        readable: true,
    };
    let categories = [
        entry(CategoryRef::Smart("s-1".into()), "Short games"),
        entry(CategoryRef::Manual("RPG".into()), "RPG"),
    ];
    let mut targets = Vec::new();
    let library = LibraryCommands {
        grouped: Some(false),
        grouped_by_mode: Some(false),
        grouped_by_genre: Some(false),
        grouped_by_category: Some(false),
        categories: &categories,
        views: &[],
        layout: LibraryLayout::Grid,
        install: None,
        favourites: None,
        quick_controls: false,
        hidden_pin: None,
    };
    Palette::items(
        None,
        None,
        library,
        false,
        &[],
        (false, false),
        &mut targets,
    );
    let shown: Vec<String> = targets
        .into_iter()
        .filter_map(|t| match t {
            PaletteTarget::Category(c) => Some(format!("{c:?}")),
            PaletteTarget::GroupByCategory(on) => Some(format!("group {on}")),
            PaletteTarget::NewSmartCategory => Some("new".into()),
            _ => None,
        })
        .collect();
    assert_eq!(
        shown,
        ["group true", "new", "Smart(\"s-1\")", "Manual(\"RPG\")"]
    );
}

#[test]
fn the_hidden_games_pin_is_offered_only_while_unlocked() {
    let offered = |hidden_pin: Option<bool>| {
        let mut targets = Vec::new();
        let library = LibraryCommands {
            grouped: None,
            grouped_by_mode: None,
            grouped_by_genre: None,
            grouped_by_category: None,
            categories: &[],
            views: &[],
            layout: LibraryLayout::Grid,
            install: None,
            favourites: None,
            quick_controls: false,
            hidden_pin,
        };
        Palette::items(
            None,
            None,
            library,
            false,
            &[],
            (false, false),
            &mut targets,
        );
        targets
            .into_iter()
            .filter_map(|t| match t {
                PaletteTarget::LockHidden => Some("lock"),
                PaletteTarget::HiddenPin => Some("pin"),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(
        offered(None).is_empty(),
        "locked: nothing about hidden games"
    );
    assert_eq!(offered(Some(false)), ["pin"]);
    assert_eq!(offered(Some(true)), ["lock", "pin"]);
}

#[test]
fn library_filters_and_quick_controls_are_offered_as_toggles() {
    assert_eq!(
        targets(Some(InstallFilter::Any), Some(false), true),
        [
            "install Installed",
            "favourites true",
            "quick false",
            "modes false"
        ]
    );
    assert_eq!(
        targets(Some(InstallFilter::Installed), Some(true), false),
        [
            "install Any",
            "favourites false",
            "quick true",
            "modes false"
        ]
    );
    assert_eq!(
        targets(Some(InstallFilter::NotInstalled), Some(false), true),
        [
            "install Installed",
            "install Any",
            "favourites true",
            "quick false",
            "modes false"
        ]
    );
    // Without a library view only the setting can change.
    assert_eq!(targets(None, None, true), ["quick false"]);
}

#[test]
fn installed_windows_games_offer_reshade_and_renodx() {
    let game = |installed: bool, platform: irori_core::InstallPlatform| GameInfo {
        runner: Some(Runner::Gog),
        app_name: "1".into(),
        title: "Hades".into(),
        is_installed: installed,
        install: installed.then(|| irori_core::InstalledInfo {
            install_path: "/games/hades".into(),
            platform,
            ..Default::default()
        }),
        ..Default::default()
    };
    let offers = |g: GameInfo| {
        game::rows(&g, None, &[])
            .into_iter()
            .any(|(_, target)| matches!(target, PaletteTarget::GraphicsMods(_)))
    };
    assert!(offers(game(true, irori_core::InstallPlatform::Windows)));
    assert!(!offers(game(true, irori_core::InstallPlatform::Linux)));
    assert!(!offers(game(false, irori_core::InstallPlatform::Windows)));
}

/// What `level_rows` offers, as short names in order.
fn level(game: &GameInfo, facts: &game::GameFacts) -> Vec<&'static str> {
    game::level_rows(game, facts)
        .into_iter()
        .filter_map(|(_, target)| {
            Some(match target {
                PaletteTarget::Play(_) => "play",
                PaletteTarget::BackToGame(_) => "back to game",
                PaletteTarget::CancelLaunch(_) => "cancel",
                PaletteTarget::Open(_) => "show",
                PaletteTarget::StorePage(..) => "store",
                PaletteTarget::Favourite(_, on) => {
                    if on {
                        "favourite"
                    } else {
                        "unfavourite"
                    }
                }
                PaletteTarget::OpenFolder(_) => "folder",
                PaletteTarget::Hide(_, on) => {
                    if on {
                        "hide"
                    } else {
                        "unhide"
                    }
                }
                PaletteTarget::Stop(_) => "stop",
                PaletteTarget::Back => "back",
                _ => return None,
            })
        })
        .collect()
}

#[test]
fn a_games_actions_follow_what_it_is_doing() {
    use irori_core::StatusKind;
    let installed = GameInfo {
        runner: Some(Runner::Gog),
        app_name: "1".into(),
        title: "Hades".into(),
        is_installed: true,
        store_url: Some("https://www.gog.com/game/hades".into()),
        install: Some(irori_core::InstalledInfo {
            install_path: "/games/hades".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let idle = game::GameFacts::default();
    assert_eq!(
        level(&installed, &idle),
        [
            "play",
            "show",
            "store",
            "favourite",
            "folder",
            "hide",
            "back"
        ]
    );
    // Running: back to it first, Stop low, never Play.
    let playing = game::GameFacts {
        kind: Some(StatusKind::Playing),
        favourite: true,
        hidden: true,
        ..Default::default()
    };
    assert_eq!(
        level(&installed, &playing),
        [
            "back to game",
            "show",
            "store",
            "unfavourite",
            "folder",
            "unhide",
            "stop",
            "back"
        ]
    );
    let launching = game::GameFacts {
        kind: Some(StatusKind::Launching),
        ..Default::default()
    };
    assert_eq!(level(&installed, &launching)[0], "cancel");
    // Not installed and without a store page: nothing to play or open.
    let owned = GameInfo {
        is_installed: false,
        install: None,
        store_url: None,
        ..installed
    };
    assert_eq!(level(&owned, &idle), ["show", "favourite", "hide", "back"]);
}

/// The palette drawn over a real backend, typed into as a user would.
mod ui {
    use super::super::view::Level;
    use super::*;
    use crate::shell::title_bar_tests::{add_game, start, wait_for_status};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use irori_core::StatusKind;

    /// What the game rows the palette lists now do, in order.
    fn shown(view: &Entity<PaletteView>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.read(|cx| {
            view.read(cx)
                .shown_rows()
                .iter()
                .filter_map(|row| match (&row.target, &row.game) {
                    (PaletteTarget::Open(id), Some(_)) => Some(format!("game {}", id.key())),
                    (PaletteTarget::Play(id), _) => Some(format!("play {}", id.key())),
                    (PaletteTarget::BackToGame(id), _) => Some(format!("back {}", id.key())),
                    _ => None,
                })
                .collect()
        })
    }

    #[gpui_kit::test]
    fn finds_games_by_search_and_follows_the_running_ones(cx: &mut TestAppContext) {
        let (backend, store) = start(cx);
        let id = add_game(cx, &backend, &store);
        let (view, cx) =
            cx.add_window_view(|window, cx| PaletteView::new(Host::Dialog, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| PaletteView::focus(&view, window, cx));
        // A typo still finds it, as the library's search does.
        cx.simulate_input("deliberatly");
        let key = id.key();
        assert_eq!(
            shown(&view, cx),
            [format!("play {key}"), format!("game {key}")],
            "an installed game is offered to play, then as a game row"
        );

        backend.statuses.set(&id, StatusKind::Playing);
        wait_for_status(cx, &id, true);
        assert_eq!(
            shown(&view, cx).first(),
            Some(&format!("back {key}")),
            "a game that starts while the palette is open comes first"
        );
        backend.statuses.clear(&id);
        wait_for_status(cx, &id, false);
        assert!(!shown(&view, cx).iter().any(|s| s.starts_with("back")));

        // → on the highlighted game lists its actions; Escape comes back.
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("right");
        assert_eq!(
            cx.read(|cx| view.read(cx).level().clone()),
            Level::Game(id.clone())
        );
        let first = cx.read(|cx| view.read(cx).shown_rows()[0].target.clone());
        assert!(
            matches!(first, PaletteTarget::Play(_)),
            "Play leads an idle game's actions"
        );
        cx.simulate_keystrokes("escape");
        assert_eq!(cx.read(|cx| view.read(cx).level().clone()), Level::Root);

        // A random pick from the installed games stays on the pick, with
        // "Pick another".
        let confirm = |target: PaletteTarget, cx: &mut VisualTestContext| {
            let row = cx
                .read(|cx| {
                    view.read(cx)
                        .shown_rows()
                        .iter()
                        .find(|r| same(&r.target, &target))
                        .cloned()
                })
                .expect("the row is listed");
            cx.update(|window, cx| PaletteView::confirm(&view, Host::Dialog, row, window, cx));
            cx.run_until_parked();
        };
        let random = cx.read(|cx| {
            view.read(cx)
                .shown_rows()
                .iter()
                .any(|r| matches!(r.target, PaletteTarget::Random))
        });
        assert!(random, "the first level offers a random pick");
        confirm(PaletteTarget::Random, cx);
        assert_eq!(cx.read(|cx| view.read(cx).level().clone()), Level::Random);
        let pool = irori_library::RandomPool::Installed;
        confirm(PaletteTarget::RandomFrom(pool.clone()), cx);
        assert_eq!(
            cx.read(|cx| view.read(cx).level().clone()),
            Level::Picked(pool.clone(), id.clone()),
            "the only installed game is picked"
        );
        // With one game, "Pick another" picks it again rather than nothing.
        confirm(PaletteTarget::RandomFrom(pool.clone()), cx);
        assert_eq!(
            cx.read(|cx| view.read(cx).level().clone()),
            Level::Picked(pool, id.clone())
        );
    }

    /// Whether two rows do the same thing, for the targets this test uses.
    fn same(a: &PaletteTarget, b: &PaletteTarget) -> bool {
        match (a, b) {
            (PaletteTarget::Random, PaletteTarget::Random) => true,
            (PaletteTarget::RandomFrom(a), PaletteTarget::RandomFrom(b)) => a == b,
            _ => false,
        }
    }
}
