//! The Discoveries tab: the period in sentences over the art of the game
//! the leading one is about, each finding a card that can be pinned (it
//! leads from then on) or hidden (its kind no longer shows), and leads to
//! its numbers (the game sheet, or the Overview); the milestones reached and
//! the next ones with when they would come at the recent pace; what was
//! played a year ago; and "Copy summary", which puts the period in a few
//! lines on the clipboard.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    rems,
};
use irori_services::LibrarySnapshot;
use irori_ui::components::{GameCover, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};

use super::discoveries::{Discoveries, Finding, Metric, Scored, arranged};
use super::hero::{big_number, eyebrow};
use super::overview::Overview;
use super::{StatsView, Tab, text};

fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.)
}

/// The sentence a finding says.
pub(super) fn sentence(f: &Finding, library: &LibrarySnapshot) -> SharedString {
    let title = |id: &irori_core::GameId| {
        library
            .get(id)
            .map_or_else(|| id.app_name.to_string(), |g| g.title.clone())
    };
    match f {
        Finding::Comeback {
            game, days_away, ..
        } => t_args(
            "library.stats.discoveries.comeback",
            &[("title", &title(game)), ("days", &days_away.to_string())],
        ),
        Finding::Marathon { game, seconds, day } => t_args(
            "library.stats.discoveries.marathon",
            &[
                ("title", &title(game)),
                ("time", &text::duration(*seconds)),
                ("date", &text::short_date(*day)),
            ],
        ),
        Finding::Streak { days } => t_args(
            "library.stats.discoveries.streak",
            &[("days", &days.to_string())],
        ),
        Finding::NewFavourite { game, seconds } => t_args(
            "library.stats.discoveries.new_favourite",
            &[("title", &title(game)), ("time", &text::duration(*seconds))],
        ),
        Finding::BusiestDay { day, seconds } => t_args(
            "library.stats.discoveries.busiest_day",
            &[
                ("date", &text::long_date(*day)),
                ("time", &text::duration(*seconds)),
            ],
        ),
        Finding::NightOwl { share } => t_args(
            "library.stats.discoveries.night_owl",
            &[("share", &percent(*share))],
        ),
        Finding::EarlyBird { share } => t_args(
            "library.stats.discoveries.early_bird",
            &[("share", &percent(*share))],
        ),
        Finding::Weekends { share } => t_args(
            "library.stats.discoveries.weekends",
            &[("share", &percent(*share))],
        ),
        Finding::Devotion { game, share } => t_args(
            "library.stats.discoveries.devotion",
            &[("title", &title(game)), ("share", &percent(*share))],
        ),
        Finding::Variety { games } => t_args(
            "library.stats.discoveries.variety",
            &[("count", &games.to_string())],
        ),
        Finding::Beaten { count } => t_args(
            "library.stats.discoveries.beaten",
            &[("count", &count.to_string())],
        ),
    }
}

/// What a kind of finding is called.
fn kind_label(kind: &str) -> SharedString {
    t(match kind {
        "comeback" => "library.stats.discoveries.kind.comeback",
        "marathon" => "library.stats.discoveries.kind.marathon",
        "streak" => "library.stats.discoveries.kind.streak",
        "new_favourite" => "library.stats.discoveries.kind.new_favourite",
        "busiest_day" => "library.stats.discoveries.kind.busiest_day",
        "night_owl" => "library.stats.discoveries.kind.night_owl",
        "early_bird" => "library.stats.discoveries.kind.early_bird",
        "weekends" => "library.stats.discoveries.kind.weekends",
        "devotion" => "library.stats.discoveries.kind.devotion",
        "variety" => "library.stats.discoveries.kind.variety",
        _ => "library.stats.discoveries.kind.beaten",
    })
}

/// What a milestone of `metric` at `value` is called.
fn milestone_name(metric: Metric, value: u64) -> SharedString {
    let value = value.to_string();
    let key = match metric {
        Metric::Hours => "library.stats.discoveries.milestone.hours",
        Metric::Games => "library.stats.discoveries.milestone.games",
        Metric::Beaten if value == "1" => "library.stats.discoveries.milestone.beaten_one",
        Metric::Beaten => "library.stats.discoveries.milestone.beaten_other",
        Metric::Days => "library.stats.discoveries.milestone.days",
    };
    t_args(key, &[("value", &value)])
}

