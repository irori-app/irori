//! Controller input for the whole application.
//!
//! A dedicated thread reads controllers through gilrs (SDL mappings for Xbox,
//! PlayStation including DualSense, Switch Pro and Steam Deck / Steam Input
//! pads), normalizes buttons and sticks into [`GamepadInput`]s with
//! repeat timing from the settings, and sends them over a channel to a GPUI
//! task. That task routes each input to the focused element of the active
//! window:
//!
//! 1. If the focused element (or an ancestor) handles the matching action
//!    from this module ([`Up`], [`Confirm`], …), the action is dispatched.
//!    Gamepad-first views such as Big Picture handle these directly.
//! 2. Otherwise the input is translated for ordinary screens: directions
//!    send arrow keys (so lists, menus and grids navigate as with a
//!    keyboard) and fall back to moving focus to the previous/next Tab stop;
//!    Confirm presses Enter (activating the focused control), Back presses
//!    Escape and falls back to navigating back, Alt sends Ctrl+Enter (play),
//!    Menu opens the command palette (Ctrl+K) and the shoulders page (Page
//!    Up and Down), else move to the previous/next Tab stop like Shift+Tab
//!    and Tab. A list or grid leaves an arrow at its edge unhandled, so the
//!    directions move on to the controls around it; the shoulders get out of
//!    one that keeps every arrow. Every screen is reachable through Menu
//!    (the palette lists them all) and left with Back. The keys typed this
//!    way count as keyboard input, so focus rings show.
//! 3. Guide toggles Big Picture from anywhere. With the in-game overlay on
//!    (`iroriOverlay`), Guide while a game Irori started runs opens the
//!    overlay instead, and every input goes to it while it is open.
//! 4. Guide + X (Square on PlayStation) saves a clip of the replay buffer,
//!    also while a game has the window. With the overlay on, Guide + RB takes
//!    a screenshot and Guide + Up/Down change the system's volume.
//!
//! Holding Start (or Select, `iroriUi.bigPictureHoldButton`) for
//! `iroriUi.bigPictureHoldMs` (a second unless changed) opens Big Picture
//! from anywhere in Irori's window while no game runs; a hint shows how long
//! is left ([`HoldHintView`]). That button's usual input (the palette) then
//! comes when it is let go, unless Big Picture opened.
//!
//! With `iroriUi.bigPictureOnController` on, a controller connecting while
//! Irori runs, or one there when it starts, brings Irori to the front in Big
//! Picture unless a game is running ([`ConnectWatch`] tells new controllers
//! from those a system hid while Irori was in the background).
//!
//! Input is ignored while `disable_controller` is set. When Irori is not the
//! active window only Guide is honoured, and only while no game is running:
//! it brings Irori to the front in Big Picture instead of acting on input
//! meant for a game (with the overlay on, it opens the overlay while a game
//! runs, whichever window is active). Guide acts on a short press, so
//! holding it to switch a controller off does nothing.
//!
//! Steam Input must not make a press count twice: controllers Steam lists in
//! `SDL_GAMECONTROLLER_IGNORE_DEVICES` (it drives them through a virtual
//! controller) are not read, and an arrow, Enter or Escape key press that
//! echoes a controller press just handled (Steam's desktop layout types
//! them) is dropped, as is a controller press echoing such a key.

// Only the Linux grab reads raw events; the tests run everywhere.
#[cfg(any(target_os = "linux", test))]
mod evdev;
pub(crate) mod grab;
mod hint;
mod input;
mod reader;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use futures::StreamExt as _;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, Entity, Global, KeyUpEvent, Keystroke,
    PlatformInput, Window, actions,
};
use irori_ui::Route;
use irori_ui::i18n::{t, t_args};
use irori_ui::nav::{self, navigator};
use parking_lot::RwLock;

pub use hint::HoldHintView;
pub use input::{
    Chord, ConnectWatch, ControllerKind, Deduper, EchoFilter, GamepadInput, GuideTap, HoldRelease,
    HoldTimer, InputSource, PadButton, PadIdentity, Repeater, SteamIgnoreList, StickAxis,
    hold_button, is_ignored_device, keyboard_equivalent, map_button,
};
use reader::{HoldEvent, Message, SharedConfig};

