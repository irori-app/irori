//! The in-game overlay (beta, `iroriOverlay.enabled`).
//!
//! With it on, the controller's Guide button over a game Irori started (or
//! Ctrl+Shift+O on Windows and macOS, `hotkey`) opens a panel above the
//! game instead of switching to Big Picture: the system's and the game's
//! volume, a screenshot, back to Irori and quitting the game on Home, then
//! the session's screenshots, the game's achievements and today's journal
//! note, one tab each (LB/RB switch them). Guide, B or
//! Escape closes it. While it is open the controllers are held from the game
//! where the system allows it (`gamepad::grab`, Linux), so its presses don't
//! reach the game, and a game that exits closes it. What each system offers
//! is in [`support`].
//!
//! It lives in Big Picture because Big Picture owns controller input; the
//! shell only names it for "Back to Irori" ([`install_show_irori`]). Nothing
//! of it exists while it is closed: no surface, no thread, no timer. Unlocks
//! the game made this session are fetched when it opens and announced in its
//! corner one after another. While it is closed, unlocks services report as
//! they happen show over the game in a card of their own ([`show_unlocks`],
//! `notice`), from the same queue, so each shows once.

#[cfg(any(target_os = "macos", windows))]
mod hotkey;
pub(crate) mod level;
pub(crate) mod model;
mod notice;
pub mod support;
mod view;
mod window;

#[cfg(any(target_os = "macos", windows))]
pub use hotkey::{GlobalShortcut, QUICK_LAUNCH_LABEL, register_quick_launch};
pub use notice::show_unlocks;
pub use window::take_foreground;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, App, AppContext as _, Entity, Global};
use irori_core::GameId;

use crate::gamepad::{GamepadInput, grab::Grab};
use model::Toasts;
use view::OverlayView;

/// Key context of the overlay's root element.
pub const OVERLAY_CONTEXT: &str = "IroriOverlay";

struct Open {
    window: AnyWindowHandle,
    view: Entity<OverlayView>,
    /// Dropped with the overlay, which lets go of the controllers.
    _grab: Option<Grab>,
    /// The window in front before the overlay took the foreground (Windows),
    /// given it back on close.
    previous: window::Foreground,
}

/// What "Back to Irori" runs.
type ShowIrori = Rc<dyn Fn(&mut App)>;

#[derive(Default)]
struct OverlayState {
    open: Option<Open>,
    show_irori: Option<ShowIrori>,
    /// Unlock notices, kept across openings so each shows once; the panel
    /// shows them while it is open, the card over the game otherwise.
    toasts: Toasts,
    /// The card over the game, while it shows.
    notice: Option<notice::Shown>,
    /// Whether the timer putting notices away runs.
    toasts_running: bool,
    /// Counts the notices put away, so the timer leaves one a click
    /// brought up its whole time.
    toast_turn: u64,
    /// When each game's achievements were last asked of its store afresh.
    refreshed: HashMap<String, Instant>,
}

/// How long the store's answer about a game's achievements stands before
/// opening the overlay asks again.
const REFRESH_AFTER: Duration = Duration::from_secs(60);

impl Global for OverlayState {}

fn state(cx: &mut App) -> &mut OverlayState {
    cx.default_global::<OverlayState>()
}

/// What "Back to Irori" does: the shell shows its main window (opening it
/// again when it was closed to the tray). Call once at startup.
pub fn install_show_irori(show: impl Fn(&mut App) + 'static, cx: &mut App) {
    state(cx).show_irori = Some(Rc::new(show));
}

/// Start the overlay's keyboard shortcut where the system has one (see
/// `hotkey`). Call once after `irori_ui::init`.
pub fn install(cx: &mut App) {
    #[cfg(any(target_os = "macos", windows))]
    hotkey::install(cx);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = cx;
}

/// Whether the overlay is turned on and this system has it.
pub fn enabled(cx: &App) -> bool {
    support::Support::current().available
        && cx
            .try_global::<irori_ui::Models>()
            .is_some_and(|m| m.settings.read(cx).app.overlay.enabled)
}

