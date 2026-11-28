//! Card 3 of `Sync`: unified Other computers card (merging previously
//! duplicated status & cloud-copies device lists) plus optional Conflicts card.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Context, IntoElement, ParentElement as _, Styled as _, div};
use irori_services::SyncDevice;
use irori_services::backup_schedule::local_time;
use irori_services::sync::copies::CopyKind;
use irori_ui::i18n::{t, t_args};

use crate::widgets::group_panel;

use super::{SyncPanel, kind_label, os_name};

/// One entry in the unified devices list, tracking whether cloud copies have
/// been discovered for bring-back actions.
struct UnifiedDevice {
    device: SyncDevice,
    has_cloud_copies: bool,
}

impl SyncPanel {
    fn merged_devices(&self) -> Vec<UnifiedDevice> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        if let Some(cloud_devices) = &self.devices {
            for d in cloud_devices {
                if seen.insert(d.id.clone()) {
                    out.push(UnifiedDevice {
                        device: d.clone(),
                        has_cloud_copies: true,
                    });
                }
            }
        }
        if let Some(last) = &self.status.last {
            for d in &last.devices {
                if seen.insert(d.id.clone()) {
                    out.push(UnifiedDevice {
                        device: d.clone(),
                        has_cloud_copies: false,
                    });
                }
            }
        }
        out
    }

    pub(super) fn render_devices_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let devices = self.merged_devices();
        let copies_loaded = self.devices.is_some();

        let header = h_flex()
            .w_full()
            .pt_4()
            .pb_1()
            .gap_2()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(irori_ui::style::caption(
                t("settings.sync.devices_title"),
                cx,
            ))
            .child(
                Button::new("copies-find")
                    .outline()
                    .xsmall()
                    .icon(IconName::CloudDownload)
                    .label(t("settings.sync.copies.find"))
                    .loading(self.finding)
                    .disabled(self.finding)
                    .on_click(cx.listener(|this, _, window, cx| this.find_devices(window, cx))),
            );

        let mut card = group_panel(cx);
        if devices.is_empty() {
            let empty_key = if copies_loaded {
                "settings.sync.copies.no_devices"
            } else {
                "settings.sync.no_devices"
            };
            card = card.child(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t(empty_key)),
            );
        } else {
            for (row, entry) in devices.iter().enumerate() {
                let d = &entry.device;
                let has_copies = entry.has_cloud_copies;
                card = card.child(
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_2p5()
                        .gap_3()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .when(row > 0, |r| r.border_t_1().border_color(border))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2p5()
                                .items_center()
                                .child(Icon::new(IconName::Gamepad).text_color(muted_fg))
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
                                                .child(d.name.clone()),
                                        )
                                        .child(
                                            div().text_xs().text_color(muted_fg).truncate().child(
                                                t_args(
                                                    "settings.sync.device_seen",
                                                    &[
                                                        ("os", os_name(&d.os)),
                                                        ("time", &local_time(&d.updated)),
                                                    ],
                                                ),
                                            ),
                                        ),
                                ),
                        )
                        .child(if has_copies {
                            h_flex()
                                .gap_1p5()
                                .flex_wrap()
                                .items_center()
                                .children(CopyKind::ALL.iter().enumerate().map(|(i, kind)| {
                                    let (kind, device) = (*kind, d.clone());
                                    let loading = self
                                        .bringing
                                        .as_ref()
                                        .is_some_and(|(id, k)| *id == device.id && *k == kind);
                                    Button::new(("copies-bring", row * 3 + i))
                                        .outline()
                                        .xsmall()
                                        .label(kind_label(kind))
                                        .loading(loading)
                                        .disabled(self.bringing.is_some())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.bring_back(device.clone(), kind, window, cx)
                                        }))
                                }))
                                .into_any_element()
                        } else {
                            Tag::secondary()
                                .xsmall()
                                .outline()
                                .child(t("settings.sync.device_synced_badge"))
                                .into_any_element()
                        }),
                );
            }
            if copies_loaded && devices.iter().any(|e| e.has_cloud_copies) {
                card = card.child(
                    div()
                        .px_4()
                        .py_2p5()
                        .border_t_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.sync.copies.bring_hint")),
                );
            } else if copies_loaded {
                card = card.child(
                    div()
                        .px_4()
                        .py_2p5()
                        .border_t_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.sync.copies.no_devices")),
                );
            }
        }

        v_flex().w_full().gap_1().child(header).child(card)
    }

    pub(super) fn render_conflicts_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg, warning) = (theme.border, theme.muted_foreground, theme.warning);

        let header = h_flex()
            .w_full()
            .pt_4()
            .pb_1()
            .gap_2()
            .items_center()
            .justify_between()
            .child(irori_ui::style::caption(
                t("settings.sync.conflicts_title"),
                cx,
            ))
            .child(
                Button::new("sync-clear-conflicts")
                    .ghost()
                    .xsmall()
                    .label(t("settings.sync.clear"))
                    .on_click(cx.listener(|this, _, _, cx| this.clear_conflicts(cx))),
            );

        let mut card = group_panel(cx).child(
            div()
                .px_4()
                .py_2p5()
                .text_xs()
                .text_color(muted_fg)
                .child(t("settings.sync.conflicts_description")),
        );
        for conflict in &self.status.conflicts {
            card = card.child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .gap_2()
                    .items_start()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .text_color(warning)
                            .mt_0p5(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(self.conflict_text(conflict)),
                    ),
            );
        }

        v_flex().w_full().gap_1().child(header).child(card)
    }
}
