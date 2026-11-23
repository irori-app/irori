//! The Journeys tab: over the art of the period's most played game, a lane
//! per game on one time axis (a dot per day played, sized by the time; a
//! ring where it started, an arrow when it came from before, a flag where it
//! was beaten and a cross where it was abandoned), with its time, sessions,
//! days to beat and pace beside it; above them, how many games were played
//! on each day. Filters (beaten, ongoing, abandoned, started here) and the
//! order are pills; a lane opens the game's sheet. The page is one list that
//! draws only the lanes on screen.

use std::ops::Range;
use std::rc::Rc;

use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, SharedString, Size, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    relative, rems, size,
};
use irori_ui::components::{EmptyState, GameCover};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::hero::{HERO_REM, big_number, dotted, eyebrow, pills};
use super::journeys::{Journeys, Lane, LaneFilter, LaneSort, position, ticks};
use super::{StatsView, text};

/// The page's padding, in rem.
const PAD_REM: f32 = 1.5;
/// Height of the filters and axis, in rem.
const CONTROLS_REM: f32 = 5.5;
/// Height of the games-at-once strip, in rem.
const AT_ONCE_REM: f32 = 4.;
/// Height of a lane, in rem.
const LANE_REM: f32 = 3.5;
/// Width of the column beside the lanes, in rem.
const LABEL_REM: f32 = 17.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JourneyItem {
    Hero,
    Controls,
    AtOnce,
    Lane(usize),
    End,
}

impl JourneyItem {
    fn height_rem(self) -> f32 {
        match self {
            JourneyItem::Hero => PAD_REM + HERO_REM + PAD_REM,
            JourneyItem::Controls => CONTROLS_REM,
            JourneyItem::AtOnce => AT_ONCE_REM,
            JourneyItem::Lane(_) => LANE_REM,
            JourneyItem::End => PAD_REM,
        }
    }
}

fn filter_label(f: LaneFilter) -> SharedString {
    t(match f {
        LaneFilter::All => "library.stats.journeys.filter.all",
        LaneFilter::Beaten => "library.stats.journeys.filter.beaten",
        LaneFilter::Ongoing => "library.stats.journeys.filter.ongoing",
        LaneFilter::Abandoned => "library.stats.journeys.filter.abandoned",
        LaneFilter::StartedHere => "library.stats.journeys.filter.started",
    })
}

fn sort_label(s: LaneSort) -> SharedString {
    t(match s {
        LaneSort::Started => "library.stats.journeys.sort.started",
        LaneSort::Time => "library.stats.journeys.sort.time",
        LaneSort::Recent => "library.stats.journeys.sort.recent",
    })
}

impl StatsView {
    /// The lanes shown, as indices into the journeys' lanes.
    fn journey_lanes(&self) -> Vec<usize> {
        self.journeys
            .as_ref()
            .map(|j| j.shown(self.lane_filter, self.lane_sort))
            .unwrap_or_default()
    }

    fn journey_items(&self) -> Vec<JourneyItem> {
        let lanes = self.journey_lanes();
        [
            JourneyItem::Hero,
            JourneyItem::Controls,
            JourneyItem::AtOnce,
        ]
        .into_iter()
        .chain((0..lanes.len()).map(JourneyItem::Lane))
        .chain(std::iter::once(JourneyItem::End))
        .collect()
    }

    /// The Journeys tab: one list of the headline, the controls, the strip
    /// and the lanes; without play in the period, the headline and why.
    pub(super) fn render_journeys(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(journeys) = self.journeys.clone() else {
            return div().into_any_element();
        };
        if journeys.lanes.is_empty() {
            return v_flex()
                .size_full()
                .p(rems(PAD_REM))
                .gap_6()
                .child(self.render_journeys_hero(&journeys, window, cx))
                .child(
                    EmptyState::new(IconName::Route, t("library.stats.journeys.empty.title"))
                        .description(t("library.stats.journeys.empty.description")),
                )
                .into_any_element();
        }
        let rem = window.rem_size();
        let sizes: Vec<Size<Pixels>> = self
            .journey_items()
            .iter()
            .map(|item| size(px(0.), rem * item.height_rem()))
            .collect();
        if *self.journey_sizes != sizes {
            self.journey_sizes = Rc::new(sizes);
        }
        v_virtual_list(
            cx.entity(),
            "stats-journeys",
            self.journey_sizes.clone(),
            |this, range, window, cx| this.render_journey_items(range, window, cx),
        )
        .track_scroll(&self.journey_scroll)
        .size_full()
        .into_any_element()
    }

