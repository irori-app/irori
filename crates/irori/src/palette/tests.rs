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
        &[],
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
        &[],
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
            &[],
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
