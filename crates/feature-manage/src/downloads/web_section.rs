//! Active `Links & torrents` section and web-only transfer rate card for
//! [`DownloadsView`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, rems,
};
use irori_core::util::format_bytes;
use irori_services::{WebDownload, WebDownloadKind, WebDownloadState};
use irori_ui::components::SectionTitle;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::{DownloadsView, list_card};
use crate::common::{backend_op, card};
use crate::format;

pub(super) fn web_icon(kind: WebDownloadKind) -> IconName {
    match kind {
        WebDownloadKind::DirectUrl => IconName::Link,
        WebDownloadKind::TorrentFile | WebDownloadKind::Magnet => IconName::Globe,
    }
}

fn active_status_right(item: &WebDownload, globally_paused: bool) -> SharedString {
    match item.state {
        WebDownloadState::Queued => t("manage.downloads.web.state.queued"),
        WebDownloadState::FetchingMetadata => t("manage.downloads.web.state.fetching"),
        WebDownloadState::Extracting => t("manage.downloads.web.state.extracting"),
        WebDownloadState::Installing => t("manage.downloads.web.state.installing"),
        WebDownloadState::Paused => t("manage.downloads.summary.paused"),
        WebDownloadState::Running if globally_paused => t("manage.downloads.summary.paused"),
        WebDownloadState::Running => match item.progress_percent() {
            Some(pct) => format!("{pct:.0}%").into(),
            None => t("manage.downloads.summary.downloading"),
        },
        _ => "".into(),
    }
}

fn active_meta_line(item: &WebDownload, globally_paused: bool) -> String {
    let paused = item.state == WebDownloadState::Paused
        || (globally_paused && item.state == WebDownloadState::Running);
    if paused {
        let mut parts = vec![t("manage.downloads.web.meta.paused_saved").to_string()];
        if let Some(bytes) = progress_bytes(item) {
            parts.push(bytes);
        }
        return parts.join(" · ");
    }
    match item.state {
        WebDownloadState::Queued => {
            let mut parts = vec![t("manage.downloads.web.meta.waiting").to_string()];
            if let Some(size) = format::size(item.bytes_total) {
                parts.push(size);
            }
            parts.join(" · ")
        }
        WebDownloadState::FetchingMetadata => t("manage.downloads.add.magnet.reading").to_string(),
        WebDownloadState::Extracting => t("manage.downloads.web.meta.extracting").to_string(),
        WebDownloadState::Installing => t("manage.downloads.web.meta.installing").to_string(),
        WebDownloadState::Running => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(bytes) = progress_bytes(item) {
                parts.push(bytes);
            }
            if item.download_speed > 0 {
                parts.push(format::rate(Some(item.download_speed)));
            }
            if item.kind.is_bittorrent()
                && let Some(peers) = item.peers
            {
                let count = peers.to_string();
                parts.push(
                    if peers == 1 {
                        t_args("manage.downloads.web.peers_one", &[("count", &count)])
                    } else {
                        t_args("manage.downloads.web.peers_other", &[("count", &count)])
                    }
                    .to_string(),
                );
            }
            if let Some(eta) = item.eta_secs.filter(|&s| s > 0) {
                parts.push(
                    t_args(
                        "manage.downloads.pill.left",
                        &[("time", &format::eta(Some(eta)))],
                    )
                    .to_string(),
                );
            }
            if parts.is_empty() {
                t("manage.downloads.summary.downloading").to_string()
            } else {
                parts.join(" · ")
            }
        }
        _ => String::new(),
    }
}

fn progress_bytes(item: &WebDownload) -> Option<String> {
    if item.bytes_total > 0 {
        Some(format!(
            "{} / {}",
            format_bytes(item.bytes_done),
            format_bytes(item.bytes_total)
        ))
    } else if item.bytes_done > 0 {
        Some(format_bytes(item.bytes_done))
    } else {
        None
    }
}

