use super::*;

fn targets(install: Option<InstallFilter>, favourites: Option<bool>) -> Vec<String> {
    let mut targets = Vec::new();
    let library = LibraryCommands {
        grouped: install.map(|_| false),
        grouped_by_mode: install.map(|_| true),
        grouped_by_genre: None,
        grouped_by_category: None,
        grouping: None,
        categories: &[],
        views: &[],
        layout: LibraryLayout::Grid,
        install,
        favourites,
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
        icon: None,
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
        grouping: None,
        categories: &categories,
        views: &[],
        layout: LibraryLayout::Grid,
        install: None,
        favourites: None,
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
            grouping: None,
            categories: &[],
            views: &[],
            layout: LibraryLayout::Grid,
            install: None,
            favourites: None,
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
fn library_filters_are_offered_as_toggles() {
    assert_eq!(
        targets(Some(InstallFilter::Any), Some(false)),
        ["install Installed", "favourites true", "modes false"]
    );
    assert_eq!(
        targets(Some(InstallFilter::Installed), Some(true)),
        ["install Any", "favourites false", "modes false"]
    );
    assert_eq!(
        targets(Some(InstallFilter::NotInstalled), Some(false)),
        [
            "install Installed",
            "install Any",
            "favourites true",
            "modes false"
        ]
    );
    assert!(targets(None, None).is_empty());
}

#[test]
fn shell_actions_are_offered_in_palette_items() {
    let mut targets = Vec::new();
    let library = LibraryCommands {
        grouped: None,
        grouped_by_mode: None,
        grouped_by_genre: None,
        grouped_by_category: None,
        grouping: None,
        categories: &[],
        views: &[],
        layout: LibraryLayout::Grid,
        install: None,
        favourites: None,
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
    assert!(
        targets
            .iter()
            .any(|t| matches!(t, PaletteTarget::ToggleSidebar))
    );
    assert!(
        targets
            .iter()
            .any(|t| matches!(t, PaletteTarget::ToggleFullscreen))
    );
    assert!(targets.iter().any(|t| matches!(t, PaletteTarget::AddGame)));
    assert!(
        targets
            .iter()
            .any(|t| matches!(t, PaletteTarget::ManageCategories))
    );
    assert!(
        targets
            .iter()
            .any(|t| matches!(t, PaletteTarget::ManageViews))
    );
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
    let offers = |g: &GameInfo| {
        let rows = game::rows(g, None, &[]);
        (
            rows.iter()
                .any(|(_, target)| matches!(target, PaletteTarget::GraphicsMods(_))),
            rows.iter()
                .any(|(_, target)| matches!(target, PaletteTarget::Optiscaler(_))),
        )
    };
    assert_eq!(
        offers(&game(true, irori_core::InstallPlatform::Windows)),
        (true, true)
    );
    assert_eq!(
        offers(&game(true, irori_core::InstallPlatform::Linux)),
        (false, false)
    );
    assert_eq!(
        offers(&game(false, irori_core::InstallPlatform::Windows)),
        (false, false)
    );
}

/// What `level_rows` offers, as short names in order.
fn level(game: &GameInfo, facts: &game::GameFacts) -> Vec<&'static str> {
    game::level_rows(game, facts)
        .into_iter()
        .filter_map(|(_, target)| {
            Some(match target {
                PaletteTarget::Play(_) => "play",
                PaletteTarget::InstallCopy(_) => "install copy",
                PaletteTarget::BackToGame(_) => "back to game",
                PaletteTarget::CancelLaunch(_) => "cancel",
                PaletteTarget::Open(_) => "show",
                PaletteTarget::OpenGameSettings(_) => "settings",
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
            "settings",
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
            "settings",
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
        ..installed.clone()
    };
    assert_eq!(
        level(&owned, &idle),
        ["show", "favourite", "settings", "hide", "back"]
    );
    // Its other copies come right after its own Play.
    let copy = |app: &str, action| game::CopyRow {
        id: irori_core::GameId::new(Runner::Steam, app),
        label: "Play with Steam".into(),
        action,
    };
    let with_copies = game::GameFacts {
        copies: vec![
            copy("2", irori_ui::copies::CopyAction::Play),
            copy("3", irori_ui::copies::CopyAction::Install),
        ],
        ..Default::default()
    };
    assert_eq!(
        level(&installed, &with_copies)[..4],
        ["play", "play", "install copy", "show"]
    );
}

/// The palette drawn over a real backend, typed into as a user would.
mod ui {
    use super::super::score::GroupKind;
    use super::super::view::Level;
    use super::*;
    use crate::shell::title_bar_tests::{add_game, add_titled_game, start, wait_for_status};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use irori_core::StatusKind;
    use irori_settings::SettingsSection;

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

    fn group_kinds(view: &Entity<PaletteView>, cx: &mut VisualTestContext) -> Vec<GroupKind> {
        cx.read(|cx| {
            view.read(cx)
                .shown_groups()
                .iter()
                .map(|g| g.kind)
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

        // A typo still finds the game without duplicating a separate Play row.
        cx.simulate_input("deliberatly");
        let key = id.key();
        assert_eq!(
            shown(&view, cx),
            [format!("game {key}")],
            "Dialog shows one game row (no duplicate Play row)"
        );

        // Explicit `play <game>` and `jogar <game>` put Play first.
        view.update(cx, |v, cx| v.set_query("play deliberatly", cx));
        assert_eq!(
            shown(&view, cx),
            [format!("play {key}"), format!("game {key}")]
        );
        view.update(cx, |v, cx| v.set_query("jogar deliberatly", cx));
        assert_eq!(
            shown(&view, cx),
            [format!("play {key}"), format!("game {key}")]
        );
        view.update(cx, |v, cx| v.set_query("deliberatly", cx));

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

        // → on the highlighted game lists its actions (including game settings); Escape comes back.
        cx.simulate_keystrokes("right");
        assert_eq!(
            cx.read(|cx| view.read(cx).level().clone()),
            Level::Game(id.clone())
        );
        let level_targets: Vec<PaletteTarget> = cx.read(|cx| {
            view.read(cx)
                .shown_rows()
                .iter()
                .map(|r| r.target.clone())
                .collect()
        });
        assert!(
            matches!(level_targets.first(), Some(PaletteTarget::Play(_))),
            "Play leads an idle game's actions"
        );
        assert!(
            level_targets
                .iter()
                .any(|t| matches!(t, PaletteTarget::OpenGameSettings(g) if *g == id)),
            "game actions include opening that game's settings"
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

        // The quick launcher plays from the game row itself, and goes back
        // to the game while it runs.
        let launcher =
            cx.update(|window, cx| cx.new(|cx| PaletteView::new(Host::Window, window, cx)));
        launcher.update(cx, |v, cx| v.set_query("deliberatly", cx));
        let game_target = |cx: &mut VisualTestContext| {
            cx.read(|cx| {
                launcher
                    .read(cx)
                    .shown_rows()
                    .iter()
                    .filter(|r| r.game.is_some())
                    .map(|r| match &r.target {
                        PaletteTarget::Play(_) => "play",
                        PaletteTarget::BackToGame(_) => "back",
                        PaletteTarget::Open(_) => "open",
                        _ => "other",
                    })
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(game_target(cx), ["play"], "and no separate Play rows");
        backend.statuses.set(&id, StatusKind::Playing);
        wait_for_status(cx, &id, true);
        assert_eq!(game_target(cx), ["back"]);
        backend.statuses.clear(&id);
        wait_for_status(cx, &id, false);
    }

    #[gpui_kit::test]
    fn curated_empty_state_and_quick_launcher_hide_window_only_commands(cx: &mut TestAppContext) {
        let _ = start(cx);
        let (dialog, cx) =
            cx.add_window_view(|window, cx| PaletteView::new(Host::Dialog, window, cx));
        cx.run_until_parked();

        // Empty Dialog shows curated Pages + QuickActions, not ~100 commands/settings.
        let kinds = group_kinds(&dialog, cx);
        assert_eq!(kinds, [GroupKind::Pages, GroupKind::QuickActions]);
        let count = cx.read(|cx| dialog.read(cx).shown_rows().len());
        assert!(
            (8..=20).contains(&count),
            "expected curated empty list, got {count} rows"
        );
        assert!(cx.read(|cx| {
            dialog
                .read(cx)
                .shown_rows()
                .iter()
                .any(|r| matches!(r.target, PaletteTarget::AddGame))
        }));

        // Quick Launcher hides window-only commands even when explicitly searched.
        let launcher =
            cx.update(|window, cx| cx.new(|cx| PaletteView::new(Host::Window, window, cx)));
        launcher.update(cx, |v, cx| v.set_query(">sidebar", cx));
        assert!(
            cx.read(|cx| {
                !launcher
                    .read(cx)
                    .shown_rows()
                    .iter()
                    .any(|r| matches!(r.target, PaletteTarget::ToggleSidebar))
            }),
            "ToggleSidebar is window-only and must not show in Quick Launcher"
        );
        launcher.update(cx, |v, cx| v.set_query(">fullscreen", cx));
        assert!(cx.read(|cx| {
            !launcher
                .read(cx)
                .shown_rows()
                .iter()
                .any(|r| matches!(r.target, PaletteTarget::ToggleFullscreen))
        }));
    }

    #[gpui_kit::test]
    fn indexes_every_supported_settings_section_individual_settings_and_accents(
        cx: &mut TestAppContext,
    ) {
        let _ = start(cx);
        let (view, cx) =
            cx.add_window_view(|window, cx| PaletteView::new(Host::Dialog, window, cx));
        cx.run_until_parked();

        // Every supported SettingsSection in SettingsSection::ALL is indexed;
        // unsupported ones (e.g. PerformanceHud on unsupported hosts) are absent.
        let all_settings_rows: Vec<PaletteTarget> =
            cx.read(|cx| settings::rows(cx).into_iter().map(|r| r.target).collect());
        for section in SettingsSection::ALL {
            let expected = section.route();
            let found = all_settings_rows
                .iter()
                .any(|t| matches!(t, PaletteTarget::Route(r) if *r == expected));
            assert_eq!(
                found,
                section.supported_on_host(),
                "section {section:?} host support mismatch"
            );
        }

        // Accent-insensitive search finds sections, sub-tabs, and individual settings.
        view.update(cx, |v, cx| v.set_query("appêarance", cx));
        assert!(cx.read(|cx| {
            view.read(cx).shown_rows().iter().any(|r| {
                matches!(
                    &r.target,
                    PaletteTarget::Route(Route::Settings(s)) if s.as_ref() == "appearance"
                )
            })
        }));

        view.update(cx, |v, cx| v.set_query("báckup export", cx));
        assert!(cx.read(|cx| {
            view.read(cx).shown_rows().iter().any(|r| {
                matches!(
                    &r.target,
                    PaletteTarget::Route(Route::Settings(s)) if s.as_ref() == "backup/export"
                )
            })
        }));

        // Individual editable setting from irori_settings::registry is reachable.
        view.update(cx, |v, cx| v.set_query("checkUpdatesInterval", cx));
        assert!(cx.read(|cx| {
            view.read(cx)
                .shown_rows()
                .iter()
                .any(|r| matches!(r.target, PaletteTarget::OpenSetting("checkUpdatesInterval")))
        }));
    }

    #[gpui_kit::test]
    fn dynamic_group_priority_and_scope_prefixes(cx: &mut TestAppContext) {
        let (backend, store) = start(cx);
        let _control = add_titled_game(cx, &backend, &store, "Control");
        let _portal = add_titled_game(cx, &backend, &store, "Portal");
        let _puzzle = add_titled_game(cx, &backend, &store, "Puzzle Quest");

        let (dialog, cx) =
            cx.add_window_view(|window, cx| PaletteView::new(Host::Dialog, window, cx));
        let launcher =
            cx.update(|window, cx| cx.new(|cx| PaletteView::new(Host::Window, window, cx)));
        cx.run_until_parked();

        // Prefix tie ("contro"): Dialog favors Pages (Controller) over Games (Control),
        // while Quick Launcher favors Games (Control) over Pages (Controller).
        dialog.update(cx, |v, cx| v.set_query("contro", cx));
        let dialog_groups = group_kinds(&dialog, cx);
        let page_pos = dialog_groups.iter().position(|k| *k == GroupKind::Pages);
        let game_pos = dialog_groups.iter().position(|k| *k == GroupKind::Games);
        assert!(
            page_pos.is_some() && game_pos.is_some() && page_pos < game_pos,
            "Dialog should put Pages above Games on prefix tie: {dialog_groups:?}"
        );

        launcher.update(cx, |v, cx| v.set_query("contro", cx));
        let launcher_groups = group_kinds(&launcher, cx);
        let l_page_pos = launcher_groups.iter().position(|k| *k == GroupKind::Pages);
        let l_game_pos = launcher_groups.iter().position(|k| *k == GroupKind::Games);
        assert!(
            l_game_pos.is_some() && l_page_pos.is_some() && l_game_pos < l_page_pos,
            "Quick Launcher should put Games above Pages on prefix tie: {launcher_groups:?}"
        );

        // Exact game match ("portal") beats settings mentioning "portal" only in help text.
        dialog.update(cx, |v, cx| v.set_query("portal", cx));
        assert_eq!(
            group_kinds(&dialog, cx).first(),
            Some(&GroupKind::Games),
            "Exact game title Portal must beat setting help matches"
        );

        // Strong game prefix ("puzzle") beats weaker matches.
        dialog.update(cx, |v, cx| v.set_query("puzzle", cx));
        assert_eq!(group_kinds(&dialog, cx).first(), Some(&GroupKind::Games));

        // Scope prefixes: `,` -> only Pages/Settings; `>` -> only Commands; `#` -> only categories/views.
        dialog.update(cx, |v, cx| v.set_query(",contro", cx));
        let comma_groups = group_kinds(&dialog, cx);
        assert!(!comma_groups.is_empty());
        assert!(
            comma_groups
                .iter()
                .all(|k| matches!(k, GroupKind::Pages | GroupKind::Settings)),
            "`,` scope must exclude games and commands: {comma_groups:?}"
        );

        dialog.update(cx, |v, cx| v.set_query(">sidebar", cx));
        let cmd_groups = group_kinds(&dialog, cx);
        assert_eq!(cmd_groups, [GroupKind::Commands]);
        assert!(cx.read(|cx| {
            dialog
                .read(cx)
                .shown_rows()
                .iter()
                .any(|r| matches!(r.target, PaletteTarget::ToggleSidebar))
        }));

        dialog.update(cx, |v, cx| v.set_query("#portal", cx));
        assert!(
            cx.read(|cx| dialog
                .read(cx)
                .shown_rows()
                .iter()
                .all(|r| r.game.is_none())),
            "`#` scope must never return game rows"
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

#[test]
fn games_of_other_stores_offer_a_steam_link_in_their_level() {
    let offers = |runner: Runner| {
        let g = GameInfo {
            runner: Some(runner),
            app_name: "1".into(),
            title: "Hades".into(),
            ..Default::default()
        };
        game::level_rows(&g, &game::GameFacts::default())
            .into_iter()
            .any(|(_, target)| matches!(target, PaletteTarget::SteamLink(_)))
    };
    assert!(offers(Runner::Gog));
    assert!(offers(Runner::Sideload));
    assert!(!offers(Runner::Steam), "a Steam game is its own app");
    assert!(!offers(Runner::Emulation));
}

#[test]
fn every_game_offers_its_links_in_its_level() {
    for runner in [Runner::Gog, Runner::Steam, Runner::Emulation] {
        let g = GameInfo {
            runner: Some(runner),
            app_name: "1".into(),
            title: "Hades".into(),
            ..Default::default()
        };
        assert!(
            game::level_rows(&g, &game::GameFacts::default())
                .into_iter()
                .any(|(_, target)| matches!(target, PaletteTarget::GameLinks(_))),
            "{runner:?}"
        );
    }
}

#[test]
fn groupings_by_a_games_facts_are_offered_and_undone() {
    let offered = |grouping: Option<Grouping>| {
        let mut targets = Vec::new();
        let library = LibraryCommands {
            grouped: None,
            grouped_by_mode: None,
            grouped_by_genre: None,
            grouped_by_category: None,
            grouping,
            categories: &[],
            views: &[],
            layout: LibraryLayout::Grid,
            install: None,
            favourites: None,
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
                PaletteTarget::GroupBy(g) => Some(g),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Without a library view, nothing to group.
    assert!(offered(None).is_empty());
    assert_eq!(offered(Some(Grouping::None)), Grouping::BY_FACT);
    // The grouping in use is offered as its undoing.
    let grouped = offered(Some(Grouping::Year));
    assert_eq!(grouped.len(), Grouping::BY_FACT.len());
    assert!(grouped.contains(&Grouping::None) && !grouped.contains(&Grouping::Year));
}
