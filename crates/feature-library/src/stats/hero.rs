//! The top of the stats page: a game's art behind everything (its blurred
//! backdrop with the art itself softly over it, darkened towards the text),
//! the tab and period controls, and each tab's headline.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, linear_color_stop, linear_gradient, relative, rems,
};
use irori_core::GameInfo;
use irori_ui::components::{GameCover, art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, art_tint};

use super::overview::Streak;
use super::period::Span;
use super::text;
use super::{StatsView, Tab};

/// Height of the art frame, in rem.
pub(super) const HERO_REM: f32 = 21.;

impl StatsView {
    /// The art frame: `game`'s art behind the controls, `headline` at the
    /// bottom left and `aside` at the bottom right.
    pub(super) fn render_hero(
        &self,
        game: Option<&Arc<GameInfo>>,
        headline: AnyElement,
        aside: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = game.and_then(|g| g.hero_art()).map(str::to_owned);
        let ambient = url
            .as_deref()
            .and_then(|url| irori_ui::art::ambient(url, cx));
        let sharp = url.as_deref().and_then(|url| {
            let width = physical_width(rems(HERO_REM * 2.4), window);
            art_img(url, width, false, cx)
        });
        let tint = ambient.as_ref().map(|a| art_tint(a.rgb, cx));
        let theme = cx.theme();
        let (bg, radius) = (theme.background, theme.radius_lg);
        let layer = || div().absolute().top_0().left_0().size_full();
        // Darken towards the text: left and bottom carry the words.
        let across = linear_gradient(
            90.,
            linear_color_stop(bg.opacity(0.92), 0.),
            linear_color_stop(bg.opacity(0.15), 0.75),
        );
        let down = linear_gradient(
            180.,
            linear_color_stop(bg.opacity(0.), 0.45),
            linear_color_stop(bg.opacity(0.85), 1.),
        );
        v_flex()
            .relative()
            .w_full()
            .h(rems(HERO_REM))
            .rounded(radius)
            .overflow_hidden()
            .bg(tint.map_or(theme.secondary, |t| t.opacity(0.25)))
            .border_1()
            .border_color(theme.border)
            .when_some(ambient, |d, a| {
                d.child(
                    gpui_kit::img(a.backdrop.clone())
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(0.7),
                )
            })
            // The art itself over its blurred copy: soft, but still the game.
            .when_some(sharp, |d, img| {
                d.child(
                    img.absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(0.6),
                )
            })
            .child(layer().bg(across))
            // A second, shorter wash keeps the headline clear of busy art.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(relative(0.55))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(bg.opacity(0.55), 0.),
                        linear_color_stop(bg.opacity(0.), 1.),
                    )),
            )
            .child(layer().bg(down))
            .child(
                v_flex()
                    .relative()
                    .size_full()
                    .p_6()
                    .justify_between()
                    .child(self.render_controls(cx))
                    .child(
                        h_flex()
                            .w_full()
                            .items_end()
                            .justify_between()
                            .gap_6()
                            .child(div().flex_1().min_w_0().child(headline))
                            .children(aside),
                    ),
            )
            .into_any_element()
    }

    /// Tabs on the left; span pills and ‹ period › on the right.
    pub(super) fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = pills(
            "stats-tab",
            [
                (
                    Tab::Overview,
                    IconName::ChartColumn,
                    t("library.stats.tab.overview"),
                ),
                (Tab::Backlog, IconName::Flag, t("library.stats.tab.backlog")),
            ]
            .into_iter()
            .map(|(tab, icon, label)| (Some(icon), label, tab == self.tab, tab))
            .collect(),
            cx,
            |this, tab, cx| this.set_tab(tab, cx),
        );
        let spans = (self.tab == Tab::Overview).then(|| {
            pills(
                "stats-span",
                Span::ALL
                    .into_iter()
                    .map(|span| (None, text::span(span), span == self.span, span))
                    .collect(),
                cx,
                |this, span, cx| this.set_span(span, cx),
            )
        });
        let label = match self.tab {
            Tab::Overview => text::period(&self.period()),
            Tab::Backlog => self.year().to_string().into(),
        };
        let stepper = (self.tab == Tab::Backlog || self.span != Span::All).then(|| {
            let (back, forward) = self.can_step(cx);
            h_flex()
                .gap_1()
                .items_center()
                .rounded_full()
                .px_1()
                .bg(cx.theme().background.opacity(0.6))
                .child(
                    Button::new("stats-previous")
                        .ghost()
                        .xsmall()
                        .rounded_full()
                        .icon(Icon::new(IconName::ChevronLeft))
                        .accessibility_label(t("library.stats.previous"))
                        .tooltip(t("library.stats.previous"))
                        .disabled(!back)
                        .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                )
                .child(
                    div()
                        .min_w(rems(7.))
                        .text_center()
                        .text_sm()
                        .font_semibold()
                        .child(label),
                )
                .child(
                    Button::new("stats-next")
                        .ghost()
                        .xsmall()
                        .rounded_full()
                        .icon(Icon::new(IconName::ChevronRight))
                        .accessibility_label(t("library.stats.next"))
                        .tooltip(t("library.stats.next"))
                        .disabled(!forward)
                        .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                )
        });
        h_flex()
            .w_full()
            .flex_wrap()
            .gap_3()
            .justify_between()
            .child(tabs)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .children(spans)
                    .children(stepper),
            )
            .into_any_element()
    }

    /// The overview's headline: the period's play time in huge type, then
    /// how it compares and how regular it was.
    pub(super) fn render_overview_headline(&self, cx: &Context<Self>) -> AnyElement {
        let Some(o) = &self.overview else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let change = o
            .change_percent()
            .zip(o.period.previous())
            .map(|(pct, before)| {
                let (color, icon, sign) = match pct.signum() {
                    1 => (theme.success, IconName::TrendingUp, "+"),
                    -1 => (theme.danger, IconName::TrendingDown, "−"),
                    _ => (muted, IconName::Minus, ""),
                };
                let value = format!("{sign}{}%", pct.unsigned_abs());
                chip(
                    Some((icon, color)),
                    t_args(
                        if o.previous_so_far {
                            "library.stats.versus_so_far"
                        } else {
                            "library.stats.versus"
                        },
                        &[("change", &value), ("period", &text::period(&before))],
                    ),
                    color,
                )
            });
        let days = t_args(
            if o.days == 1 {
                "library.stats.days_played_one"
            } else {
                "library.stats.days_played_other"
            },
            &[
                ("played", &o.days_played.to_string()),
                ("count", &o.days.to_string()),
            ],
        );
        let streak = match o.streak {
            Streak::Current(0) | Streak::Longest(0) => None,
            Streak::Current(n) => Some(text::count(
                "library.stats.streak_one",
                "library.stats.streak_other",
                n,
            )),
            Streak::Longest(n) => Some(text::count(
                "library.stats.best_streak_one",
                "library.stats.best_streak_other",
                n,
            )),
        };
        let facts: Vec<SharedString> = [
            Some(days),
            streak,
            Some(text::count(
                "library.stats.sessions_one",
                "library.stats.sessions_other",
                o.sessions as u64,
            )),
        ]
        .into_iter()
        .flatten()
        .collect();
        v_flex()
            .gap_2()
            .min_w_0()
            .child(eyebrow(
                t_args(
                    "library.stats.hero.time_played",
                    &[("period", &text::period(&o.period))],
                ),
                cx,
            ))
            .child(big_number(text::duration(o.seconds)))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .items_center()
                    .text_sm()
                    .children(change)
                    .children(dotted(facts, muted)),
            )
            .into_any_element()
    }

    /// Credit for the art: whose it is and why.
    pub(super) fn render_art_credit(
        &self,
        game: &Arc<GameInfo>,
        why: SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .flex_shrink_0()
            .max_w(rems(22.))
            .gap_2()
            .items_center()
            .rounded_full()
            .pl_1()
            .pr_3()
            .py_1()
            .bg(theme.background.opacity(0.6))
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                GameCover::new(game.clone())
                    .width(rems(1.25))
                    .radius(gpui_kit::px(3.)),
            )
            .child(div().min_w_0().truncate().child(why))
            .into_any_element()
    }
}

