//! "Back to the game": bring a running game's window to the front
//! (`Backend::focus_game_window`), and say so when the desktop can't or the
//! game has no window yet. The game page, the library, Big Picture and the
//! command palette all call this one function.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Window};
use irori_core::GameId;
use irori_services::game_window::Raised;

use crate::i18n::t;

/// Bring the running game `id`'s window to the front, telling the user in
/// `window` when that can't be done.
pub fn back_to_game(id: GameId, window: &mut Window, cx: &mut App) {
    let b = crate::backend(cx);
    let task = crate::run(cx, async move { b.focus_game_window(&id).await });
    window
        .spawn(cx, async move |cx| {
            let note = match task.await {
                Ok(Raised::Raised) => return,
                Ok(Raised::NoWindow) => Notification::info(t("ui.game_window.no_window")),
                Ok(Raised::Unsupported) => Notification::info(t("ui.game_window.unsupported")),
                Err(e) => Notification::error(format!("{e:#}")).title(t("ui.game_window.failed")),
            };
            let _ = cx.update(|window, cx| window.push_notification(note, cx));
        })
        .detach();
}