actions!(
    irori_gamepad,
    [
        /// Move up (D-pad, left stick, or the Up arrow in Big Picture).
        Up,
        /// Move down.
        Down,
        /// Move left.
        Left,
        /// Move right.
        Right,
        /// Activate the focused item (A / Cross).
        Confirm,
        /// Close the topmost layer or go back (B / Circle).
        Back,
        /// Secondary action of the focused item, e.g. play directly (X / Square).
        Alt,
        /// Open the context or quick menu (Y / Triangle / Start).
        Menu,
        /// Previous page (LB / L1).
        PrevPage,
        /// Next page (RB / R1).
        NextPage,
        /// Guide / PS / Home button.
        Guide
    ]
);

/// Charge state of a controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Battery {
    #[default]
    Unknown,
    Wired,
    /// Running on battery, level in percent.
    Discharging(u8),
    /// Charging, level in percent.
    Charging(u8),
    Full,
}

impl Battery {
    /// Short translated text such as "80%" or "Charging 40%"; `None` when
    /// unknown or wired.
    pub fn label(self) -> Option<String> {
        crate::register_catalogs();
        let text = match self {
            Battery::Discharging(level) => t_args(
                "bigpicture.battery.level",
                &[("level", level.to_string().as_str())],
            ),
            Battery::Charging(level) => t_args(
                "bigpicture.battery.charging",
                &[("level", level.to_string().as_str())],
            ),
            Battery::Full => t("bigpicture.battery.full"),
            Battery::Unknown | Battery::Wired => return None,
        };
        Some(text.to_string())
    }

    /// Whether the level is low enough to warn about.
    pub fn is_low(self) -> bool {
        matches!(self, Battery::Discharging(level) if level <= 20)
    }
}

/// A connected controller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ControllerInfo {
    /// Device name as reported by the driver or SDL mapping.
    pub name: String,
    pub kind: ControllerKind,
    pub battery: Battery,
    /// USB ids, when the system reports them.
    pub vendor: Option<u16>,
    pub product: Option<u16>,
}

impl ControllerInfo {
    /// Who this controller is, to tell a new one from one already there.
    pub fn identity(&self) -> PadIdentity {
        PadIdentity {
            vendor: self.vendor,
            product: self.product,
            name: self.name.clone(),
        }
    }
}

static CONNECTED: RwLock<Vec<ControllerInfo>> = RwLock::new(Vec::new());

pub(crate) fn set_connected(list: Vec<ControllerInfo>) {
    *CONNECTED.write() = list;
}

/// Names of the connected controllers, for the settings page. Empty until
/// [`install`] ran and a controller is plugged in.
pub fn connected_controllers() -> Vec<String> {
    CONNECTED.read().iter().map(|c| c.name.clone()).collect()
}

/// Details of the connected controllers.
pub fn controllers() -> Vec<ControllerInfo> {
    CONNECTED.read().clone()
}

/// Observable controller state: connected devices and the family of the
/// controller used last (for button glyphs). Owned by the gamepad global.
#[derive(Default)]
pub struct GamepadModel {
    controllers: Vec<ControllerInfo>,
    last_kind: Option<ControllerKind>,
    hold: Option<HoldHint>,
}

/// A button being held to open Big Picture: since when, and for how long
/// it must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldHint {
    pub since: Instant,
    pub duration: std::time::Duration,
}

impl GamepadModel {
    pub fn controllers(&self) -> &[ControllerInfo] {
        &self.controllers
    }

    /// Whether any controller is connected.
    pub fn is_connected(&self) -> bool {
        !self.controllers.is_empty()
    }

    /// Family of the controller used most recently, else of the first
    /// connected one; `None` without controllers.
    pub fn active_kind(&self) -> Option<ControllerKind> {
        if self.controllers.is_empty() {
            return None;
        }
        self.last_kind
            .or_else(|| self.controllers.first().map(|c| c.kind))
    }

    /// The hold under way that opens Big Picture, for its hint.
    pub fn hold(&self) -> Option<HoldHint> {
        self.hold
    }

    /// The controller to show in status indicators.
    pub fn primary(&self) -> Option<&ControllerInfo> {
        let kind = self.active_kind()?;
        self.controllers
            .iter()
            .find(|c| c.kind == kind)
            .or_else(|| self.controllers.first())
    }
}

struct GamepadGlobal {
    model: Entity<GamepadModel>,
    shared: Arc<SharedConfig>,
    /// The reader's channel, for the thread that reads the controllers
    /// while the overlay holds them.
    tx: futures::channel::mpsc::UnboundedSender<Message>,
}

impl Global for GamepadGlobal {}

