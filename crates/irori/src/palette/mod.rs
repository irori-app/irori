//! Command palette (Ctrl+K): find any game (with its cover, and a preview of
//! the highlighted row beside the list), play installed games, go back to a
//! running one, cancel a launch being prepared, pick a random one, set the
//! play status of the game whose page is open, group the library by play
//! status, show only installed games or favourites, turn the library
//! header's quick controls on or off, show or save a library view, switch
//! its view (grid, list, Focus), back up, sync, save a clip while the replay
//! buffer records, and run window commands, all from the keyboard.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    App, AppContext as _, ParentElement as _, SharedString, Styled as _, WeakEntity, Window,
};
use irori_core::settings::LibraryLayout;
use irori_core::{GameId, GameInfo, Runner};
use irori_library::{Grouping, InstallFilter};
use irori_services::PlayStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::theme::SizeStep;
use irori_ui::{Route, models};

use entry::Entry;
use view::Row;
pub(crate) use view::{Host, PaletteView};

/// The game whose page is open: the game, its play status and its own
/// actions with their place.
type OpenGame = (
    Arc<GameInfo>,
    Option<PlayStatus>,
    Vec<(usize, irori_core::UserAction)>,
);

/// What confirming a palette row does.
#[derive(Clone)]
pub(crate) enum PaletteTarget {
    Open(GameId),
    Play(GameId),
    /// Cancel the launch being prepared.
    CancelLaunch(GameId),
    /// Bring the running game's window to the front.
    BackToGame(GameId),
    /// Stop a running game.
    Stop(GameId),
    /// List a game's actions (the palette's second level).
    GameActions(GameId),
    /// Leave a game's actions for the palette's first level.
    Back,
    /// Show a game's page on its store's website.
    StorePage(Runner, String),
    /// Add a game to the favourites (`true`) or take it out.
    Favourite(GameId, bool),
    /// Hide a game (`true`) or show it again.
    Hide(GameId, bool),
    /// Open a game's install folder in the file manager.
    OpenFolder(GameId),
    /// Open the folder of a game's screenshots and clips.
    CapturesFolder(GameId),
    Route(Route),
    Refresh,
    /// Choose which games to pick a random one from (a palette level).
    Random,
    /// Pick a random game from these games.
    RandomFrom(irori_library::RandomPool),
    SetUpEmulation,
    /// Add an emulator of the user's own.
    AddEmulator,
    /// Start an emulator without a game (its id and name).
    OpenEngine(String, SharedString),
    /// Start a third-party launcher.
    OpenLauncher(irori_launchers::LauncherKind),
    /// Add desktop and menu shortcuts that open a third-party launcher.
    LauncherShortcut(irori_launchers::LauncherKind),
    /// A set-up launcher's Wine build and variables.
    LauncherWine(irori_launchers::LauncherKind),
    /// Review added games that are probably one game twice.
    Duplicates,
    /// The Getting started checklist.
    GettingStarted,
    /// Show (`true`) or hide the Stores page's deals.
    Deals(bool),
    /// The guided tour of the main window.
    Tour,
    /// Save the categories of both kinds to a file.
    ExportCategories,
    /// Merge the categories of a file.
    ImportCategories,
    /// The usual ROM folders found on this computer, to add.
    FindRoms,
    /// The Steam friends window.
    SteamFriends,
    XboxOfflinePlay,
    XboxFriends,
    /// Work offline (`true`) or follow the connection again.
    WorkOffline(bool),
    /// Show a store's website in the store window.
    OpenStore(Runner),
    LutrisScript,
    /// Read the Steam library from the Steam client.
    SteamClientLibrary,
    /// Download LaunchBox's game list for the ROM library's box art.
    LaunchBox,
    /// Set (or with `None` clear) a game's play status.
    PlayStatus(GameId, Option<PlayStatus>),
    /// Group the library by play status (`true`) or stop, and show it.
    GroupByStatus(bool),
    /// Show the library's games by install state, and show it.
    Install(InstallFilter),
    /// Show only favourites (`true`) or every game, and show the library.
    FavouritesOnly(bool),
    /// Show the library header's quick controls (`true`) or stop.
    QuickControls(bool),
    /// Lock hidden games behind their PIN again.
    LockHidden,
    /// Set, change or remove the hidden games' PIN.
    HiddenPin,
    /// Group the library by game mode (`true`) or stop, and show it.
    GroupByMode(bool),
    /// Group the library by genre (`true`) or stop, and show it.
    GroupByGenre(bool),
    /// Group the library by category (`true`) or stop, and show it.
    GroupByCategory(bool),
    /// Group the library by one of a game's facts (store, year…), or stop
    /// with `Grouping::None`, and show it.
    GroupBy(Grouping),
    /// Show the library's games in one category of either kind.
    Category(irori_services::CategoryRef),
    /// Make a smart category.
    NewSmartCategory,
    /// Show a saved library view.
    View(u64),
    /// Save the library's current criteria as a view.
    SaveView,
    /// Pick a folder and add the games in it.
    AddFolder,
    /// Rename, merge or remove genres and companies across the library.
    ManageFields,
    /// Show the library as a grid, a list or in Focus view, and show it.
    LibraryView(LibraryLayout),
    /// Sync a game's cloud saves now.
    SyncSaves(GameId),
    /// Play or pause the trailer on a game's page.
    Trailer(GameId),
    /// Remove the games picked in the library from it, after asking.
    RemovePicked,
    /// Read the picked games' details again.
    RefreshPicked,
    /// Edit the developer, publisher and genres of the picked games.
    EditPickedMetadata,
    /// Edit the developer, publisher and genres of one game.
    EditMetadata(GameId),
    /// Link the picked games to Steam by their titles.
    LinkPickedToSteam,
    /// Back up a game's saves with Ludusavi now.
    BackUpSaves(GameId),
    /// Show a game's Ludusavi backups.
    SaveBackups(GameId),
    /// Check why a game's last run failed.
    Troubleshoot(GameId),
    /// Show a game's ReShade and RenoDX (its settings' Tools section).
    GraphicsMods(GameId),
    /// Run one of the actions the user added to a game, by its place.
    UserAction(GameId, usize),
    /// Edit the actions of a game.
    EditActions(GameId),
    /// Show a Steam game's Steam Play choice (Linux).
    SteamPlay(GameId),
    /// Link a game of another store to its Steam app.
    SteamLink(GameId),
    /// Edit a game's own links.
    GameLinks(GameId),
    /// Choose which rows the game page's Details show.
    DetailRows,
    /// Set a game's time played by hand.
    EditPlaytime(GameId),
    /// Install one copy of a game's entry.
    InstallCopy(GameId),
    /// Turn a ROM library entry into an added PC game.
    PlayAsPcGame(GameId),
    /// Add a game to Steam as a non-Steam shortcut.
    AddToSteam(GameId),
    /// Remove Irori's Steam shortcut of a game.
    RemoveFromSteam(GameId),
    BackUp,
    Sync,
    /// Save a clip of the replay buffer.
    SaveClip,
    /// Pause or resume downloads, or choose what the computer does once
    /// they finish.
    Downloads(irori_manage::DownloadCommand),
    /// Make the interface larger, smaller or its default size.
    InterfaceSize(SizeStep),
    /// Report a problem, suggest an idea, ask a question or read the
    /// known issues, in the browser.
    Feedback(irori_settings::feedback::Feedback),
    /// Open the most recent saved crash report sheet.
    LatestCrashReport(std::path::PathBuf),
    /// Download Irori's update, or restart into it once it is ready.
    AppUpdate {
        ready: bool,
    },
}

