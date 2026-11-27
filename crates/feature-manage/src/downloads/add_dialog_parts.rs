//! Sub-sections of the `+ Add download` modal: opt-in BitTorrent consent and
//! `aria2c` readiness card, torrent/magnet file picker, and collapsible options.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_core::util::format_bytes;
use irori_services::Aria2Status;
use irori_ui::i18n::{t, t_args};

use super::add_dialog::{AddDownloadDialog, PreviewState};

impl AddDownloadDialog {
    pub(super) fn options_summary(&self) -> SharedString {
        let mut parts: Vec<SharedString> = Vec::new();
        if self.auto_extract {
            parts.push(t("manage.downloads.add.summary.extract"));
            if self.delete_archive_after_extract {
                parts.push(t("manage.downloads.add.summary.trash_archive"));
            }
        }
        if self.auto_add_to_library {
            parts.push(t("manage.downloads.add.summary.add_library"));
        }
        if parts.is_empty() {
            t("manage.downloads.add.summary.none")
        } else {
            parts.join(" · ").into()
        }
    }

    pub(super) fn render_bt_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_bt_source() {
            return None;
        }
        let theme = cx.theme();
        let radius_lg = theme.radius_lg;
        let radius = theme.radius;
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let group_box = theme.group_box;
        let fg = theme.foreground;