/// A row of pill buttons, one selected.
fn pills<T: Copy + 'static>(
    id: &'static str,
    items: Vec<(Option<IconName>, SharedString, bool, T)>,
    cx: &mut Context<StatsView>,
    on_pick: fn(&mut StatsView, T, &mut Context<StatsView>),
) -> AnyElement {
    let theme = cx.theme();
    let (track, active, active_fg, fg, hover) = (
        theme.background.opacity(0.6),
        theme.foreground,
        theme.background,
        theme.foreground,
        theme.foreground.opacity(0.1),
    );
    h_flex()
        .gap_0p5()
        .p_1()
        .rounded_full()
        .bg(track)
        .children(
            items
                .into_iter()
                .enumerate()
                .map(|(ix, (icon, label, selected, value))| {
                    h_flex()
                        .id(ElementId::Name(format!("{id}-{ix}").into()))
                        .keyboard_focusable(cx)
                        .gap_1p5()
                        .items_center()
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .text_sm()
                        .font_medium()
                        .cursor_pointer()
                        .map(|d| {
                            if selected {
                                d.bg(active).text_color(active_fg)
                            } else {
                                d.text_color(fg).hover(move |s| s.bg(hover))
                            }
                        })
                        .children(icon.map(|i| Icon::new(i).small()))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| on_pick(this, value, cx)))
                }),
        )
        .into_any_element()
}

/// The small caption above a headline: "— TIME PLAYED · OCTOBER 2026".
pub(super) fn eyebrow(text: SharedString, cx: &Context<StatsView>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap_2()
        .items_center()
        .text_xs()
        .font_bold()
        .text_color(muted)
        .child(div().w(rems(1.5)).h(gpui_kit::px(1.)).bg(muted))
        .child(SharedString::from(text.to_uppercase()))
        .into_any_element()
}

/// A headline figure in display type.
pub(super) fn big_number(text: SharedString) -> AnyElement {
    div()
        .text_size(rems(4.25))
        .line_height(relative(1.))
        .font_extrabold()
        .child(text)
        .into_any_element()
}

/// Short facts separated by dots.
pub(super) fn dotted(facts: Vec<SharedString>, muted: Hsla) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for (ix, fact) in facts.into_iter().enumerate() {
        if ix > 0 {
            out.push(div().text_color(muted).child("·").into_any_element());
        }
        out.push(div().child(fact).into_any_element());
    }
    out
}

/// A tinted pill around a short text, with an optional icon.
pub(super) fn chip(icon: Option<(IconName, Hsla)>, text: SharedString, color: Hsla) -> AnyElement {
    h_flex()
        .gap_1()
        .items_center()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(color.opacity(0.16))
        .text_color(color)
        .text_xs()
        .font_semibold()
        .children(icon.map(|(i, c)| Icon::new(i).xsmall().text_color(c)))
        .child(text)
        .into_any_element()
}