/// The period in a few lines, for the clipboard.
pub(super) fn summary(
    o: &Overview,
    d: &Discoveries,
    library: &LibrarySnapshot,
    pinned: &[String],
    hidden: &[String],
) -> String {
    let mut lines = vec![
        t_args(
            "library.stats.discoveries.summary",
            &[
                ("period", &text::period(&o.period)),
                ("time", &text::duration(o.seconds)),
                ("days", &o.days_played.to_string()),
                ("games", &o.games.len().to_string()),
            ],
        )
        .to_string(),
    ];
    if let Some(top) = o.games.first() {
        let title = library
            .get(&top.id)
            .map_or_else(|| top.id.app_name.to_string(), |g| g.title.clone());
        lines.push(
            t_args(
                "library.stats.discoveries.summary_top",
                &[("title", &title), ("time", &text::duration(top.seconds))],
            )
            .to_string(),
        );
    }
    for f in arranged(&d.findings, pinned, hidden).into_iter().take(3) {
        lines.push(format!("• {}", sentence(&f.finding, library)));
    }
    lines.join("\n")
}

impl StatsView {
    pub(super) fn render_discoveries(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (Some(d), Some(o)) = (self.discoveries.clone(), self.overview.clone()) else {
            return div().into_any_element();
        };
        let library = models_snapshot(cx);
        let shown: Vec<Scored> = arranged(&d.findings, &self.prefs.pinned, &self.prefs.hidden)
            .into_iter()
            .cloned()
            .collect();
        let lead = shown.first();
        let art = lead
            .and_then(|f| f.finding.game())
            .or_else(|| o.games.first().map(|g| &g.id))
            .and_then(|id| library.get(id))
            .and_then(|g| g.hero_art().map(str::to_owned));
        let headline = v_flex()
            .gap_2()
            .min_w_0()
            .child(eyebrow(
                t_args(
                    "library.stats.discoveries.eyebrow",
                    &[("period", &text::period(&o.period))],
                ),
                cx,
            ))
            .child(match lead {
                Some(f) => div()
                    .text_3xl()
                    .font_bold()
                    .line_clamp(2)
                    .child(sentence(&f.finding, &library))
                    .into_any_element(),
                None => big_number(t("library.stats.discoveries.quiet")),
            })
            .into_any_element();
        let summary_text = summary(&o, &d, &library, &self.prefs.pinned, &self.prefs.hidden);
        let copy = Button::new("stats-copy-summary")
            .small()
            .outline()
            .icon(IconName::Copy)
            .label(t("library.stats.discoveries.copy"))
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(summary_text.clone()));
                window.push_notification(
                    Notification::success(t("library.stats.discoveries.copied")),
                    cx,
                );
            })
            .into_any_element();
        let hero = self.render_hero(art, headline, Some(copy), window, cx);
        let cards: Vec<AnyElement> = shown
            .iter()
            .skip(1)
            .map(|f| self.render_finding(f, &library, cx))
            .collect();
        v_flex()
            .gap_6()
            .child(hero)
            .when(!cards.is_empty(), |c| {
                c.child(h_flex().flex_wrap().gap_3().children(cards))
            })
            .when(!self.prefs.hidden.is_empty(), |c| {
                c.child(
                    Button::new("stats-show-hidden")
                        .ghost()
                        .small()
                        .icon(IconName::Eye)
                        .label(t_args(
                            "library.stats.discoveries.show_hidden",
                            &[("count", &self.prefs.hidden.len().to_string())],
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.prefs.hidden.clear();
                            this.save_prefs(cx);
                        })),
                )
            })
            .child(self.render_milestones(&d, cx))
            .children(self.render_memories(&d, &library, cx))
            .into_any_element()
    }

    fn render_finding(
        &self,
        f: &Scored,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = f.finding.kind();
        let pinned = self.prefs.pinned.iter().any(|p| p == kind);
        let game = f.finding.game().cloned();
        let cover = game
            .as_ref()
            .and_then(|id| library.get(id).cloned())
            .map(|g| GameCover::new(g).width(rems(2.5)));
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let target = game.clone();
        h_flex()
            .id(ElementId::Name(format!("stats-finding-{kind}").into()))
            .keyboard_focusable(cx)
            .flex_1()
            .min_w(rems(18.))
            .gap_3()
            .items_center()
            .p_3()
            .rounded(theme.radius_lg)
            .bg(theme.secondary)
            .border_1()
            .border_color(if pinned { theme.ring } else { theme.border })
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| match &target {
                Some(id) => this.open_game_sheet(id.clone(), window, cx),
                None => this.set_tab(Tab::Overview, cx),
            }))
            .children(cover)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_bold()
                            .text_color(muted)
                            .truncate()
                            .child(SharedString::from(kind_label(kind).to_uppercase())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .line_clamp(3)
                            .child(sentence(&f.finding, library)),
                    ),
            )
            .child(
                Button::new(ElementId::Name(format!("stats-pin-{kind}").into()))
                    .ghost()
                    .xsmall()
                    .icon(if pinned {
                        IconName::PinOff
                    } else {
                        IconName::Pin
                    })
                    .tooltip(if pinned {
                        t("library.stats.discoveries.unpin")
                    } else {
                        t("library.stats.discoveries.pin")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_pin(kind, cx)
                    })),
            )
            .child(
                Button::new(ElementId::Name(format!("stats-hide-{kind}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::EyeOff)
                    .tooltip(t("library.stats.discoveries.hide"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.hide_kind(kind, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_milestones(&self, d: &Discoveries, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let row = |icon: IconName, name: SharedString, when: SharedString| {
            h_flex()
                .gap_3()
                .items_center()
                .text_sm()
                .child(Icon::new(icon).small().text_color(muted))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(div().flex_shrink_0().text_color(muted).child(when))
                .into_any_element()
        };
        let next: Vec<AnyElement> = d
            .next
            .iter()
            .map(|n| {
                let progress = t_args(
                    "library.stats.discoveries.progress",
                    &[("now", &n.now.to_string()), ("value", &n.value.to_string())],
                );
                let when = match n.estimate {
                    Some(day) => t_args(
                        "library.stats.discoveries.around",
                        &[("progress", &progress), ("date", &text::short_date(day))],
                    ),
                    None => progress,
                };
                row(IconName::Milestone, milestone_name(n.metric, n.value), when)
            })
            .collect();
        let reached: Vec<AnyElement> = d
            .reached
            .iter()
            .take(8)
            .map(|m| {
                let when = m
                    .day
                    .map(text::long_date)
                    .unwrap_or_else(|| t("library.stats.discoveries.before_history"));
                row(IconName::Trophy, milestone_name(m.metric, m.value), when)
            })
            .collect();
        panel(cx)
            .gap_4()
            .child(SectionTitle::new(t("library.stats.discoveries.milestones")))
            .when(!next.is_empty(), |c| {
                c.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("library.stats.discoveries.next")),
                        )
                        .children(next),
                )
            })
            .when(!reached.is_empty(), |c| {
                c.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("library.stats.discoveries.reached")),
                        )
                        .children(reached),
                )
            })
            .into_any_element()
    }

    fn render_memories(
        &self,
        d: &Discoveries,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let m = d.memories.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let covers: Vec<AnyElement> = m
            .games
            .iter()
            .take(6)
            .filter_map(|(id, seconds)| {
                let game = library.get(id).cloned()?;
                let open = id.clone();
                Some(
                    v_flex()
                        .id(ElementId::Name(format!("stats-memory-{}", id.key()).into()))
                        .keyboard_focusable(cx)
                        .w(rems(6.))
                        .gap_1()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_game_sheet(open.clone(), window, cx)
                        }))
                        .child(GameCover::new(game.clone()).width(rems(6.)))
                        .child(div().text_xs().truncate().child(game.title.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(text::duration(*seconds)),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let notes: Vec<AnyElement> = m
            .notes
            .iter()
            .take(4)
            .map(|(id, day, words)| {
                let title = library
                    .get(id)
                    .map_or_else(|| id.app_name.to_string(), |g| g.title.clone());
                div()
                    .text_sm()
                    .line_clamp(2)
                    .child(t_args(
                        "library.stats.discoveries.memory_note",
                        &[
                            ("text", words),
                            ("title", &title),
                            ("date", &text::short_date(*day)),
                        ],
                    ))
                    .into_any_element()
            })
            .collect();
        Some(
            panel(cx)
                .gap_4()
                .child(SectionTitle::new(t_args(
                    "library.stats.discoveries.year_ago",
                    &[("date", &text::long_date(m.from + chrono::Days::new(3)))],
                )))
                .when(!covers.is_empty(), |c| {
                    c.child(h_flex().flex_wrap().gap_4().items_start().children(covers))
                })
                .children(notes)
                .into_any_element(),
        )
    }
}

fn models_snapshot(cx: &Context<StatsView>) -> Arc<LibrarySnapshot> {
    irori_ui::models(cx).library.read(cx).snapshot.clone()
}
