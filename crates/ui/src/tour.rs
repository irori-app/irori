//! The guided tour ("Show me around"): a few steps over the main window,
//! each lighting up one part of it with a word on what it does. Started only
//! from Help (Settings → About, the Getting started hub, the palette), never
//! on its own; Escape skips it, and arrows, Enter and a controller move
//! through it.
//!
//! Steps point at parts by a stable anchor id, not by position: the part
//! carries [`mark`], an empty canvas that notes where the part was drawn
//! while a tour runs (nothing is noted otherwise). A part the window
//! doesn't show (a narrow window hides the sidebar) leaves its step without
//! a light, and the card says what it would have shown.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render,
    Styled as _, Window, canvas, div, point, px,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::i18n::{t, t_args};
use crate::nav::{Route, navigate};

/// Anchor ids the parts of the window carry; never change one a step uses.
pub mod anchors {
    pub const SIDEBAR: &str = "tour.sidebar";
    pub const LIBRARY_HEADER: &str = "tour.library_header";
    pub const LIBRARY_GAMES: &str = "tour.library_games";
    pub const PALETTE: &str = "tour.palette";
    pub const BIG_PICTURE: &str = "tour.big_picture";
}

/// One step: the part it lights and what it says (translation keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub anchor: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

/// The tour, in order. Every step's part is on the library's page.
pub const STEPS: [Step; 5] = [
    Step {
        anchor: anchors::SIDEBAR,
        title: "ui.tour.sidebar.title",
        body: "ui.tour.sidebar.body",
    },
    Step {
        anchor: anchors::LIBRARY_HEADER,
        title: "ui.tour.header.title",
        body: "ui.tour.header.body",
    },
    Step {
        anchor: anchors::LIBRARY_GAMES,
        title: "ui.tour.games.title",
        body: "ui.tour.games.body",
    },
    Step {
        anchor: anchors::PALETTE,
        title: "ui.tour.palette.title",
        body: "ui.tour.palette.body",
    },
    Step {
        anchor: anchors::BIG_PICTURE,
        title: "ui.tour.big_picture.title",
        body: "ui.tour.big_picture.body",
    },
];

