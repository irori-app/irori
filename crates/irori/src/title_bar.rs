//! The title bar Irori draws when the desktop puts its window buttons
//! elsewhere than gpui-kit's title bar does (Linux, client-side decorations).
//!
//! gpui-kit's `TitleBar` always ends with minimize, maximize and close. GNOME
//! and other desktops let the user move them (close on the left, only close,
//! a different order), and GPUI reads that choice (`App::button_layout`) and
//! signals its changes. With the default arrangement the shell keeps
//! gpui-kit's bar; otherwise it draws this one, which behaves the same:
//! dragging moves the window, a double click maximizes it, a right click
//! opens the window menu, and the buttons the window manager can't honour are
//! left out.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, TITLE_BAR_HEIGHT, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Render, RenderOnce, Rgba, StatefulInteractiveElement as _, Styled as _, Window, WindowButton,
    WindowButtonLayout, WindowControlArea, WindowControls, div, linear_color_stop, linear_gradient,
};

/// The buttons of one side to draw, in the layout's order: minimize and
/// maximize only where the window manager offers them, close always.
pub fn shown(side: &[Option<WindowButton>], controls: WindowControls) -> Vec<WindowButton> {
    side.iter()
        .flatten()
        .copied()
        .filter(|b| match b {
            WindowButton::Minimize => controls.minimize,
            WindowButton::Maximize => controls.maximize,
            WindowButton::Close => true,
        })
        .collect()
}

/// Whether gpui-kit's own bar already draws this layout.
pub fn is_kits_layout(layout: &WindowButtonLayout) -> bool {
    layout.left.iter().all(Option::is_none)
        && layout.right
            == [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ]
}

/// Whether a dragged title bar should move the window on the next move.
struct DragState {
    pressed: bool,
}

impl Render for DragState {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The title bar with the desktop's button layout around `content`.
#[derive(IntoElement)]
pub struct DesktopTitleBar {
    layout: WindowButtonLayout,
    content: AnyElement,
    background: Option<Hsla>,
}

impl DesktopTitleBar {
    pub fn new(layout: WindowButtonLayout, content: AnyElement) -> Self {
        Self {
            layout,
            content,
            background: None,
        }
    }

    /// One flat colour instead of the title bar's gradient (translucency).
    pub fn flat(mut self, color: Hsla) -> Self {
        self.background = Some(color);
        self
    }
}

/// gpui-kit's gradient, from the title bar colour mixed into the page's at
/// the top to the title bar colour, so both bars look alike.
fn gradient(title_bar: Hsla, background: Hsla) -> gpui_kit::Background {
    let (a, b) = (title_bar.to_rgb(), background.to_rgb());
    let mix = |x: f32, y: f32| x * 0.55 + y * 0.45;
    let top = Hsla::from(Rgba {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    });
    linear_gradient(
        180.,
        linear_color_stop(top, 0.),
        linear_color_stop(title_bar, 1.),
    )
}

fn button(kind: WindowButton, window: &Window, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let close = kind == WindowButton::Close;
    let (hover_bg, hover_fg, active_bg) = if close {
        (theme.danger, theme.danger_foreground, theme.danger_active)
    } else {
        (
            theme.secondary_hover,
            theme.secondary_foreground,
            theme.secondary_active,
        )
    };
    let icon = match kind {
        WindowButton::Minimize => IconName::WindowMinimize,
        WindowButton::Maximize if window.is_maximized() => IconName::WindowRestore,
        WindowButton::Maximize => IconName::WindowMaximize,
        WindowButton::Close => IconName::WindowClose,
    };
    div()
        .id(kind.id())
        .debug_selector(|| format!("window-button-{}", kind.id()))
        .flex()
        .w(TITLE_BAR_HEIGHT)
        .h_full()
        .flex_shrink_0()
        .justify_center()
        .items_center()
        .text_color(theme.foreground)
        .hover(|s| s.bg(hover_bg).text_color(hover_fg))
        .active(|s| s.bg(active_bg).text_color(hover_fg))
        // Not the start of a window drag.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            match kind {
                WindowButton::Minimize => window.minimize_window(),
                WindowButton::Maximize => window.zoom_window(),
                WindowButton::Close => window.remove_window(),
            }
        })
        .child(Icon::new(icon).small())
        .into_any_element()
}

impl RenderOnce for DesktopTitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let controls = window.window_controls();
        let left = shown(&self.layout.left, controls);
        let right = shown(&self.layout.right, controls);
        let state = window.use_state(cx, |_, _| DragState { pressed: false });
        let theme = cx.theme();
        let border = theme.title_bar_border;
        let background = match self.background {
            Some(color) => color.into(),
            None => gradient(theme.title_bar, theme.background),
        };
        h_flex()
            .id("title-bar")
            .flex_shrink_0()
            .h(TITLE_BAR_HEIGHT)
            .border_b_1()
            .border_color(border)
            .bg(background)
            .window_control_area(WindowControlArea::Drag)
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&state, |s, _, _, _| s.pressed = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&state, |s, _, _, _| s.pressed = false),
            )
            .on_mouse_down_out(window.listener_for(&state, |s, _, _, _| s.pressed = false))
            .on_mouse_move(window.listener_for(&state, |s, _, window, _| {
                if s.pressed {
                    s.pressed = false;
                    window.start_window_move();
                }
            }))
            .on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
            .children(left.into_iter().map(|b| button(b, window, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(self.layout.left.iter().all(Option::is_none), |d| d.pl_3())
                    .child(self.content),
            )
            .children(right.into_iter().map(|b| button(b, window, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> WindowControls {
        WindowControls::default()
    }

    #[test]
    fn keeps_the_layouts_order_and_only_offered_buttons() {
        let side = [
            Some(WindowButton::Close),
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
        ];
        assert_eq!(
            shown(&side, all()),
            [
                WindowButton::Close,
                WindowButton::Minimize,
                WindowButton::Maximize
            ]
        );
        // A tiling compositor offers neither; close stays.
        let tiling = WindowControls {
            minimize: false,
            maximize: false,
            ..all()
        };
        assert_eq!(shown(&side, tiling), [WindowButton::Close]);
        assert!(shown(&[None, None, None], all()).is_empty());
    }

    #[test]
    fn knows_the_kits_own_arrangement() {
        let kit = WindowButtonLayout {
            left: [None; 3],
            right: [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ],
        };
        assert!(is_kits_layout(&kit));
        let gnome = WindowButtonLayout {
            left: [None; 3],
            right: [Some(WindowButton::Close), None, None],
        };
        assert!(!is_kits_layout(&gnome));
        let left = WindowButtonLayout {
            left: [Some(WindowButton::Close), None, None],
            right: [None; 3],
        };
        assert!(!is_kits_layout(&left));
    }
}