/// The controller model, if [`install`] ran.
pub fn model(cx: &App) -> Option<Entity<GamepadModel>> {
    cx.try_global::<GamepadGlobal>().map(|g| g.model.clone())
}

/// Start controller support: the reader thread and the GPUI task that routes
/// its inputs. Call once after `irori_ui::init`; later calls do nothing.
/// Follows `disable_controller`, `gamepad_initial_repeat_delay` and
/// `gamepad_repeat_delay` live.
pub fn install(cx: &mut App) {
    if cx.has_global::<GamepadGlobal>() {
        return;
    }
    let settings = cx
        .try_global::<irori_ui::Models>()
        .map(|m| m.settings.clone());
    let shared = {
        let (disabled, initial, repeat, overlay) = settings
            .as_ref()
            .map(|s| {
                let app = &s.read(cx).app;
                (
                    app.disable_controller,
                    app.gamepad_initial_repeat_delay,
                    app.gamepad_repeat_delay,
                    overlay_shortcuts(app),
                )
            })
            .unwrap_or((false, 300, 50, false));
        let shared = SharedConfig::new(disabled, initial, repeat, overlay);
        if let Some(settings) = &settings {
            shared.set_hold(&settings.read(cx).app.ui);
        }
        Arc::new(shared)
    };
    if let Some(nav) = cx.try_global::<irori_ui::nav::NavigatorHandle>() {
        let nav = nav.0.clone();
        let shared = shared.clone();
        let showing = |nav: &irori_ui::nav::Navigator| nav.current() == &Route::BigPicture;
        shared
            .big_picture
            .store(showing(nav.read(cx)), Ordering::Relaxed);
        cx.observe(&nav, move |nav, cx| {
            shared
                .big_picture
                .store(showing(nav.read(cx)), Ordering::Relaxed);
        })
        .detach();
    }
    if let Some(settings) = settings {
        let shared = shared.clone();
        cx.observe(&settings, move |settings, cx| {
            let app = &settings.read(cx).app;
            shared.set_hold(&app.ui);
            shared
                .disabled
                .store(app.disable_controller, Ordering::Relaxed);
            shared
                .initial_ms
                .store(app.gamepad_initial_repeat_delay, Ordering::Relaxed);
            shared
                .repeat_ms
                .store(app.gamepad_repeat_delay, Ordering::Relaxed);
            shared
                .overlay
                .store(overlay_shortcuts(app), Ordering::Relaxed);
        })
        .detach();
    }

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let model = cx.new(|_| GamepadModel::default());
    cx.set_global(GamepadGlobal {
        model: model.clone(),
        shared: shared.clone(),
        tx: tx.clone(),
    });

    let thread_shared = shared.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("irori-gamepad".into())
        .spawn(move || reader::run(thread_shared, tx))
    {
        tracing::warn!("starting the controller reader: {e}");
        return;
    }
    let echoes = Rc::new(RefCell::new(EchoFilter::default()));
    {
        let echoes = echoes.clone();
        cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = &event.keystroke;
            if SYNTHESIZING.get() || keystroke.modifiers.modified() {
                return;
            }
            if let Some(input) = keyboard_equivalent(&keystroke.key)
                && !echoes
                    .borrow_mut()
                    .accept(InputSource::Keyboard, input, Instant::now())
            {
                cx.stop_propagation();
            }
        })
        .detach();
    }
    let watch = RefCell::new(ConnectWatch::default());
    cx.spawn(async move |cx| {
        while let Some(message) = rx.next().await {
            let disabled = shared.disabled.load(Ordering::Relaxed);
            cx.update(|cx| on_message(message, disabled, &watch, &model, &echoes, cx));
        }
    })
    .detach();
}

/// Whether the overlay's controller shortcuts are on.
fn overlay_shortcuts(app: &irori_core::settings::AppSettings) -> bool {
    crate::overlay::support::Support::current().available && app.overlay.enabled
}

/// Hold the controllers from the game while the overlay is open (see
/// `grab`); `None` when there is nothing to hold or this system can't.
pub(crate) fn start_grab(cx: &App) -> Option<grab::Grab> {
    let global = cx.try_global::<GamepadGlobal>()?;
    let devices = global.shared.devices.lock().clone();
    grab::Grab::start(devices, global.shared.clone(), global.tx.clone())
}

/// Whether a controller connecting opens Big Picture: when the user asked
/// for that and Irori isn't `busy` (see [`open_for_controller`]).
fn opens_big_picture(enabled: bool, connected: bool, busy: bool) -> bool {
    enabled && connected && !busy
}