/// The library's state that its palette commands change.
struct LibraryCommands<'a> {
    /// Whether the library groups by play status, while a library view
    /// exists.
    grouped: Option<bool>,
    /// Whether the library groups by game mode, while a library view
    /// exists.
    grouped_by_mode: Option<bool>,
    /// Whether the library groups by genre, while a library view exists.
    grouped_by_genre: Option<bool>,
    /// Whether the library groups by category, while a library view exists.
    grouped_by_category: Option<bool>,
    /// How the library groups its games, while a library view exists: the
    /// groupings by a game's facts are offered from it.
    grouping: Option<Grouping>,
    /// Every category of both kinds, to show one.
    categories: &'a [irori_ui::categories::CategoryEntry],
    /// The saved library views, to show one.
    views: &'a [(u64, SharedString)],
    /// How the library shows games; the other layouts are offered.
    layout: LibraryLayout,
    /// Which games it shows by install state, while a library view exists.
    install: Option<InstallFilter>,
    /// Whether it shows only favourites, while a library view exists.
    favourites: Option<bool>,
    /// Whether its header shows the quick controls.
    quick_controls: bool,
    /// While hidden games aren't locked: whether a PIN guards them (it can
    /// be locked again). `None` while locked: nothing about them is offered.
    hidden_pin: Option<bool>,
}

/// The palette the shell owns, shown as a dialog of the main window.
#[derive(Default)]
pub struct Palette {
    open: Rc<Cell<bool>>,
    view: Option<WeakEntity<PaletteView>>,
}

