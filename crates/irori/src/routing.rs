//! Cross-feature requests: a feature asks for an action owned by another
//! feature (the library asks the game feature to install), and the shell
//! routes it.

use gpui_kit::{AnyView, App, AppContext as _, Entity, Window};
use irori_core::GameId;
use irori_game::{GamePage, GamePageEvent};
use irori_library::LibraryEvent;
use irori_ui::hooks::{CloseFn, GamePanel};

pub fn library_event(event: &LibraryEvent, window: &mut Window, cx: &mut App) {
    match event {
        LibraryEvent::RequestInstall(id) => irori_game::open_install_sheet(id.clone(), window, cx),
        LibraryEvent::RequestUninstall(id) => {
            irori_game::open_uninstall_dialog(id.clone(), window, cx)
        }
        LibraryEvent::RequestLogs(id) => irori_game::open_log_sheet(id.clone(), window, cx),
        LibraryEvent::RequestLocate(id) => irori_game::ops::locate_program(id, window, cx),
        LibraryEvent::RequestImportInstallers => {
            irori_game::open_import_installers_dialog(window, cx)
        }
    }
}

pub fn game_event(
    page: &Entity<GamePage>,
    event: &GamePageEvent,
    window: &mut Window,
    cx: &mut App,
) {
    match event {
        GamePageEvent::TitleChanged(_) => {}
        GamePageEvent::RequestEdit(id) => irori_library::open_edit_game_dialog(id, window, cx),
        GamePageEvent::RequestCategories => {
            let id = page.read(cx).game_id().clone();
            irori_library::open_categories_dialog(Some(&id), window, cx);
        }
    }
}

/// The game page shaped for the library's Focus view or grid drawer, with its
/// requests routed like the page's own.
pub fn game_panel(id: GameId, panel: GamePanel, window: &mut Window, cx: &mut App) -> AnyView {
    let page = cx.new(|cx| GamePage::embedded(id, panel, window, cx));
    // Dropped with the page.
    window
        .subscribe(&page, cx, |page, event, window, cx| {
            game_event(&page, event, window, cx)
        })
        .detach();
    page.into()
}

/// A game's settings shown in place by another screen (Big Picture), closed
/// with `on_close`.
pub fn game_settings(id: GameId, on_close: CloseFn, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| irori_settings::GameSettingsView::embedded(id, on_close, window, cx))
        .into()
}
