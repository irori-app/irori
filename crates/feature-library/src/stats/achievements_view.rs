//! The achievements section of the overview: what every game's achievements
//! add up to (unlocked, completion, perfect games, points, hardcore), the
//! rarest unlock and the latest ones across the library, and the insight
//! card counting the period's unlocks. Totals come from the answers stores
//! gave so far (`irori_services::achievements`); "Check played games" asks
//! the stores of every game with playtime, one after another.

use chrono::{Local, NaiveDate, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div,
    rems,
};
use irori_core::GameId;
use irori_services::{AchievementTotals, LibrarySnapshot, Unlock};
use irori_ui::components::{SectionTitle, art_img};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{Route, backend, models, navigate};

use super::StatsView;
use super::period::Period;
use super::text;

/// Latest unlocks listed.
const RECENT: usize = 6;

/// Unix time of a local day's start.
fn day_start(day: NaiveDate) -> i64 {
    day.and_hms_opt(0, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map_or(0, |t| t.timestamp())
}

/// The game of a totals key, when it is still in the library.
fn game_of(key: &str, library: &LibrarySnapshot) -> Option<GameId> {
    let (runner, app) = key.split_once(':')?;
    let id = GameId::new(irori_core::Runner::from_id(runner)?, app);
    library.get(&id).is_some().then_some(id)
}

impl StatsView {
    /// Read the achievement totals (off the UI thread).
    pub(super) fn reload_achievements(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.achievement_index().await });
        cx.spawn(async move |this, cx| {
            if let Some(index) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.achievements = Some(index);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Ask the stores of every played game for its achievements.
    fn scan_achievements(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.achievements_scanning {
            return;
        }
        let l = models(cx).library.read(cx);
        let ids: Vec<GameId> = l
            .snapshot
            .games
            .iter()
            .filter(|g| g.runner() != irori_core::Runner::Sideload)
            .map(|g| g.id())
            .filter(|id| {
                l.user
                    .playtime
                    .get(&id.key())
                    .is_some_and(|p| p.total_minutes > 0)
            })
            .collect();
        self.achievements_scanning = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            let found = b.scan_achievements(ids).await;
            (found, b.achievement_index().await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.achievements_scanning = false;
                if let Some((found, index)) = result {
                    this.achievements = Some(index);
                    window.push_notification(
                        gpui_kit::component::notification::Notification::success(text::count(
                            "library.stats.achievements.scanned_one",
                            "library.stats.achievements.scanned_other",
                            found as u64,
                        )),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn achievement_totals(&self, library: &LibrarySnapshot) -> Option<AchievementTotals> {
        let index = self.achievements.as_ref()?;
        Some(index.totals(|k| game_of(k, library).is_some(), RECENT))
    }

    /// The insight card counting the period's unlocks, when there are any.
    pub(super) fn achievement_insight(
        &self,
        period: Period,
        library: &LibrarySnapshot,
    ) -> Option<(SharedString, Option<GameId>)> {
        let index = self.achievements.as_ref()?;
        let today = Local::now().date_naive();
        let (from, until) = (day_start(period.start), day_start(period.end(today)));
        let count = index.unlocked_between(|k| game_of(k, library).is_some(), from, until);
        if count == 0 {
            return None;
        }
        let latest = index
            .totals(|k| game_of(k, library).is_some(), 1)
            .recent
            .first()
            .and_then(|(k, _)| game_of(k, library));
        Some((
            text::count(
                "library.stats.insight.achievements_one",
                "library.stats.insight.achievements_other",
                count as u64,
            ),
            latest,
        ))
    }

    pub(super) fn render_achievements_section(
        &self,
        library: &LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let totals = self.achievement_totals(library).unwrap_or_default();
        let scanning = self.achievements_scanning;
        let check = Button::new("stats-achievements-scan")
            .small()
            .ghost()
            .icon(Icon::new(IconName::RefreshCw))
            .label(t("library.stats.achievements.scan"))
            .loading(scanning)
            .disabled(scanning)
            .on_click(cx.listener(|this, _, window, cx| this.scan_achievements(window, cx)));
        let header = h_flex()
            .flex_wrap()
            .justify_between()
            .items_center()
            .gap_2()
            .child(SectionTitle::new(t("library.stats.achievements.title")))
            .child(check);
        if totals.games == 0 {
            return panel(cx)
                .gap_3()
                .child(header)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("library.stats.achievements.none")),
                )
                .into_any_element();
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let percent = if totals.total == 0 {
            0.0
        } else {
            totals.unlocked as f32 / totals.total as f32 * 100.0
        };
        let ring = ProgressCircle::new("stats-achievements-ring")
            .size(rems(6.))
            .color(theme.chart_2)
            .value(percent)
            .accessibility_label(t_args(
                "library.stats.achievements.completion",
                &[("percent", &format!("{}", percent.round() as u32))],
            ))
            .child(
                v_flex()
                    .items_center()
                    .child(
                        div()
                            .text_xl()
                            .font_extrabold()
                            .child(format!("{}%", percent.round() as u32)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("library.stats.achievements.done")),
                    ),
            );
        let tile = |value: String, label: SharedString| {
            v_flex()
                .min_w(rems(7.))
                .gap_0p5()
                .child(div().text_2xl().font_extrabold().child(value))
                .child(div().text_xs().text_color(muted).truncate().child(label))
        };
        let mut tiles = vec![
            tile(
                totals.unlocked.to_string(),
                t_args(
                    "library.stats.achievements.unlocked_of",
                    &[("total", &totals.total.to_string())],
                ),
            ),
            tile(
                totals.perfect.to_string(),
                text::count_word(
                    "library.stats.achievements.perfect_one",
                    "library.stats.achievements.perfect_other",
                    totals.perfect as u64,
                ),
            ),
            tile(
                totals.games.to_string(),
                text::count_word(
                    "library.stats.achievements.games_one",
                    "library.stats.achievements.games_other",
                    totals.games as u64,
                ),
            ),
        ];
        if totals.points > 0 {
            tiles.push(tile(
                totals.points.to_string(),
                t("library.stats.achievements.points"),
            ));
        }
        if totals.hardcore > 0 {
            tiles.push(tile(
                totals.hardcore.to_string(),
                t("library.stats.achievements.hardcore"),
            ));
        }
        let icon_px = {
            let px: f32 = (window.rem_size() * 2.5 * window.scale_factor())
                .ceil()
                .into();
            px as u32
        };
        let rarest = totals
            .rarest
            .as_ref()
            .map(|(key, u)| self.render_unlock("stats-rarest", key, u, true, icon_px, library, cx));
        let recent: Vec<AnyElement> = totals
            .recent
            .iter()
            .enumerate()
            .map(|(ix, (key, u))| {
                self.render_unlock(
                    SharedString::from(format!("stats-unlock-{ix}")),
                    key,
                    u,
                    false,
                    icon_px,
                    library,
                    cx,
                )
            })
            .collect();
        let theme = cx.theme();
        panel(cx)
            .gap_4()
            .child(header)
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_6()
                    .child(ring)
                    .child(h_flex().flex_wrap().gap_6().children(tiles)),
            )
            .children(rarest.map(|r| {
                v_flex()
                    .gap_2()
                    .child(caption(t("library.stats.achievements.rarest"), muted))
                    .child(r)
            }))
            .when(!recent.is_empty(), |d| {
                d.child(
                    v_flex()
                        .gap_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(caption(t("library.stats.achievements.recent"), muted))
                        .child(
                            div()
                                .grid()
                                .grid_cols(2)
                                .gap_x_6()
                                .gap_y_3()
                                .children(recent),
                        ),
                )
            })
            .into_any_element()
    }

    /// One unlock: its image, name, game and date (or rarity), opening the
    /// game's page.
    #[allow(clippy::too_many_arguments)] // One row of the section; splitting it hides nothing.
    fn render_unlock(
        &self,
        id: impl Into<SharedString>,
        key: &str,
        u: &Unlock,
        show_rarity: bool,
        icon_px: u32,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let game = game_of(key, library);
        let title = game
            .as_ref()
            .and_then(|g| library.get(g))
            .map(|g| g.title.clone())
            .unwrap_or_default();
        let when = if show_rarity && u.rarity > 0.0 {
            t_args(
                "library.stats.achievements.rarity",
                &[("percent", &format!("{:.1}", u.rarity))],
            )
        } else {
            irori_core::util::parse_rfc3339(&u.at)
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                .map(|d| d.with_timezone(&Local).date_naive())
                .map(text::short_date)
                .unwrap_or_default()
        };
        let image = (!u.icon.is_empty())
            .then(|| art_img(&u.icon, icon_px, true, cx))
            .flatten();
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let id: SharedString = id.into();
        h_flex()
            .id(id)
            .min_w_0()
            .gap_3()
            .items_center()
            .rounded(theme.radius)
            .when(game.is_some(), |d| {
                d.keyboard_focusable(cx)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
            })
            .child(
                div()
                    .size(rems(2.5))
                    .flex_shrink_0()
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .map(|d| match image {
                        Some(img) => d.child(
                            img.image_cache(&self.images)
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        ),
                        None => d
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Trophy).small().text_color(muted)),
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .truncate()
                            .child(u.name.clone()),
                    )
                    .child(div().text_xs().text_color(muted).truncate().child(
                        if title.is_empty() {
                            when.clone()
                        } else {
                            t_args(
                                "library.stats.achievements.in_game",
                                &[("title", &title), ("when", &when)],
                            )
                        },
                    )),
            )
            .when_some(game, |d, id| {
                d.on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx))
            })
            .into_any_element()
    }
}

fn caption(text: SharedString, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .text_xs()
        .font_bold()
        .text_color(color)
        .child(SharedString::from(text.to_uppercase()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_start_at_local_midnight() {
        let day = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let next = NaiveDate::from_ymd_opt(2024, 3, 2).unwrap();
        let span = day_start(next) - day_start(day);
        // A day is 23 to 25 hours long across a clock change.
        assert!((23 * 3600..=25 * 3600).contains(&span));
    }
}
