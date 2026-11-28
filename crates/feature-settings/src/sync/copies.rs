//! Card 2 of `Sync`: cloud copies of Irori backups, Ludusavi save backups,
//! and screenshots, with storage breakdown bar and per-kind mode controls.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, SharedString, Styled as _, div, px, relative,
};
use irori_core::CopyMode;
use irori_core::util::format_bytes;
use irori_services::backup_schedule::local_time;
use irori_services::sync::copies::{CopyKind, CopySize, CopyStatus};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::widgets::group_panel;

use super::{SyncPanel, SyncPanelEvent, kind_label};

const COPY_MODES: [CopyMode; 3] = [CopyMode::Off, CopyMode::Manual, CopyMode::Automatic];

/// "1.2 GB in 40 files · 300 MB to send" for one kind, or what it says when
/// it holds nothing or everything is sent.
fn size_text(size: &CopySize) -> SharedString {
    if size.files == 0 {
        return t("settings.sync.copies.size_empty");
    }
    let held = t_args(
        if size.files == 1 {
            "settings.sync.copies.size_one"
        } else {
            "settings.sync.copies.size_other"
        },
        &[
            ("size", &format_bytes(size.bytes)),
            ("count", &size.files.to_string()),
        ],
    );
    let waiting = if size.waiting_files == 0 {
        t("settings.sync.copies.all_sent")
    } else {
        t_args(
            "settings.sync.copies.waiting",
            &[("size", &format_bytes(size.waiting_bytes))],
        )
    };
    t_args(
        "settings.sync.copies.size_line",
        &[("held", &held), ("waiting", &waiting)],
    )
}

impl SyncPanel {
    pub(super) fn render_copies_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let statuses = backend(cx).cloud_copies_status();
        let any_on = statuses.iter().any(|s| s.mode != CopyMode::Off);
        let ludusavi_missing = matches!(self.ludusavi.read(cx).info(), Some(None));

        let header = h_flex()
            .w_full()
            .pt_4()
            .pb_1()
            .gap_2()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(irori_ui::style::caption(
                t("settings.sync.copies.title"),
                cx,
            ))
            .child(
                Button::new("copies-send")
                    .outline()
                    .xsmall()
                    .icon(IconName::CloudUpload)
                    .label(if self.sending {
                        t("settings.sync.copies.sending")
                    } else {
                        t("settings.sync.copies.send_now")
                    })
                    .loading(self.sending)
                    .disabled(self.sending || !any_on)
                    .on_click(cx.listener(|this, _, window, cx| this.send_copies(window, cx))),
            );

        let mut summary = v_flex().w_full().px_4().py_3().gap_2p5().child(
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(t("settings.sync.copies.description")),
        );
        if let Some(bar) = self.render_copies_bar(cx) {
            summary = summary.child(bar);
        }

        let mut card = group_panel(cx).child(summary);
        for (row, status) in statuses.iter().enumerate() {
            card = card.child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(self.render_copy_row(row, status, ludusavi_missing, cx)),
            );
        }

        v_flex().w_full().gap_1().child(header).child(card)
    }

    /// The kinds' shares of what the cloud copies hold here, as one bar with
    /// a legend; `None` before they are measured or when they hold nothing.
    fn render_copies_bar(&self, cx: &Context<Self>) -> Option<gpui_kit::Div> {
        let sizes = self.sizes.as_ref()?;
        let total: u64 = sizes.iter().map(|s| s.bytes).sum();
        if total == 0 {
            return None;
        }
        let theme = cx.theme();
        let colors = [theme.chart_1, theme.chart_2, theme.chart_3];
        let (track, muted_fg) = (theme.muted, theme.muted_foreground);
        let bar = h_flex()
            .w_full()
            .h(px(8.))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(track)
            .children(sizes.iter().zip(colors).map(|(size, color)| {
                div()
                    .h_full()
                    .w(relative(size.bytes as f32 / total as f32))
                    .bg(color)
            }));
        let legend = h_flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .text_xs()
            .text_color(muted_fg)
            .children(sizes.iter().zip(colors).map(|(size, color)| {
                h_flex()
                    .gap_1p5()
                    .child(div().size_2p5().rounded_full().bg(color))
                    .child(t_args(
                        "settings.sync.copies.legend",
                        &[
                            ("what", &kind_label(size.kind)),
                            ("size", &format_bytes(size.bytes)),
                        ],
                    ))
            }));
        Some(v_flex().gap_1p5().child(bar).child(legend))
    }

    fn render_copy_row(
        &self,
        row: usize,
        status: &CopyStatus,
        ludusavi_missing: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let muted_fg = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let kind = status.kind;
        let size = self
            .sizes
            .as_ref()
            .and_then(|s| s.iter().find(|s| s.kind == kind))
            .map(size_text);
        let modes = ButtonGroup::new(SharedString::from(format!("copies-mode-{row}")))
            .outline()
            .xsmall()
            .children(COPY_MODES.iter().enumerate().map(|(ix, m)| {
                let label = match m {
                    CopyMode::Off => t("settings.sync.copies.off"),
                    CopyMode::Manual => t("settings.sync.copies.manual"),
                    CopyMode::Automatic => t("settings.sync.copies.automatic"),
                };
                Button::new(SharedString::from(format!("copies-mode-{row}-{ix}")))
                    .label(label)
                    .selected(status.mode == *m)
            }))
            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                if let Some(&ix) = clicked.first() {
                    this.set_copy_mode(kind, COPY_MODES[ix], cx);
                }
            }));
        let last = status.last.as_ref().map(|last| match &last.error {
            Some(e) => SharedString::from(e.clone()),
            None => t_args(
                "settings.sync.copies.last",
                &[
                    ("time", &local_time(&last.at)),
                    ("sent", &last.sent.to_string()),
                    ("kept", &(last.sent + last.unchanged).to_string()),
                ],
            ),
        });
        let show_setup_ludusavi = kind == CopyKind::Saves && ludusavi_missing;

        h_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(kind_label(kind)),
                            )
                            .when(show_setup_ludusavi, |r| {
                                r.child(
                                    Button::new("copies-setup-ludusavi")
                                        .ghost()
                                        .xsmall()
                                        .text_color(warning)
                                        .label(t("settings.sync.copies.setup_ludusavi"))
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            cx.emit(SyncPanelEvent::OpenSaves);
                                        })),
                                )
                            }),
                    )
                    .when_some(size, |col, size| {
                        col.child(div().text_xs().text_color(muted_fg).child(size))
                    })
                    .when_some(last, |col, last| {
                        col.child(div().text_xs().text_color(muted_fg).child(last))
                    }),
            )
            .child(modes)
    }
}
