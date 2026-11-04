//! Command palette (Ctrl+K): jump to any game, play installed games, pick a
//! random one, set the play status of the game whose page is open, group the
//! library by play status, show or save a library view, switch its view
//! (grid, list, Focus), and run window commands, all from the keyboard.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _};
use gpui_kit::{App, AppContext as _, Entity, ParentElement as _, Styled as _, Window, px};
use irori_core::settings::LibraryLayout;
use irori_core::{GameId, GameInfo};
use irori_services::PlayStatus;
use irori_ui::i18n::{t, t_args};
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
    LutrisScript,
    /// Set (or with `None` clear) a game's play status.
    PlayStatus(GameId, Option<PlayStatus>),
    /// Group the library by play status (`true`) or stop, and show it.
    GroupByStatus(bool),
    /// Show a saved library view.
    View(u64),
    /// Save the library's current criteria as a view.
    SaveView,
    /// Pick a folder and add the games in it.
    AddFolder,
    /// Show the library as a grid, a list or in Focus view, and show it.
    LibraryView(LibraryLayout),
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

    /// Rows for `games`. `open` is the game whose page is shown, with its
    /// play status: its status commands come first. `grouped` is whether the
    /// library groups by play status, while a library view exists. `layout`
    /// is how the library shows games; the other views are offered.
    fn items(
        games: &[Arc<GameInfo>],
        open: Option<(Arc<GameInfo>, Option<PlayStatus>)>,
        grouped: Option<bool>,
        views: &[(u64, gpui_kit::SharedString)],
        layout: LibraryLayout,
        targets: &mut Vec<PaletteTarget>,
    ) -> Vec<CommandItem> {
        let mut items = Vec::with_capacity(games.len() * 2 + 16);
        let mut push =
            |item: CommandItem, target: PaletteTarget, targets: &mut Vec<PaletteTarget>| {
                items.push(item);
                targets.push(target);
            };
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
            (library.snapshot.games.clone(), open)
        };
        let mut targets = Vec::new();
        let grouped = irori_library::groups_by_status(cx);
        let views = irori_library::saved_view_list(cx);
        let layout = irori_library::library_layout(cx);
        let items = Rc::new(Self::items(
            &games,
            open,
            grouped,
            &views,
            layout,
            &mut targets,
        ));
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
        PaletteTarget::LutrisScript => irori_library::open_lutris_installer(None, window, cx),
        PaletteTarget::Play(id) => crate::shell::launch_game(id, window, cx),
        PaletteTarget::PlayStatus(id, status) => irori_ui::play_status::set(&id, status, cx),
        PaletteTarget::View(id) => irori_library::apply_view(id, cx),
        PaletteTarget::AddFolder => irori_library::open_add_folder_dialog(window, cx),
        PaletteTarget::SaveView => {
            irori_ui::navigate(Route::Library, cx);
            // After the palette's own dialog has closed.
            window.defer(cx, irori_library::open_save_view_dialog);
        }
        PaletteTarget::GroupByStatus(on) => {
            irori_library::set_groups_by_status(on, cx);
            irori_ui::navigate(Route::Library, cx);
        }
        PaletteTarget::LibraryView(layout) => {
            irori_library::set_library_layout(layout, cx);
            irori_ui::navigate(Route::Library, cx);
        }
    }
}
