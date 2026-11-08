//! Command palette (Ctrl+K): jump to any game, play installed games, pick a
//! random one, set the play status of the game whose page is open, group the
//! library by play status, show only installed games or favourites, turn the
//! library header's quick controls on or off, show or save a library view, switch its view
//! (grid, list, Focus), back up, sync, save a clip while the replay buffer
//! records, and run window commands, all from the keyboard.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _};
use gpui_kit::{
    App, AppContext as _, Entity, ParentElement as _, SharedString, Styled as _, Window, px,
};
use irori_core::settings::LibraryLayout;
use irori_core::{GameId, GameInfo, Runner};
use irori_library::InstallFilter;
use irori_services::PlayStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::theme::SizeStep;
use irori_ui::{Route, models};

/// What confirming a palette row does.
#[derive(Clone)]
enum PaletteTarget {
    Open(GameId),
    Play(GameId),
    Route(Route),
    Refresh,
    Random,
    SetUpEmulation,
    /// The Steam friends window.
    SteamFriends,
    /// Show a store's website in the store window.
    OpenStore(Runner),
    LutrisScript,
    /// Read the Steam library from the Steam client.
    SteamClientLibrary,
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
    /// Show a saved library view.
    View(u64),
    /// Save the library's current criteria as a view.
    SaveView,
    /// Pick a folder and add the games in it.
    AddFolder,
    /// Show the library as a grid, a list or in Focus view, and show it.
    LibraryView(LibraryLayout),
    /// Back up a game's saves with Ludusavi now.
    BackUpSaves(GameId),
    /// Show a game's Ludusavi backups.
    SaveBackups(GameId),
    /// Check why a game's last run failed.
    Troubleshoot(GameId),
    /// Show a Steam game's Steam Play choice (Linux).
    SteamPlay(GameId),
    BackUp,
    Sync,
    /// Save a clip of the replay buffer.
    SaveClip,
    /// Make the interface larger, smaller or its default size.
    InterfaceSize(SizeStep),
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

/// Retained palette state, owned by the shell.
pub struct Palette {
    state: Entity<CommandState>,
    targets: Rc<std::cell::RefCell<Vec<PaletteTarget>>>,
    open: Rc<Cell<bool>>,
}

impl Palette {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            targets: Rc::default(),
            open: Rc::default(),
        }
    }

    /// Rows for `games`. `replay` is the game the replay buffer records:
    /// saving a clip comes first. `open` is the game whose page is shown,
    /// with its play status: its status commands come next. `library` is
    /// what the library commands change. `sync` is whether library sync is
    /// on. `stores` have a store window. `steam_friends`: a Steam account can
    /// show its friends.
    #[allow(clippy::too_many_arguments)] // One row source per argument, all needed.
    fn items(
        games: &[Arc<GameInfo>],
        replay: Option<&str>,
        open: Option<(Arc<GameInfo>, Option<PlayStatus>)>,
        library: LibraryCommands<'_>,
        sync: bool,
        stores: &[Runner],
        steam_friends: bool,
        targets: &mut Vec<PaletteTarget>,
    ) -> Vec<CommandItem> {
        let LibraryCommands {
            grouped,
            grouped_by_mode,
            grouped_by_genre,
            views,
            layout,
            install,
            favourites,
            quick_controls,
            hidden_pin,
        } = library;
        let mut items = Vec::with_capacity(games.len() * 2 + 16);
        let mut push =
            |item: CommandItem, target: PaletteTarget, targets: &mut Vec<PaletteTarget>| {
                items.push(item);
                targets.push(target);
            };
        if let Some(title) = replay {
            push(
                CommandItem::new()
                    .label(t_args("shell.palette.save_clip", &[("title", title)]))
                    .keywords([t("shell.palette.save_clip_keywords")])
                    .icon(IconName::Clapperboard),
                PaletteTarget::SaveClip,
                targets,
            );
        }
        if let Some((game, current)) = open {
            let keywords = [irori_ui::play_status::title()];
            for status in PlayStatus::ALL.into_iter().filter(|s| current != Some(*s)) {
                let label = t_args(
                    "shell.palette.play_status",
                    &[
                        ("title", &game.title),
                        ("status", &irori_ui::play_status::label(status)),
                    ],
                );
                push(
                    CommandItem::new()
                        .label(label)
                        .keywords(keywords.clone())
                        .icon(irori_ui::play_status::icon(status)),
                    PaletteTarget::PlayStatus(game.id(), Some(status)),
                    targets,
                );
            }
            if current.is_some() {
                push(
                    CommandItem::new()
                        .label(t_args(
                            "shell.palette.clear_play_status",
                            &[("title", &game.title)],
                        ))
                        .keywords(keywords)
                        .icon(IconName::X),
                    PaletteTarget::PlayStatus(game.id(), None),
                    targets,
                );
            }
            if game.is_installed && game.runner().manages_installs() {
                push(
                    CommandItem::new()
                        .label(t_args(
                            "shell.palette.troubleshoot",
                            &[("title", &game.title)],
                        ))
                        .keywords([t("shell.palette.troubleshoot_keywords")])
                        .icon(IconName::Stethoscope),
                    PaletteTarget::Troubleshoot(game.id()),
                    targets,
                );
            }
            if irori_game::ops::offers_steam_play(&game.id()) {
                push(
                    CommandItem::new()
                        .label(t_args(
                            "shell.palette.steam_play",
                            &[("title", &game.title)],
                        ))
                        .keywords([t("shell.palette.steam_play_keywords")])
                        .icon(IconName::Wine),
                    PaletteTarget::SteamPlay(game.id()),
                    targets,
                );
            }
            if irori_ui::save_backups::offers_backups(&game) {
                let keywords = [SharedString::new_static("Ludusavi")];
                push(
                    CommandItem::new()
                        .label(t_args(
                            "shell.palette.back_up_saves",
                            &[("title", &game.title)],
                        ))
                        .keywords(keywords.clone())
                        .icon(IconName::DatabaseBackup),
                    PaletteTarget::BackUpSaves(game.id()),
                    targets,
                );
                push(
                    CommandItem::new()
                        .label(t_args(
                            "shell.palette.save_backups",
                            &[("title", &game.title)],
                        ))
                        .keywords(keywords)
                        .icon(IconName::ArchiveRestore),
                    PaletteTarget::SaveBackups(game.id()),
                    targets,
                );
            }
        }
        for (label, icon, route) in [
            ("shell.nav.library", IconName::LibraryBig, Route::Library),
            ("shell.nav.downloads", IconName::Download, Route::Downloads),
            ("shell.nav.updates", IconName::RefreshCw, Route::Updates),
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
            push(
                CommandItem::new()
                    .label(t_args("shell.palette.go_to", &[("place", &t(label))]))
                    .keywords([t(label)])
                    .icon(icon),
                PaletteTarget::Route(route),
                targets,
            );
        }
        push(
            CommandItem::new()
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
                CommandItem::new()
                    .label(t(label))
                    .keywords([t("shell.palette.interface_size_keywords")])
                    .icon(icon),
                PaletteTarget::InterfaceSize(step),
                targets,
            );
        }
        push(
            CommandItem::new()
                .label(t("shell.palette.storage"))
                .keywords([t("shell.palette.storage_keywords")])
                .icon(IconName::HardDrive),
            PaletteTarget::Route(Route::Settings("storage".into())),
            targets,
        );
        for &runner in stores {
            push(
                CommandItem::new()
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
        if steam_friends {
            push(
                CommandItem::new()
                    .label(t("shell.palette.steam_friends"))
                    .keywords([t("shell.palette.steam_friends_keywords")])
                    .icon(IconName::Users),
                PaletteTarget::SteamFriends,
                targets,
            );
        }
        push(
            CommandItem::new()
                .label(t("shell.palette.set_up_emulation"))
                .keywords([t("shell.palette.set_up_emulation_keywords")])
                .icon(IconName::Joystick),
            PaletteTarget::SetUpEmulation,
            targets,
        );
        push(
            CommandItem::new()
                .label(t("shell.palette.add_folder"))
                .keywords([t("shell.palette.add_folder_keywords")])
                .icon(IconName::FolderPlus),
            PaletteTarget::AddFolder,
            targets,
        );
        push(
            CommandItem::new()
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
                CommandItem::new()
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
                    CommandItem::new()
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
                CommandItem::new()
                    .label(t(label))
                    .keywords([t("shell.palette.filter_keywords")])
                    .icon(IconName::Star),
                PaletteTarget::FavouritesOnly(!favourites),
                targets,
            );
        }
        push(
            CommandItem::new()
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
                    CommandItem::new()
                        .label(t("shell.palette.lock_hidden"))
                        .keywords([t("shell.palette.hidden_keywords")])
                        .icon(IconName::Lock),
                    PaletteTarget::LockHidden,
                    targets,
                );
            }
            push(
                CommandItem::new()
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
                CommandItem::new()
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
                CommandItem::new()
                    .label(t(label))
                    .keywords([t("shell.palette.genre_keywords")])
                    .icon(IconName::Tag),
                PaletteTarget::GroupByGenre(!grouped),
                targets,
            );
        }
        for (id, name) in views {
            push(
                CommandItem::new()
                    .label(t_args("shell.palette.show_view", &[("name", name)]))
                    .keywords([t("shell.palette.views_keywords")])
                    .icon(IconName::Bookmark),
                PaletteTarget::View(*id),
                targets,
            );
        }
        if grouped.is_some() {
            push(
                CommandItem::new()
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
                CommandItem::new()
                    .label(t(label))
                    .keywords([t("shell.palette.layout_keywords")])
                    .icon(icon),
                PaletteTarget::LibraryView(view),
                targets,
            );
        }
        push(
            CommandItem::new()
                .label(t("shell.palette.refresh"))
                .icon(IconName::RefreshCw),
            PaletteTarget::Refresh,
            targets,
        );
        push(
            CommandItem::new()
                .label(t("shell.palette.back_up"))
                .keywords([t("shell.palette.back_up_keywords")])
                .icon(IconName::Save),
            PaletteTarget::BackUp,
            targets,
        );
        if sync {
            push(
                CommandItem::new()
                    .label(t("shell.palette.sync"))
                    .keywords([t("shell.palette.sync_keywords")])
                    .icon(IconName::RefreshCw),
                PaletteTarget::Sync,
                targets,
            );
        }
        push(
            CommandItem::new()
                .label(t("shell.palette.random"))
                .keywords([t("shell.palette.random_keywords")])
                .icon(IconName::Dices),
            PaletteTarget::Random,
            targets,
        );
        for g in games.iter().filter(|g| g.is_installed) {
            push(
                CommandItem::new()
                    .label(t_args("shell.palette.play", &[("title", &g.title)]))
                    .icon(IconName::Play)
                    .keywords([g.runner().short_name()]),
                PaletteTarget::Play(g.id()),
                targets,
            );
        }
        for g in games {
            push(
                CommandItem::new()
                    .label(g.title.clone())
                    .icon(IconName::Gamepad2)
                    .keywords([g.runner().short_name(), g.runner().store_name()]),
                PaletteTarget::Open(g.id()),
                targets,
            );
        }
        items
    }

    /// Open the palette as a dialog in `window`.
    pub fn open(&self, window: &mut Window, cx: &mut App) {
        if self.open.get() {
            self.state.update(cx, |s, cx| s.focus(window, cx));
            return;
        }
        self.open.set(true);
        let (games, open) = {
            let library = models(cx).library.read(cx);
            let open = match irori_ui::nav::navigator(cx).read(cx).current() {
                Route::Game(id) => library
                    .snapshot
                    .get(id)
                    .map(|g| (g.clone(), library.user.play_status(&id.key()))),
                _ => None,
            };
            // Hidden games behind the PIN aren't found here either.
            let games = if library.snapshot.hidden_locked {
                library
                    .snapshot
                    .games
                    .iter()
                    .filter(|g| !library.user.hidden.contains(&g.id().key()))
                    .cloned()
                    .collect()
            } else {
                library.snapshot.games.clone()
            };
            (games, open)
        };
        let mut targets = Vec::new();
        let grouped = irori_library::groups_by_status(cx);
        let grouped_by_mode = irori_library::groups_by_mode(cx);
        let grouped_by_genre = irori_library::groups_by_genre(cx);
        let views = irori_library::saved_view_list(cx);
        let layout = irori_library::library_layout(cx);
        let install = irori_library::install_filter(cx);
        let favourites = irori_library::favourites_only(cx);
        let quick_controls = irori_library::quick_controls(cx);
        let sync = models(cx).settings.read(cx).app.sync.mode != irori_core::SyncMode::Off;
        let replay = irori_ui::backend(cx).replay().map(|r| r.title);
        let hidden_pin =
            (!irori_ui::hidden_lock::locked(cx)).then(|| irori_ui::hidden_lock::pin_set(cx));
        let library = LibraryCommands {
            grouped,
            grouped_by_mode,
            grouped_by_genre,
            views: &views,
            layout,
            install,
            favourites,
            quick_controls,
            hidden_pin,
        };
        let mut items = Self::items(
            &games,
            replay.as_deref(),
            open,
            library,
            sync,
            &irori_manage::store_window_stores(cx),
            irori_manage::steam_friends_available(cx),
            &mut targets,
        );
        if irori_manage::steam_client_library_state(cx)
            .is_some_and(|s| s.steam_found && !s.signed_in)
        {
            items.push(
                CommandItem::new()
                    .label(t("shell.palette.steam_client_library"))
                    .keywords([t("shell.palette.steam_client_library_keywords")])
                    .icon(IconName::LibraryBig),
            );
            targets.push(PaletteTarget::SteamClientLibrary);
        }
        let items = Rc::new(items);
        *self.targets.borrow_mut() = targets;
        self.state.update(cx, |s, cx| s.set_query("", window, cx));

        let state = self.state.clone();
        let targets = self.targets.clone();
        let open = self.open.clone();
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let state = state.clone();
            let targets = targets.clone();
            let items = items.clone();
            let open = open.clone();
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .on_close(move |_, _, _| open.set(false))
                .content(move |content, window, cx| {
                    if focus_on_mount.replace(false) {
                        let state = state.clone();
                        window.defer(cx, move |window, cx| {
                            state.update(cx, |s, cx| s.focus(window, cx))
                        });
                    }
                    let targets = targets.clone();
                    content.child(
                        Command::new(&state)
                            .bordered(false)
                            .rounded(cx.theme().radius_lg)
                            .placeholder(t("shell.search.placeholder"))
                            .max_h(px(440.))
                            .items(items.iter().cloned())
                            .on_confirm(move |ix: IndexPath, window, cx| {
                                let target = targets.borrow().get(ix.row).cloned();
                                window.close_dialog(cx);
                                if let Some(t) = target {
                                    run_target(t, window, cx);
                                }
                            }),
                    )
                })
        });
    }
}

