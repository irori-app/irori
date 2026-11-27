//! Presentational pieces shared by the settings screens.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, App, ClickEvent, Div, ElementId, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, rems,
};
use irori_ui::i18n::{t, t_args};

/// Decorations of one field row besides its control.
#[derive(Default)]
pub(crate) struct RowState {
    pub label: SharedString,
    pub help: SharedString,
    /// `(id_key, text)` shown in a wrapped tooltip when hovering the label or its info icon.
    pub tooltip: Option<(SharedString, SharedString)>,
    pub tag: Option<SharedString>,
    /// Shown as "Overridden" with this reset button next to the control.
    pub reset: Option<AnyElement>,
    pub disabled_reason: Option<SharedString>,
    pub error: Option<SharedString>,
    /// Lay the control out under the label (lists, wide editors).
    pub stacked: bool,
}

/// Wrapped plain-language tooltip built lazily on hover.
pub(crate) fn plain_tooltip(text: SharedString, window: &mut Window, cx: &mut App) -> AnyView {
    Tooltip::element(move |_, _| {
        div()
            .max_w(rems(22.))
            .text_xs()
            .whitespace_normal()
            .child(text.clone())
    })
    .py_1p5()
    .px_2p5()
    .build(window, cx)
}

fn label_with_tooltip(
    label: SharedString,
    tooltip: Option<(SharedString, SharedString)>,
    muted: Hsla,
) -> AnyElement {
    let Some((key, tip)) = tooltip.filter(|(_, tip)| !tip.is_empty()) else {
        return div()
            .text_sm()
            .font_medium()
            .child(label)
            .into_any_element();
    };
    let hover_tip = tip.clone();
    let tip_id = ElementId::Name(format!("tip-{key}").into());
    let icon_id = ElementId::Name(format!("tip-icon-{key}").into());
    // Hovering either the label or the `(i)` button triggers the single wrapped
    // tooltip on the parent cluster; the icon remains focusable for keyboards.
    let icon = Button::new(icon_id)
        .ghost()
        .xsmall()
        .compact()
        .icon(IconName::Info)
        .text_color(muted)
        .accessibility_label(tip);
    h_flex()
        .id(tip_id)
        .gap_1()
        .items_center()
        .tooltip(move |window, cx| plain_tooltip(hover_tip.clone(), window, cx))
        .child(div().text_sm().font_medium().child(label))
        .child(icon)
        .into_any_element()
}

/// A labelled settings row: label, qualifiers and help on the leading side,
/// the control on the trailing side (or below for stacked editors).
pub(crate) fn field_row(state: RowState, control: AnyElement, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let danger = theme.danger;
    let label = h_flex()
        .gap_2()
        .items_center()
        .flex_wrap()
        .child(label_with_tooltip(state.label, state.tooltip, muted))
        .when_some(state.tag, |this, tag| {
            this.child(Tag::secondary().xsmall().outline().child(tag))
        })
        .when(state.reset.is_some(), |this| {
            this.child(Tag::info().xsmall().child(t("settings.field.overridden")))
        });
    let text = v_flex()
        .gap_0p5()
        .min_w_0()
        .flex_1()
        .child(label)
        .when(!state.help.is_empty(), |this| {
            this.child(div().text_xs().text_color(muted).child(state.help))
        })
        .when_some(state.disabled_reason, |this, reason| {
            this.child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).xsmall())
                    .child(reason),
            )
        });
    let error = state.error.map(|e| {
        h_flex()
            .gap_1()
            .text_xs()
            .text_color(danger)
            .child(Icon::new(IconName::CircleAlert).xsmall())
            .child(e)
    });
    let trailing = h_flex()
        .gap_2()
        .flex_shrink_0()
        .when_some(state.reset, |this, reset| this.child(reset));
    if state.stacked {
        v_flex()
            .w_full()
            .gap_2()
            .px_4()
            .py_2()
            .child(h_flex().gap_4().items_start().child(text).child(trailing))
            .child(control)
            .children(error)
    } else {
        v_flex()
            .w_full()
            .gap_1()
            .px_4()
            .py_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .justify_between()
                    .child(text)
                    .child(trailing.child(div().flex_shrink_0().child(control))),
            )
            .children(error)
    }
}

/// Two closely related controls laid out side by side on a single row.
pub(crate) struct PairedRow {
    pub label: SharedString,
    pub help: SharedString,
    /// `(id_key, text)` shown in a wrapped tooltip when hovering the label or its info icon.
    pub tooltip: Option<(SharedString, SharedString)>,
    pub first_sublabel: SharedString,
    pub first_ctrl: AnyElement,
    pub separator: Option<&'static str>,
    pub second_sublabel: SharedString,
    pub second_ctrl: AnyElement,
    pub error: Option<SharedString>,
}