    fn render_journey_items(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(journeys) = self.journeys.clone() else {
            return Vec::new();
        };
        let items = self.journey_items();
        let lanes = self.journey_lanes();
        range
            .filter_map(|ix| {
                let item = *items.get(ix)?;
                let content = match item {
                    JourneyItem::Hero => self.render_journeys_hero(&journeys, window, cx),
                    JourneyItem::Controls => self.render_journey_controls(&journeys, cx),
                    JourneyItem::AtOnce => render_at_once(&journeys, Self::today(), cx),
                    JourneyItem::Lane(n) => {
                        let lane = journeys.lanes.get(*lanes.get(n)?)?;
                        self.render_lane(n, lane, &journeys, cx)
                    }
                    JourneyItem::End => div().into_any_element(),
                };
                Some(
                    div()
                        .w_full()
                        .h(rems(item.height_rem()))
                        .px(rems(PAD_REM))
                        .when(item == JourneyItem::Hero, |d| d.pt(rems(PAD_REM)))
                        .child(content)
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_journeys_hero(
        &self,
        j: &Journeys,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        let top = j.lanes.iter().max_by_key(|l| l.seconds);
        let art = top
            .and_then(|l| library.get(&l.id))
            .and_then(|g| g.hero_art().map(str::to_owned));
        let beaten = j.lanes.iter().filter(|l| l.beaten.is_some()).count();
        let started = j.lanes.iter().filter(|l| l.started_here).count();
        let muted = cx.theme().muted_foreground;
        let headline = v_flex()
            .gap_2()
            .min_w_0()
            .child(eyebrow(
                t_args(
                    "library.stats.journeys.eyebrow",
                    &[("period", &text::period(&j.period))],
                ),
                cx,
            ))
            .child(big_number(j.lanes.len().to_string().into()))
            .child(h_flex().flex_wrap().gap_x_3().text_sm().children(dotted(
                vec![
                    t_args(
                        "library.stats.journeys.started",
                        &[("count", &started.to_string())],
                    ),
                    t_args(
                        "library.stats.journeys.beaten",
                        &[("count", &beaten.to_string())],
                    ),
                    t_args(
                        "library.stats.journeys.at_once",
                        &[("count", &j.most_at_once.to_string())],
                    ),
                ],
                muted,
            )))
            .into_any_element();
        self.render_hero(art, headline, None, window, cx)
    }

    fn render_journey_controls(&self, j: &Journeys, cx: &mut Context<Self>) -> AnyElement {
        let today = Self::today();
        let muted = cx.theme().muted_foreground;
        let filters = pills(
            "journeys-filter",
            LaneFilter::ALL
                .into_iter()
                .map(|f| (None, filter_label(f), f == self.lane_filter, f))
                .collect(),
            cx,
            |this, f, cx| {
                this.lane_filter = f;
                cx.notify();
            },
        );
        let sorts = pills(
            "journeys-sort",
            LaneSort::ALL
                .into_iter()
                .map(|s| (None, sort_label(s), s == self.lane_sort, s))
                .collect(),
            cx,
            |this, s, cx| {
                this.lane_sort = s;
                cx.notify();
            },
        );
        let marks = ticks(&j.period, today).into_iter().map(|d| {
            div()
                .absolute()
                .left(relative(position(&j.period, d, today)))
                .text_xs()
                .text_color(muted)
                .child(text::month_short(d))
        });
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .child(filters)
                    .child(sorts),
            )
            .child(
                h_flex()
                    .w_full()
                    .child(div().w(rems(LABEL_REM)).flex_shrink_0())
                    .child(div().relative().flex_1().h(rems(1.)).children(marks)),
            )
            .into_any_element()
    }

    fn render_lane(
        &self,
        n: usize,
        lane: &Lane,
        j: &Journeys,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let today = Self::today();
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        let game = library.get(&lane.id).cloned();
        let title = game
            .as_ref()
            .map_or_else(|| lane.id.app_name.to_string(), |g| g.title.clone());
        let theme = cx.theme();
        let (muted, dot, line, success, danger, hover) = (
            theme.muted_foreground,
            theme.chart_1,
            theme.border,
            theme.success,
            theme.danger,
            theme.list_hover,
        );
        let mut facts = vec![
            text::duration(lane.seconds),
            text::count(
                "library.stats.journeys.sessions_one",
                "library.stats.journeys.sessions_other",
                lane.sessions as u64,
            ),
        ];
        if let Some(days) = lane.days_to_beat {
            facts.push(t_args(
                "library.stats.journeys.days_to_beat",
                &[("days", &days.to_string())],
            ));
        }
        facts.push(t_args(
            "library.stats.journeys.pace",
            &[("hours", &format!("{:.1}", lane.hours_per_week))],
        ));
        let most = lane.days.iter().map(|(_, s)| *s).max().unwrap_or(1).max(1);
        let at = |d: NaiveDate| relative(position(&j.period, d, today));
        let (first, last) = (lane.first(), lane.last());
        let marker = |d: NaiveDate, icon: IconName, color| {
            div()
                .absolute()
                .left(at(d))
                .top(rems(-0.1))
                .ml(rems(-0.45))
                .child(Icon::new(icon).xsmall().text_color(color))
        };
        let track = div()
            .relative()
            .flex_1()
            .h(rems(1.6))
            .when_some(first.zip(last), |t, (a, b)| {
                let (from, to) = (position(&j.period, a, today), position(&j.period, b, today));
                t.child(
                    div()
                        .absolute()
                        .top(rems(0.75))
                        .left(relative(from))
                        .w(relative((to - from).max(0.002)))
                        .h(px(2.))
                        .bg(line),
                )
            })
            .children(lane.days.iter().map(|(d, s)| {
                let size = 0.35 + 0.65 * (*s as f32 / most as f32).sqrt();
                div()
                    .absolute()
                    .left(at(*d))
                    .top(rems(0.8 - size / 2.))
                    .ml(rems(-size / 2.))
                    .size(rems(size))
                    .rounded_full()
                    .bg(dot)
            }))
            .when(!lane.started_here, |t| {
                t.child(marker(j.period.start, IconName::ChevronsLeft, muted))
            })
            .when_some(lane.beaten, |t, d| {
                t.child(marker(d, IconName::Flag, success))
            })
            .when_some(lane.abandoned, |t, d| {
                t.child(marker(d, IconName::X, danger))
            });
        let id = lane.id.clone();
        h_flex()
            .id(("journey-lane", n))
            .keyboard_focusable(cx)
            .w_full()
            .h(rems(LANE_REM - 0.25))
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.open_game_sheet(id.clone(), window, cx)
                }),
            )
            .child(
                h_flex()
                    .w(rems(LABEL_REM))
                    .flex_shrink_0()
                    .gap_2()
                    .pr_3()
                    .items_center()
                    .children(game.map(|g| GameCover::new(g).width(rems(1.75))))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(div().text_sm().font_medium().truncate().child(title))
                            .child(
                                div().text_xs().text_color(muted).truncate().child(
                                    SharedString::from(
                                        facts
                                            .iter()
                                            .map(|f| f.to_string())
                                            .collect::<Vec<_>>()
                                            .join(" · "),
                                    ),
                                ),
                            ),
                    ),
            )
            .child(track)
            .into_any_element()
    }
}

