//! What each story of the Year in review shows: a caption, a headline in
//! display type and the figures or games behind it, laid out over the
//! backdrop in the colours [`BackdropInk`] holds legible there.

use std::sync::Arc;

use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, px, relative, rems,
};
use irori_core::{GameId, GameInfo};
use irori_services::LibrarySnapshot;
use irori_ui::components::{BackdropInk, GameCover};
use irori_ui::i18n::{t, t_args};

use super::text;
use super::year::{Daypart, GameYear, Story, YearReview};
use super::year_view::YearReviewView;

/// Most covers a row of covers shows.
const COVER_ROW: usize = 6;
/// Most games the ranking shows.
const TOP_GAMES: usize = 5;
/// Height of the bar charts, in rem.
const CHART_REM: f32 = 11.;

const WEEKDAYS: [&str; 7] = [
    "library.year.weekday.monday",
    "library.year.weekday.tuesday",
    "library.year.weekday.wednesday",
    "library.year.weekday.thursday",
    "library.year.weekday.friday",
    "library.year.weekday.saturday",
    "library.year.weekday.sunday",
];

/// A game's title, or its app name once it left the library.
pub(super) fn title(id: &GameId, snapshot: &LibrarySnapshot) -> SharedString {
    snapshot
        .get(id)
        .map_or_else(|| id.app_name.to_string(), |g| g.title.clone())
        .into()
}

/// A share of the year as a whole percent; "<1%" for a sliver.
pub(super) fn percent(share: f32) -> String {
    let pct = (share * 100.).round() as u32;
    if pct == 0 && share > 0. {
        "<1%".into()
    } else {
        format!("{pct}%")
    }
}

/// The small caption over a story.
fn eyebrow(text: SharedString, ink: BackdropInk) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .text_sm()
        .font_bold()
        .text_color(ink.muted)
        .child(div().w(rems(2.)).h(px(2.)).bg(ink.muted))
        .child(SharedString::from(text.to_uppercase()))
        .into_any_element()
}

/// A story's headline in display type.
fn display(text: impl Into<SharedString>, size: f32) -> AnyElement {
    div()
        .text_size(rems(size))
        .line_height(relative(1.05))
        .font_extrabold()
        .child(text.into())
        .into_any_element()
}

/// A line of supporting text under a headline.
fn lede(text: impl Into<SharedString>, ink: BackdropInk) -> AnyElement {
    div()
        .text_xl()
        .text_color(ink.muted)
        .child(text.into())
        .into_any_element()
}

/// A figure with its caption, on a panel.
fn tile(
    value: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    ink: BackdropInk,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(rems(9.))
        .gap_1()
        .p_5()
        .rounded_xl()
        .bg(ink.panel)
        .child(
            div()
                .text_size(rems(2.5))
                .line_height(relative(1.))
                .font_extrabold()
                .child(value.into()),
        )
        .child(
            div()
                .text_sm()
                .text_color(ink.panel_muted)
                .child(caption.into()),
        )
        .into_any_element()
}

/// A game's cover, or nothing once it left the library.
fn cover(game: Option<&Arc<GameInfo>>, width: f32) -> Option<AnyElement> {
    game.map(|g| {
        GameCover::new(g.clone())
            .width(rems(width))
            .into_any_element()
    })
}

/// A horizontal bar `fraction` full.
fn bar(fraction: f32, fill: Hsla, track: Hsla) -> AnyElement {
    div()
        .w_full()
        .h(rems(0.5))
        .rounded_full()
        .bg(track)
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(fraction.clamp(0.01, 1.)))
                .bg(fill),
        )
        .into_any_element()
}