/// Two closely related controls (such as width and height, or start and end
/// hours) laid out side by side on a single compact settings row.
pub(crate) fn paired_field_row(pair: PairedRow, cx: &App) -> Div {
    let theme = cx.theme();
    let muted_fg = theme.muted_foreground;
    let danger = theme.danger;
    let text = v_flex()
        .gap_0p5()
        .min_w_0()
        .flex_1()
        .child(label_with_tooltip(pair.label, pair.tooltip, muted_fg))
        .when(!pair.help.is_empty(), |this| {
            this.child(div().text_xs().text_color(muted_fg).child(pair.help))
        });
    let labeled = |sublabel: SharedString, ctrl: AnyElement| {
        v_flex()
            .gap_0p5()
            .child(div().text_xs().text_color(muted_fg).child(sublabel))
            .child(ctrl)
    };
    let controls = h_flex()
        .gap_2()
        .items_end()
        .flex_shrink_0()
        .flex_wrap()
        .child(labeled(pair.first_sublabel, pair.first_ctrl))
        .when_some(pair.separator, |row, sep| {
            row.child(div().pb_1p5().text_sm().text_color(muted_fg).child(sep))
        })
        .child(labeled(pair.second_sublabel, pair.second_ctrl));
    let error = pair.error.map(|e| {
        h_flex()
            .gap_1()
            .text_xs()
            .text_color(danger)
            .child(Icon::new(IconName::CircleAlert).xsmall())
            .child(e)
    });
    v_flex()
        .w_full()
        .gap_1()
        .px_4()
        .py_2()
        .child(
            h_flex()
                .w_full()
                .gap_4()
                .justify_between()
                .child(text)
                .child(controls),
        )
        .children(error)
}

/// Collapsible container holding advanced or niche options for one sub-tab.
pub(crate) fn more_options_disclosure(
    id: impl Into<ElementId>,
    open: bool,
    count: usize,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    build_content: impl FnOnce() -> AnyElement,
) -> AnyElement {
    let id = id.into();
    let label = if open {
        t("settings.form.hide_options")
    } else {
        t_args(
            "settings.form.more_options",
            &[("count", &count.to_string())],
        )
    };
    let icon = if open {
        IconName::ChevronUp
    } else {
        IconName::ChevronDown
    };
    let mut box_ = Collapsible::new().w_full().open(open).child(
        h_flex().pt_3().child(
            Button::new(id.clone())
                .outline()
                .small()
                .icon(icon)
                .label(label)
                .on_click(on_toggle),
        ),
    );
    if open {
        box_ = box_.content(irori_ui::motion::fade_in(
            (id, "content"),
            div().pt_1().child(build_content()),
        ));
    }
    box_.into_any_element()
}

/// Compact sub-heading inside a section, with an optional one-line summary.
pub(crate) fn group_header(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    first: bool,
    cx: &App,
) -> Div {
    v_flex()
        .when(first, |d| d.pt_2())
        .when(!first, |d| d.pt_4())
        .pb_1()
        .gap_0p5()
        .child(irori_ui::style::caption(title, cx))
        .when_some(description, |d, desc| {
            d.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(desc),
            )
        })
}

/// Sub-heading inside a section.
pub(crate) fn group_heading(title: impl Into<SharedString>, cx: &App) -> Div {
    group_header(title, None, false, cx)
}

/// Subtle group caption inside the settings navigation sidebar.
pub(crate) fn sidebar_section_label(title: impl Into<SharedString>, first: bool, cx: &App) -> Div {
    div()
        .px_2()
        .when(!first, |d| d.pt_3())
        .when(first, |d| d.pt_1())
        .pb_0p5()
        .child(irori_ui::style::caption(title, cx))
}

/// The surface holding one group's rows; rows after the first get a divider.
pub(crate) fn group_panel(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .rounded(theme.radius_lg)
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
}

/// Heading of a section when several sections are listed (search results,
/// game defaults).
pub(crate) fn section_heading(title: impl Into<SharedString>) -> Div {
    div().pt_6().text_lg().font_semibold().child(title.into())
}

/// A row in a section navigation column.
pub(crate) fn nav_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    nav_button_with_dot(id, icon, label, selected, None, on_click)
}

/// A row in a section navigation column with an optional status dot at the end.
pub(crate) fn nav_button_with_dot(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    selected: bool,
    dot: Option<gpui_kit::Hsla>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let label = label.into();
    // Button centres its content row, so the icon and label go in a child
    // that fills it and lines them up at the start.
    Button::new(id)
        .ghost()
        .large()
        .h_9()
        .w_full()
        .accessibility_label(label.clone())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(Icon::new(icon))
                .child(div().flex_1().min_w_0().text_ellipsis().child(label))
                .when_some(dot, |row, colour| {
                    row.child(div().flex_none().size_2().rounded_full().bg(colour))
                }),
        )
        .selected(selected)
        .on_click(on_click)
}

/// Bordered list surface for row editors and panels.
pub(crate) fn panel(cx: &App) -> Div {
    v_flex()
        .w_full()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
}

/// Muted explanatory line.
pub(crate) fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