        if !self.bittorrent_enabled(cx) {
            return Some(
                h_flex()
                    .p_3()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .rounded(radius_lg)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(t("manage.downloads.add.bt.enable_title")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("manage.downloads.add.bt.consent")),
                            ),
                    )
                    .child(
                        Button::new("add-bt-enable")
                            .primary()
                            .small()
                            .label(t("manage.downloads.add.bt.enable"))
                            .on_click(cx.listener(|this, _, _, cx| this.enable_bittorrent(cx))),
                    )
                    .into_any_element(),
            );
        }
        if self.checking_aria2 {
            return Some(
                h_flex()
                    .p_3()
                    .gap_2p5()
                    .items_center()
                    .rounded(radius_lg)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .child(Spinner::new().small())
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.downloads.add.bt.checking")),
                    )
                    .into_any_element(),
            );
        }
        let Some(Aria2Status::Missing {
            can_auto_install,
            install_hint,
        }) = &self.aria2_status
        else {
            return None;
        };
        let can_auto = *can_auto_install;
        let hint = install_hint.clone();
        let action: AnyElement = if can_auto {
            Button::new("add-aria2-install")
                .primary()
                .small()
                .label(t("manage.downloads.add.bt.install_aria2"))
                .loading(self.installing_aria2)
                .disabled(self.installing_aria2)
                .on_click(cx.listener(|this, _, window, cx| this.install_aria2(window, cx)))
                .into_any_element()
        } else {
            Button::new("add-aria2-recheck")
                .outline()
                .small()
                .label(t("manage.downloads.add.bt.check_again"))
                .on_click(cx.listener(|this, _, window, cx| this.check_aria2_status(window, cx)))
                .into_any_element()
        };
        let body = if can_auto {
            t("manage.downloads.add.bt.aria2_auto_body")
        } else {
            t("manage.downloads.add.bt.aria2_manual_body")
        };
        Some(
            v_flex()
                .p_3()
                .gap_2()
                .rounded(radius_lg)
                .bg(bg)
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_medium()
                                        .child(t("manage.downloads.add.bt.aria2_missing_title")),
                                )
                                .child(div().text_xs().text_color(muted).child(body)),
                        )
                        .child(action),
                )
                .when(!can_auto && !hint.trim().is_empty(), |box_el| {
                    box_el.child(
                        div()
                            .px_2p5()
                            .py_1p5()
                            .rounded(radius)
                            .bg(group_box)
                            .border_1()
                            .border_color(border)
                            .text_xs()
                            .text_color(fg)
                            .child(hint),
                    )
                })
                .into_any_element(),
        )
    }

    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let radius_lg = theme.radius_lg;
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let danger = theme.danger;

        match &self.preview {
            PreviewState::Idle => None,
            PreviewState::Loading { is_magnet, .. } => {
                Some(
                    h_flex()
                        .p_3()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .rounded(radius_lg)
                        .bg(bg)
                        .border_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .gap_2p5()
                                .items_center()
                                .min_w_0()
                                .child(Spinner::new().small())
                                .child(div().text_xs().font_medium().truncate().child(
                                    if *is_magnet {
                                        t("manage.downloads.add.magnet.reading")
                                    } else {
                                        t("manage.downloads.add.torrent.reading")
                                    },
                                )),
                        )
                        .child(
                            Button::new("add-preview-cancel")
                                .outline()
                                .xsmall()
                                .label(t("manage.downloads.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_preview(cx))),
                        )
                        .into_any_element(),
                )
            }
            PreviewState::Error { message, .. } => Some(
                h_flex()
                    .p_3()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .rounded(radius_lg)
                    .bg(bg)
                    .border_1()
                    .border_color(danger.opacity(0.4))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(danger)
                            .line_clamp(2)
                            .child(message.clone()),
                    )
                    .child(
                        Button::new("add-preview-retry")
                            .outline()
                            .xsmall()
                            .label(t("manage.downloads.history.retry"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.preview = PreviewState::Idle;
                                this.maybe_start_preview(true, window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
            PreviewState::Ready {
                preview, selected, ..
            } => {
                let selected_bytes: u64 = preview
                    .files
                    .iter()
                    .filter(|f| selected.contains(&f.index))
                    .map(|f| f.length)
                    .fold(0u64, u64::saturating_add);
                let rows: Vec<AnyElement> = preview
                    .files
                    .iter()
                    .enumerate()
                    .map(|(ix, entry)| {
                        let index = entry.index;
                        let checked = selected.contains(&index);
                        h_flex()
                            .id(ElementId::Name(format!("torrent-file-{index}").into()))
                            .px_3()
                            .py_1p5()
                            .gap_2()
                            .items_center()
                            .justify_between()
                            .when(ix > 0, |d| d.border_t_1().border_color(border))
                            .child(
                                Checkbox::new(SharedString::from(format!("torrent-check-{index}")))
                                    .checked(checked)
                                    .label(entry.path.clone())
                                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                                        if let PreviewState::Ready { selected, .. } =
                                            &mut this.preview
                                        {
                                            if *on {
                                                selected.insert(index);
                                            } else {
                                                selected.remove(&index);
                                            }
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format_bytes(entry.length)),
                            )
                            .into_any_element()
                    })
                    .collect();
                Some(
                    v_flex()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .text_xs()
                                .child(div().text_color(muted).child(t_args(
                                    "manage.downloads.add.files_selected",
                                    &[("size", &format_bytes(selected_bytes))],
                                )))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new("add-files-all")
                                                .ghost()
                                                .xsmall()
                                                .label(t("manage.downloads.add.files_all"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let PreviewState::Ready {
                                                        preview,
                                                        selected,
                                                        ..
                                                    } = &mut this.preview
                                                    {
                                                        *selected = preview
                                                            .files
                                                            .iter()
                                                            .map(|f| f.index)
                                                            .collect();
                                                        cx.notify();
                                                    }
                                                })),
                                        )
                                        .child(
                                            Button::new("add-files-none")
                                                .ghost()
                                                .xsmall()
                                                .label(t("manage.downloads.add.files_none"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let PreviewState::Ready {
                                                        selected, ..
                                                    } = &mut this.preview
                                                    {
                                                        selected.clear();
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("torrent-files-list")
                                .max_h(rems(10.5))
                                .overflow_y_scrollbar()
                                .rounded(radius_lg)
                                .bg(bg)
                                .border_1()
                                .border_color(border)
                                .child(v_flex().children(rows)),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    pub(super) fn render_more_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let radius_lg = theme.radius_lg;
        let border = theme.border;
        let muted_bg = theme.muted.opacity(0.35);
        let muted = theme.muted_foreground;
        let body_bg = theme.background.opacity(0.6);
        let more_open = self.more_open;
        let auto_extract = self.auto_extract;
        let trash_after = self.delete_archive_after_extract;
        let auto_add = self.auto_add_to_library;

        v_flex()
            .rounded(radius_lg)
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(
                h_flex()
                    .id("add-more-toggle")
                    .px_3()
                    .py_2()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .hover(move |s| s.bg(muted_bg))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more_open = !this.more_open;
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .font_medium()
                            .text_color(muted)
                            .child(
                                Icon::new(if more_open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall(),
                            )
                            .child(t("manage.downloads.add.more_options")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(self.options_summary()),
                    ),
            )
            .when(more_open, |box_el| {
                box_el.child(
                    v_flex()
                        .px_3()
                        .py_2p5()
                        .gap_2p5()
                        .border_t_1()
                        .border_color(border)
                        .bg(body_bg)
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .child(t("manage.downloads.add.option.extract")),
                                )
                                .child(
                                    Switch::new("add-opt-extract")
                                        .small()
                                        .checked(auto_extract)
                                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                                            this.auto_extract = *on;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t("manage.downloads.add.option.trash_archive")),
                                )
                                .child(
                                    Switch::new("add-opt-trash")
                                        .small()
                                        .checked(auto_extract && trash_after)
                                        .disabled(!auto_extract)
                                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                                            this.delete_archive_after_extract = *on;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .child(t("manage.downloads.add.option.add_library")),
                                )
                                .child(
                                    Switch::new("add-opt-add-library")
                                        .small()
                                        .checked(auto_add)
                                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                                            this.auto_add_to_library = *on;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}
