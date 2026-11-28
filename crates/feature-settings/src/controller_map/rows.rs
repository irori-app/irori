//! Binding rows and inline reassignment picker for [`ButtonMapPanel`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_bigpicture::gamepad::bindings::{ButtonMap, REBINDABLE};
use irori_bigpicture::gamepad::{ControllerKind, GamepadInput, PadButton};
use irori_ui::i18n::{t, t_args};

use super::{
    ButtonMapPanel, Entry, Job, guide_label, is_rebindable, is_round_button, job_label, job_title,
    render_keycap,
};

pub(super) struct RowContext<'a> {
    pub map: &'a ButtonMap,
    pub actual: ControllerKind,
    pub shown: ControllerKind,
}

impl ButtonMapPanel {
    pub(super) fn render_row(
        &self,
        ix: usize,
        e: Entry,
        ctx: &RowContext<'_>,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (border, muted_fg, primary, muted_bg) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.primary,
                theme.muted,
            )
        };
        let (map, actual, shown) = (ctx.map, ctx.actual, ctx.shown);
        let highlighted = self.is_entry_highlighted(e, map, actual);
        let rebindable = is_rebindable(self.place, e.job);
        let editing = matches!(e.job, Job::Input(j) if self.editing_job == Some(j));

        let keycaps: AnyElement = match e.job {
            Job::Move => h_flex()
                .gap_1()
                .items_center()
                .child(render_keycap(
                    shown.button_label(PadButton::DPadUp).into(),
                    false,
                    highlighted,
                    cx,
                ))
                .child(render_keycap("L".into(), true, highlighted, cx))
                .into_any_element(),
            Job::Input(GamepadInput::Guide) => {
                render_keycap(guide_label(shown).into(), false, highlighted, cx)
            }
            Job::Input(input) => match map.button_for(input, actual) {
                Some(b) => render_keycap(
                    shown.button_label(b).into(),
                    is_round_button(b),
                    highlighted,
                    cx,
                ),
                None => render_keycap(
                    t("settings.controller_map.no_button"),
                    false,
                    highlighted,
                    cx,
                ),
            },
            Job::Chord(input) => {
                let target_label: SharedString = match input {
                    GamepadInput::Up | GamepadInput::Down => shown.glyph(input).into(),
                    _ => map
                        .button_for(input, actual)
                        .map_or_else(|| "?".into(), |b| shown.button_label(b).into()),
                };
                let target_round = map.button_for(input, actual).is_some_and(is_round_button)
                    && !matches!(input, GamepadInput::Up | GamepadInput::Down);
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(render_keycap(guide_label(shown).into(), false, false, cx))
                    .child(div().text_xs().text_color(muted_fg).child("+"))
                    .child(render_keycap(target_label, target_round, highlighted, cx))
                    .into_any_element()
            }
        };

        let trailing: AnyElement = match e.job {
            Job::Input(job) if rebindable => Button::new(("controller-change", ix))
                .when(editing, |b| b.primary())
                .when(!editing, |b| b.outline())
                .xsmall()
                .label(t(if editing {
                    "settings.common.cancel"
                } else {
                    "settings.controller_map.change"
                }))
                .tooltip(t_args(
                    "settings.controller_map.change_tooltip",
                    &[("job", &job_label(job))],
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.editing_job = (this.editing_job != Some(job)).then_some(job);
                    cx.notify();
                }))
                .into_any_element(),
            _ => Tag::secondary()
                .xsmall()
                .outline()
                .child(t("settings.controller_map.fixed"))
                .into_any_element(),
        };

        let inline_picker = match e.job {
            Job::Input(job) if editing => {
                let current = map.button_for(job, actual);
                let picks = REBINDABLE.into_iter().enumerate().map(|(bix, b)| {
                    let selected = current == Some(b);
                    Button::new(("controller-pick", ix * 16 + bix))
                        .when(selected, |btn| btn.primary())
                        .when(!selected, |btn| btn.outline())
                        .xsmall()
                        .label(shown.button_label(b))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.assign(job, b, window, cx);
                        }))
                });
                Some(
                    h_flex()
                        .w_full()
                        .pt_2()
                        .mt_1p5()
                        .border_t_1()
                        .border_color(primary.opacity(0.35))
                        .gap_1p5()
                        .flex_wrap()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(primary)
                                .child(t("settings.controller_map.assign_button")),
                        )
                        .children(picks),
                )
            }
            _ => None,
        };

        let job = e.job;
        v_flex()
            .id(ElementId::Name(format!("controller-row-{ix}").into()))
            .px_3()
            .py_2()
            .when(!first, |r| r.border_t_1().border_color(border))
            .when(editing, |r| r.bg(primary.opacity(0.10)))
            .when(highlighted && !editing, |r| r.bg(muted_bg.opacity(0.55)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = hovered.then_some(job);
                if this.hovered_job != next {
                    this.hovered_job = next;
                    if next.is_some() {
                        this.hovered_part = None;
                    }
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .min_w(rems(if matches!(e.job, Job::Chord(_)) {
                                6.25
                            } else {
                                4.5
                            }))
                            .child(keycaps),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(job_title(e.job)),
                            )
                            .child(div().text_xs().text_color(muted_fg).child(t(e.text))),
                    )
                    .child(div().flex_shrink_0().child(trailing)),
            )
            .children(inline_picker)
            .into_any_element()
    }
}