/// Whether a tour runs: anchors note where they were drawn only then.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Where each anchor was last drawn, in window coordinates.
static PLACES: Lazy<Mutex<HashMap<&'static str, Bounds<Pixels>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Mark the element this is a child of as the anchor `id`: an empty canvas
/// filling it that notes its bounds while a tour runs. Put it last among
/// the element's children; the element needs no other change.
pub fn mark(id: &'static str) -> AnyElement {
    canvas(
        move |bounds, _, _| {
            if ACTIVE.load(Ordering::Relaxed) {
                PLACES.lock().insert(id, bounds);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Where the card sits beside a lit part: below it when there is room,
/// else above it, else over the window's middle; kept inside `window`.
pub fn card_origin(
    target: Option<Bounds<Pixels>>,
    card: gpui_kit::Size<Pixels>,
    window: Bounds<Pixels>,
    gap: Pixels,
) -> gpui_kit::Point<Pixels> {
    let clamp_x = |x: Pixels| {
        x.max(window.origin.x + gap)
            .min(window.origin.x + window.size.width - card.width - gap)
    };
    let middle = point(
        window.origin.x + (window.size.width - card.width) / 2.,
        window.origin.y + (window.size.height - card.height) / 2.,
    );
    let Some(target) = target else {
        return middle;
    };
    let bottom = window.origin.y + window.size.height;
    let below = target.origin.y + target.size.height + gap;
    let above = target.origin.y - gap - card.height;
    let right = target.origin.x + target.size.width + gap;
    if below + card.height <= bottom - gap {
        point(clamp_x(target.origin.x), below)
    } else if above >= window.origin.y + gap {
        point(clamp_x(target.origin.x), above)
    } else if right + card.width <= window.origin.x + window.size.width - gap {
        // A tall part (the sidebar): beside it, near its top.
        let y = (target.origin.y + gap)
            .min(bottom - card.height - gap)
            .max(window.origin.y + gap);
        point(right, y)
    } else {
        middle
    }
}

/// Maximum frames to re-probe when an anchor has not painted yet (such as
/// immediately after switching routes). Capping prevents an unbounded repaint
/// loop if a region is legitimately hidden in a narrow window.
const MAX_PROBE_FRAMES: u8 = 3;

fn should_retry_probe(found: bool, probe_frames: u8) -> bool {
    !found && probe_frames < MAX_PROBE_FRAMES
}

/// The running tour.
pub struct Tour {
    step: Option<usize>,
    probe_frames: u8,
    focus: FocusHandle,
}

struct TourHandle(Entity<Tour>);
impl gpui_kit::Global for TourHandle {}

/// The tour's entity, made on first use.
pub fn tour(cx: &mut App) -> Entity<Tour> {
    if let Some(h) = cx.try_global::<TourHandle>() {
        return h.0.clone();
    }
    let entity = cx.new(|cx| Tour {
        step: None,
        probe_frames: 0,
        focus: cx.focus_handle(),
    });
    cx.set_global(TourHandle(entity.clone()));
    entity
}

/// Start the tour on the library's page, from its first step.
pub fn start(window: &mut Window, cx: &mut App) {
    navigate(Route::Library, cx);
    PLACES.lock().clear();
    ACTIVE.store(true, Ordering::Relaxed);
    let entity = tour(cx);
    entity.update(cx, |tour, cx| {
        tour.step = Some(0);
        tour.probe_frames = 0;
        cx.notify();
    });
    let focus = entity.read(cx).focus.clone();
    window.focus(&focus, cx);
    // The anchors note their places as the next frame draws them.
    window.refresh();
}

/// Whether a tour runs.
pub fn running(cx: &App) -> bool {
    cx.try_global::<TourHandle>()
        .is_some_and(|h| h.0.read(cx).step.is_some())
}

impl Tour {
    fn go(&mut self, to: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.step = to.filter(|s| *s < STEPS.len());
        self.probe_frames = 0;
        if self.step.is_none() {
            ACTIVE.store(false, Ordering::Relaxed);
            PLACES.lock().clear();
        }
        window.refresh();
        cx.notify();
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.step.map(|s| s + 1);
        self.go(next, window, cx);
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = self.step.map(|s| s.saturating_sub(1));
        self.go(back, window, cx);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.go(None, window, cx),
            "enter" | "right" | "down" | "space" => self.next(window, cx),
            "left" | "up" => self.back(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for Tour {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Tour {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(index) = self.step else {
            return div().into_any_element();
        };
        let step = STEPS[index];
        let area = crate::style::visible_bounds(window);
        let target = PLACES.lock().get(step.anchor).copied();
        let theme = cx.theme();
        let dim = crate::theme::dim_colour(cx);
        let rem = window.rem_size();
        let card_size = gpui_kit::size(rem * 24., rem * 11.);
        let origin = card_origin(target, card_size, area, rem);
        let last = index + 1 == STEPS.len();

        // The dim around the lit part, as four bands; all of it without one.
        let band = |x: Pixels, y: Pixels, w: Pixels, h: Pixels| {
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(w.max(px(0.)))
                .h(h.max(px(0.)))
                .bg(dim)
        };
        let (ax, ay) = (area.origin.x, area.origin.y);
        let (aw, ah) = (area.size.width, area.size.height);
        let bands: Vec<AnyElement> = match target {
            Some(b) => {
                let pad = rem * 0.25;
                let (x0, y0) = (b.origin.x - pad, b.origin.y - pad);
                let (x1, y1) = (
                    b.origin.x + b.size.width + pad,
                    b.origin.y + b.size.height + pad,
                );
                vec![
                    band(ax, ay, aw, y0 - ay).into_any_element(),
                    band(ax, y1, aw, ay + ah - y1).into_any_element(),
                    band(ax, y0, x0 - ax, y1 - y0).into_any_element(),
                    band(x1, y0, ax + aw - x1, y1 - y0).into_any_element(),
                    div()
                        .absolute()
                        .left(x0)
                        .top(y0)
                        .w(x1 - x0)
                        .h(y1 - y0)
                        .rounded(theme.radius_lg)
                        .border_2()
                        .border_color(theme.ring)
                        .into_any_element(),
                ]
            }
            None => vec![band(ax, ay, aw, ah).into_any_element()],
        };

        let card = v_flex()
            .id("tour-card")
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(card_size.width)
            .gap_2()
            .p_4()
            .rounded(theme.radius_lg)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t_args(
                        "ui.tour.progress",
                        &[
                            ("step", &(index + 1).to_string()),
                            ("total", &STEPS.len().to_string()),
                        ],
                    )),
            )
            .child(div().font_semibold().child(t(step.title)))
            .child(div().text_sm().child(t(step.body)))
            .when(target.is_none(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t("ui.tour.not_shown")),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("tour-skip")
                            .ghost()
                            .small()
                            .label(t("ui.tour.skip"))
                            .on_click(cx.listener(|this, _, window, cx| this.go(None, window, cx))),
                    )
                    .child(div().flex_1())
                    .when(index > 0, |el| {
                        el.child(
                            Button::new("tour-back")
                                .small()
                                .label(t("ui.tour.back"))
                                .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                        )
                    })
                    .child(
                        Button::new("tour-next")
                            .primary()
                            .small()
                            .label(if last {
                                t("ui.tour.done")
                            } else {
                                t("ui.tour.next")
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            );

        // Until the part is drawn once, look again for a few frames without
        // spinning forever when a region is genuinely absent.
        if should_retry_probe(target.is_some(), self.probe_frames) {
            self.probe_frames += 1;
            window.request_animation_frame();
        }
        div()
            .id("tour")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Tour")
            .on_key_down(cx.listener(Self::on_key))
            // The window under the tour doesn't take the pointer meanwhile.
            .occlude()
            .children(bands)
            .child(card)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{bounds, size};

    fn window() -> Bounds<Pixels> {
        bounds(point(px(20.), px(20.)), size(px(1000.), px(700.)))
    }

    #[test]
    fn the_card_sits_below_above_or_beside_its_part() {
        let card = size(px(300.), px(150.));
        let gap = px(16.);
        // Room below: under the part, at its left edge.
        let header = bounds(point(px(300.), px(60.)), size(px(600.), px(50.)));
        assert_eq!(
            card_origin(Some(header), card, window(), gap),
            point(px(300.), px(126.))
        );
        // No room below: above it.
        let footer = bounds(point(px(300.), px(600.)), size(px(600.), px(80.)));
        assert_eq!(
            card_origin(Some(footer), card, window(), gap),
            point(px(300.), px(434.))
        );
        // A part as tall as the window: beside it.
        let sidebar = bounds(point(px(20.), px(20.)), size(px(200.), px(700.)));
        assert_eq!(
            card_origin(Some(sidebar), card, window(), gap),
            point(px(236.), px(36.))
        );
        // Kept inside the window on the right.
        let corner = bounds(point(px(950.), px(60.)), size(px(60.), px(30.)));
        assert_eq!(
            card_origin(Some(corner), card, window(), gap).x,
            px(20. + 1000. - 300. - 16.)
        );
        // No part: the window's middle.
        assert_eq!(
            card_origin(None, card, window(), gap),
            point(px(370.), px(295.))
        );
    }

    #[test]
    fn every_step_has_its_own_part() {
        let mut anchors: Vec<&str> = STEPS.iter().map(|s| s.anchor).collect();
        anchors.sort_unstable();
        anchors.dedup();
        assert_eq!(anchors.len(), STEPS.len());
    }

    #[test]
    fn probe_retries_are_bounded() {
        assert!(should_retry_probe(false, 0));
        assert!(should_retry_probe(false, MAX_PROBE_FRAMES - 1));
        assert!(!should_retry_probe(false, MAX_PROBE_FRAMES));
        assert!(!should_retry_probe(true, 0));
    }
}
