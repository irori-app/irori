//! Hiding the mouse pointer while a controller is in use.
//!
//! A pointer left over a cover or a row sits in the middle of a screen the
//! user is steering from the sofa, and the item under it keeps its hover
//! look. So a controller input hides the pointer until the mouse moves, as
//! typing does in text editors.
//!
//! GPUI offers no public call for that. It hides the pointer itself when a
//! key binding's action is handled after keyboard input
//! (`CursorHideMode::OnTypingAndAction`, its default), and shows it again on
//! the next mouse movement. A controller input therefore counts as keyboard
//! input first (an unbound key press, which also makes GPUI drop hover looks
//! and show focus rings, as for the keys the controller types on ordinary
//! screens), and then an action of this module is dispatched that a global
//! handler takes. The platform does the rest: macOS hides the cursor until
//! the mouse moves, Windows and Linux likewise where their GPUI backends
//! can.

use gpui_kit::{App, KeyDownEvent, Keystroke, PlatformInput, Window, actions};

actions!(
    irori_gamepad_pointer,
    [
        /// Taken by a global handler, so GPUI hides the pointer (see the
        /// module docs). Never bound to a key.
        HidePointer
    ]
);

/// A key no binding of Irori's uses and that types nothing, pressed to make
/// GPUI count a controller input as keyboard input.
const INERT_KEY: &str = "f24";

/// Install the handler that takes [`HidePointer`]. Call once.
pub(crate) fn install(cx: &mut App) {
    cx.on_action(|_: &HidePointer, _| {});
}

/// Hide the pointer over `window` until the mouse moves, after a controller
/// input. Does nothing while `enabled` is off.
pub(crate) fn hide(enabled: bool, window: &mut Window, cx: &mut App) {
    if !enabled {
        return;
    }
    mark_keyboard(window, cx);
    window.dispatch_action(Box::new(HidePointer), cx);
}

/// Make the window's last input keyboard input, unless it already is.
fn mark_keyboard(window: &mut Window, cx: &mut App) {
    if window.last_input_was_keyboard() {
        return;
    }
    let Ok(keystroke) = Keystroke::parse(INERT_KEY) else {
        return;
    };
    super::synthesize(|| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )
    });
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers,
        MouseMoveEvent, ParentElement as _, Render, TestAppContext, Window, div, point, px,
    };

    use super::*;
    use crate::gamepad::{Down, GamepadInput, route_in_window};

    struct Pad {
        focus: FocusHandle,
        downs: usize,
        keys: usize,
    }

    impl Render for Pad {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| this.keys += 1))
                .on_action(cx.listener(|this, _: &Down, _, cx| {
                    this.downs += 1;
                    cx.notify();
                }))
                .child("Pad")
        }
    }

    #[gpui_kit::test]
    fn a_controller_input_counts_as_keyboard_input_and_asks_to_hide(cx: &mut TestAppContext) {
        let taken = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            install(cx);
            // Bubbling runs the latest global handler first; it takes the
            // action as `install`'s does.
            let taken = taken.clone();
            cx.on_action(move |_: &HidePointer, _| taken.set(taken.get() + 1));
        });
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Pad {
                focus,
                downs: 0,
                keys: 0,
            }
        });
        cx.run_until_parked();
        let keyboard = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, _| {
                window.last_input_was_keyboard()
            })
            .unwrap()
        };
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(5.), px(5.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .unwrap();
        assert!(!keyboard(cx), "the mouse moved last");

        cx.update_window(window.into(), |_, window, cx| {
            hide(true, window, cx);
            route_in_window(GamepadInput::Down, window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(keyboard(cx), "a controller input is keyboard input");
        let (downs, keys) = window
            .update(cx, |pad, _, _| (pad.downs, pad.keys))
            .unwrap();
        assert_eq!(downs, 1, "the input itself is handled once");
        assert_eq!(keys, 1, "one inert key press, nothing typed");

        // Already keyboard input: no further key press.
        cx.update_window(window.into(), |_, window, cx| hide(true, window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(window.update(cx, |pad, _, _| pad.keys).unwrap(), 1);
        // The action reached the global handler both times, so GPUI hid
        // the pointer.
        assert_eq!(taken.get(), 2);
    }

    #[gpui_kit::test]
    fn turned_off_it_leaves_the_pointer_alone(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Pad {
                focus,
                downs: 0,
                keys: 0,
            }
        });
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| hide(false, window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(window.update(cx, |pad, _, _| pad.keys).unwrap(), 0);
    }
}