/// The overlay's settings (defaults before the models exist).
pub(crate) fn prefs(cx: &App) -> irori_core::settings::OverlayPrefs {
    cx.try_global::<irori_ui::Models>()
        .map(|m| m.settings.read(cx).app.overlay.clone())
        .unwrap_or_default()
}

/// How far one press of a volume control moves a volume, in percent
/// (`iroriOverlay.volumeStep`).
pub(crate) fn volume_step(cx: &App) -> i32 {
    prefs(cx).volume_step()
}

/// The game the overlay is for: the one Irori started last that runs.
pub(crate) fn running_game(cx: &App) -> Option<GameId> {
    let models = cx.try_global::<irori_ui::Models>()?;
    let backend = irori_ui::backend(cx);
    models
        .statuses
        .read(cx)
        .playing()
        .min_by_key(|id| backend.playing_for(id).unwrap_or_default())
        .cloned()
}

/// Whether Guide opens the overlay now: it is on and a game runs.
pub(crate) fn wanted(cx: &App) -> bool {
    enabled(cx) && running_game(cx).is_some()
}

/// Whether the overlay is open.
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<OverlayState>()
        .is_some_and(|s| s.open.is_some())
}

/// Open the overlay over the running game. Does nothing without one, or
/// when it is open already.
pub fn open(cx: &mut App) {
    if is_open(cx) {
        return;
    }
    let Some(game) = running_game(cx) else {
        return;
    };
    if window::covered_by_exclusive_fullscreen() {
        // Nothing can show above it, and a window coming up could take the
        // game out of fullscreen: say so instead, for after the game.
        tracing::info!("the in-game overlay can't cover a game in exclusive fullscreen");
        irori_ui::backend(cx).bus.notice(
            irori_core::Notice::new(
                irori_core::NoticeLevel::Warning,
                "The overlay can't show over this game",
            )
            .message("It runs in exclusive fullscreen; switch it to borderless or windowed.")
            .keys(
                "shell.notice.overlay_fullscreen",
                Some("shell.notice.overlay_fullscreen_message"),
                &[],
            )
            .game(game),
        );
        return;
    }
    let previous = window::Foreground::current();
    let opened = window::open(cx, |window, cx| {
        let game = game.clone();
        cx.new(|cx| OverlayView::new(game, window, cx))
    });
    match opened {
        Ok((window, view)) => {
            window::take_foreground(window, cx);
            let grab = crate::gamepad::start_grab(cx);
            state(cx).open = Some(Open {
                window,
                view,
                _grab: grab,
                previous,
            });
            // The panel shows the notices in its own corner.
            if let Some(shown) = state(cx).notice.take() {
                notice::close(shown, cx);
            }
        }
        Err(e) => tracing::warn!("opening the in-game overlay: {e:#}"),
    }
}

/// Close the overlay. Deferred, since it is often asked from inside the
/// overlay's own window, which can't be removed from within its update.
pub fn close(cx: &mut App) {
    cx.defer(|cx| {
        if let Some(open) = state(cx).open.take() {
            window::close(open.window, cx);
            // Notices still waiting go on over the game.
            notice::resume(cx);
            open.previous.give_back();
        }
    });
}

/// Close the overlay and show Irori's window.
/// Mark this moment of `game`: after the panel closed (`delay`, so the
/// screenshot shows the game), a screenshot and a journal note to write
/// later, then a notice saying so.
pub(crate) fn mark_moment(game: irori_core::GameId, delay: bool, cx: &App) {
    use irori_core::{Notice, NoticeLevel};
    let backend = irori_ui::backend(cx);
    let b = backend.clone();
    backend.spawn(async move {
        if delay {
            tokio::time::sleep(view::SCREENSHOT_DELAY).await;
        }
        let marked = b.mark_moment(&game).await;
        let title = marked.title.as_str();
        let notice = match &marked.capture {
            Ok(_) => Notice::new(NoticeLevel::Success, format!("Moment of {title} marked")).keys(
                "bigpicture.overlay.moment_marked",
                Some("bigpicture.overlay.moment_marked_message"),
                &[("title", title)],
            ),
            Err(e) => Notice::new(
                NoticeLevel::Warning,
                format!("Moment of {title} marked without a screenshot"),
            )
            .message(e.clone())
            .keys(
                "bigpicture.overlay.moment_no_screenshot",
                None,
                &[("title", title)],
            ),
        };
        b.bus.notice(notice.game(game));
    });
}