/// Vertical bars of `values`, labelled under them where `label` says, the
/// largest in `strong`.
fn columns(
    values: &[u64],
    label: impl Fn(usize) -> Option<SharedString>,
    strong: Hsla,
    quiet: Hsla,
    ink: BackdropInk,
) -> AnyElement {
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    let busiest = values.iter().position(|v| *v == max);
    h_flex()
        .w_full()
        .gap_1()
        .items_end()
        .children(values.iter().enumerate().map(|(ix, value)| {
            let height = *value as f32 / max as f32;
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w_full()
                        .h(rems(CHART_REM))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(
                            div()
                                .w_full()
                                .h(relative(if *value > 0 { height.max(0.02) } else { 0. }))
                                .rounded_t(px(4.))
                                .bg(if Some(ix) == busiest { strong } else { quiet }),
                        ),
                )
                .child(
                    div()
                        .h(rems(1.))
                        .max_w_full()
                        .truncate()
                        .text_xs()
                        .text_color(ink.muted)
                        .children(label(ix)),
                )
        }))
        .into_any_element()
}

/// The accent over the backdrop: the theme's primary, held legible.
fn accent(ink: BackdropInk, cx: &Context<YearReviewView>) -> Hsla {
    irori_ui::style::legible(
        cx.theme().primary,
        &ink.surfaces,
        irori_ui::style::TEXT_CONTRAST,
        cx,
    )
}

impl YearReviewView {
    pub(super) fn render_story(
        &mut self,
        story: Story,
        review: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match story {
            Story::Intro => self.story_intro(review, snapshot, ink),
            Story::Numbers => self.story_numbers(review, ink, cx),
            Story::TopGame => self.story_top_game(review, snapshot, ink),
            Story::TopGames => self.story_top_games(review, snapshot, ink, cx),
            Story::Months => self.story_months(review, ink, cx),
            Story::Records => self.story_records(review, snapshot, ink),
            Story::Rhythm => self.story_rhythm(review, ink, cx),
            Story::Comebacks => self.story_comebacks(review, snapshot, ink),
            Story::NewAndBeaten => self.story_new_and_beaten(review, snapshot, ink),
            Story::Stores => self.story_stores(review, ink, cx),
            Story::Summary => self.story_summary(review, snapshot, ink, cx),
        };
        v_flex()
            .size_full()
            .justify_center()
            .child(div().w_full().max_w(rems(64.)).child(content))
            .into_any_element()
    }

    fn story_intro(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
    ) -> AnyElement {
        let covers = r
            .games
            .iter()
            .filter_map(|g| cover(snapshot.get(&g.id), 7.))
            .take(COVER_ROW);
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.intro.eyebrow"), ink))
            .child(display(r.year.to_string(), 10.))
            .child(lede(t("library.year.intro.title"), ink))
            .child(h_flex().pt_4().gap_3().items_end().children(covers))
            .child(
                div()
                    .pt_2()
                    .text_sm()
                    .text_color(ink.muted)
                    .child(t("library.year.intro.hint")),
            )
            .into_any_element()
    }

    fn story_numbers(&self, r: &YearReview, ink: BackdropInk, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let change = r.change_percent().map(|pct| {
            let (color, icon, sign) = match pct.signum() {
                1 => (theme.success, IconName::TrendingUp, "+"),
                -1 => (theme.danger, IconName::TrendingDown, "−"),
                _ => (ink.muted, IconName::Minus, ""),
            };
            let color =
                irori_ui::style::legible(color, &ink.surfaces, irori_ui::style::TEXT_CONTRAST, cx);
            h_flex()
                .gap_2()
                .items_center()
                .text_color(color)
                .font_semibold()
                .child(Icon::new(icon).small())
                .child(t_args(
                    "library.year.numbers.versus",
                    &[
                        ("change", &format!("{sign}{}%", pct.unsigned_abs())),
                        ("year", &(r.year - 1).to_string()),
                    ],
                ))
        });
        let average = r.seconds / u64::from(r.sessions.max(1));
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.numbers.eyebrow"), ink))
            .child(display(text::hours(r.seconds / 60), 8.))
            .child(lede(t("library.year.numbers.played"), ink))
            .children(change)
            .child(
                h_flex()
                    .pt_4()
                    .gap_3()
                    .flex_wrap()
                    .child(tile(
                        r.days_played.to_string(),
                        text::count_word(
                            "library.year.word.days_played_one",
                            "library.year.word.days_played_other",
                            r.days_played.into(),
                        ),
                        ink,
                    ))
                    .child(tile(
                        r.games.len().to_string(),
                        text::count_word(
                            "library.year.word.games_one",
                            "library.year.word.games_other",
                            r.games.len() as u64,
                        ),
                        ink,
                    ))
                    .child(tile(
                        r.sessions.to_string(),
                        text::count_word(
                            "library.year.word.sessions_one",
                            "library.year.word.sessions_other",
                            r.sessions.into(),
                        ),
                        ink,
                    ))
                    .child(tile(
                        text::duration(average),
                        t("library.year.word.average_session"),
                        ink,
                    )),
            )
            .into_any_element()
    }