impl Palette {
    /// The command rows (games get rows of their own per query, see
    /// [`view`]). `replay` is the game the replay buffer records:
    /// saving a clip comes first. `open` is the game whose page is shown,
    /// with its play status: its status commands come next. `library` is
    /// what the library commands change. `sync` is whether library sync is
    /// on. `stores` have a store window. `friends`: a Steam and an Xbox
    /// account can show their friends.
    fn items(
        replay: Option<&str>,
        open: Option<OpenGame>,
        library: LibraryCommands<'_>,
        sync: bool,
        stores: &[Runner],
        friends: (bool, bool),
        targets: &mut Vec<PaletteTarget>,
    ) -> Vec<Entry> {
        let LibraryCommands {
            grouped,
            grouped_by_mode,
            grouped_by_genre,
            grouped_by_category,
            grouping,
            categories,
            views,
            layout,
            install,
            favourites,
            quick_controls,
            hidden_pin,
        } = library;
        let mut items = Vec::with_capacity(64);
        let mut push = |item: Entry, target: PaletteTarget, targets: &mut Vec<PaletteTarget>| {
            items.push(item);
            targets.push(target);
        };
        if let Some(title) = replay {
            push(
                Entry::new()
                    .label(t_args("shell.palette.save_clip", &[("title", title)]))
                    .keywords([t("shell.palette.save_clip_keywords")])
                    .icon(IconName::Clapperboard),
                PaletteTarget::SaveClip,
                targets,
            );
        }
        if let Some((game, current, actions)) = open {
            push(
                Entry::new()
                    .label(t_args(
                        "shell.palette.game_actions",
                        &[("title", &game.title)],
                    ))
                    .keywords([t("shell.palette.actions")])
                    .icon(IconName::ChevronRight),
                PaletteTarget::GameActions(game.id()),
                targets,
            );
            for (item, target) in game::rows(&game, current, &actions) {
                push(item, target, targets);
            }
        }
        for (label, icon, route) in [
            ("shell.nav.library", IconName::LibraryBig, Route::Library),
            ("shell.nav.downloads", IconName::Download, Route::Downloads),
            ("shell.nav.stores", IconName::Store, Route::Stores),
            ("shell.nav.stats", IconName::ChartColumn, Route::Stats),
            (
                if cfg!(target_os = "macos") {
                    "manage.wine.title"
                } else {
                    "shell.nav.wine"
                },
                IconName::Wine,
                Route::WineManager,
            ),
            ("shell.nav.emulators", IconName::Joystick, Route::Emulators),
            ("shell.nav.accounts", IconName::CircleUser, Route::Accounts),
            (
                "shell.nav.settings",
                IconName::Settings,
                Route::Settings("general".into()),
            ),
            (
                "shell.nav.big_picture",
                IconName::Gamepad2,
                Route::BigPicture,
            ),
        ] {
            if cfg!(windows) && route == Route::WineManager {
                continue;
            }
            let mut keywords = vec![t(label)];
            if route == Route::Downloads {
                keywords.push(t("shell.nav.updates"));
            }
            push(
                Entry::new()
                    .label(t_args("shell.palette.go_to", &[("place", &t(label))]))
                    .keywords(keywords)
                    .icon(icon),
                PaletteTarget::Route(route),
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.year_in_review"))
                .keywords([
                    t("shell.palette.year_in_review_keywords"),
                    t("shell.nav.stats"),
                ])
                .icon(IconName::Sparkles),
            PaletteTarget::Route(Route::YearInReview(None)),
            targets,
        );
        for (step, label, icon) in [
            (
                SizeStep::Larger,
                "shell.palette.interface_larger",
                IconName::ZoomIn,
            ),
            (
                SizeStep::Smaller,
                "shell.palette.interface_smaller",
                IconName::ZoomOut,
            ),
            (
                SizeStep::Reset,
                "shell.palette.interface_reset",
                IconName::RotateCcw,
            ),
        ] {
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([t("shell.palette.interface_size_keywords")])
                    .icon(icon),
                PaletteTarget::InterfaceSize(step),
                targets,
            );
        }
        for way in irori_settings::feedback::Feedback::ALL {
            push(
                Entry::new()
                    .label(t(way.label_key()))
                    .keywords([t("shell.palette.feedback_keywords")])
                    .icon(way.icon()),
                PaletteTarget::Feedback(way),
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.storage"))
                .keywords([t("shell.palette.storage_keywords")])
                .icon(IconName::HardDrive),
            PaletteTarget::Route(Route::Settings("storage".into())),
            targets,
        );
        if irori_core::HudBackend::current().is_supported() {
            push(
                Entry::new()
                    .label(t("shell.palette.performance_hud"))
                    .keywords([t("shell.palette.performance_hud_keywords")])
                    .icon(IconName::Gauge),
                PaletteTarget::Route(Route::Settings("hud".into())),
                targets,
            );
        }
        for &runner in stores {
            push(
                Entry::new()
                    .label(t_args(
                        "shell.palette.open_store",
                        &[("store", runner.store_name())],
                    ))
                    .keywords([t("shell.nav.stores")])
                    .icon(IconName::Store),
                PaletteTarget::OpenStore(runner),
                targets,
            );
        }
        let (steam_friends, xbox_friends) = friends;
        if steam_friends {
            push(
                Entry::new()
                    .label(t("shell.palette.steam_friends"))
                    .keywords([t("shell.palette.steam_friends_keywords")])
                    .icon(IconName::Users),
                PaletteTarget::SteamFriends,
                targets,
            );
        }
        if xbox_friends {
            push(
                Entry::new()
                    .label(t("shell.palette.xbox_friends"))
                    .keywords([t("shell.palette.xbox_friends_keywords")])
                    .icon(IconName::Users),
                PaletteTarget::XboxFriends,
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.getting_started"))
                .keywords([t("shell.palette.getting_started_keywords")])
                .icon(IconName::Sparkles),
            PaletteTarget::GettingStarted,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.tour"))
                .keywords([t("shell.palette.getting_started_keywords")])
                .icon(IconName::Compass),
            PaletteTarget::Tour,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.find_roms"))
                .keywords([t("shell.palette.find_roms_keywords")])
                .icon(IconName::Search),
            PaletteTarget::FindRoms,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.set_up_emulation"))
                .keywords([t("shell.palette.set_up_emulation_keywords")])
                .icon(IconName::Joystick),
            PaletteTarget::SetUpEmulation,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.add_emulator"))
                .keywords([t("shell.palette.add_emulator_keywords")])
                .icon(IconName::Plus),
            PaletteTarget::AddEmulator,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.add_folder"))
                .keywords([t("shell.palette.add_folder_keywords")])
                .icon(IconName::FolderPlus),
            PaletteTarget::AddFolder,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.manage_fields"))
                .keywords([t("shell.palette.manage_fields_keywords")])
                .icon(IconName::ListFilter),
            PaletteTarget::ManageFields,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.lutris_script"))
                .keywords([t("shell.palette.lutris_script_keywords")])
                .icon(IconName::ScrollText),
            PaletteTarget::LutrisScript,
            targets,
        );
        if let Some(grouped) = grouped {
            let label = if grouped {
                "shell.palette.ungroup_by_status"
            } else {
                "shell.palette.group_by_status"
            };
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([irori_ui::play_status::title()])
                    .icon(IconName::Rows3),
                PaletteTarget::GroupByStatus(!grouped),
                targets,
            );
        }
        if let Some(install) = install {
            for (value, label) in [
                (InstallFilter::Installed, "shell.palette.show_installed"),
                (InstallFilter::Any, "shell.palette.show_any_install"),
            ] {
                if value == install {
                    continue;
                }
                push(
                    Entry::new()
                        .label(t(label))
                        .keywords([t("shell.palette.filter_keywords")])
                        .icon(IconName::HardDrive),
                    PaletteTarget::Install(value),
                    targets,
                );
            }
        }
        if let Some(favourites) = favourites {
            let label = if favourites {
                "shell.palette.show_all_favourites"
            } else {
                "shell.palette.show_favourites"
            };
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([t("shell.palette.filter_keywords")])
                    .icon(IconName::Star),
                PaletteTarget::FavouritesOnly(!favourites),
                targets,
            );
        }
        push(
            Entry::new()
                .label(t(if quick_controls {
                    "shell.palette.hide_quick_controls"
                } else {
                    "shell.palette.show_quick_controls"
                }))
                .keywords([t("shell.palette.quick_controls_keywords")])
                .icon(IconName::SlidersHorizontal),
            PaletteTarget::QuickControls(!quick_controls),
            targets,
        );
        if let Some(pin_set) = hidden_pin {
            if pin_set {
                push(
                    Entry::new()
                        .label(t("shell.palette.lock_hidden"))
                        .keywords([t("shell.palette.hidden_keywords")])
                        .icon(IconName::Lock),
                    PaletteTarget::LockHidden,
                    targets,
                );
            }
            push(
                Entry::new()
                    .label(t("shell.palette.hidden_pin"))
                    .keywords([t("shell.palette.hidden_keywords")])
                    .icon(IconName::Lock),
                PaletteTarget::HiddenPin,
                targets,
            );
        }
        if let Some(grouped) = grouped_by_mode {
            let label = if grouped {
                "shell.palette.ungroup_by_mode"
            } else {
                "shell.palette.group_by_mode"
            };
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([
                        irori_ui::game_mode::title(),
                        t("shell.palette.game_mode_keywords"),
                    ])
                    .icon(IconName::Users),
                PaletteTarget::GroupByMode(!grouped),
                targets,
            );
        }
        if let Some(grouped) = grouped_by_genre {
            let label = if grouped {
                "shell.palette.ungroup_by_genre"
            } else {
                "shell.palette.group_by_genre"
            };
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([t("shell.palette.genre_keywords")])
                    .icon(IconName::Tag),
                PaletteTarget::GroupByGenre(!grouped),
                targets,
            );
        }
        if let Some(grouped) = grouped_by_category {
            let label = if grouped {
                "shell.palette.ungroup_by_category"
            } else {
                "shell.palette.group_by_category"
            };
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([t("shell.palette.category_keywords")])
                    .icon(IconName::Tags),
                PaletteTarget::GroupByCategory(!grouped),
                targets,
            );
        }
        if let Some(current) = grouping {
            for group in Grouping::BY_FACT {
                let Some(label) = group.palette_label(current) else {
                    continue;
                };
                let target = if current == group {
                    Grouping::None
                } else {
                    group
                };
                push(
                    Entry::new()
                        .label(label)
                        .keywords([group.label()])
                        .icon(IconName::Rows3),
                    PaletteTarget::GroupBy(target),
                    targets,
                );
            }
        }
        if grouped_by_category.is_some() {
            push(
                Entry::new()
                    .label(t("shell.palette.new_smart_category"))
                    .keywords([t("shell.palette.category_keywords")])
                    .icon(IconName::Sparkles),
                PaletteTarget::NewSmartCategory,
                targets,
            );
        }
        if !categories.is_empty() {
            push(
                Entry::new()
                    .label(t("shell.palette.export_categories"))
                    .keywords([t("shell.palette.category_keywords")])
                    .icon(IconName::Upload),
                PaletteTarget::ExportCategories,
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.import_categories"))
                .keywords([t("shell.palette.category_keywords")])
                .icon(IconName::Download),
            PaletteTarget::ImportCategories,
            targets,
        );
        for entry in categories {
            push(
                Entry::new()
                    .label(t_args(
                        "shell.palette.show_category",
                        &[("name", &entry.name)],
                    ))
                    .keywords([t("shell.palette.category_keywords")])
                    .icon(irori_ui::categories::icon(&entry.category)),
                PaletteTarget::Category(entry.category.clone()),
                targets,
            );
        }
        for (id, name) in views {
            push(
                Entry::new()
                    .label(t_args("shell.palette.show_view", &[("name", name)]))
                    .keywords([t("shell.palette.views_keywords")])
                    .icon(IconName::Bookmark),
                PaletteTarget::View(*id),
                targets,
            );
        }
        if grouped.is_some() {
            push(
                Entry::new()
                    .label(t("shell.palette.save_view"))
                    .keywords([t("shell.palette.views_keywords")])
                    .icon(IconName::BookmarkPlus),
                PaletteTarget::SaveView,
                targets,
            );
        }
        for (view, label, icon) in [
            (
                LibraryLayout::Grid,
                "shell.palette.show_grid",
                IconName::LayoutGrid,
            ),
            (
                LibraryLayout::List,
                "shell.palette.show_list",
                IconName::List,
            ),
            (
                LibraryLayout::Focus,
                "shell.palette.show_focus",
                IconName::LayoutPanelLeft,
            ),
        ] {
            if view == layout {
                continue;
            }
            push(
                Entry::new()
                    .label(t(label))
                    .keywords([t("shell.palette.layout_keywords")])
                    .icon(icon),
                PaletteTarget::LibraryView(view),
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.refresh"))
                .icon(IconName::RefreshCw),
            PaletteTarget::Refresh,
            targets,
        );
        push(
            Entry::new()
                .label(t("shell.palette.back_up"))
                .keywords([t("shell.palette.back_up_keywords")])
                .icon(IconName::Save),
            PaletteTarget::BackUp,
            targets,
        );
        if sync {
            push(
                Entry::new()
                    .label(t("shell.palette.sync"))
                    .keywords([t("shell.palette.sync_keywords")])
                    .icon(IconName::RefreshCw),
                PaletteTarget::Sync,
                targets,
            );
        }
        push(
            Entry::new()
                .label(t("shell.palette.random"))
                .keywords([t("shell.palette.random_keywords")])
                .icon(IconName::Dices),
            PaletteTarget::Random,
            targets,
        );
        items
    }

    /// Open the palette as a dialog in `window`, or put the keyboard back
    /// in it when it is open.
    pub fn open(&mut self, window: &mut Window, cx: &mut App) {
        if self.open.get()
            && let Some(view) = self.view.as_ref().and_then(|v| v.upgrade())
        {
            PaletteView::focus(&view, window, cx);
            return;
        }
        self.open.set(true);
        let view = cx.new(|cx| PaletteView::new(Host::Dialog, window, cx));
        self.view = Some(view.downgrade());
        let open = self.open.clone();
        {
            let view = view.clone();
            window.defer(cx, move |window, cx| PaletteView::focus(&view, window, cx));
        }
        window.open_dialog(cx, move |dialog, window, _| {
            let open = open.clone();
            let view = view.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(view::width(window))
                .on_close(move |_, _, _| open.set(false))
                .content(move |content, _, _| content.child(view.clone()))
        });
    }
}