pub(crate) fn back_to_irori(cx: &mut App) {
    close(cx);
    let show = state(cx).show_irori.clone();
    cx.defer(move |cx| {
        if let Some(show) = show {
            show(cx);
        }
    });
}

/// Whether to ask the store afresh about the achievements of the game
/// `key` (and note that it was asked now).
pub(crate) fn should_refresh(key: &str, cx: &mut App) -> bool {
    let refreshed = &mut state(cx).refreshed;
    let now = Instant::now();
    if refreshed
        .get(key)
        .is_some_and(|at| now.duration_since(*at) < REFRESH_AFTER)
    {
        return false;
    }
    refreshed.insert(key.to_string(), now);
    true
}

/// The unlock notices, for the views.
pub(crate) fn toasts(cx: &mut App) -> &mut Toasts {
    &mut state(cx).toasts
}

/// Show the queued notices one after another, each for its time, and take
/// the card over the game away after the last. Does nothing while they are
/// being shown already.
pub(crate) fn start_toasts(cx: &mut App) {
    refresh_toasts(cx);
    let s = state(cx);
    if s.toasts_running || s.toasts.current().is_none() {
        return;
    }
    s.toasts_running = true;
    cx.spawn(async move |cx| {
        loop {
            let turn = cx.update(|cx| state(cx).toast_turn);
            cx.background_executor().timer(notice::TOAST_TIME).await;
            let more = cx.update(|cx| {
                if state(cx).toast_turn == turn {
                    advance_toast(cx)
                } else {
                    // A click put that one away: the next gets its time.
                    state(cx).toasts.current().is_some()
                }
            });
            if !more {
                cx.update(|cx| state(cx).toasts_running = false);
                break;
            }
        }
    })
    .detach();
}

/// Put the notice shown away now (a click on it). Deferred, since the card
/// it may close is the window asking.
pub(crate) fn next_toast(cx: &mut App) {
    cx.defer(|cx| {
        advance_toast(cx);
    });
}

/// Done with the notice shown; returns whether another follows.
fn advance_toast(cx: &mut App) -> bool {
    let s = state(cx);
    s.toast_turn += 1;
    let more = s.toasts.advance();
    if !more && let Some(shown) = s.notice.take() {
        notice::close(shown, cx);
    }
    refresh_toasts(cx);
    more
}

/// Draw the views that show the notices again.
fn refresh_toasts(cx: &mut App) {
    let s = state(cx);
    let panel = s.open.as_ref().map(|o| o.view.clone());
    let card = s.notice.as_ref().map(|n| n.view.clone());
    if let Some(view) = panel {
        view.update(cx, |_, cx| cx.notify());
    }
    if let Some(view) = card {
        view.update(cx, |_, cx| cx.notify());
    }
}

/// Hand `input` to the overlay's panel, or to the dialog open over it (the
/// journal editor) as the keys it stands for. The panel stops handling
/// actions while a dialog is open, so the keys never come back to it.
fn deliver(
    input: GamepadInput,
    view: &Entity<OverlayView>,
    window: &mut gpui_kit::Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    if !window.has_active_dialog(cx) {
        view.update(cx, |view, cx| view.handle(input, window, cx));
    } else if input == GamepadInput::Guide {
        window.close_all_dialogs(cx);
        close(cx);
    } else {
        crate::gamepad::route_in_window(input, window, cx);
    }
}

/// Give one controller input to the open overlay. Returns false when it
/// isn't open (any more), so the input goes where it would otherwise.
pub(crate) fn input(input: GamepadInput, cx: &mut App) -> bool {
    let Some((window, view)) = cx
        .try_global::<OverlayState>()
        .and_then(|s| s.open.as_ref())
        .map(|o| (o.window, o.view.clone()))
    else {
        return false;
    };
    let handled = window.update(cx, |_, window, cx| deliver(input, &view, window, cx));
    if handled.is_err() {
        // The surface went away under us (the compositor closed it).
        state(cx).open = None;
        return false;
    }
    true
}
