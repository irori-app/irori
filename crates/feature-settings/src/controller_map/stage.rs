//! Interactive controller diagram stage and live readout bar for
//! [`ButtonMapPanel`].

use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Hsla, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    div, img, rems,
};
use irori_bigpicture::gamepad::bindings::{ButtonMap, REBINDABLE};
use irori_bigpicture::gamepad::drawing::{self, Colours, Part, Shape};
use irori_bigpicture::gamepad::{ControllerKind, GamepadInput, PadButton};
use irori_ui::i18n::{t, t_args};

use crate::appearance::hex;
use crate::controllers::kind_text;

use super::{
    ButtonMapPanel, Job, Place, entries, entry_for_part, guide_label, is_rebindable,
    is_round_button, job_label, job_title, lit_parts, render_keycap,
};

const DRAWING_REM: f32 = 18.5;

fn readout_line(text: SharedString, color: Hsla, bold: bool) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_xs()
        .text_color(color)
        .when(bold, |d| d.font_medium())
        .child(text)
        .into_any_element()
}

impl ButtonMapPanel {
    fn part_hover_target(
        &self,
        id: impl Into<ElementId>,
        part: Part,
        (left, top, w, h): (f32, f32, f32, f32),
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(id.into())
            .absolute()
            .left(rems(left))
            .top(rems(top))
            .w(rems(w))
            .h(rems(h))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = hovered.then_some(part);
                if this.hovered_part != next {
                    this.hovered_part = next;
                    if next.is_some() {
                        this.hovered_job = None;
                    }
                    cx.notify();
                }
            }))
    }

    pub(super) fn render_drawing_stage(
        &self,
        map: &ButtonMap,
        actual: ControllerKind,
        shown: ControllerKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (fg_main, fg_muted, fg_primary, primary, border, bg, body, muted, ring) = (
            theme.foreground,
            theme.muted_foreground,
            theme.primary_foreground,
            theme.primary,
            theme.border,
            theme.background,
            theme.secondary,
            theme.muted,
            theme.ring,
        );
        let radius_lg = theme.radius_lg;

        let shape = Shape::of(shown);
        let colours = Colours {
            body: hex(body),
            outline: hex(border),
            part: hex(muted),
            lit: hex(body.blend(primary.opacity(0.24))),
            lit_stroke: hex(body.blend(primary.opacity(0.68))),
            active: hex(primary),
            ring: hex(ring),
        };
        let lit = lit_parts(self.place, map, actual);
        let active = self.active_parts(map, actual);
        let chord_guide = self.place == Place::Guide;
        let svg = drawing::svg(shape, &colours, &lit, &active, chord_guide);
        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        let (w, h) = (DRAWING_REM, DRAWING_REM * drawing::HEIGHT / drawing::WIDTH);

        let text_for = |part: Part| {
            if active.contains(&part) {
                fg_primary
            } else if lit.contains(&part) {
                fg_main
            } else {
                fg_muted
            }
        };
        let caption_under = |lx: f32, ly: f32, lit_on: bool, label: &'static str| {
            div()
                .absolute()
                .left(rems(w * lx - 1.8))
                .top(rems(h * ly - 0.55))
                .w(rems(3.6))
                .h(rems(1.1))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(if lit_on { fg_main } else { fg_muted })
                .child(label)
                .into_any_element()
        };

        let mut overlays: Vec<AnyElement> = Vec::with_capacity(REBINDABLE.len() + 6);
        for button in REBINDABLE {
            let part = Part::Button(button);
            let (sx, sy) = drawing::spot(shape, part);
            let under_capsule = matches!(button, PadButton::Select | PadButton::Start);
            let label = shown.button_label(button);
            let (hw, hh) = match button {
                PadButton::LeftShoulder | PadButton::RightShoulder => (3.8, 1.5),
                PadButton::Select | PadButton::Start => (2.4, 1.5),
                _ => (1.8, 1.8),
            };
            let hit = self
                .part_hover_target(
                    ElementId::Name(format!("ctrl-hit-{}", button.id()).into()),
                    part,
                    (w * sx - hw * 0.5, h * sy - hh * 0.5, hw, hh),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(job) = this.editing_job {
                        this.assign(job, button, window, cx);
                        return;
                    }
                    let map = ButtonMap::from_prefs(&Self::prefs(cx));
                    let actual = Self::actual_kind(cx);
                    if let Some(entry) = entry_for_part(this.place, part, &map, actual)
                        && is_rebindable(this.place, entry.job)
                        && let Job::Input(job) = entry.job
                    {
                        this.editing_job = Some(job);
                        cx.notify();
                    }
                }))
                .when(!under_capsule, |el| {
                    el.text_xs()
                        .font_bold()
                        .text_color(text_for(part))
                        .child(label)
                });
            overlays.push(hit.into_any_element());
            if under_capsule {
                let (lx, ly) = drawing::label_spot(shape, part);
                overlays.push(caption_under(
                    lx,
                    ly,
                    active.contains(&part) || lit.contains(&part),
                    label,
                ));
            }
        }

        let (lsx, lsy) = drawing::spot(shape, Part::LeftStick);
        let (ux, uy) = drawing::spot(shape, Part::Button(PadButton::DPadUp));
        let (_, dy) = drawing::spot(shape, Part::Button(PadButton::DPadDown));
        let guide = Part::Button(PadButton::Guide);
        let (gx, gy) = drawing::spot(shape, guide);
        let (glx, gly) = drawing::label_spot(shape, guide);
        let lstick_hit = self
            .part_hover_target(
                "ctrl-hit-lstick",
                Part::LeftStick,
                (w * lsx - 1.15, h * lsy - 1.15, 2.3, 2.3),
                cx,
            )
            .text_xs()
            .font_bold()
            .text_color(text_for(Part::LeftStick))
            .child("L")
            .into_any_element();
        let dpad_hit = self
            .part_hover_target(
                "ctrl-hit-dpad",
                Part::Button(PadButton::DPadUp),
                (w * ux - 1.35, h * ((uy + dy) * 0.5) - 1.35, 2.7, 2.7),
                cx,
            )
            .into_any_element();
        let guide_hit = self
            .part_hover_target(
                "ctrl-hit-guide",
                guide,
                (w * gx - 0.9, h * gy - 0.9, 1.8, 1.8),
                cx,
            )
            .into_any_element();
        let guide_cap = caption_under(
            glx,
            gly,
            active.contains(&guide) || lit.contains(&guide) || chord_guide,
            guide_label(shown),
        );
        overlays.extend([lstick_hit, dpad_hit, guide_hit, guide_cap]);

        let editing = self.editing_job.is_some();
        let hint_key = if editing {
            "settings.controller_map.stage_pick"
        } else if chord_guide {
            "settings.controller_map.stage_chord"
        } else {
            "settings.controller_map.stage_hint"
        };
        let stage_top = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(fg_main)
                    .child(t_args(
                        "settings.controller_map.layout_name",
                        &[("family", kind_text(shown).as_ref())],
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if editing { primary } else { fg_muted })
                    .child(t(hint_key)),
            );

        let readout = self.render_stage_readout(map, actual, shown, cx);
        v_flex()
            .w(rems(20.5))
            .flex_shrink_0()
            .gap_3()
            .p_3()
            .rounded(radius_lg)
            .bg(bg.opacity(0.45))
            .border_1()
            .border_color(if editing { primary } else { border })
            .child(stage_top)
            .child(
                div()
                    .image_cache(self.images.clone())
                    .relative()
                    .mx_auto()
                    .flex_shrink_0()
                    .w(rems(w))
                    .h(rems(h))
                    .child(img(image).size_full())
                    .children(overlays),
            )
            .child(readout)
            .into_any_element()
    }

    fn render_stage_readout(
        &self,
        map: &ButtonMap,
        actual: ControllerKind,
        shown: ControllerKind,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let box_base = h_flex()
            .w_full()
            .min_h(rems(2.25))
            .px_2p5()
            .py_1p5()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .border_1()
            .border_color(if self.editing_job.is_some() {
                theme.primary
            } else {
                theme.border
            });

        if let Some(job) = self.editing_job {
            return box_base
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(t_args(
                            "settings.controller_map.remapping_prompt",
                            &[("job", job_label(job).as_ref())],
                        )),
                )
                .into_any_element();
        }

        let inspected = self
            .hovered_job
            .and_then(|job| entries(self.place).iter().copied().find(|e| e.job == job))
            .or_else(|| {
                self.hovered_part
                    .and_then(|part| entry_for_part(self.place, part, map, actual))
            });

        if let Some(entry) = inspected {
            let badge: SharedString = match entry.job {
                Job::Move => t("settings.controller_map.dpad_and_stick"),
                Job::Input(GamepadInput::Guide) => guide_label(shown).into(),
                Job::Input(input) => map.button_for(input, actual).map_or_else(
                    || t("settings.controller_map.no_button"),
                    |b| shown.button_label(b).into(),
                ),
                Job::Chord(input) => {
                    let button = match input {
                        GamepadInput::Up | GamepadInput::Down => shown.glyph(input).to_owned(),
                        _ => map
                            .button_for(input, actual)
                            .map_or("?", |b| shown.button_label(b))
                            .to_owned(),
                    };
                    t_args("settings.controller_map.with_guide", &[("button", &button)])
                }
            };
            return box_base
                .child(render_keycap(badge, false, true, cx))
                .child(readout_line(job_title(entry.job), theme.foreground, true))
                .into_any_element();
        }

        if let Some(Part::Button(button)) = self.hovered_part {
            let label: SharedString = if button == PadButton::Guide {
                guide_label(shown).into()
            } else {
                shown.button_label(button).into()
            };
            let msg = if self.place == Place::Guide && button == PadButton::Guide {
                t("settings.controller_map.guide_modifier_hint")
            } else {
                t("settings.controller_map.unmapped_button")
            };
            return box_base
                .child(render_keycap(label, is_round_button(button), false, cx))
                .child(readout_line(msg, theme.muted_foreground, false))
                .into_any_element();
        }

        box_base
            .child(readout_line(
                t("settings.controller_map.stage_idle"),
                theme.muted_foreground,
                false,
            ))
            .into_any_element()
    }
}