/// The command rows, for the palette opening now: the open game's, then the
/// pages and the commands the library and the accounts offer.
fn command_rows(_window: &mut Window, cx: &mut App) -> Vec<Row> {
    let open = {
        let library = models(cx).library.read(cx);
        match irori_ui::nav::navigator(cx).read(cx).current() {
            Route::Game(id) => library
                .snapshot
                .get(id)
                .map(|g| (g.clone(), library.user.play_status(&id.key()))),
            _ => None,
        }
    };
    let open = open.map(|(game, status)| {
        let actions = irori_ui::user_actions::list(&game.id(), cx);
        (game, status, actions)
    });
    let categories = irori_library::category_list(cx);
    let views = irori_library::saved_view_list(cx);
    let library = LibraryCommands {
        grouped: irori_library::groups_by_status(cx),
        grouped_by_mode: irori_library::groups_by_mode(cx),
        grouped_by_genre: irori_library::groups_by_genre(cx),
        grouped_by_category: irori_library::groups_by_category(cx),
        grouping: irori_library::grouping(cx),
        categories: &categories,
        views: &views,
        layout: irori_library::library_layout(cx),
        install: irori_library::install_filter(cx),
        favourites: irori_library::favourites_only(cx),
        quick_controls: irori_library::quick_controls(cx),
        hidden_pin: (!irori_ui::hidden_lock::locked(cx))
            .then(|| irori_ui::hidden_lock::pin_set(cx)),
    };
    let sync = models(cx).settings.read(cx).app.sync.mode != irori_core::SyncMode::Off;
    let replay = irori_ui::backend(cx).replay().map(|r| r.title);
    // The shown game's cloud saves, when its store keeps them.
    let syncable = open
        .as_ref()
        .map(|(game, _, _)| game.clone())
        .filter(|game| irori_game::offers_sync(game, cx));
    // The shown game's trailer, when its page has one.
    let with_trailer = open
        .as_ref()
        .map(|(game, _, _)| game.clone())
        .filter(|game| irori_ui::trailer::has_trailer(&game.id(), cx));
    let mut targets = Vec::new();
    let mut items = Palette::items(
        replay.as_deref(),
        open,
        library,
        sync,
        &irori_manage::store_window_stores(cx),
        (
            irori_manage::steam_friends_available(cx),
            irori_manage::xbox_friends_available(cx),
        ),
        &mut targets,
    );
    if let Some(game) = syncable {
        items.push(
            Entry::new()
                .label(t_args(
                    "shell.palette.sync_saves",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.sync_saves_keywords")])
                .icon(IconName::RefreshCw),
        );
        targets.push(PaletteTarget::SyncSaves(game.id()));
    }
    let update = models(cx).app_update.read(cx).status.clone();
    if irori_settings::app_update::can_install(&update)
        && let Some(release) = &update.available
    {
        let ready = matches!(update.install, irori_services::SelfInstall::Ready { .. });
        let label = if ready {
            "shell.palette.restart_update"
        } else {
            "shell.palette.install_update"
        };
        items.push(
            Entry::new()
                .label(t_args(label, &[("version", &release.version)]))
                .keywords([t("shell.palette.install_update_keywords")])
                .icon(if ready {
                    IconName::RotateCw
                } else {
                    IconName::Download
                }),
        );
        targets.push(PaletteTarget::AppUpdate { ready });
    }
    if let Some(game) = with_trailer {
        items.push(
            Entry::new()
                .label(t_args("shell.palette.trailer", &[("title", &game.title)]))
                .keywords([t("shell.palette.trailer_keywords")])
                .icon(IconName::Clapperboard),
        );
        targets.push(PaletteTarget::Trailer(game.id()));
    }
    let deals = models(cx).settings.read(cx).app.deals;
    items.push(
        Entry::new()
            .label(t(if deals {
                "shell.palette.hide_deals"
            } else {
                "shell.palette.show_deals"
            }))
            .keywords([t("shell.palette.deals_keywords"), t("shell.nav.stores")])
            .icon(if deals {
                IconName::EyeOff
            } else {
                IconName::Eye
            }),
    );
    targets.push(PaletteTarget::Deals(!deals));
    if let Some(count) = irori_library::removable_picks(cx) {
        items.push(
            Entry::new()
                .label(t_args(
                    "shell.palette.remove_picked",
                    &[("count", &count.to_string())],
                ))
                .keywords([t("shell.palette.remove_picked_keywords")])
                .icon(IconName::ListX),
        );
        targets.push(PaletteTarget::RemovePicked);
    }
    if let Some(count) = irori_library::picked_count(cx) {
        let count = count.to_string();
        items.push(
            Entry::new()
                .label(t_args("shell.palette.refresh_picked", &[("count", &count)]))
                .keywords([t("shell.palette.refresh_picked_keywords")])
                .icon(IconName::RefreshCw),
        );
        targets.push(PaletteTarget::RefreshPicked);
        items.push(
            Entry::new()
                .label(t_args(
                    "shell.palette.edit_picked_metadata",
                    &[("count", &count)],
                ))
                .keywords([t("shell.palette.edit_metadata_keywords")])
                .icon(IconName::Tags),
        );
        targets.push(PaletteTarget::EditPickedMetadata);
        items.push(
            Entry::new()
                .label(t_args("shell.palette.link_picked", &[("count", &count)]))
                .keywords([t("shell.palette.steam_link_keywords")])
                .icon(IconName::Link),
        );
        targets.push(PaletteTarget::LinkPickedToSteam);
    }
    for engine in irori_manage::openable_engines(cx) {
        items.push(
            Entry::new()
                .label(t_args(
                    "shell.palette.open_engine",
                    &[("engine", &engine.name)],
                ))
                .keywords([
                    t("shell.palette.open_engine_keywords"),
                    t("shell.nav.emulators"),
                ])
                .icon(IconName::Joystick),
        );
        targets.push(PaletteTarget::OpenEngine(engine.id, engine.name));
    }
    for launcher in irori_manage::known_launchers(cx) {
        let name = launcher.display_name();
        items.push(
            Entry::new()
                .label(t_args("shell.palette.open_launcher", &[("launcher", name)]))
                .keywords([t("shell.nav.stores")])
                .icon(IconName::ExternalLink),
        );
        targets.push(PaletteTarget::OpenLauncher(launcher));
        items.push(
            Entry::new()
                .label(t_args(
                    "shell.palette.launcher_shortcut",
                    &[("launcher", name)],
                ))
                .keywords([t("shell.palette.launcher_shortcut_keywords")])
                .icon(IconName::Link),
        );
        targets.push(PaletteTarget::LauncherShortcut(launcher));
    }
    items.push(
        Entry::new()
            .label(t("shell.palette.duplicates"))
            .keywords([t("shell.palette.duplicates_keywords")])
            .icon(IconName::Copy),
    );
    targets.push(PaletteTarget::Duplicates);
    if !cfg!(windows) {
        for launcher in irori_manage::wine_launchers(cx) {
            items.push(
                Entry::new()
                    .label(t_args(
                        "shell.palette.launcher_wine",
                        &[("launcher", launcher.display_name())],
                    ))
                    .keywords([t("shell.palette.launcher_wine_keywords")])
                    .icon(IconName::Wine),
            );
            targets.push(PaletteTarget::LauncherWine(launcher));
        }
    }
    if irori_manage::steam_client_library_state(cx).is_some_and(|s| s.steam_found && !s.signed_in) {
        items.push(
            Entry::new()
                .label(t("shell.palette.steam_client_library"))
                .keywords([t("shell.palette.steam_client_library_keywords")])
                .icon(IconName::LibraryBig),
        );
        targets.push(PaletteTarget::SteamClientLibrary);
    }
    if irori_library::offers_launchbox(cx) {
        items.push(
            Entry::new()
                .label(t("shell.palette.launchbox"))
                .keywords([t("shell.palette.launchbox_keywords")])
                .icon(IconName::ArrowDown),
        );
        targets.push(PaletteTarget::LaunchBox);
    }
    if irori_manage::xbox_offline_available(cx) {
        items.push(
            Entry::new()
                .label(t("shell.palette.xbox_offline_play"))
                .keywords([t("shell.palette.xbox_offline_play_keywords")])
                .icon(IconName::HardDrive),
        );
        targets.push(PaletteTarget::XboxOfflinePlay);
    }
    {
        let offline = irori_ui::models::working_offline(cx);
        items.push(
            Entry::new()
                .label(t(if offline {
                    "shell.palette.work_online"
                } else {
                    "shell.palette.work_offline"
                }))
                .keywords([t("shell.palette.work_offline_keywords")])
                .icon(if offline {
                    IconName::Wifi
                } else {
                    IconName::WifiOff
                }),
        );
        targets.push(PaletteTarget::WorkOffline(!offline));
    }
    for (label, icon, command) in irori_manage::download_commands(cx) {
        items.push(
            Entry::new()
                .label(label)
                .keywords([irori_manage::download_command_keywords()])
                .icon(icon),
        );
        targets.push(PaletteTarget::Downloads(command));
    }
    if let Some(latest) = irori_core::crash::list_reports(&irori_core::Paths::get().logs_dir())
        .into_iter()
        .next()
    {
        items.push(
            Entry::new()
                .label(t("shell.palette.latest_crash_report"))
                .keywords([t("shell.palette.latest_crash_report_keywords")])
                .icon(IconName::TriangleAlert),
        );
        targets.push(PaletteTarget::LatestCrashReport(latest.path));
    }
    items
        .into_iter()
        .zip(targets)
        .map(|(entry, target)| Row::command(entry, target))
        .collect()
}

pub(crate) fn run_target(target: PaletteTarget, window: &mut Window, cx: &mut App) {
    match target {
        PaletteTarget::Open(id) => irori_ui::navigate(Route::Game(id), cx),
        PaletteTarget::Route(r) => irori_ui::navigate(r, cx),
        PaletteTarget::Refresh => irori_ui::models::refresh_library(false, cx),
        PaletteTarget::Random => irori_library::open_random_game(window, cx),
        PaletteTarget::SetUpEmulation => irori_manage::open_emulation_setup(window, cx),
        PaletteTarget::AddEmulator => irori_manage::open_custom_engine_dialog(None, window, cx),
        PaletteTarget::OpenEngine(id, name) => irori_manage::open_engine(id, name, window, cx),
        PaletteTarget::OpenLauncher(launcher) => irori_manage::open_launcher(launcher, window, cx),
        PaletteTarget::LauncherShortcut(launcher) => {
            irori_manage::launcher_shortcut(launcher, true, window, cx)
        }
        PaletteTarget::LauncherWine(launcher) => {
            irori_manage::open_launcher_wine_dialog(launcher, window, cx)
        }
        PaletteTarget::Duplicates => irori_library::open_duplicates_dialog(window, cx),
        PaletteTarget::GettingStarted => irori_manage::open_getting_started(window, cx),
        PaletteTarget::Deals(shown) => irori_manage::set_deals_shown(shown, window, cx),
        PaletteTarget::Tour => irori_ui::tour::start(window, cx),
        PaletteTarget::ExportCategories => irori_library::export_categories(window, cx),
        PaletteTarget::ImportCategories => irori_library::import_categories(window, cx),
        PaletteTarget::FindRoms => irori_manage::open_rom_sources_dialog(window, cx),
        PaletteTarget::SteamFriends => irori_manage::open_steam_friends_dialog(window, cx),
        PaletteTarget::XboxOfflinePlay => irori_manage::open_offline_play_dialog(window, cx),
        PaletteTarget::WorkOffline(offline) => irori_ui::models::set_working_offline(offline, cx),
        PaletteTarget::XboxFriends => {
            irori_manage::open_friends_dialog(irori_manage::FriendsSource::Xbox, window, cx)
        }
        PaletteTarget::OpenStore(runner) => {
            irori_manage::open_store_window(runner, None, window, cx)
        }
        PaletteTarget::LutrisScript => irori_library::open_lutris_installer(None, window, cx),
        PaletteTarget::SteamClientLibrary => irori_manage::open_steam_library_dialog(window, cx),
        PaletteTarget::LaunchBox => irori_library::download_launchbox(window, cx),
        PaletteTarget::Play(id) => crate::shell::launch_game(id, window, cx),
        PaletteTarget::CancelLaunch(id) => {
            irori_ui::backend(cx).cancel_launch(&id);
        }
        PaletteTarget::BackToGame(id) => irori_ui::game_window::back_to_game(id, window, cx),
        PaletteTarget::Stop(id) => irori_game::ops::stop(&id, window, cx).detach(),
        PaletteTarget::StorePage(runner, url) => {
            irori_ui::hooks::open_store_page(runner, url, window, cx)
        }
        PaletteTarget::Favourite(id, on) => irori_game::ops::set_favourite(&id, on, cx),
        PaletteTarget::Hide(id, on) => irori_game::ops::set_hidden(&id, on, cx),
        PaletteTarget::OpenFolder(id) => irori_game::ops::open_install_folder(&id, window, cx),
        PaletteTarget::CapturesFolder(id) => irori_game::ops::open_captures_folder(&id, window, cx),
        // The palette's own levels; its view handles them.
        PaletteTarget::GameActions(_) | PaletteTarget::Back | PaletteTarget::RandomFrom(_) => {}
        PaletteTarget::PlayStatus(id, status) => irori_ui::play_status::set(&id, status, cx),
        PaletteTarget::View(id) => irori_library::apply_view(id, cx),
        PaletteTarget::AddFolder => irori_library::open_add_folder_dialog(window, cx),
        // After the palette's own dialog has closed.
        PaletteTarget::ManageFields => window.defer(cx, irori_library::open_fields_dialog),
        PaletteTarget::SaveView => {
            irori_ui::navigate(Route::Library, cx);
            // After the palette's own dialog has closed.
            window.defer(cx, irori_library::open_save_view_dialog);
        }
        PaletteTarget::SyncSaves(id) => irori_game::sync_saves_now(&id, window, cx),
        PaletteTarget::Trailer(id) => irori_ui::trailer::toggle_trailer(&id, cx),
        // After the palette's own dialog has closed.
        PaletteTarget::RemovePicked => window.defer(cx, irori_library::remove_picked_from_library),
        PaletteTarget::RefreshPicked => irori_library::refresh_picked_details(window, cx),
        // After the palette's own dialog has closed.
        PaletteTarget::EditPickedMetadata => window.defer(cx, irori_library::edit_picked_metadata),
        PaletteTarget::EditMetadata(id) => window.defer(cx, move |window, cx| {
            irori_library::open_metadata_dialog(vec![id], window, cx)
        }),
        PaletteTarget::LinkPickedToSteam => irori_library::link_picked_to_steam(window, cx),
        PaletteTarget::BackUpSaves(id) => irori_ui::save_backups::back_up_now(&id, window, cx),
        PaletteTarget::SaveBackups(id) => {
            irori_ui::save_backups::open_save_backups_sheet(id, window, cx)
        }
        PaletteTarget::Troubleshoot(id) => irori_game::open_troubleshooter(id, window, cx),
        PaletteTarget::GraphicsMods(id) => irori_game::ops::open_settings(&id, "reshade", cx),
        PaletteTarget::UserAction(id, index) => irori_ui::user_actions::run(id, index, window, cx),
        PaletteTarget::EditActions(id) => irori_ui::user_actions::edit(&id, cx),
        PaletteTarget::SaveClip => irori_ui::backend(cx).save_clip(),
        PaletteTarget::Downloads(command) => irori_manage::run_download_command(command, cx),
        PaletteTarget::InterfaceSize(step) => irori_ui::theme::change_interface_size(step, cx),
        PaletteTarget::Feedback(way) => irori_settings::feedback::open(way, window, cx).detach(),
        PaletteTarget::LatestCrashReport(path) => window.defer(cx, move |window, cx| {
            irori_settings::open_crash_report_sheet(path, window, cx)
        }),
        PaletteTarget::AppUpdate { ready: true } => {
            irori_settings::app_update::restart_into_update(window, cx)
        }
        PaletteTarget::AppUpdate { ready: false } => {
            irori_settings::app_update::install_update(cx);
            irori_ui::navigate(Route::Settings("about".into()), cx);
        }
        PaletteTarget::SteamPlay(id) => irori_game::ops::open_steam_play(&id, cx),
        // After the palette's own dialog has closed.
        PaletteTarget::SteamLink(id) => window.defer(cx, move |window, cx| {
            irori_game::open_steam_link_dialog(id, window, cx)
        }),
        PaletteTarget::GameLinks(id) => window.defer(cx, move |window, cx| {
            irori_game::open_links_dialog(id, window, cx)
        }),
        // After the palette's own dialog has closed.
        PaletteTarget::DetailRows => window.defer(cx, irori_game::open_detail_rows_dialog),
        PaletteTarget::EditPlaytime(id) => window.defer(cx, move |window, cx| {
            irori_game::open_playtime_dialog(id, window, cx)
        }),
        // After the palette's own dialog has closed: the sheet is a dialog.
        PaletteTarget::InstallCopy(id) => window.defer(cx, move |window, cx| {
            irori_game::ops::install(&id, window, cx)
        }),
        PaletteTarget::PlayAsPcGame(id) => {
            irori_library::open_play_as_pc_game_dialog(&id, window, cx)
        }
        PaletteTarget::AddToSteam(id) => {
            irori_game::ops::add_shortcuts(&id, true, window, cx).detach()
        }
        PaletteTarget::RemoveFromSteam(id) => {
            irori_game::ops::remove_from_steam(&id, window, cx).detach()
        }
        PaletteTarget::GroupByStatus(on) => {
            irori_library::set_groups_by_status(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::Install(install) => {
            irori_library::set_install_filter(install, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::FavouritesOnly(on) => {
            irori_library::set_favourites_only(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::QuickControls(on) => {
            irori_library::set_quick_controls(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::LockHidden => irori_ui::hidden_lock::lock(cx),
        PaletteTarget::HiddenPin => irori_ui::hidden_lock::open_pin_dialog(window, cx),
        PaletteTarget::GroupByMode(on) => {
            irori_library::set_groups_by_mode(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::GroupByGenre(on) => {
            irori_library::set_groups_by_genre(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::GroupByCategory(on) => {
            irori_library::set_groups_by_category(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::GroupBy(group) => {
            irori_library::set_grouping(group, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::Category(category) => irori_library::show_category(category, cx),
        PaletteTarget::NewSmartCategory => {
            irori_ui::navigate(Route::Library, cx);
            // After the palette's own dialog has closed.
            window.defer(cx, |window, cx| {
                irori_library::open_smart_category_editor(None, None, window, cx)
            });
        }
        PaletteTarget::LibraryView(layout) => {
            irori_library::set_library_layout(layout, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::BackUp => {
            let b = irori_ui::backend(cx);
            let task = irori_ui::run(cx, async move { b.back_up_now().await.map(|_| ()) });
            report(
                task,
                "shell.palette.backed_up",
                "shell.notice.backup_failed",
                window,
                cx,
            );
        }
        PaletteTarget::Sync => {
            let b = irori_ui::backend(cx);
            let task = irori_ui::run(cx, async move { b.sync_now().await.map(|_| ()) });
            report(
                task,
                "shell.palette.synced",
                "shell.notice.sync_failed",
                window,
                cx,
            );
        }
    }
}

/// Tell how a command run in the background went.
fn report(
    task: gpui_kit::Task<anyhow::Result<()>>,
    done: &'static str,
    failed: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let note = match result {
                    Ok(()) => Notification::success(t(done)),
                    Err(e) => Notification::error(format!("{e:#}")).title(t(failed)),
                };
                window.push_notification(note, cx);
            });
        })
        .detach();
}

mod entry;
mod game;
mod preview;
mod search;
#[cfg(test)]
mod tests;
mod view;