/// Bring Irori to the front in Big Picture for a controller that just
/// connected (see [`ConnectWatch`]), when the user asked for that. Not while
/// a game runs, Big Picture or the welcome flow shows, or Irori sits in the
/// tray without a window.
fn open_for_controller(cx: &mut App) {
    let enabled = cx
        .try_global::<irori_ui::Models>()
        .is_some_and(|m| m.settings.read(cx).app.ui.big_picture_on_controller);
    let busy = game_running(cx)
        || cx.windows().is_empty()
        || !cx.has_global::<irori_ui::nav::NavigatorHandle>()
        || matches!(
            navigator(cx).read(cx).current(),
            Route::BigPicture | Route::Onboarding
        );
    if !opens_big_picture(enabled, true, busy) {
        return;
    }
    if let Some(handle) = cx.windows().into_iter().next() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
    nav::navigate(Route::BigPicture, cx);
}

thread_local! {
    /// Set while Irori types a key itself for a controller input, so the
    /// echo filter lets it through.
    static SYNTHESIZING: Cell<bool> = const { Cell::new(false) };
}

fn on_message(
    message: Message,
    disabled: bool,
    watch: &RefCell<ConnectWatch>,
    model: &Entity<GamepadModel>,
    echoes: &RefCell<EchoFilter>,
    cx: &mut App,
) {
    match message {
        Message::Controllers(list) => {
            let pads: Vec<PadIdentity> = list.iter().map(ControllerInfo::identity).collect();
            let active = cx.active_window().is_some();
            let connected = watch.borrow_mut().update(&pads, active, Instant::now());
            model.update(cx, |m, cx| {
                if list.is_empty() {
                    m.last_kind = None;
                }
                m.controllers = list;
                cx.notify();
            });
            if connected && !disabled {
                open_for_controller(cx);
            }
        }
        Message::Chord(chord) => on_chord(chord, cx),
        Message::Hold(event) => on_hold(event, disabled, model, cx),
        Message::Input { input, kind } => {
            if disabled
                || !echoes
                    .borrow_mut()
                    .accept(InputSource::Controller, input, Instant::now())
            {
                return;
            }
            model.update(cx, |m, cx| {
                if m.last_kind != Some(kind) {
                    m.last_kind = Some(kind);
                    cx.notify();
                }
            });
            if crate::overlay::input(input, cx) {
                return;
            }
            if input == GamepadInput::Guide && crate::overlay::wanted(cx) {
                crate::overlay::open(cx);
                return;
            }
            dispatch_input(input, cx);
        }
    }
}

/// Holding the button that opens Big Picture (see the module docs). It
/// counts only while Irori's window is active, outside Big Picture and with
/// no game running, as a press of Start in another program or a game is
/// none of Irori's business.
fn on_hold(event: HoldEvent, disabled: bool, model: &Entity<GamepadModel>, cx: &mut App) {
    let applies = !disabled
        && !game_running(cx)
        && cx.active_window().is_some()
        && cx.has_global::<irori_ui::nav::NavigatorHandle>()
        && navigator(cx).read(cx).current() != &Route::BigPicture;
    let hint = match event {
        HoldEvent::Started(duration) if applies => Some(HoldHint {
            since: Instant::now(),
            duration,
        }),
        _ => None,
    };
    model.update(cx, |m, cx| {
        if m.hold != hint {
            m.hold = hint;
            cx.notify();
        }
    });
    if event == HoldEvent::Completed && applies {
        nav::navigate(Route::BigPicture, cx);
    }
}

/// A button pressed with Guide held (see the module docs).
fn on_chord(chord: Chord, cx: &mut App) {
    let backend = irori_ui::backend(cx);
    match chord {
        Chord::Clip => {
            if backend.replay().is_some() || game_running(cx) {
                backend.save_clip();
            }
        }
        Chord::Screenshot => {
            let game = cx
                .try_global::<irori_ui::Models>()
                .and_then(|m| m.statuses.read(cx).playing().next().cloned());
            backend.take_screenshot(game);
        }
        Chord::VolumeUp => backend.step_system_volume(irori_services::audio::VOLUME_STEP),
        Chord::VolumeDown => backend.step_system_volume(-irori_services::audio::VOLUME_STEP),
    }
}

