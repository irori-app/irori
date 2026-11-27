//! Offline files and finished download history sections for [`DownloadsView`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_core::StatusKind;
use irori_core::util::unix_now;
use irori_services::{DownloadState, FileState};
use irori_ui::components::{GameCover, SectionTitle, StoreBadge};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

use super::{DownloadsView, HistoryFilter, list_card, queue};
use crate::common::backend_op;
use crate::format;

/// History rows rendered per "Show more" step.
pub(super) const HISTORY_PAGE: usize = 25;

impl DownloadsView {
    /// Offline installers and goodies being or recently downloaded.
    pub(super) fn render_files(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = models(cx).downloads.read(cx).files.clone();
        if files.is_empty() {
            return None;
        }
        let library = models(cx).library.read(cx).snapshot.clone();
        let any_finished = files.iter().any(|f| !f.is_active());
        let rows = files.iter().rev().enumerate().map(|(ix, f)| {
            let theme = cx.theme();
            let border = theme.border;
            let key = f.key.clone();
            let game = library.get(&f.game).cloned();
            let name = f.name.clone().unwrap_or_else(|| f.file.name.clone());
            let (state, color) = match f.state {
                FileState::Queued => (t("manage.downloads.files.queued"), theme.muted_foreground),
                FileState::Running => (t("manage.downloads.files.running"), theme.muted_foreground),
                FileState::Done => (t("manage.downloads.history.done"), theme.success),
                FileState::Failed => (t("manage.downloads.history.failed"), theme.danger),
                FileState::Canceled => (
                    t("manage.downloads.history.canceled"),
                    theme.muted_foreground,
                ),
            };
            let progress = (f.state == FileState::Running && f.file.size > 0)
                .then(|| (f.downloaded as f32 / f.file.size as f32 * 100.0).clamp(0.0, 100.0));
            let meta = [
                game.as_ref().map(|g| g.title.clone()),
                format::size(f.file.size),
                Some(match progress {
                    Some(p) => format!("{state} · {p:.0}%"),
                    None => state.to_string(),
                }),
                f.error.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let path = f.path().filter(|_| f.state == FileState::Done);
            let retry = (matches!(f.state, FileState::Failed | FileState::Canceled))
                .then(|| (f.game.clone(), f.file.clone(), f.folder.clone()));
            let cancel_key = key.clone();
            h_flex()
                .id(ElementId::Name(format!("file-{key}").into()))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .child(match game {
                    Some(g) => GameCover::new(g).width(rems(3.5)).into_any_element(),
                    None => div().w(rems(3.5)).into_any_element(),
                })
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_base().font_semibold().truncate().child(name))
                        .child(div().text_sm().text_color(color).truncate().child(meta))
                        .children(progress.map(|p| {
                            irori_ui::motion::gliding_progress(
                                ElementId::Name(format!("file-progress-{key}").into()),
                                Some(p),
                            )
                        })),
                )
                .when(f.is_active(), |d| {
                    d.child(
                        Button::new(ElementId::Name(format!("file-cancel-{key}").into()))
                            .ghost()
                            .small()
                            .label(t("manage.downloads.files.cancel"))
                            .on_click(move |_, _, cx| backend(cx).cancel_extra(&cancel_key)),
                    )
                })
                .children(path.map(|path| {
                    Button::new(ElementId::Name(format!("file-show-{key}").into()))
                        .ghost()
                        .small()
                        .label(t("manage.downloads.files.show"))
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children(retry.map(|(game, file, folder)| {
                    Button::new(ElementId::Name(format!("file-retry-{key}").into()))
                        .ghost()
                        .small()
                        .label(t("manage.downloads.history.retry"))
                        .on_click(move |_, _, cx| {
                            backend(cx).download_extra(game.clone(), file.clone(), folder.clone())
                        })
                }))
        });
        Some(
            v_flex()
                .gap_2()
                .child(SectionTitle::new(t("manage.downloads.files.title")).when(
                    any_finished,
                    |s| {
                        s.trailing(
                            Button::new("clear-files")
                                .ghost()
                                .small()
                                .label(t("manage.downloads.files.clear"))
                                .on_click(|_, _, cx| backend(cx).clear_finished_extras()),
                        )
                    },
                ))
                .child(list_card(cx).children(rows))
                .into_any_element(),
        )
    }

    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = unix_now();
        let filter = self.history_filter;
        let total = self.queue.history_in(filter).count();
        let shown: Vec<_> = {
            let statuses = models(cx).statuses.read(cx);
            self.queue
                .history_in(filter)
                .take(self.history_shown)
                .map(|row| (row, statuses.get(&row.item.id).map(|s| s.kind)))
                .collect()
        };
        let rows = shown.into_iter().enumerate().map(|(ix, (row, status))| {
            // Launching or already running: Play waits, or is not offered.
            let launching = status == Some(StatusKind::Launching);
            let playable = row.is_playable()
                && !status.is_some_and(|k| k.is_busy() && k != StatusKind::Launching);
            let theme = cx.theme();
            let border = theme.border;
            let id = row.item.id.clone();
            let key = format!("{}-{}", id.key(), row.item.finished_at.unwrap_or(0));
            let (icon, color, pill_bg, state) = match row.item.state {
                DownloadState::Done => (
                    IconName::CircleCheck,
                    theme.success,
                    theme.success.opacity(0.14),
                    t("manage.downloads.history.done"),
                ),
                DownloadState::Failed => (
                    IconName::CircleX,
                    theme.danger,
                    theme.danger.opacity(0.14),
                    t("manage.downloads.history.failed"),
                ),
                _ => (
                    IconName::Ban,
                    theme.muted_foreground,
                    theme.muted,
                    t("manage.downloads.history.canceled"),
                ),
            };
            let when = row
                .item
                .finished_at
                .map(|at| format::relative_time(at, now).to_string())
                .unwrap_or_default();
            let meta = [
                Some(queue::kind_label(&row.item.kind).to_string()),
                format::size(row.item.download_size),
                (!when.is_empty()).then_some(when),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let (retry_id, remove_id, page_id, open_id, play_id) =
                (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
            let play_title = row.item.title.clone();
            let retryable = row.is_retryable();
            let is_done = row.item.state == DownloadState::Done;
            let open_key = format!("history-open-{key}");
            let open_sel = open_key.clone();
            h_flex()
                .id(ElementId::Name(format!("history-{key}").into()))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .hover(|s| s.bg(theme.muted.opacity(0.35)))
                .child(
                    h_flex()
                        .id(ElementId::Name(open_key.into()))
                        .debug_selector(move || open_sel.clone())
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| navigate(Route::Game(open_id.clone()), cx))
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
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .flex_shrink_0()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(pill_bg)
                                                .text_xs()
                                                .font_medium()
                                                .text_color(color)
                                                .child(Icon::new(icon).xsmall().text_color(color))
                                                .child(state),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .truncate()
                                                .child(meta),
                                        ),
                                )
                                .when_some(row.item.error.clone(), |this, err| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.danger)
                                            .line_clamp(2)
                                            .child(err),
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .flex_shrink_0()
                        .when(retryable, |this| {
                            this.child(
                                Button::new("retry")
                                    .small()
                                    .outline()
                                    .icon(IconName::RotateCcw)
                                    .label(t("manage.downloads.history.retry"))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.retry(&retry_id, cx)
                                        }),
                                    ),
                            )
                        })
                        .when(playable, |this| {
                            this.child(
                                Button::new("play")
                                    .small()
                                    .primary()
                                    .icon(IconName::Play)
                                    .label(t("manage.downloads.history.play"))
                                    .loading(launching)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !launching {
                                            this.play(&play_id, &play_title, window, cx)
                                        }
                                    })),
                            )
                        })
                        .when(is_done, |this| {
                            this.child(
                                Button::new("open")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .tooltip(t("manage.downloads.game_page"))
                                    .accessibility_label(t("manage.downloads.game_page"))
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Game(page_id.clone()), cx)
                                    }),
                            )
                        })
                        .child(
                            Button::new("forget")
                                .ghost()
                                .small()
                                .icon(IconName::X)
                                .tooltip(t("manage.downloads.history.remove"))
                                .on_click(move |_, _, cx| {
                                    let id = remove_id.clone();
                                    backend_op(cx, move |b| b.remove_finished_download(&id));
                                }),
                        ),
                )
        });
        let filters = ButtonGroup::new("history-filter")
            .small()
            .outline()
            .children(HistoryFilter::ALL.iter().enumerate().map(|(ix, f)| {
                Button::new(ElementId::Name(format!("history-filter-{ix}").into()))
                    .label(f.label())
                    .selected(*f == filter)
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                if let Some(f) = clicked.first().and_then(|ix| HistoryFilter::ALL.get(*ix)) {
                    this.set_history_filter(*f, cx);
                }
            }));
        v_flex()
            .gap_2()
            .child(
                SectionTitle::new(t("manage.downloads.history.title")).trailing(
                    h_flex().gap_2().child(filters).child(
                        Button::new("clear-history")
                            .ghost()
                            .small()
                            .label(t("manage.downloads.history.clear"))
                            .on_click(|_, _, cx| backend_op(cx, |b| b.clear_finished_downloads())),
                    ),
                ),
            )
            .child(if total == 0 {
                list_card(cx).child(
                    div()
                        .px_4()
                        .py_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("manage.downloads.history.filter.none")),
                )
            } else {
                list_card(cx).children(rows)
            })
            .when(total > self.history_shown, |this| {
                this.child(
                    div().child(
                        Button::new("history-more")
                            .ghost()
                            .small()
                            .label(t_args(
                                "manage.downloads.history.show_more",
                                &[("count", &(total - self.history_shown).to_string())],
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history_shown += HISTORY_PAGE;
                                cx.notify();
                            })),
                    ),
                )
            })
    }
}
