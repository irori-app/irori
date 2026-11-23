//! "Can my PC run it?" above the Requirements tab: the store page's needs
//! compared with this computer's memory, graphics card and processor
//! ([`irori_extras::pc_check`]), with a verdict that says what it couldn't
//! tell. Worked out once when the store page and this computer's details
//! are both read, never per frame.

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, rems,
};
use irori_extras::pc_check::{self, Fit, PartCheck, PartKind, PcCheck, ThisPc, Verdict};
use irori_ui::i18n::t;

use super::GamePage;

/// What this computer has, from the system details Irori reads once.
async fn this_pc() -> ThisPc {
    let info = irori_runtime::sysinfo::get(false).await;
    ThisPc {
        memory_bytes: info.memory.total,
        cpu: info.cpu.model.clone(),
        cpu_threads: info.cpu.cores,
        gpus: info
            .gpus
            .iter()
            .filter_map(|g| g.device_name.clone().or_else(|| g.vendor_name.clone()))
            .collect(),
    }
}

/// The verdict's text and colour.
pub(super) fn verdict_tag(verdict: Verdict) -> Tag {
    let (tag, key) = match verdict {
        Verdict::Recommended => (Tag::success(), "game.pc_check.verdict.recommended"),
        Verdict::Minimum => (Tag::warning(), "game.pc_check.verdict.minimum"),
        Verdict::Below => (Tag::danger(), "game.pc_check.verdict.below"),
        Verdict::Unknown => (Tag::secondary(), "game.pc_check.verdict.unknown"),
    };
    tag.xsmall().child(t(key))
}

fn part_label(kind: PartKind) -> SharedString {
    t(match kind {
        PartKind::Memory => "game.pc_check.part.memory",
        PartKind::Graphics => "game.pc_check.part.graphics",
        PartKind::Processor => "game.pc_check.part.processor",
    })
}

/// How the part compares, in one short word.
fn fit_text(part: &PartCheck) -> (SharedString, Option<IconName>) {
    match (part.at_minimum, part.at_recommended) {
        (Fit::Short, _) => (t("game.pc_check.fit.below"), Some(IconName::CircleX)),
        (_, Fit::Meets) => (
            t("game.pc_check.fit.recommended"),
            Some(IconName::CircleCheck),
        ),
        (Fit::Meets, _) => (t("game.pc_check.fit.minimum"), Some(IconName::CircleCheck)),
        (Fit::NotGiven, Fit::Short) => (t("game.pc_check.fit.below_recommended"), None),
        _ => (
            t("game.pc_check.fit.unknown"),
            Some(IconName::CircleQuestionMark),
        ),
    }
}

impl GamePage {
    /// Read this computer's details for the comparison, once.
    pub(super) fn load_this_pc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.this_pc.is_some() {
            return;
        }
        let task = irori_ui::run(cx, async move { Ok(this_pc().await) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(pc) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.this_pc = Some(pc);
                    this.refresh_pc_check();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Compare again when the store page or this computer's details change.
    pub(super) fn refresh_pc_check(&mut self) {
        self.pc_check = match (self.extra.ready(), &self.this_pc) {
            (Some(extra), Some(pc)) => pc_check::check(&extra.requirements, pc),
            _ => None,
        };
    }

    /// The comparison card, when the page names any part it compares.
    pub(super) fn render_pc_check(&self, cx: &App) -> Option<AnyElement> {
        let check = self.pc_check.as_ref()?;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(div().font_semibold().child(t("game.pc_check.title")))
                        .child(verdict_tag(check.verdict)),
                )
                .child(div().text_xs().text_color(muted).child(summary(check)))
                .children(check.parts.iter().map(|p| part_row(p, muted)))
                .into_any_element(),
        )
    }
}

/// What the verdict rests on, honestly.
fn summary(check: &PcCheck) -> SharedString {
    let mut text = t(if check.partial {
        "game.pc_check.partial"
    } else {
        "game.pc_check.rough"
    })
    .to_string();
    if let Some(system) = &check.system {
        text.push(' ');
        text.push_str(&irori_ui::i18n::t_args(
            "game.pc_check.system",
            &[("system", system)],
        ));
    }
    text.into()
}

fn part_row(part: &PartCheck, muted: Hsla) -> AnyElement {
    let (fit, icon) = fit_text(part);
    let have = part
        .have
        .clone()
        .unwrap_or_else(|| t("game.pc_check.have_unknown").to_string());
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .text_sm()
        .child(
            div()
                .w(rems(8.))
                .flex_shrink_0()
                .text_color(muted)
                .child(part_label(part.kind)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().min_w_0().child(have))
                .child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .when_some(icon, |el, icon| el.child(Icon::new(icon).xsmall()))
                        .child(fit),
                ),
        )
        .into_any_element()
}
