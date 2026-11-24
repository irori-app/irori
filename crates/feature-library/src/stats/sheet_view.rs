//! The game sheet: a drawer opened from any cover or game on the stats page
//! with that game's numbers over the period shown (see [`super::sheet`]): its
//! time in the period and in all, a bar per day, week or month, its sessions,
//! their average and longest, the usual hour, the weekend share, first and
//! last play and its status changes. "Open game page" leaves for the game;
//! Escape or the scrim closes it.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, relative, rems,
};
use irori_core::{GameId, GameInfo};
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models, navigate};

use super::sheet::{self, GameSheet, SheetUnit};
use super::{StatsView, text};

/// Height of the sheet's bars, in rem.
const BARS_REM: f32 = 6.;

impl StatsView {
    /// Open the sheet of `id` over the period shown.
    pub(super) fn open_game_sheet(
        &mut self,
        id: GameId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(history) = self.history.clone() else {
            return;
        };
        let (game, user) = {
            let library = models(cx).library.read(cx);
            (library.snapshot.get(&id).cloned(), library.user.clone())
        };
        let Some(game) = game else {
            return;
        };
        let changes = user
            .status_history
            .get(&id.key())
            .map_or(&[][..], Vec::as_slice);
        let data = Arc::new(sheet::compute(
            &history,
            &id,
            self.period(),
            &user.playtime,
            changes,
            Self::today(),
        ));
        let title = SharedString::from(game.title.clone());
        window.open_sheet(cx, move |sheet, _, cx| {
            sheet
                .title(title.clone())
                .size(rems(30.))
                .child(body(&data, &game, cx))
        });
    }
}

/// A fact: a caption over its value.
fn fact(caption: SharedString, value: SharedString, cx: &App) -> AnyElement {
    v_flex()
        .min_w(rems(7.))
        .flex_1()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(caption),
        )
        .child(div().font_semibold().truncate().child(value))
        .into_any_element()
}

/// What a bar's start is called under the chart.
fn bar_name(unit: SheetUnit, start: chrono::NaiveDate) -> SharedString {
    match unit {
        SheetUnit::Day | SheetUnit::Week => text::short_date(start),
        SheetUnit::Month => text::month_year(start),
    }
}

fn body(s: &GameSheet, game: &Arc<GameInfo>, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (muted, bar, empty) = (theme.muted_foreground, theme.chart_1, theme.muted);
    let none = || SharedString::from("—");
    let date = |d: Option<chrono::NaiveDate>| d.map(text::long_date).unwrap_or_else(none);
    let max = s.bars.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let bars = h_flex()
        .w_full()
        .h(rems(BARS_REM))
        .items_end()
        .gap(rems(if s.bars.len() > 40 { 0.05 } else { 0.15 }))
        .children(s.bars.iter().map(|(_, seconds)| {
            div()
                .flex_1()
                .min_w_0()
                .rounded_sm()
                .bg(if *seconds > 0 { bar } else { empty })
                .h(relative((*seconds as f32 / max as f32).max(0.03)))
        }));
    let changes: Vec<AnyElement> = s
        .changes
        .iter()
        .rev()
        .take(6)
        .map(|(day, status)| {
            let label = match status {
                Some(status) => irori_ui::play_status::label(*status),
                None => t("library.stats.sheet.status_cleared"),
            };
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w(rems(9.))
                        .flex_shrink_0()
                        .text_color(muted)
                        .child(text::long_date(*day)),
                )
                .child(div().min_w_0().truncate().child(label))
                .into_any_element()
        })
        .collect();
    let id = s.id.clone();
    v_flex()
        .gap_5()
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(GameCover::new(game.clone()).width(rems(5.)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(text::period(&s.period)),
                        )
                        .child(
                            div()
                                .text_3xl()
                                .font_bold()
                                .child(text::duration(s.seconds)),
                        )
                        .child(div().text_sm().text_color(muted).child(t_args(
                            "library.stats.sheet.all_time",
                            &[("time", &text::duration(s.total_seconds))],
                        ))),
                ),
        )
        .child(v_flex().gap_1().child(bars).when_some(
            s.bars.first().zip(s.bars.last()),
            |c, (a, b)| {
                c.child(
                    h_flex()
                        .justify_between()
                        .text_xs()
                        .text_color(muted)
                        .child(bar_name(s.unit, a.0))
                        .child(bar_name(s.unit, b.0)),
                )
            },
        ))
        .child(
            h_flex()
                .flex_wrap()
                .gap_4()
                .child(fact(
                    t("library.stats.sheet.sessions"),
                    s.sessions.to_string().into(),
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.days"),
                    s.days.to_string().into(),
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.average"),
                    if s.sessions > 0 {
                        text::duration(s.average_seconds)
                    } else {
                        none()
                    },
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.longest"),
                    if s.sessions > 0 {
                        text::duration(s.longest_seconds)
                    } else {
                        none()
                    },
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.usual_hour"),
                    s.usual_hour
                        .map(|h| SharedString::from(format!("{h:02}:00")))
                        .unwrap_or_else(none),
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.weekends"),
                    s.weekend_share
                        .map(|w| SharedString::from(format!("{:.0}%", w * 100.)))
                        .unwrap_or_else(none),
                    cx,
                ))
                .child(fact(
                    t("library.stats.sheet.first"),
                    date(s.first_played),
                    cx,
                ))
                .child(fact(t("library.stats.sheet.last"), date(s.last_played), cx)),
        )
        .when(!changes.is_empty(), |c| {
            c.child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .font_semibold()
                            .child(Icon::new(IconName::Flag).small())
                            .child(t("library.stats.sheet.status_changes")),
                    )
                    .children(changes),
            )
        })
        .child(
            h_flex().justify_end().child(
                Button::new("stats-sheet-open-game")
                    .primary()
                    .icon(IconName::Gamepad2)
                    .label(t("library.stats.sheet.open_game"))
                    .on_click(move |_, window, cx| {
                        window.close_sheet(cx);
                        navigate(Route::Game(id.clone()), cx)
                    }),
            ),
        )
        .into_any_element()
}