/// How many games were played each day, on the lanes' axis.
fn render_at_once(j: &Journeys, today: NaiveDate, cx: &mut Context<StatsView>) -> AnyElement {
    let theme = cx.theme();
    let (bar, muted) = (theme.chart_2, theme.muted_foreground);
    let most = j.most_at_once.max(1) as f32;
    let width = (1. / (j.period.end(today) - j.period.start).num_days().max(1) as f32).max(0.002);
    h_flex()
        .id(ElementId::Name("journeys-at-once".into()))
        .w_full()
        .h(rems(AT_ONCE_REM - 0.5))
        .child(
            v_flex()
                .w(rems(LABEL_REM))
                .flex_shrink_0()
                .pr_3()
                .text_xs()
                .text_color(muted)
                .child(t("library.stats.journeys.games_at_once"))
                .child(div().font_semibold().child(t_args(
                    "library.stats.journeys.most_at_once",
                    &[("count", &j.most_at_once.to_string())],
                ))),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .h_full()
                .children(j.at_once.iter().map(|(d, n)| {
                    div()
                        .absolute()
                        .bottom_0()
                        .left(relative(position(&j.period, *d, today)))
                        .w(relative(width))
                        .h(relative(*n as f32 / most))
                        .rounded_sm()
                        .bg(bar)
                })),
        )
        .into_any_element()
}
