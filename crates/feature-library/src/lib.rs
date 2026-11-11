//! The library screen: every game of every store in a virtualized cover grid
//! (a click opens a drawer about the game beside it), a list, or Focus (a
//! column of covers beside the selected game's page), with instant search, store switcher, filters, sorting, a
//! "Continue playing" row, keyboard navigation, per-game context menus,
//! categories (manual and smart), saved views, manually added apps and
//! per-game title/art overrides.
//!
//! # Wiring (shell)
//!
//! 1. Call [`init`] once at startup (binds the library's keys and registers
//!    its translations) and
//!    [`install_sideload_store`] with the same `Arc<SideloadStore>` handed to the
//!    backend, so the "Add game" and "Edit app" dialogs can reach it.
//! 2. Create the view with [`LibraryView::new`] and render it for
//!    [`irori_ui::Route::Library`].
//! 3. Subscribe to [`LibraryEvent`]s: the library asks the game feature to open
//!    its install sheet, uninstall confirmation and log viewer, which it does
//!    not own.
//!
//! Other features can open the library's dialogs directly:
//! [`open_add_game_dialog`], [`open_edit_game_dialog`] (a game page's
//! "Edit…") and [`open_categories_dialog`] (`Some(id)` for a game page's
//! "Categories…", `None` for the category manager).

mod availability;
mod categories;
mod commands;
mod dialogs;
mod filter;
mod filters_panel;
mod format;
mod grid;
mod i18n;
mod launch;
mod layouts;
mod quick;
mod random;
mod selection;
mod stats;
mod top;
mod view;
mod views;

use std::sync::Arc;

use gpui_kit::{App, Global, KeyBinding, Window};
use irori_core::GameId;
use irori_sideload::SideloadStore;

pub use commands::{GameCommand, GameState};
pub use dialogs::categories::open_categories_dialog;
pub use dialogs::edit_game::open_edit_game_dialog;
pub use dialogs::folder::open_add_folder_dialog;
pub use dialogs::lutris::open_lutris_installer;
pub use dialogs::sideload::open_add_game_dialog;
pub use dialogs::smart::open_smart_category_editor;
pub use dialogs::steamgrid::open_art_picker_dialog;
pub use dialogs::views::{open_manage_views_dialog, open_save_view_dialog};
pub use filter::{CategoryFilter, GroupKind, Grouping, HiddenMode, InstallFilter, ViewFilter};
pub use filters_panel::active_filters;
pub use i18n::CATALOGS;
pub use random::open_random_game;
pub use stats::StatsView;
pub use stats::year_view::{YEAR_REVIEW_CONTEXT, YearReviewView};
pub use view::LibraryView;
pub use view::remove::{removable_picks, remove_picked_from_library};
pub use views::{BuiltinView, active_view, apply_view, list as saved_view_list, saved_views};

/// Requests for actions owned by other features. The shell routes them (for
/// example to the game feature's install sheet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryEvent {
    /// Open the install sheet for a game.
    RequestInstall(GameId),
    /// Open the uninstall confirmation for a game (sideloaded apps included).
    RequestUninstall(GameId),
    /// Open the log viewer of a game.
    RequestLogs(GameId),
    /// Choose where an added game's program went.
    RequestLocate(GameId),
    /// Pick a folder of GOG offline installers and install from it.
    RequestImportInstallers,
}

gpui_kit::actions!(
    library,
    [
        /// Move keyboard focus to the search field.
        FocusSearch,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectFirst,
        SelectLast,
        SelectPageUp,
        SelectPageDown,
        /// Open the focused game's page.
        OpenSelected,
        /// Run the focused game's primary action (Play, Stop or Install).
        ActivateSelected,
        /// Pick every shown game for bulk actions.
        SelectAllGames,
        /// Remove the picked games (or the focused one) from the library,
        /// after asking: added games and ROMs only.
        RemovePicked,
        /// Leave selection mode.
        EndSelection,
        /// Turn grouping the library by play status on or off.
        ToggleStatusGroups,
        /// Turn grouping the library by game mode on or off.
        ToggleModeGroups,
        /// Show the library as a cover grid.
        ShowGrid,
        /// Show the library as a list.
        ShowList,
        /// Show the library as a column of covers beside the selected game.
        ShowFocus,
        /// Show the library the next way (grid, list, Focus).
        NextLibraryView,
        /// Open the grid's drawer on the focused game, or close it.
        ToggleDrawer,
    ]
);

