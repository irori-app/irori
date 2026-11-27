//! "Minimize when a game starts" with "Close to tray": the window closes to
//! the tray while the game plays, as closing it there does, and opens again
//! where it was once that game has ended.
//!
//! Closing to the tray drops the main window and its `AppShell`, so the
//! games the window was hidden for are kept here, beside the window and the
//! tray, and the app follows the "playing" statuses to bring it back. The
//! window is brought back only for a hide this module did: one the user
//! opened meanwhile (the tray, another launch of Irori) forgets it, and a
//! window already in the tray before the launch was never hidden here.

use std::sync::Arc;

use gpui_kit::{App, Global};
use irori_core::GameId;
use irori_ui::{Route, models::models, nav};
use parking_lot::Mutex;

use crate::AppHandles;

/// What starting a game does to the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnLaunch {
    /// Leave it as it is.
    Stay,
    /// Minimize it to the taskbar or dock.
    Minimize,
    /// Close it to the tray.
    HideToTray,
}

/// What a game starting does with "Minimize when a game starts" (`enabled`).
/// Big Picture stays fullscreen behind the game, so the compositor gives it
/// the focus back when the game exits; a window this already handled for a
/// game stays as it is. The tray is used when `tray_hide` says it can be
/// (see [`can_hide_to_tray`]), else the window minimizes as it always has.
pub fn on_launch(enabled: bool, big_picture: bool, already: bool, tray_hide: bool) -> OnLaunch {
    if !enabled || big_picture || already {
        OnLaunch::Stay
    } else if tray_hide {
        OnLaunch::HideToTray
    } else {
        OnLaunch::Minimize
    }
}

/// Whether the window may close to the tray for a game: the user keeps Irori
/// there on close, a tray icon is running to come back from, and this isn't
/// Steam's Game Mode, which shows no tray.
pub fn can_hide_to_tray(exit_to_tray: bool, tray_running: bool, steam_game_mode: bool) -> bool {
    exit_to_tray && tray_running && !steam_game_mode
}

/// Whether the window hidden for `hidden_for` should open again: none of
/// those games is still playing. Games started while it was hidden don't
/// keep it there; the user started them without the window.
pub fn should_restore<'a>(
    hidden_for: &[GameId],
    mut playing: impl Iterator<Item = &'a GameId>,
) -> bool {
    !hidden_for.is_empty() && !playing.any(|id| hidden_for.contains(id))
}

/// The window closed to the tray for these games, showing `route`.
#[derive(Debug, Clone)]
pub struct Hidden {
    pub games: Vec<GameId>,
    pub route: Route,
}

struct TrayHide(Arc<Mutex<AppHandles>>);

impl Global for TrayHide {}

/// Make hiding to the tray available to the shell and bring the window back
/// when the games it was hidden for end.
pub fn install(app: Arc<Mutex<AppHandles>>, cx: &mut App) {
    cx.set_global(TrayHide(app.clone()));
    let statuses = models(cx).statuses.clone();
    cx.observe(&statuses, move |statuses, cx| {
        let hidden = {
            let mut handles = app.lock();
            let restore = handles
                .hidden
                .as_ref()
                .is_some_and(|hidden| should_restore(&hidden.games, statuses.read(cx).playing()));
            if !restore {
                return;
            }
            handles.hidden.take()
        };
        let Some(hidden) = hidden else {
            return;
        };
        crate::open_main_window(&app, cx);
        nav::navigator(cx).update(cx, |n, cx| n.replace(hidden.route, cx));
    })
    .detach();
}

/// Whether a system tray icon is running and usable on this desktop.
pub fn tray_running(cx: &App) -> bool {
    let Some(TrayHide(app)) = cx.try_global::<TrayHide>() else {
        return false;
    };
    app.lock().tray.is_some() && !irori_core::platform::in_steam_gamepad_ui()
}

/// Whether a game starting may close the window to the tray now.
pub fn available(cx: &App) -> bool {
    let Some(TrayHide(app)) = cx.try_global::<TrayHide>() else {
        return false;
    };
    let tray_running = app.lock().tray.is_some();
    can_hide_to_tray(
        irori_ui::backend(cx).settings.app().exit_to_tray,
        tray_running,
        irori_core::platform::in_steam_gamepad_ui(),
    )
}

/// Note that the window is closing to the tray for `games`, to open again
/// on `route` when they have ended.
pub fn note(games: Vec<GameId>, route: Route, cx: &App) {
    if let Some(TrayHide(app)) = cx.try_global::<TrayHide>() {
        app.lock().hidden = Some(Hidden { games, route });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    #[test]
    fn launch_hides_to_the_tray_only_when_it_can() {
        assert_eq!(on_launch(true, false, false, true), OnLaunch::HideToTray);
        assert_eq!(on_launch(true, false, false, false), OnLaunch::Minimize);
        assert_eq!(on_launch(false, false, false, true), OnLaunch::Stay);
        assert_eq!(on_launch(true, false, true, true), OnLaunch::Stay);
        assert_eq!(on_launch(true, false, true, false), OnLaunch::Stay);
    }

    #[test]
    fn big_picture_is_never_hidden_or_minimized() {
        assert_eq!(on_launch(true, true, false, true), OnLaunch::Stay);
        assert_eq!(on_launch(true, true, false, false), OnLaunch::Stay);
    }

    #[test]
    fn the_tray_needs_close_to_tray_and_a_running_tray() {
        assert!(can_hide_to_tray(true, true, false));
        assert!(!can_hide_to_tray(false, true, false));
        assert!(!can_hide_to_tray(true, false, false));
        assert!(!can_hide_to_tray(true, true, true));
    }

    #[test]
    fn the_window_returns_once_every_game_it_hid_for_has_ended() {
        let a = GameId::new(Runner::Gog, "a");
        let b = GameId::new(Runner::Legendary, "b");
        let other = GameId::new(Runner::Steam, "other");

        assert!(!should_restore(std::slice::from_ref(&a), [&a].into_iter()));
        assert!(should_restore(std::slice::from_ref(&a), [].into_iter()));
        // A game started from the tray meanwhile doesn't hold it back.
        assert!(should_restore(
            std::slice::from_ref(&a),
            [&other].into_iter()
        ));
        let both = [a.clone(), b.clone()];
        assert!(!should_restore(&both, [&b].into_iter()));
        assert!(should_restore(&both, [&other].into_iter()));
        // Nothing hidden, nothing to bring back.
        assert!(!should_restore(&[], [].into_iter()));
    }
}
