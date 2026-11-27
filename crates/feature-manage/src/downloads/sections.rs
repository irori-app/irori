//! Active transfer hero card, rate sparkline, and drag-and-drop queue section
//! for [`DownloadsView`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, rems,
};
use irori_core::util::{format_bytes, unix_now};
use irori_core::{GameId, GameStatus, InstallProgress, StatusKind};
use irori_services::{DownloadKind, DownloadState};
use irori_ui::art::ambient;
use irori_ui::components::{
    ArtBackdrop, BackdropInk, GameCover, ProgressSummary, SectionTitle, StoreBadge,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models, navigate};

use super::{DownloadsView, Step, list_card, queue};
use crate::common::{backend_op, card};
use crate::format;

impl DownloadsView {
    pub(super) fn render_current(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(current) = self.queue.current.as_ref() else {
            if self.queue.waiting.is_empty() {
                return None;
            }
            return Some(
                card(cx)
                    .child(
                        h_flex()
                            .gap_3()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::CirclePause).small())
                            .child(t("manage.downloads.current.paused_queue")),
                    )
                    .into_any_element(),
            );
        };
        let id = current.item.id.clone();
        let status = models(cx).statuses.read(cx).get(&id).cloned();
        let progress = status.as_ref().and_then(|s| s.progress);
        let backdrop_url = (!irori_ui::theme::reduce_transparency(cx))
            .then(|| current.game.hero_art().map(str::to_owned))
            .flatten();
        // Calling `ambient` warms the blur cache off-thread when missing, and
        // we only paint `ArtBackdrop` once ready so the card keeps `group_box`
        // instead of flashing `theme.background` while loading.
        let has_backdrop = backdrop_url
            .as_deref()
            .and_then(|url| ambient(url, cx))
            .is_some();
        let ink = BackdropInk::of(
            has_backdrop.then_some(backdrop_url.as_deref()).flatten(),
            cx,
        );
        let theme = cx.theme();
        let radius_lg = theme.radius_lg;
        let border = theme.border;
        let group_box = theme.group_box;
        let install_folder = match &current.item.kind {
            DownloadKind::Install(req) if !req.path.as_os_str().is_empty() => {
                Some(req.path.display().to_string())
            }
            _ => None,
        };
        let meta = [
            Some(queue::kind_label(&current.item.kind).to_string()),
            format::size(current.item.download_size),
            install_folder,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let paused_progress = progress.or(current.item.progress);
        let progress_block: AnyElement = if current.paused {
            let left_status = h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .text_sm()
                .font_medium()
                .text_color(ink.muted)
                .child(Icon::new(IconName::CirclePause).small())
                .child(div().truncate().child(t("manage.downloads.current.paused")));
            if let Some(p) = paused_progress
                && let Some(pct) = p.percent
            {
                let paused_pct_color = if has_backdrop {
                    ink.text.opacity(0.85)
                } else {
                    theme.foreground.opacity(0.8)
                };
                let mut right_parts: Vec<AnyElement> = vec![
                    div()
                        .font_bold()
                        .text_color(paused_pct_color)
                        .child(format!("{pct:.0}%"))
                        .into_any_element(),
                ];
                match (p.bytes_done, p.bytes_total) {
                    (Some(d), Some(total)) if total > 0 => {
                        right_parts.push(div().text_color(ink.muted).child("·").into_any_element());
                        right_parts.push(
                            h_flex()
                                .items_baseline()
                                .gap_1()
                                .child(
                                    div()
                                        .font_semibold()
                                        .text_color(ink.muted)
                                        .child(format_bytes(d)),
                                )
                                .child(
                                    div()
                                        .font_medium()
                                        .text_color(ink.muted.opacity(0.75))
                                        .child(format!("/ {}", format_bytes(total))),
                                )
                                .into_any_element(),
                        );
                        if total > d {
                            right_parts
                                .push(div().text_color(ink.muted).child("·").into_any_element());
                            right_parts.push(
                                div()
                                    .font_medium()
                                    .text_color(ink.muted.opacity(0.75))
                                    .child(t_args(
                                        "manage.downloads.pill.left",
                                        &[("time", &format_bytes(total - d))],
                                    ))
                                    .into_any_element(),
                            );
                        }
                    }
                    (Some(d), _) if d > 0 => {
                        right_parts.push(div().text_color(ink.muted).child("·").into_any_element());
                        right_parts.push(
                            div()
                                .font_semibold()
                                .text_color(ink.muted)
                                .child(format_bytes(d))
                                .into_any_element(),
                        );
                    }
                    _ => {}
                }
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        h_flex()
                            .w_full()
                            .items_baseline()
                            .justify_between()
                            .gap_4()
                            .child(left_status)
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_baseline()
                                    .gap_1p5()
                                    .text_sm()
                                    .children(right_parts),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .opacity(0.45)
                            .child(Progress::new("current-progress-paused").value(pct)),
                    )
                    .into_any_element()
            } else {
                left_status.into_any_element()
            }
        } else {
            let status = status.clone().unwrap_or_else(|| {
                let kind = match current.item.kind {
                    DownloadKind::Install(_) => StatusKind::Installing,
                    DownloadKind::Update(_) => StatusKind::Updating,
                };
                GameStatus::new(id.clone(), kind)
            });
            ProgressSummary::new("current-progress", status)
                .prominent()
                .when(has_backdrop, |p| p.ink(ink))
                .into_any_element()
        };

        let game_id = id.clone();
        let cover_id = id.clone();
        let title_id = id.clone();
        let paused = current.paused;
        Some(
            div()
                .id("current-download")
                .relative()
                .overflow_hidden()
                .rounded(radius_lg)
                .border_1()
                .border_color(border)
                .bg(group_box)
                .when_some(backdrop_url.filter(|_| has_backdrop), |card, url| {
                    card.child(
                        ArtBackdrop::new(Some(url))
                            .strength(0.55)
                            .dim(0.25)
                            .image_cache(&self.images),
                    )
                })
                .child(
                    v_flex().relative().p_5().gap_4().child(
                        h_flex()
                            .items_start()
                            .gap_6()
                            .child(
                                div()
                                    .id("hero-cover-open")
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Game(cover_id.clone()), cx)
                                    })
                                    .child(GameCover::new(current.game.clone()).width(rems(8.5))),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .items_start()
                                            .justify_between()
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_1()
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .min_w_0()
                                                            .child(
                                                                div()
                                                                    .id("hero-title-open")
                                                                    .cursor_pointer()
                                                                    .text_2xl()
                                                                    .font_semibold()
                                                                    .truncate()
                                                                    .when(has_backdrop, |d| {
                                                                        d.text_color(ink.text)
                                                                    })
                                                                    .on_click(move |_, _, cx| {
                                                                        navigate(
                                                                            Route::Game(
                                                                                title_id.clone(),
                                                                            ),
                                                                            cx,
                                                                        )
                                                                    })
                                                                    .child(
                                                                        current.item.title.clone(),
                                                                    ),
                                                            )
                                                            .child(
                                                                StoreBadge::new(id.runner).large(),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .truncate()
                                                            .text_color(ink.muted)
                                                            .child(meta),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .flex_shrink_0()
                                                    .gap_1p5()
                                                    .when(!paused, |row| {
                                                        row.child(
                                                            Button::new("hero-toggle-pause")
                                                                .outline()
                                                                .small()
                                                                .icon(IconName::Pause)
                                                                .tooltip(t(
                                                                    "manage.downloads.pause",
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.toggle_pause(cx)
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            Button::new("cancel-current")
                                                                .outline()
                                                                .small()
                                                                .icon(IconName::X)
                                                                .tooltip(t(
                                                                    "manage.downloads.cancel",
                                                                ))
                                                                .accessibility_label(t(
                                                                    "manage.downloads.cancel",
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |this, _, window, cx| {
                                                                        this.confirm_cancel(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                )),
                                                        )
                                                    })
                                                    .child(
                                                        Button::new("open-current")
                                                            .ghost()
                                                            .small()
                                                            .icon(IconName::ExternalLink)
                                                            .tooltip(t(
                                                                "manage.downloads.game_page",
                                                            ))
                                                            .accessibility_label(t(
                                                                "manage.downloads.game_page",
                                                            ))
                                                            .on_click(move |_, _, cx| {
                                                                navigate(
                                                                    Route::Game(game_id.clone()),
                                                                    cx,
                                                                )
                                                            }),
                                                    ),
                                            ),
                                    )
                                    .child(progress_block)
                                    .when(!current.paused, |this| {
                                        this.child(self.render_rates(
                                            progress,
                                            ink,
                                            has_backdrop,
                                            window,
                                            cx,
                                        ))
                                    })
                                    .when(current.paused, |this| {
                                        this.child(
                                            h_flex()
                                                .pt_1()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    Button::new("hero-toggle-pause")
                                                        .primary()
                                                        .icon(IconName::Play)
                                                        .label(t("manage.downloads.resume"))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.toggle_pause(cx)
                                                        })),
                                                )
                                                .child(
                                                    Button::new("cancel-current")
                                                        .outline()
                                                        .icon(IconName::X)
                                                        .label(t("manage.downloads.cancel"))
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.confirm_cancel(window, cx)
                                                            },
                                                        )),
                                                ),
                                        )
                                    }),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }

    /// Rate sparkline plus the current download/disk rates, time left and
    /// what limits the speed.
    fn render_rates(
        &self,
        progress: Option<InstallProgress>,
        ink: BackdropInk,
        has_backdrop: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let label_color = if has_backdrop {
            ink.muted
        } else {
            theme.muted_foreground
        };
        let value_color = if has_backdrop {
            ink.text
        } else {
            theme.foreground
        };
        let progress = progress.unwrap_or_default();
        let stat = |label: SharedString, value: String, dot: Option<gpui_kit::Hsla>| {
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .when_some(dot, |row, color| {
                            row.child(div().size(rems(0.45)).rounded_full().bg(color))
                        })
                        .child(div().text_xs().text_color(label_color).child(label)),
                )
                .child(
                    div()
                        .text_base()
                        .font_semibold()
                        .text_color(value_color)
                        .child(value),
                )
        };
        let chart: AnyElement = if self.speeds.len() >= 2 {
            const MIB: f64 = 1024.0 * 1024.0;
            let data: Vec<(usize, f64, f64)> = self
                .speeds
                .samples()
                .enumerate()
                .map(|(ix, s)| (ix, s.download as f64 / MIB, s.disk as f64 / MIB))
                .collect();
            AreaChart::new(data)
                .id("download-rates")
                .x(|d: &(usize, f64, f64)| d.0.to_string())
                .y(|d: &(usize, f64, f64)| d.1)
                .stroke(theme.chart_1)
                .fill(theme.chart_1.opacity(0.25))
                .name(t("manage.downloads.chart.download"))
                .y(|d: &(usize, f64, f64)| d.2)
                .stroke(theme.chart_2)
                .fill(theme.chart_2.opacity(0.0))
                .name(t("manage.downloads.chart.disk"))
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
                .text_color(label_color)
                .child(t("manage.downloads.measuring"))
                .into_any_element()
        };
        h_flex()
            .gap_6()
            .items_end()
            .child(div().flex_1().min_w_0().h(rems(5.)).child(chart))
            .child(stat(
                t("manage.downloads.stat.download"),
                format::rate(progress.download_speed),
                Some(theme.chart_1),
            ))
            .child(stat(
                t("manage.downloads.stat.disk"),
                format::rate(progress.disk_speed),
                Some(theme.chart_2),
            ))
            .child(stat(
                t("manage.downloads.stat.time_left"),
                format::eta(progress.eta_secs),
                None,
            ))
            .child(stat(
                t("manage.downloads.stat.finishes"),
                self.estimates
                    .current
                    .map(|secs| format::finish_from_now(secs).to_string())
                    .unwrap_or_else(|| "—".to_string()),
                None,
            ))
            .child(stat(
                t("manage.downloads.stat.limit"),
                format::bottleneck(progress.bottleneck).to_string(),
                None,
            ))
    }

    pub(super) fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = unix_now();
        let rows = self.queue.waiting.iter().enumerate().map(|(ix, row)| {
            let theme = cx.theme();
            let border = theme.border;
            let id = row.item.id.clone();
            let key = id.key();
            let starts = self
                .estimates
                .starts
                .get(row.waiting_ix)
                .copied()
                .flatten()
                .map(|secs| {
                    t_args(
                        "manage.downloads.queue.starts",
                        &[("when", &format::finish_from_now(secs))],
                    )
                    .to_string()
                });
            let paused_state = (row.item.state == DownloadState::Paused).then(|| {
                match row.item.progress.and_then(|p| p.percent) {
                    Some(pct) => format!("{} · {pct:.0}%", t("manage.downloads.summary.paused")),
                    None => t("manage.downloads.summary.paused").to_string(),
                }
            });
            let meta = [
                Some(queue::kind_label(&row.item.kind).to_string()),
                paused_state,
                format::size(row.item.download_size),
                starts,
                Some(
                    t_args(
                        "manage.downloads.queue.added",
                        &[("time", &format::relative_time(row.item.added_at, now))],
                    )
                    .to_string(),
                ),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let dragged = DraggedDownload {
                id: id.clone(),
                title: row.item.title.clone().into(),
            };
            let (prioritize_id, up_id, down_id, remove_id, drop_id, page_id) = (
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
            );
            h_flex()
                .id(ElementId::Name(format!("queue-{key}").into()))
                .debug_selector(|| format!("queue-{key}"))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .hover(|s| s.bg(theme.muted.opacity(0.35)))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedDownload>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                .on_drop(cx.listener(move |this, d: &DraggedDownload, _, cx| {
                    this.drop_item(&d.id, &drop_id, cx)
                }))
                .child(
                    Icon::new(IconName::GripVertical)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    h_flex()
                        .id(ElementId::Name(format!("queue-open-{key}").into()))
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| navigate(Route::Game(page_id.clone()), cx))
                        .child(GameCover::new(row.game.clone()).width(rems(3.5)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_semibold()
                                                .truncate()
                                                .child(row.item.title.clone()),
                                        )
                                        .child(StoreBadge::new(id.runner).medium()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(meta),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            Button::new("download-now")
                                .ghost()
                                .small()
                                .icon(IconName::Play)
                                .tooltip(t("manage.downloads.queue.download_now"))
                                .accessibility_label(t("manage.downloads.queue.download_now"))
                                .on_click(move |_, _, cx| {
                                    let id = prioritize_id.clone();
                                    backend_op(cx, move |b| b.prioritize_download(&id));
                                }),
                        )
                        .child(
                            Button::new("move-up")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowUp)
                                .tooltip(t("manage.downloads.queue.move_up"))
                                .disabled(row.is_first)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_item(&up_id, Step::Up, cx)
                                })),
                        )
                        .child(
                            Button::new("move-down")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowDown)
                                .tooltip(t("manage.downloads.queue.move_down"))
                                .disabled(row.is_last)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_item(&down_id, Step::Down, cx)
                                })),
                        )
                        .child(
                            Button::new("remove")
                                .ghost()
                                .small()
                                .icon(IconName::X)
                                .tooltip(t("manage.downloads.queue.remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_item(&remove_id, window, cx)
                                })),
                        ),
                )
        });
        v_flex()
            .gap_2()
            .child(SectionTitle::new(t_args(
                "manage.downloads.queue.title",
                &[("count", &self.queue.waiting.len().to_string())],
            )))
            .child(list_card(cx).children(rows))
    }
}

/// Drag payload and preview of a queued download.
#[derive(Clone)]
struct DraggedDownload {
    id: GameId,
    title: SharedString,
}

impl Render for DraggedDownload {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .text_sm()
            .child(self.title.clone())
    }
}
