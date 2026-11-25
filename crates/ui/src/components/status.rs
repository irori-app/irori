use irori_core::util::{format_bytes, format_duration};
use irori_core::{GameStatus, InstallProgress, PrepareStage};

use crate::i18n::{status_label, t_args};
use crate::style::{TEXT_CONTRAST, legible};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window, div,
};

use super::BackdropInk;

fn preparing_text(p: &InstallProgress) -> Option<SharedString> {
    let prep = p.preparing?;
    let key = match prep.stage {
        PrepareStage::FileLists => "ui.progress.file_lists",
        PrepareStage::CheckingFiles => "ui.progress.checking_files",
    };
    Some(t_args(
        key,
        &[
            ("done", &prep.done.to_string()),
            ("total", &prep.total.to_string()),
        ],
    ))
}

/// "42% · 1.2 GiB of 4.0 GiB · 18.3 MiB/s · 3m 10s left".
pub fn progress_text(p: &InstallProgress) -> String {
    let mut parts = Vec::with_capacity(5);
    if let Some(prep) = preparing_text(p) {
        parts.push(prep.to_string());
    }
    if let Some(pc) = p.percent {
        parts.push(format!("{pc:.0}%"));
    }
    match (p.bytes_done, p.bytes_total) {
        (Some(d), Some(t)) if t > 0 => {
            parts.push(format!("{} of {}", format_bytes(d), format_bytes(t)))
        }
        (Some(d), _) if d > 0 => parts.push(format_bytes(d)),
        _ => {}
    }
    if let Some(s) = p.download_speed.or(p.disk_speed).filter(|s| *s > 0) {
        parts.push(t_args("ui.progress.speed", &[("speed", &format_bytes(s))]).to_string());
    }
    if let Some(eta) = p.eta_secs.filter(|e| *e > 0) {
        parts.push(t_args("ui.progress.left", &[("time", &format_duration(eta))]).to_string());
    }
    parts.join(" · ")
}

/// Status label, progress bar and detail text of a running operation.
#[derive(IntoElement)]
pub struct ProgressSummary {
    id: ElementId,
    status: GameStatus,
    ink: Option<BackdropInk>,
    prominent: bool,
}

impl ProgressSummary {
    pub fn new(id: impl Into<ElementId>, status: GameStatus) -> Self {
        Self {
            id: id.into(),
            status,
            ink: None,
            prominent: false,
        }
    }

    /// Use legible text colors computed for an [`super::ArtBackdrop`].
    pub fn ink(mut self, ink: BackdropInk) -> Self {
        self.ink = Some(ink);
        self
    }

    /// Larger centered percentage and details for the Downloads hero card.
    pub fn prominent(mut self) -> Self {
        self.prominent = true;
        self
    }
}

impl RenderOnce for ProgressSummary {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let label = match &self.status.context {
            Some(c) => format!("{} — {c}", status_label(self.status.kind)),
            None => status_label(self.status.kind).to_string(),
        };
        let progress = self.status.progress.unwrap_or_default();
        let detail_color = self
            .ink
            .map(|ink| ink.muted)
            .unwrap_or_else(|| cx.theme().muted_foreground);

        if self.prominent {
            let percent_text = progress.percent.map(|pc| format!("{pc:.0}%"));
            let theme = cx.theme();
            let percent_color = self
                .ink
                .map(|ink| legible(theme.primary, &ink.surfaces, TEXT_CONTRAST, cx))
                .unwrap_or(theme.primary);
            let value_color = self.ink.map(|ink| ink.text).unwrap_or(theme.foreground);
            let mut right_parts: Vec<AnyElement> = Vec::with_capacity(5);
            let push_sep = |parts: &mut Vec<AnyElement>| {
                if !parts.is_empty() {
                    parts.push(div().text_color(detail_color).child("·").into_any_element());
                }
            };
            if let Some(pct) = percent_text {
                right_parts.push(
                    div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(percent_color)
                        .child(pct)
                        .into_any_element(),
                );
            }
            if let Some(prep) = preparing_text(&progress) {
                push_sep(&mut right_parts);
                right_parts.push(
                    div()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(detail_color)
                        .child(prep)
                        .into_any_element(),
                );
            } else {
                match (progress.bytes_done, progress.bytes_total) {
                    (Some(d), Some(t)) if t > 0 => {
                        push_sep(&mut right_parts);
                        right_parts.push(
                            h_flex()
                                .items_baseline()
                                .gap_1()
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(value_color)
                                        .child(format_bytes(d)),
                                )
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .text_color(detail_color)
                                        .child(format!("/ {}", format_bytes(t))),
                                )
                                .into_any_element(),
                        );
                        if t > d {
                            push_sep(&mut right_parts);
                            right_parts.push(
                                div()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(detail_color)
                                    .child(t_args(
                                        "ui.progress.left",
                                        &[("time", &format_bytes(t - d))],
                                    ))
                                    .into_any_element(),
                            );
                        }
                    }
                    (Some(d), _) if d > 0 => {
                        push_sep(&mut right_parts);
                        right_parts.push(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(value_color)
                                .child(format_bytes(d))
                                .into_any_element(),
                        );
                    }
                    _ => {}
                }
            }
            return v_flex()
                .gap_2()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .items_baseline()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .truncate()
                                .text_color(detail_color)
                                .child(label),
                        )
                        .when(!right_parts.is_empty(), |row| {
                            row.child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_baseline()
                                    .gap_1p5()
                                    .text_sm()
                                    .children(right_parts),
                            )
                        }),
                )
                .child(crate::motion::gliding_progress(self.id, progress.percent));
        }

        let detail = progress_text(&progress);
        v_flex()
            .gap_1()
            .w_full()
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .when_some(self.ink, |d, ink| d.text_color(ink.text))
                            .child(label),
                    )
                    .child(div().text_color(detail_color).child(detail)),
            )
            .child(crate::motion::gliding_progress(self.id, progress.percent))
    }
}