impl DownloadsView {
    /// Clean aggregate speed card shown when only direct links / torrents are active.
    pub(super) fn render_web_only_speed(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.queue.has_active() {
            return None;
        }
        let web = models(cx).downloads.read(cx).web.clone();
        let transferring = web.active.iter().any(|w| w.state.is_network_active());
        if web.active.is_empty() || web.paused || !transferring {
            return None;
        }
        let theme = cx.theme();
        let chart: AnyElement = if self.speeds.len() >= 2 {
            const MIB: f64 = 1024.0 * 1024.0;
            let data: Vec<(usize, f64)> = self
                .speeds
                .samples()
                .enumerate()
                .map(|(ix, s)| (ix, s.download as f64 / MIB))
                .collect();
            AreaChart::new(data)
                .id("web-download-rates")
                .x(|d: &(usize, f64)| d.0.to_string())
                .y(|d: &(usize, f64)| d.1)
                .stroke(theme.chart_1)
                .fill(theme.chart_1.opacity(0.22))
                .name(t("manage.downloads.chart.download"))
                .x_axis(false)
                .y_axis(false)
                .grid(false)
                .point_count(self.speeds.capacity())
                .appear(false)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t("manage.downloads.measuring"))
                .into_any_element()
        };
        Some(
            card(cx)
                .id("web-only-speed")
                .child(
                    h_flex()
                        .items_baseline()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(theme.muted_foreground)
                                .child(t("manage.downloads.web.transfer_rate")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(theme.primary)
                                .child(format::rate(Some(web.total_speed))),
                        ),
                )
                .child(div().w_full().h(rems(3.75)).child(chart))
                .into_any_element(),
        )
    }

    /// Active `Links & torrents` section: only active/queued/fetching/paused/extracting/installing items.
    pub(super) fn render_web_active(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let web = models(cx).downloads.read(cx).web.clone();
        if web.active.is_empty() {
            return None;
        }
        let max_slots = models(cx)
            .settings
            .read(cx)
            .app
            .web_downloads
            .max_concurrent
            .clamp(1, 5);
        let globally_paused = web.paused;
        let rows: Vec<AnyElement> = web
            .active
            .iter()
            .enumerate()
            .map(|(ix, item)| self.render_active_web_row(item, ix, globally_paused, cx))
            .collect();
        let active_slots = web
            .active
            .iter()
            .filter(|w| w.state.is_network_active())
            .count();
        let count_text = t_args(
            "manage.downloads.web.slots_count",
            &[
                ("active", &active_slots.to_string()),
                ("max", &max_slots.to_string()),
            ],
        );
        let muted = cx.theme().muted_foreground;
        Some(
            v_flex()
                .gap_2()
                .child(
                    SectionTitle::new(t("manage.downloads.web.section_title"))
                        .trailing(div().text_xs().text_color(muted).child(count_text)),
                )
                .child(list_card(cx).children(rows))
                .into_any_element(),
        )
    }

    fn render_active_web_row(
        &self,
        item: &WebDownload,
        ix: usize,
        globally_paused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let hover_bg = theme.muted.opacity(0.35);
        let badge_bg = theme.muted;
        let radius_lg = theme.radius_lg;
        let fg = theme.foreground;
        let id = item.id.clone();
        let key = id.as_str().to_string();
        let paused = item.state == WebDownloadState::Paused
            || (globally_paused && item.state == WebDownloadState::Running);
        let pausable = matches!(
            item.state,
            WebDownloadState::Queued
                | WebDownloadState::FetchingMetadata
                | WebDownloadState::Running
                | WebDownloadState::Paused
        );
        let show_bar = !matches!(item.state, WebDownloadState::Queued) || item.bytes_done > 0;
        let progress_pct = match item.state {
            WebDownloadState::FetchingMetadata
            | WebDownloadState::Extracting
            | WebDownloadState::Installing => None,
            _ => item.progress_percent(),
        };
        let status_right = active_status_right(item, globally_paused);
        let meta = active_meta_line(item, globally_paused);
        let toggle_id = id.clone();
        let menu_id = id.clone();
        let title = item.title.clone();

        h_flex()
            .id(ElementId::Name(format!("web-active-{key}").into()))
            .debug_selector(move || format!("web-active-{key}"))
            .gap_3()
            .px_4()
            .py_3p5()
            .items_center()
            .when(ix > 0, |d| d.border_t_1().border_color(border))
            .hover(move |s| s.bg(hover_bg))
            .child(
                div()
                    .size(rems(2.25))
                    .flex_shrink_0()
                    .rounded(radius_lg)
                    .bg(badge_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(web_icon(item.kind)).small().text_color(muted)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_baseline()
                            .justify_between()
                            .gap_3()
                            .child(div().text_sm().font_semibold().truncate().child(title))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if paused { muted } else { fg })
                                    .child(status_right),
                            ),
                    )
                    .when(show_bar, |col| {
                        let bar = irori_ui::motion::gliding_progress(
                            ElementId::Name(format!("web-progress-{}", id.as_str()).into()),
                            progress_pct,
                        );
                        col.child(div().w_full().when(paused, |d| d.opacity(0.45)).child(bar))
                    })
                    .child(div().text_xs().text_color(muted).truncate().child(meta)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .items_center()
                    .when(pausable, |row| {
                        row.child(
                            Button::new(ElementId::Name(
                                format!("web-toggle-{}", id.as_str()).into(),
                            ))
                            .ghost()
                            .small()
                            .icon(if paused {
                                IconName::Play
                            } else {
                                IconName::Pause
                            })
                            .tooltip(if paused {
                                t("manage.downloads.web.resume_one")
                            } else {
                                t("manage.downloads.web.pause_one")
                            })
                            .on_click(move |_, _, cx| {
                                let id = toggle_id.clone();
                                if paused {
                                    backend_op(cx, move |b| b.resume_web_download(&id));
                                } else {
                                    backend_op(cx, move |b| b.pause_web_download(&id));
                                }
                            }),
                        )
                    })
                    .child(
                        Button::new(ElementId::Name(
                            format!("web-active-menu-{}", id.as_str()).into(),
                        ))
                        .ghost()
                        .small()
                        .icon(IconName::EllipsisVertical)
                        .tooltip(t("manage.downloads.web.more"))
                        .dropdown_menu(move |mut menu, _, _| {
                            if pausable {
                                let toggle = menu_id.clone();
                                menu = menu
                                    .item(
                                        PopupMenuItem::new(if paused {
                                            t("manage.downloads.web.resume_one")
                                        } else {
                                            t("manage.downloads.web.pause_one")
                                        })
                                        .icon(if paused {
                                            IconName::Play
                                        } else {
                                            IconName::Pause
                                        })
                                        .on_click(
                                            move |_, _, cx| {
                                                let id = toggle.clone();
                                                if paused {
                                                    backend_op(cx, move |b| {
                                                        b.resume_web_download(&id)
                                                    });
                                                } else {
                                                    backend_op(cx, move |b| {
                                                        b.pause_web_download(&id)
                                                    });
                                                }
                                            },
                                        ),
                                    )
                                    .separator();
                            }
                            let keep_id = menu_id.clone();
                            let delete_id = menu_id.clone();
                            menu.item(
                                PopupMenuItem::new(t("manage.downloads.web.cancel_keep"))
                                    .icon(IconName::X)
                                    .on_click(move |_, _, cx| {
                                        let id = keep_id.clone();
                                        backend_op(cx, move |b| b.cancel_web_download(&id, false));
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(t("manage.downloads.web.cancel_delete"))
                                    .icon(IconName::Trash)
                                    .on_click(move |_, _, cx| {
                                        let id = delete_id.clone();
                                        backend_op(cx, move |b| b.cancel_web_download(&id, true));
                                    }),
                            )
                        }),
                    ),
            )
            .into_any_element()
    }
}
