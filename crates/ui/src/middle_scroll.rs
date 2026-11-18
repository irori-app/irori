//! Scrolling by holding the middle mouse button (`iroriUi.middleClickScroll`,
//! off unless turned on), as browser extensions such as ScrollAnywhere do:
//! while the button is held, moving the mouse scrolls whatever is under it,
//! in the direction the mouse moves; letting go stops.
//!
//! The movement becomes ordinary scroll-wheel events at the pointer, so every
//! scrolling region (lists, grids, pages, dialogs) follows without code of
//! its own, and each keeps its own limits. The listeners are the window's
//! own, so they see the button over dialogs and sheets too.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    DispatchPhase, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, Styled as _,
    TouchPhase, canvas, div, point, px,
};

/// How far the content moves for each pixel the mouse moves.
const SPEED: f32 = 2.0;

/// The scroll-wheel delta for the mouse moving from `last` to `now` with
/// the button held: content moves against the mouse, so moving down shows
/// what is further down.
pub fn scroll_delta(last: Point<Pixels>, now: Point<Pixels>) -> Point<Pixels> {
    point((last.x - now.x) * SPEED, (last.y - now.y) * SPEED)
}

/// The drag in progress, kept by the window's root view across frames.
#[derive(Clone, Default)]
pub struct MiddleScroll {
    /// Where the mouse was at the last step, while the button is held.
    last: Rc<Cell<Option<Point<Pixels>>>>,
}

impl MiddleScroll {
    /// An element that listens for the middle button while `enabled`; it
    /// draws nothing.
    pub fn element(&self, enabled: bool) -> impl IntoElement {
        if !enabled {
            self.last.set(None);
        }
        let last = self.last.clone();
        div().absolute().size_0().child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    if !enabled {
                        return;
                    }
                    let down = last.clone();
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, _, _| {
                        if phase == DispatchPhase::Capture && e.button == MouseButton::Middle {
                            down.set(Some(e.position));
                        }
                    });
                    let up = last.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, _| {
                        if phase == DispatchPhase::Capture && e.button == MouseButton::Middle {
                            up.set(None);
                        }
                    });
                    let moved = last.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let Some(from) = moved.get() else {
                            return;
                        };
                        // A release outside the window never arrives.
                        if e.pressed_button != Some(MouseButton::Middle) {
                            moved.set(None);
                            return;
                        }
                        moved.set(Some(e.position));
                        let delta = scroll_delta(from, e.position);
                        if delta.x == px(0.) && delta.y == px(0.) {
                            return;
                        }
                        let wheel = ScrollWheelEvent {
                            position: e.position,
                            delta: ScrollDelta::Pixels(delta),
                            modifiers: e.modifiers,
                            touch_phase: TouchPhase::Moved,
                        };
                        // Not from inside this event's own dispatch.
                        window.defer(cx, move |window, cx| {
                            window.dispatch_event(PlatformInput::ScrollWheel(wheel), cx);
                        });
                    });
                },
            )
            .size_0(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_down_scrolls_down() {
        let d = scroll_delta(point(px(10.), px(10.)), point(px(10.), px(30.)));
        // A negative wheel delta moves the content up: further down the page.
        assert_eq!(d, point(px(0.), px(-40.)));
        let d = scroll_delta(point(px(50.), px(10.)), point(px(40.), px(10.)));
        assert_eq!(d, point(px(20.), px(0.)));
    }
}