    fn story_top_game(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
    ) -> AnyElement {
        let Some(top) = r.games.first() else {
            return div().into_any_element();
        };
        let facts = [
            t_args(
                "library.year.top.share",
                &[("percent", &percent(r.share(top)))],
            ),
            text::count(
                "library.year.top.days_one",
                "library.year.top.days_other",
                top.days,
            ),
            text::count(
                "library.stats.sessions_one",
                "library.stats.sessions_other",
                top.sessions,
            ),
        ];
        h_flex()
            .w_full()
            .gap_10()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_5()
                    .child(eyebrow(t("library.year.top.eyebrow"), ink))
                    .child(
                        div()
                            .line_clamp(2)
                            .child(display(title(&top.id, snapshot), 4.5)),
                    )
                    .child(display(text::hours(top.seconds / 60), 3.))
                    .child(
                        v_flex()
                            .gap_1()
                            .text_lg()
                            .text_color(ink.muted)
                            .children(facts.into_iter().map(|f| div().child(f))),
                    ),
            )
            .children(cover(snapshot.get(&top.id), 15.))
            .into_any_element()
    }

    fn story_top_games(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
        cx: &Context<Self>,
    ) -> AnyElement {
        let fill = accent(ink, cx);
        let track = ink.text.opacity(0.15);
        let top = r.games.first().map_or(0., |g| r.share(g)).max(f32::EPSILON);
        let rows = r.games.iter().take(TOP_GAMES).enumerate().map(|(ix, g)| {
            let game = snapshot.get(&g.id);
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .child(
                    div()
                        .w(rems(2.))
                        .flex_shrink_0()
                        .text_2xl()
                        .font_extrabold()
                        .text_color(ink.muted)
                        .child((ix + 1).to_string()),
                )
                .children(cover(game, 3.5))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .w_full()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_lg()
                                        .font_semibold()
                                        .child(title(&g.id, snapshot)),
                                )
                                .child(div().flex_shrink_0().text_color(ink.muted).child(format!(
                                    "{} · {}",
                                    text::hours(g.seconds / 60),
                                    percent(r.share(g))
                                ))),
                        )
                        .child(bar(r.share(g) / top, fill, track)),
                )
        });
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.top_games.eyebrow"), ink))
            .child(display(t("library.year.top_games.title"), 3.5))
            .child(v_flex().pt_2().gap_4().children(rows))
            .into_any_element()
    }

    fn story_months(&self, r: &YearReview, ink: BackdropInk, cx: &Context<Self>) -> AnyElement {
        let headline = r.busiest_month().map(|m| {
            let day = NaiveDate::from_ymd_opt(r.year, m as u32 + 1, 1).unwrap_or_default();
            t_args(
                "library.year.months.busiest",
                &[
                    ("month", &text::month(day)),
                    ("hours", &text::hours(r.months[m] / 60)),
                ],
            )
        });
        let year = r.year;
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.months.eyebrow"), ink))
            .children(headline.map(|h| display(h, 3.5)))
            .child(div().pt_4().child(columns(
                &r.months,
                |m| NaiveDate::from_ymd_opt(year, m as u32 + 1, 1).map(text::month_short),
                accent(ink, cx),
                ink.text.opacity(0.3),
                ink,
            )))
            .into_any_element()
    }

    fn story_records(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
    ) -> AnyElement {
        let mut cards: Vec<AnyElement> = Vec::new();
        if let Some(s) = &r.longest_session {
            cards.push(record(
                IconName::Timer,
                t("library.year.records.session"),
                text::duration(s.seconds),
                t_args(
                    "library.year.records.session_detail",
                    &[
                        ("title", &title(&s.game, snapshot)),
                        ("date", &text::long_date(s.start.date())),
                    ],
                ),
                ink,
            ));
        }
        if let Some(run) = r.longest_streak.filter(|run| run.days > 1) {
            let end = run.start + chrono::Days::new(u64::from(run.days - 1));
            cards.push(record(
                IconName::Flame,
                t("library.year.records.streak"),
                text::count(
                    "library.year.records.streak_days_one",
                    "library.year.records.streak_days_other",
                    run.days,
                ),
                t_args(
                    "library.year.records.streak_detail",
                    &[
                        ("from", &text::long_date(run.start)),
                        ("to", &text::long_date(end)),
                    ],
                ),
                ink,
            ));
        }
        if let Some((day, seconds)) = r.busiest_day {
            cards.push(record(
                IconName::CalendarDays,
                t("library.year.records.day"),
                text::duration(seconds),
                text::long_date(day),
                ink,
            ));
        }
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.records.eyebrow"), ink))
            .child(display(t("library.year.records.title"), 3.5))
            .child(h_flex().pt_2().gap_4().flex_wrap().children(cards))
            .into_any_element()
    }

    fn story_rhythm(&self, r: &YearReview, ink: BackdropInk, cx: &Context<Self>) -> AnyElement {
        let headline = r.daypart().map(|part| {
            t(match part {
                Daypart::Morning => "library.year.rhythm.morning",
                Daypart::Afternoon => "library.year.rhythm.afternoon",
                Daypart::Evening => "library.year.rhythm.evening",
                Daypart::Night => "library.year.rhythm.night",
            })
        });
        let weekday = r.busiest_weekday().map(|d| {
            t_args(
                "library.year.rhythm.weekday",
                &[("weekday", &t(WEEKDAYS[d]))],
            )
        });
        let strong = accent(ink, cx);
        let quiet = ink.text.opacity(0.3);
        let weekday_labels: Vec<SharedString> = (0..7)
            .map(|d| {
                // 5 January 2026 is a Monday.
                let day = NaiveDate::from_ymd_opt(2026, 1, 5 + d as u32).unwrap_or_default();
                text::weekday_short(day)
            })
            .collect();
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.rhythm.eyebrow"), ink))
            .children(headline.map(|h| display(h, 3.5)))
            .children(weekday.map(|w| lede(w, ink)))
            .child(
                h_flex()
                    .pt_4()
                    .gap_8()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_grow(3.)
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(t("library.year.rhythm.hours")),
                            )
                            .child(columns(
                                &r.hours,
                                |h| (h % 6 == 0).then(|| SharedString::from(format!("{h:02}"))),
                                strong,
                                quiet,
                                ink,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_grow(1.)
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(t("library.year.rhythm.days")),
                            )
                            .child(columns(
                                &r.weekdays,
                                |d| weekday_labels.get(d).cloned(),
                                strong,
                                quiet,
                                ink,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn story_comebacks(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
    ) -> AnyElement {
        let rows = r.comebacks.iter().map(|c| {
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .children(cover(snapshot.get(&c.id), 4.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_xl()
                                .font_semibold()
                                .child(title(&c.id, snapshot)),
                        )
                        .child(div().text_color(ink.muted).child(t_args(
                            if c.rested == 1 {
                                "library.year.comebacks.after_one"
                            } else {
                                "library.year.comebacks.after_other"
                            },
                            &[
                                ("count", &c.rested.to_string()),
                                ("date", &text::long_date(c.back)),
                            ],
                        ))),
                )
        });
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.comebacks.eyebrow"), ink))
            .child(display(t("library.year.comebacks.title"), 3.5))
            .child(v_flex().pt_2().gap_4().children(rows))
            .into_any_element()
    }

    fn story_new_and_beaten(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
    ) -> AnyElement {
        let column = |count: usize, one: &str, other: &str, icon: IconName, ids: Vec<&GameId>| {
            v_flex()
                .flex_1()
                .min_w(rems(16.))
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(Icon::new(icon).large().text_color(ink.muted))
                        .child(display(count.to_string(), 4.)),
                )
                .child(lede(text::count_word(one, other, count as u64), ink))
                .child(
                    h_flex().pt_2().gap_2().flex_wrap().children(
                        ids.into_iter()
                            .filter_map(|id| cover(snapshot.get(id), 4.5))
                            .take(COVER_ROW),
                    ),
                )
                .into_any_element()
        };
        let mut columns_row: Vec<AnyElement> = Vec::new();
        if !r.new_games.is_empty() {
            columns_row.push(column(
                r.new_games.len(),
                "library.year.word.new_games_one",
                "library.year.word.new_games_other",
                IconName::Sparkles,
                r.new_games.iter().collect(),
            ));
        }
        if !r.beaten.is_empty() {
            columns_row.push(column(
                r.beaten.len(),
                "library.year.word.beaten_one",
                "library.year.word.beaten_other",
                IconName::Trophy,
                r.beaten.iter().map(|(id, _)| id).collect(),
            ));
        }
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.new.eyebrow"), ink))
            .child(display(t("library.year.new.title"), 3.5))
            .child(
                h_flex()
                    .pt_2()
                    .gap_10()
                    .flex_wrap()
                    .items_start()
                    .children(columns_row),
            )
            .into_any_element()
    }

    fn story_stores(&self, r: &YearReview, ink: BackdropInk, cx: &Context<Self>) -> AnyElement {
        let fill = accent(ink, cx);
        let track = ink.text.opacity(0.15);
        let top = r.stores.first().map_or(1, |(_, s)| *s).max(1);
        let rows = r.stores.iter().map(|(runner, seconds)| {
            v_flex()
                .w_full()
                .gap_1p5()
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_lg()
                                .font_semibold()
                                .child(runner.store_name()),
                        )
                        .child(div().text_color(ink.muted).child(text::hours(seconds / 60))),
                )
                .child(bar(*seconds as f32 / top as f32, fill, track))
        });
        let systems = r.systems.iter().map(|(id, seconds)| {
            let name = irori_engines::systems::get(id).map_or(id.clone(), |s| s.name.clone());
            h_flex()
                .gap_2()
                .px_3()
                .py_1()
                .rounded_full()
                .bg(ink.panel)
                .text_sm()
                .child(div().font_semibold().child(name))
                .child(
                    div()
                        .text_color(ink.panel_muted)
                        .child(text::hours(seconds / 60)),
                )
        });
        v_flex()
            .gap_5()
            .child(eyebrow(t("library.year.stores.eyebrow"), ink))
            .child(display(t("library.year.stores.title"), 3.5))
            .child(v_flex().pt_2().gap_4().max_w(rems(44.)).children(rows))
            .when(!r.systems.is_empty(), |d| {
                d.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(t("library.year.stores.systems")),
                        )
                        .child(h_flex().gap_2().flex_wrap().children(systems)),
                )
            })
            .into_any_element()
    }

    fn story_summary(
        &self,
        r: &YearReview,
        snapshot: &LibrarySnapshot,
        ink: BackdropInk,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let games = r.games.iter().take(3).map(|g: &GameYear| {
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .children(cover(snapshot.get(&g.id), 2.5))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_semibold()
                        .child(title(&g.id, snapshot)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(ink.panel_muted)
                        .child(text::hours(g.seconds / 60)),
                )
        });
        let facts = [
            (
                r.days_played.to_string(),
                text::count_word(
                    "library.year.word.days_played_one",
                    "library.year.word.days_played_other",
                    r.days_played.into(),
                ),
            ),
            (
                r.games.len().to_string(),
                text::count_word(
                    "library.year.word.games_one",
                    "library.year.word.games_other",
                    r.games.len() as u64,
                ),
            ),
            (
                r.sessions.to_string(),
                text::count_word(
                    "library.year.word.sessions_one",
                    "library.year.word.sessions_other",
                    r.sessions.into(),
                ),
            ),
        ];
        let card = v_flex()
            .w(rems(26.))
            .max_w_full()
            .flex_shrink_0()
            .gap_4()
            .p_6()
            .rounded(theme.radius_lg * 2.)
            .bg(ink.panel)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(ink.panel_muted)
                    .child(SharedString::from(t("library.year.eyebrow").to_uppercase())),
            )
            .child(display(r.year.to_string(), 4.5))
            .child(
                div()
                    .text_xl()
                    .font_bold()
                    .text_color(accent(ink, cx))
                    .child(t_args(
                        "library.year.card.headline",
                        &[("hours", &text::hours(r.seconds / 60))],
                    )),
            )
            .child(h_flex().gap_4().children(facts.into_iter().map(|(v, c)| {
                v_flex()
                    .min_w_0()
                    .child(div().text_xl().font_extrabold().child(v))
                    .child(
                        div()
                            .text_xs()
                            .text_color(ink.panel_muted)
                            .truncate()
                            .child(c),
                    )
            })))
            .child(v_flex().gap_2().children(games));
        let saving = self.saving;
        h_flex()
            .w_full()
            .gap_10()
            .items_center()
            .flex_wrap()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(18.))
                    .gap_5()
                    .child(eyebrow(t("library.year.summary.eyebrow"), ink))
                    .child(display(
                        t_args(
                            "library.year.summary.title",
                            &[("year", &r.year.to_string())],
                        ),
                        4.,
                    ))
                    .child(lede(t("library.year.summary.description"), ink))
                    .child(
                        // Clicks on the buttons stay off the zones behind.
                        h_flex()
                            .id("year-summary-actions")
                            .occlude()
                            .pt_2()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("year-save-card")
                                    .primary()
                                    .icon(IconName::ImageDown)
                                    .label(t("library.year.card.save"))
                                    .loading(saving)
                                    .disabled(saving)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_card(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("year-again")
                                    .outline()
                                    .icon(IconName::RotateCcw)
                                    .label(t("library.year.summary.again"))
                                    .on_click(cx.listener(|this, _, _, cx| this.go_to(0, cx))),
                            ),
                    ),
            )
            .child(card)
            .into_any_element()
    }
}

/// One record: an icon, what it is, the figure and what it was.
fn record(
    icon: IconName,
    caption: SharedString,
    value: SharedString,
    detail: SharedString,
    ink: BackdropInk,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(rems(16.))
        .gap_2()
        .p_6()
        .rounded_xl()
        .bg(ink.panel)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .font_semibold()
                .text_color(ink.panel_muted)
                .child(Icon::new(icon).small())
                .child(caption),
        )
        .child(
            div()
                .text_size(rems(2.75))
                .line_height(relative(1.05))
                .font_extrabold()
                .child(value),
        )
        .child(
            div()
                .text_color(ink.panel_muted)
                .line_clamp(2)
                .child(detail),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_read_as_whole_percents() {
        assert_eq!(percent(0.294), "29%");
        assert_eq!(percent(0.001), "<1%");
        assert_eq!(percent(0.), "0%");
    }
}