fn run_target(target: PaletteTarget, window: &mut Window, cx: &mut App) {
    match target {
        PaletteTarget::Open(id) => irori_ui::navigate(Route::Game(id), cx),
        PaletteTarget::Route(r) => irori_ui::navigate(r, cx),
        PaletteTarget::Refresh => irori_ui::models::refresh_library(false, cx),
        PaletteTarget::Random => irori_library::open_random_game(window, cx),
        PaletteTarget::SetUpEmulation => irori_manage::open_emulation_setup(window, cx),
        PaletteTarget::SteamFriends => irori_manage::open_steam_friends_dialog(window, cx),
        PaletteTarget::OpenStore(runner) => {
            irori_manage::open_store_window(runner, None, window, cx)
        }
        PaletteTarget::LutrisScript => irori_library::open_lutris_installer(None, window, cx),
        PaletteTarget::SteamClientLibrary => irori_manage::open_steam_library_dialog(window, cx),
        PaletteTarget::Play(id) => crate::shell::launch_game(id, window, cx),
        PaletteTarget::PlayStatus(id, status) => irori_ui::play_status::set(&id, status, cx),
        PaletteTarget::View(id) => irori_library::apply_view(id, cx),
        PaletteTarget::AddFolder => irori_library::open_add_folder_dialog(window, cx),
        PaletteTarget::SaveView => {
            irori_ui::navigate(Route::Library, cx);
            // After the palette's own dialog has closed.
            window.defer(cx, irori_library::open_save_view_dialog);
        }
        PaletteTarget::BackUpSaves(id) => irori_ui::save_backups::back_up_now(&id, window, cx),
        PaletteTarget::SaveBackups(id) => {
            irori_ui::save_backups::open_save_backups_sheet(id, window, cx)
        }
        PaletteTarget::Troubleshoot(id) => irori_game::open_troubleshooter(id, window, cx),
        PaletteTarget::SaveClip => irori_ui::backend(cx).save_clip(),
        PaletteTarget::InterfaceSize(step) => irori_ui::theme::change_interface_size(step, cx),
        PaletteTarget::SteamPlay(id) => irori_game::ops::open_steam_play(&id, cx),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(
        install: Option<InstallFilter>,
        favourites: Option<bool>,
        quick: bool,
    ) -> Vec<String> {
        let mut targets = Vec::new();
        let library = LibraryCommands {
            grouped: install.map(|_| false),
            grouped_by_mode: install.map(|_| true),
            grouped_by_genre: None,
            views: &[],
            layout: LibraryLayout::Grid,
            install,
            favourites,
            quick_controls: quick,
            hidden_pin: None,
        };
        Palette::items(&[], None, None, library, false, &[], false, &mut targets);
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
    fn the_hidden_games_pin_is_offered_only_while_unlocked() {
        let offered = |hidden_pin: Option<bool>| {
            let mut targets = Vec::new();
            let library = LibraryCommands {
                grouped: None,
                grouped_by_mode: None,
                grouped_by_genre: None,
                views: &[],
                layout: LibraryLayout::Grid,
                install: None,
                favourites: None,
                quick_controls: false,
                hidden_pin,
            };
            Palette::items(&[], None, None, library, false, &[], false, &mut targets);
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
}