/// Key context of the whole library screen.
pub(crate) const LIBRARY_CONTEXT: &str = "Library";
/// Key context of the focused cover grid or list.
pub(crate) const GRID_CONTEXT: &str = "LibraryGrid";

struct KeysBound;
impl Global for KeysBound {}

/// Register the library's translations and bind its keys. Idempotent;
/// [`LibraryView::new`] calls it too.
pub fn init(cx: &mut App) {
    i18n::register();
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    let grid = Some(GRID_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-f", FocusSearch, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("ctrl-g", ToggleStatusGroups, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("ctrl-alt-g", ToggleModeGroups, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("alt-1", ShowGrid, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("alt-2", ShowList, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("alt-3", ShowFocus, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("ctrl-shift-v", NextLibraryView, Some(LIBRARY_CONTEXT)),
        KeyBinding::new("/", FocusSearch, grid),
        KeyBinding::new("left", SelectLeft, grid),
        KeyBinding::new("right", SelectRight, grid),
        KeyBinding::new("up", SelectUp, grid),
        KeyBinding::new("down", SelectDown, grid),
        KeyBinding::new("home", SelectFirst, grid),
        KeyBinding::new("end", SelectLast, grid),
        KeyBinding::new("pageup", SelectPageUp, grid),
        KeyBinding::new("pagedown", SelectPageDown, grid),
        KeyBinding::new("enter", OpenSelected, grid),
        KeyBinding::new("i", ToggleDrawer, grid),
        KeyBinding::new("space", ActivateSelected, grid),
        KeyBinding::new("ctrl-enter", ActivateSelected, grid),
        KeyBinding::new("ctrl-a", SelectAllGames, grid),
        KeyBinding::new("delete", RemovePicked, grid),
        KeyBinding::new("escape", EndSelection, grid),
    ]);
}

struct SideloadHandle(Arc<SideloadStore>);
impl Global for SideloadHandle {}

/// Give the library access to the sideload store (the backend only exposes
/// stores as `dyn Store`). Without it, adding and editing apps reports that
/// the feature is unavailable.
pub fn install_sideload_store(store: Arc<SideloadStore>, cx: &mut App) {
    cx.set_global(SideloadHandle(store));
}

pub(crate) fn sideload_store(cx: &App) -> Option<Arc<SideloadStore>> {
    cx.try_global::<SideloadHandle>().map(|h| h.0.clone())
}

struct EmulationHandle(Arc<irori_emulation::EmulationStore>);
impl Global for EmulationHandle {}

/// Give the library access to the ROM library store, so Add game can put a
/// ROM there.
pub fn install_emulation_store(store: Arc<irori_emulation::EmulationStore>, cx: &mut App) {
    cx.set_global(EmulationHandle(store));
}

pub(crate) fn emulation_store(cx: &App) -> Option<Arc<irori_emulation::EmulationStore>> {
    cx.try_global::<EmulationHandle>().map(|h| h.0.clone())
}

/// Whether the library groups its games by play status; `None` until a
/// library view exists.
pub fn groups_by_status(cx: &App) -> Option<bool> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().group == Grouping::Status)
}

/// Group the library by play status, or stop. Does nothing until a library
/// view exists.
pub fn set_groups_by_status(on: bool, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        let group = if on { Grouping::Status } else { Grouping::None };
        model.update(cx, |m, cx| m.update(|f| f.group = group, cx));
    }
}

/// Which games the library shows by install state; `None` until a library
/// view exists.
pub fn install_filter(cx: &App) -> Option<InstallFilter> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().install)
}

/// Show all games, only installed or only not installed ones, as the
/// header's quick controls and the Filters popover do. Does nothing until a
/// library view exists.
pub fn set_install_filter(install: InstallFilter, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        model.update(cx, |m, cx| m.update(|f| f.install = install, cx));
    }
}

/// Whether the library shows only favourites; `None` until a library view
/// exists.
pub fn favourites_only(cx: &App) -> Option<bool> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().favourites_only)
}

/// Show only favourites, or every game. Does nothing until a library view
/// exists.
pub fn set_favourites_only(on: bool, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        model.update(cx, |m, cx| m.update(|f| f.favourites_only = on, cx));
    }
}