/// The action dispatched for an input when the focused element handles it.
pub fn action_for(input: GamepadInput) -> Box<dyn Action> {
    match input {
        GamepadInput::Up => Box::new(Up),
        GamepadInput::Down => Box::new(Down),
        GamepadInput::Left => Box::new(Left),
        GamepadInput::Right => Box::new(Right),
        GamepadInput::Confirm => Box::new(Confirm),
        GamepadInput::Back => Box::new(Back),
        GamepadInput::Alt => Box::new(Alt),
        GamepadInput::Menu => Box::new(Menu),
        GamepadInput::PrevPage => Box::new(PrevPage),
        GamepadInput::NextPage => Box::new(NextPage),
        GamepadInput::Guide => Box::new(Guide),
    }
}

fn game_running(cx: &App) -> bool {
    cx.try_global::<irori_ui::Models>()
        .is_some_and(|m| m.statuses.read(cx).playing().next().is_some())
}

fn active_window(cx: &mut App) -> Option<AnyWindowHandle> {
    if let Some(window) = cx.active_window() {
        return Some(window);
    }
    cx.windows().into_iter().find(|handle| {
        handle
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false)
    })
}

/// Route one input to the active window (see the module docs).
pub fn dispatch_input(input: GamepadInput, cx: &mut App) {
    match active_window(cx) {
        Some(handle) => {
            let _ = handle.update(cx, |_, window, cx| route_in_window(input, window, cx));
        }
        None if input == GamepadInput::Guide && !game_running(cx) => {
            if let Some(handle) = cx.windows().into_iter().next() {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                if navigator(cx).read(cx).current() != &Route::BigPicture {
                    nav::navigate(Route::BigPicture, cx);
                }
            }
        }
        None => {}
    }
}

/// Route one input inside `window`: the matching action when the focused
/// element handles it, otherwise the keyboard/focus translation.
pub fn route_in_window(input: GamepadInput, window: &mut Window, cx: &mut App) {
    let action = action_for(input);
    if window.is_action_available(action.as_ref(), cx) {
        window.dispatch_action(action, cx);
        return;
    }
    match input {
        GamepadInput::Guide => crate::toggle_big_picture(cx),
        GamepadInput::Up | GamepadInput::Left => {
            let key = if input == GamepadInput::Up {
                "up"
            } else {
                "left"
            };
            if !press(window, key, cx) {
                window.focus_prev(cx);
            }
        }
        GamepadInput::Down | GamepadInput::Right => {
            let key = if input == GamepadInput::Down {
                "down"
            } else {
                "right"
            };
            if !press(window, key, cx) {
                window.focus_next(cx);
            }
        }
        GamepadInput::Confirm => activate(window, cx),
        GamepadInput::Back => {
            if !press(window, "escape", cx) {
                nav::go_back(cx);
            }
        }
        GamepadInput::Alt => {
            press(window, "ctrl-enter", cx);
        }
        GamepadInput::Menu => {
            press(window, "ctrl-k", cx);
        }
        GamepadInput::PrevPage => {
            if !press(window, "pageup", cx) {
                window.focus_prev(cx);
            }
        }
        GamepadInput::NextPage => {
            if !press(window, "pagedown", cx) {
                window.focus_next(cx);
            }
        }
    }
}

/// Press a key as if typed; returns whether something handled it.
fn press(window: &mut Window, key: &str, cx: &mut App) -> bool {
    match Keystroke::parse(key) {
        Ok(keystroke) => synthesize(|| window.dispatch_keystroke(keystroke, cx)),
        Err(_) => false,
    }
}

/// Run `f`, which types keys for a controller input, past the echo filter.
fn synthesize<R>(f: impl FnOnce() -> R) -> R {
    let previous = SYNTHESIZING.replace(true);
    let result = f();
    SYNTHESIZING.set(previous);
    result
}

