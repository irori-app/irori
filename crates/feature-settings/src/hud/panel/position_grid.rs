//! Compact visual monitor anchor grids for HUD screen placement
//! (3x3 edge/corner grid for MangoHud, 2x2 corner grid for Metal HUD).

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use irori_core::{HudPosition, MetalHudAlignment};

/// Map [`HudPosition`] to its index in `HudLabels::position_labels` (`HudPosition::ALL`).
pub(super) fn position_label_index(pos: HudPosition) -> usize {
    HudPosition::ALL.iter().position(|&p| p == pos).unwrap_or(0)
}

/// Interactive 3x3 16:9 monitor anchor picker for MangoHud's 8 screen positions.
pub(super) fn render_mangohud_position_grid(
    current: HudPosition,
    position_labels: &[SharedString; 8],
    on_select: impl Fn(HudPosition, &ClickEvent, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let (bg, card_bg, border, hover_bg, primary, muted_fg) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.secondary,
            theme.border,
            theme.muted,
            theme.primary,
            theme.muted_foreground,
        )
    };

    let cell = |id: &'static str, pos: HudPosition| {
        let active = current == pos;
        let tip = position_labels[position_label_index(pos)].clone();
        let on_select = on_select.clone();
        div()
            .id(id)
            .flex_1()
            .h_full()
            .rounded(px(4.))
            .border_1()
            .border_color(if active { primary } else { border })
            .bg(if active { primary } else { card_bg })
            .cursor_pointer()
            .hover(move |s| if active { s } else { s.bg(hover_bg) })
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(move |ev, window, cx| on_select(pos, ev, window, cx))
    };

    let center_cell = div()
        .flex_1()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(muted_fg.opacity(0.6))
        .child(SharedString::new_static("16:9"));

    v_flex()
        .w(px(184.))
        .h(px(108.))
        .p(px(6.))
        .gap(px(5.))
        .rounded(px(8.))
        .bg(bg)
        .border_2()
        .border_color(border)
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .gap(px(5.))
                .child(cell("hud-pos-top-left", HudPosition::TopLeft))
                .child(cell("hud-pos-top-center", HudPosition::TopCenter))
                .child(cell("hud-pos-top-right", HudPosition::TopRight)),
        )
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .gap(px(5.))
                .child(cell("hud-pos-middle-left", HudPosition::MiddleLeft))
                .child(center_cell)
                .child(cell("hud-pos-middle-right", HudPosition::MiddleRight)),
        )
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .gap(px(5.))
                .child(cell("hud-pos-bottom-left", HudPosition::BottomLeft))
                .child(cell("hud-pos-bottom-center", HudPosition::BottomCenter))
                .child(cell("hud-pos-bottom-right", HudPosition::BottomRight)),
        )
        .into_any_element()
}

/// Interactive 2x2 16:9 monitor corner picker for Apple's Metal Performance HUD.
pub(super) fn render_metal_position_grid(
    current: MetalHudAlignment,
    position_labels: &[SharedString; 8],
    on_select: impl Fn(MetalHudAlignment, &ClickEvent, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let (bg, card_bg, border, hover_bg, primary) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.secondary,
            theme.border,
            theme.muted,
            theme.primary,
        )
    };

    let cell = |id: &'static str, align: MetalHudAlignment, label_ix: usize| {
        let active = current == align;
        let tip = position_labels[label_ix].clone();
        let on_select = on_select.clone();
        div()
            .id(id)
            .flex_1()
            .h_full()
            .rounded(px(4.))
            .border_1()
            .border_color(if active { primary } else { border })
            .bg(if active { primary } else { card_bg })
            .cursor_pointer()
            .hover(move |s| if active { s } else { s.bg(hover_bg) })
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(move |ev, window, cx| on_select(align, ev, window, cx))
    };

    v_flex()
        .w(px(164.))
        .h(px(96.))
        .p(px(6.))
        .gap(px(6.))
        .rounded(px(8.))
        .bg(bg)
        .border_2()
        .border_color(border)
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .gap(px(6.))
                .child(cell("hud-metal-top-left", MetalHudAlignment::TopLeft, 0))
                .child(cell("hud-metal-top-right", MetalHudAlignment::TopRight, 1)),
        )
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .gap(px(6.))
                .child(cell(
                    "hud-metal-bottom-left",
                    MetalHudAlignment::BottomLeft,
                    2,
                ))
                .child(cell(
                    "hud-metal-bottom-right",
                    MetalHudAlignment::BottomRight,
                    3,
                )),
        )
        .into_any_element()
}
