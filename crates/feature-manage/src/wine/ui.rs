//! Shared low-glare UI primitives for the Wine manager views.

use std::fmt::Display;

use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div, transparent_black,
};

pub(super) const PAGE_MAX_WIDTH_REM: f32 = 78.;

#[derive(Clone, Copy)]
pub(super) enum PillTone {
    Accent,
    Success,
    Info,
    Warning,
    Muted,
}

fn tone_colors(tone: PillTone, cx: &App) -> (Hsla, Hsla, Hsla) {
    let t = cx.theme();
    match tone {
        PillTone::Accent => (t.primary, t.primary.opacity(0.13), t.primary.opacity(0.32)),
        PillTone::Success => (t.success, t.success.opacity(0.12), t.success.opacity(0.28)),
        PillTone::Info => (t.info, t.info.opacity(0.12), t.info.opacity(0.28)),
        PillTone::Warning => (t.warning, t.warning.opacity(0.13), t.warning.opacity(0.30)),
        PillTone::Muted => (t.muted_foreground, t.muted.opacity(0.5), t.border),
    }
}

pub(super) fn eid(prefix: &str, suffix: impl Display) -> ElementId {
    ElementId::Name(format!("{prefix}-{suffix}").into())
}

pub(super) fn muted_xs(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .truncate()
        .child(text.into())
}

pub(super) fn pill(label: SharedString, tone: PillTone, dot: bool, cx: &App) -> Div {
    let (fg, bg, border) = tone_colors(tone, cx);
    h_flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .font_medium()
        .text_color(fg)
        .flex_shrink_0()
        .when(dot, |p| {
            p.child(div().w_1p5().h_1p5().rounded_full().bg(fg))
        })
        .child(label)
}

pub(super) fn icon_tile(icon: IconName, tone: Option<PillTone>, cx: &App) -> Div {
    let t = cx.theme();
    let (fg, bg, border) = tone.map_or((t.muted_foreground, t.background, t.border), |tone| {
        tone_colors(tone, cx)
    });
    h_flex()
        .w_8()
        .h_8()
        .items_center()
        .justify_center()
        .rounded(t.radius)
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex_shrink_0()
        .child(Icon::new(icon).small().text_color(fg))
}

pub(super) fn seg_group(cx: &App) -> Div {
    let t = cx.theme();
    h_flex()
        .p_1()
        .gap_1()
        .rounded(t.radius_lg)
        .bg(t.background)
        .border_1()
        .border_color(t.border)
}

pub(super) fn seg_button(
    id: impl Into<ElementId>,
    active: bool,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = cx.theme();
    let (fg, muted, border, box_bg) = (t.foreground, t.muted_foreground, t.border, t.group_box);
    let el = h_flex()
        .id(id)
        .cursor_pointer()
        .px_3()
        .py_1()
        .gap_1p5()
        .rounded(t.radius)
        .border_1()
        .items_center()
        .justify_center()
        .text_xs()
        .font_semibold()
        .on_click(on_click);
    if active {
        el.bg(box_bg).border_color(border).text_color(fg)
    } else {
        el.border_color(transparent_black())
            .text_color(muted)
            .hover(move |s| s.text_color(fg))
    }
}

pub(super) fn choice_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = cx.theme();
    let (fg, muted, border, primary) = (t.foreground, t.muted_foreground, t.border, t.primary);
    let el = div()
        .id(id)
        .cursor_pointer()
        .px_2p5()
        .py_1()
        .rounded(t.radius)
        .border_1()
        .text_xs()
        .on_click(on_click)
        .child(label.into());
    if active {
        el.bg(primary.opacity(0.13))
            .border_color(primary.opacity(0.36))
            .text_color(primary)
            .font_semibold()
    } else {
        el.border_color(border)
            .text_color(muted)
            .hover(move |s| s.text_color(fg))
    }
}

pub(super) fn pane_shell(
    id: &'static str,
    scroll: &'static str,
    header: Div,
    body: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = cx.theme();
    v_flex()
        .id(id)
        .debug_selector(move || id.into())
        .size_full()
        .min_h_0()
        .rounded(t.radius_lg)
        .border_1()
        .border_color(t.border)
        .bg(t.group_box)
        .overflow_hidden()
        .child(
            header
                .w_full()
                .p_4()
                .gap_3()
                .border_b_1()
                .border_color(t.border),
        )
        .child(
            div()
                .id(scroll)
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(body),
        )
        .into_any_element()
}

pub(super) fn control_switch(
    id: &'static str,
    checked: bool,
    title: SharedString,
    subtitle: SharedString,
    info: Option<AnyElement>,
    cx: &App,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Div {
    let fg = cx.theme().foreground;
    h_flex()
        .gap_2p5()
        .items_center()
        .child(Switch::new(id).checked(checked).on_change(on_change))
        .child(
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().text_xs().font_semibold().text_color(fg).child(title))
                        .children(info),
                )
                .child(muted_xs(subtitle, cx)),
        )
}