/// Press and release Enter, which activates the focused control (GPUI
/// clicks a focused element on the release of Enter after its press).
fn activate(window: &mut Window, cx: &mut App) {
    let Ok(keystroke) = Keystroke::parse("enter") else {
        return;
    };
    if synthesize(|| window.dispatch_keystroke(keystroke.clone(), cx)) {
        return;
    }
    window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        TestAppContext, Window, div,
    };

    use super::*;

    /// An ordinary screen: two Tab stops, no gamepad handlers.
    struct Plain {
        first: FocusHandle,
        second: FocusHandle,
    }

    impl Render for Plain {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(div().id("first").track_focus(&self.first).child("First"))
                .child(div().id("second").track_focus(&self.second).child("Second"))
        }
    }

    /// A gamepad-aware screen that counts `Down` actions.
    struct Aware {
        focus: FocusHandle,
        downs: usize,
    }

    impl Render for Aware {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &Down, _, cx| {
                    this.downs += 1;
                    cx.notify();
                }))
                .child("Aware")
        }
    }

    #[gpui_kit::test]
    fn directions_move_focus_on_ordinary_screens(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| Plain {
            first: cx.focus_handle().tab_stop(true),
            second: cx.focus_handle().tab_stop(true),
        });
        let focused = |cx: &mut TestAppContext, second: bool| {
            window
                .update(cx, |plain, window, _| {
                    let handle = if second { &plain.second } else { &plain.first };
                    handle.is_focused(window)
                })
                .unwrap()
        };
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::Down, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(focused(cx, false));
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::Right, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(focused(cx, true));
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::Up, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(focused(cx, false));
        // Nothing pages here: the shoulders move like Tab and Shift+Tab.
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::NextPage, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(focused(cx, true));
        let keyboard = cx
            .update_window(window.into(), |_, window, _| {
                window.last_input_was_keyboard()
            })
            .unwrap();
        assert!(keyboard, "focus rings show for controller moves");
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::PrevPage, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(focused(cx, false));
    }

    #[gpui_kit::test]
    fn gamepad_aware_views_receive_actions(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Aware { focus, downs: 0 }
        });
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            route_in_window(GamepadInput::Down, window, cx)
        })
        .unwrap();
        cx.run_until_parked();
        let downs = window.update(cx, |aware, _, _| aware.downs).unwrap();
        assert_eq!(downs, 1);
    }

    #[gpui_kit::test]
    fn holding_the_button_shows_its_hint_then_opens_big_picture(cx: &mut TestAppContext) {
        use std::time::Duration;
        cx.executor().allow_parking();
        let backend = crate::view::tests::start_backend();
        cx.update(|cx| {
            gpui_kit::init(cx);
            irori_ui::init(backend, cx);
            crate::register_catalogs();
        });
        let model = cx.update(|cx| {
            let model = cx.new(|_| GamepadModel::default());
            let (tx, _rx) = futures::channel::mpsc::unbounded();
            cx.set_global(GamepadGlobal {
                model: model.clone(),
                shared: Arc::new(SharedConfig::new(false, 300, 50, false)),
                tx,
            });
            model
        });
        let hint = cx.update(HoldHintView::new);
        let window = cx.add_window(|_, cx| Plain {
            first: cx.focus_handle().tab_stop(true),
            second: cx.focus_handle().tab_stop(true),
        });
        cx.update_window(window.into(), |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        let held = |cx: &mut TestAppContext| cx.read(|cx| model.read(cx).hold().is_some());
        let route =
            |cx: &mut TestAppContext| cx.read(|cx| navigator(cx).read(cx).current().clone());
        let second = Duration::from_secs(1);

        // Input turned off: no hint.
        cx.update(|cx| on_hold(HoldEvent::Started(second), true, &model, cx));
        assert!(!held(cx));

        cx.update(|cx| on_hold(HoldEvent::Started(second), false, &model, cx));
        cx.run_until_parked();
        assert!(held(cx), "the hint shows while the button is held");
        assert!(
            cx.read(|cx| hint.read(cx).hint.is_some()),
            "the view follows"
        );
        cx.update(|cx| on_hold(HoldEvent::Cancelled, false, &model, cx));
        assert!(!held(cx), "let go early");
        assert_ne!(route(cx), Route::BigPicture);

        cx.update(|cx| on_hold(HoldEvent::Started(second), false, &model, cx));
        cx.update(|cx| on_hold(HoldEvent::Completed, false, &model, cx));
        cx.run_until_parked();
        assert!(!held(cx));
        assert_eq!(route(cx), Route::BigPicture, "held long enough");

        // Inside Big Picture the button keeps its own meaning.
        cx.update(|cx| on_hold(HoldEvent::Started(second), false, &model, cx));
        assert!(!held(cx));
    }

    #[test]
    fn a_new_controller_opens_big_picture_only_when_asked() {
        use super::opens_big_picture;
        assert!(opens_big_picture(true, true, false));
        assert!(!opens_big_picture(false, true, false), "off");
        assert!(!opens_big_picture(true, false, false), "no new one");
        assert!(!opens_big_picture(true, true, true), "playing or open");
    }
}
