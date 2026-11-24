//! The backlog tab's other cards (see [`super::backlog::Extras`]): the queue
//! of Plan to play games with when each would be done at the user's pace,
//! the shelf of games in play or on hold that went quiet, the pile (games
//! added, started and beaten each month) and the year's beaten games beside
//! How Long To Beat. A game opens its sheet.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, relative, rems,
};
use irori_core::GameId;
use irori_services::LibrarySnapshot;
use irori_ui::components::{GameCover, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{models, play_status};

use super::backlog::Extras;
use super::{StatsView, text};

/// Most rows the queue and the shelf list.
const ROWS: usize = 8;
/// Height of the pile's bars, in rem.
const PILE_REM: f32 = 5.;

fn hours(h: f32) -> String {
    if h >= 10. {
        format!("{h:.0}")
    } else {
        format!("{h:.1}")
    }
}

impl StatsView {
    /// The extra cards, those with something to show.
    pub(super) fn render_backlog_extras(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(e) = self.backlog_extras.clone() else {
            return Vec::new();
        };
        let library = models(cx).library.read(cx).snapshot.clone();
        let mut out = Vec::new();
        if !e.queue.is_empty() {
            out.push(self.render_queue(&e, &library, cx));
        }
        if !e.shelf.is_empty() {
            out.push(self.render_shelf(&e, &library, cx));
        }
        if e.pile.iter().any(|m| m.added + m.started + m.beaten > 0) {
            out.push(render_pile(&e, cx));
        }
        if !e.against_hltb.is_empty() {
            out.push(self.render_against_hltb(&e, &library, cx));
        }
        out
    }

    /// One game's row: its cover and title, then `detail`; it opens the
    /// game's sheet.
    fn game_row(
        &self,
        name: &str,
        id: &GameId,
        library: &LibrarySnapshot,
        detail: SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let game = library.get(id).cloned()?;
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let open = id.clone();
        Some(
            h_flex()
                .id(ElementId::Name(format!("{name}-{}", id.key()).into()))
                .keyboard_focusable(cx)
                .w_full()
                .gap_3()
                .p_1()
                .items_center()
                .rounded(theme.radius)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_game_sheet(open.clone(), window, cx)
                }))
                .child(GameCover::new(game.clone()).width(rems(2.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_medium()
                        .child(game.title.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(muted)
                        .child(detail),
                )
                .into_any_element(),
        )
    }

    fn render_queue(
        &self,
        e: &Extras,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = e
            .queue
            .iter()
            .take(ROWS)
            .filter_map(|q| {
                let mut parts: Vec<String> = Vec::new();
                if let Some(h) = q.hours {
                    let key = if q.guessed {
                        "library.stats.backlog.queue.guessed"
                    } else {
                        "library.stats.backlog.queue.hours"
                    };
                    parts.push(t_args(key, &[("hours", &hours(h))]).to_string());
                }
                if let Some(day) = q.done {
                    parts.push(
                        t_args(
                            "library.stats.backlog.queue.done",
                            &[("date", &text::short_date(day))],
                        )
                        .to_string(),
                    );
                }
                self.game_row("stats-queue", &q.id, library, parts.join(" · ").into(), cx)
            })
            .collect();
        panel(cx)
            .gap_3()
            .child(SectionTitle::new(t("library.stats.backlog.queue.title")))
            .child(div().text_sm().text_color(muted).child(t_args(
                "library.stats.backlog.queue.pace",
                &[("hours", &hours(e.weekly_hours))],
            )))
            .children(rows)
            .into_any_element()
    }

    fn render_shelf(
        &self,
        e: &Extras,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = e
            .shelf
            .iter()
            .take(ROWS)
            .filter_map(|s| {
                let idle = match s.idle_days {
                    Some(days) => t_args(
                        "library.stats.backlog.shelf.idle",
                        &[("days", &days.to_string())],
                    ),
                    None => t("library.stats.backlog.shelf.never"),
                };
                let detail = format!("{} · {idle}", play_status::label(s.status));
                self.game_row("stats-shelf", &s.id, library, detail.into(), cx)
            })
            .collect();
        panel(cx)
            .gap_3()
            .child(SectionTitle::new(t("library.stats.backlog.shelf.title")))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.stats.backlog.shelf.help")),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_against_hltb(
        &self,
        e: &Extras,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = e
            .against_hltb
            .iter()
            .take(ROWS)
            .filter_map(|a| {
                let detail = t_args(
                    "library.stats.backlog.hltb.row",
                    &[("hours", &hours(a.hours)), ("hltb", &a.hltb.to_string())],
                );
                self.game_row("stats-hltb", &a.id, library, detail, cx)
            })
            .collect();
        panel(cx)
            .gap_3()
            .child(SectionTitle::new(t("library.stats.backlog.hltb.title")))
            .children(rows)
            .into_any_element()
    }
}

/// The pile: per month, three bars (added, started, beaten) and a legend.
fn render_pile(e: &Extras, cx: &mut Context<StatsView>) -> AnyElement {
    let theme = cx.theme();
    let (muted, added, started, beaten) = (
        theme.muted_foreground,
        theme.chart_3,
        theme.chart_2,
        theme.chart_1,
    );
    let most = e
        .pile
        .iter()
        .map(|m| m.added.max(m.started).max(m.beaten))
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let bar = |n: usize, color: Hsla| {
        div()
            .flex_1()
            .rounded_sm()
            .bg(color)
            .h(relative((n as f32 / most).max(if n > 0 {
                0.04
            } else {
                0.
            })))
    };
    let legend = |color: Hsla, label: SharedString| {
        h_flex()
            .gap_1()
            .items_center()
            .child(div().size_2().rounded_full().bg(color))
            .child(label)
    };
    panel(cx)
        .gap_3()
        .child(SectionTitle::new(t("library.stats.backlog.pile.title")))
        .child(
            h_flex()
                .gap_4()
                .text_xs()
                .text_color(muted)
                .child(legend(added, t("library.stats.backlog.pile.added")))
                .child(legend(started, t("library.stats.backlog.pile.started")))
                .child(legend(beaten, t("library.stats.backlog.pile.beaten"))),
        )
        .child(h_flex().w_full().gap_2().children(e.pile.iter().map(|m| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .h(rems(PILE_REM))
                        .items_end()
                        .gap_px()
                        .child(bar(m.added, added))
                        .child(bar(m.started, started))
                        .child(bar(m.beaten, beaten)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .text_center()
                        .truncate()
                        .child(text::month_short(m.month)),
                )
        })))
        .into_any_element()
}