/// Whether the library's header shows its quick sort and filter controls
/// (`iroriUi.libraryQuickControls`).
pub fn quick_controls(cx: &App) -> bool {
    irori_ui::models(cx)
        .settings
        .read(cx)
        .app
        .ui
        .library_quick_controls
}

/// Show the header's quick controls, or stop. An open library view follows
/// the setting.
pub fn set_quick_controls(on: bool, cx: &mut App) {
    let backend = irori_ui::backend(cx);
    irori_ui::state::run_infallible(cx, async move {
        backend
            .settings
            .update_app(|s| s.ui.library_quick_controls = on)
    })
    .detach();
}

/// Whether the library groups its games by game mode; `None` until a
/// library view exists.
pub fn groups_by_mode(cx: &App) -> Option<bool> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().group == Grouping::Mode)
}

/// Group the library by game mode, or stop. Does nothing until a library
/// view exists.
pub fn set_groups_by_mode(on: bool, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        let group = if on { Grouping::Mode } else { Grouping::None };
        model.update(cx, |m, cx| m.update(|f| f.group = group, cx));
    }
}

/// Whether the library groups its games by genre; `None` until a library
/// view exists.
pub fn groups_by_genre(cx: &App) -> Option<bool> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().group == Grouping::Genre)
}

/// Group the library by genre, or stop. Does nothing until a library view
/// exists.
pub fn set_groups_by_genre(on: bool, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        let group = if on { Grouping::Genre } else { Grouping::None };
        model.update(cx, |m, cx| m.update(|f| f.group = group, cx));
    }
}

/// Whether the library groups its games by category; `None` until a
/// library view exists.
pub fn groups_by_category(cx: &App) -> Option<bool> {
    let model = filters_panel::active_filters(cx)?;
    Some(model.read(cx).filter().group == Grouping::Category)
}

/// Group the library by category, or stop. Does nothing until a library
/// view exists.
pub fn set_groups_by_category(on: bool, cx: &mut App) {
    if let Some(model) = filters_panel::active_filters(cx) {
        let group = if on {
            Grouping::Category
        } else {
            Grouping::None
        };
        model.update(cx, |m, cx| m.update(|f| f.group = group, cx));
    }
}

/// Every category of both kinds with its count, pinned first (the command
/// palette's "Show category" rows).
pub fn category_list(cx: &App) -> Vec<irori_ui::categories::CategoryEntry> {
    let library = irori_ui::models(cx).library.read(cx);
    irori_ui::categories::entries(&library.user, &library.categories)
}

/// Show the library's games in `category` alone, as a palette row or a
/// menu does: the Filters popover's criteria give way to it, while the
/// search, store, sort and grouping stay.
pub fn show_category(category: irori_services::CategoryRef, cx: &mut App) {
    filters_panel::change_filters(move |f| f.show_only(&category), cx);
    irori_ui::navigate(irori_ui::Route::Library, cx);
}

/// Show the library's games played in `mode` (a game page's mode chip):
/// the Filters popover's criteria give way to that mode alone, while the
/// search, store, sort and grouping stay. Without a library view yet, the
/// view the navigation creates applies it ([`filters_panel::change_filters`]).
pub fn show_game_mode(mode: irori_core::GameMode, cx: &mut App) {
    filters_panel::change_filters(
        move |f| {
            f.reset_filters();
            f.modes = [mode].into();
        },
        cx,
    );
    irori_ui::navigate(irori_ui::Route::Library, cx);
}

/// How the library shows games: grid, list or Focus (`iroriUi.libraryLayout`).
pub fn library_layout(cx: &App) -> irori_core::settings::LibraryLayout {
    irori_ui::models(cx).settings.read(cx).app.ui.library_layout
}

/// Show the library as `layout` from now on. An open library view follows
/// the setting.
pub fn set_library_layout(layout: irori_core::settings::LibraryLayout, cx: &mut App) {
    let backend = irori_ui::backend(cx);
    irori_ui::state::run_infallible(cx, async move {
        backend
            .settings
            .update_app(|s| s.ui.library_layout = layout)
    })
    .detach();
}

/// Show an error notification in `window`.
pub(crate) fn notify_error(
    title: impl Into<gpui_kit::SharedString>,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::notification::Notification;
    window.push_notification(
        Notification::error(format!("{error:#}")).title(title.into()),
        cx,
    );
}

#[cfg(test)]
mod ui_tests;
