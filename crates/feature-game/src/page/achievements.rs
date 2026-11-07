//! The achievements tab: a summary (a ring of the share unlocked, points,
//! hardcore unlocks and the rarest one), the latest unlocks, the likeliest
//! next ones, and every achievement in a list filtered by state. Hidden
//! achievements stay covered until the user reveals one (by clicking it) or
//! all of them. A game whose store groups its achievements into sets (the
//! base game, each DLC) lists them under a heading per set with that set's
//! progress. Rows are virtualized, so games with hundreds of achievements
//! render only what is visible.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::{Progress, ProgressCircle};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit, ParentElement, Pixels,
    SharedString, Size, StatefulInteractiveElement as _, Styled, StyledImage as _, Window, div, px,
    rems, size,
};
use irori_core::util::{parse_rfc3339, unix_now};
use irori_core::{Achievement, AchievementGroup};
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::GamePage;
use crate::load::format_day;

/// Height of one row, in rems.
const ROW_REMS: f32 = 4.5;
/// Height of a set's heading, in rems.
const SET_REMS: f32 = 3.0;
/// Rows visible before the list scrolls on its own.
const VISIBLE_ROWS: usize = 8;
/// Tiles in the "recently unlocked" and "up next" strips.
const STRIP: usize = 3;

/// Which achievements the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AchievementFilter {
    All,
    Unlocked,
    Locked,
}

impl AchievementFilter {
    const ALL: [AchievementFilter; 3] = [
        AchievementFilter::All,
        AchievementFilter::Unlocked,
        AchievementFilter::Locked,
    ];

    fn label(self) -> SharedString {
        t(match self {
            AchievementFilter::All => "game.achievements.filter_all",
            AchievementFilter::Unlocked => "game.achievements.filter_unlocked",
            AchievementFilter::Locked => "game.achievements.filter_locked",
        })
    }
}

fn unlock_ts(a: &Achievement) -> Option<i64> {
    a.unlock_time().and_then(parse_rfc3339)
}

/// The order of locked achievements: furthest progress first, then the
/// most common (the likeliest next unlock).
fn next_first(a: &Achievement, b: &Achievement) -> Ordering {
    let pa = a.progress.unwrap_or(0.0);
    let pb = b.progress.unwrap_or(0.0);
    pb.total_cmp(&pa)
        .then_with(|| b.rarity.partial_cmp(&a.rarity).unwrap_or(Ordering::Equal))
}

/// Filter and order achievements: unlocked first (newest unlock first), then
/// locked ones in [`next_first`] order. Returns indices into `list`.
pub fn arrange(list: &[Achievement], filter: AchievementFilter) -> Vec<usize> {
    let mut out: Vec<usize> = list
        .iter()
        .enumerate()
        .filter(|(_, a)| match filter {
            AchievementFilter::All => true,
            AchievementFilter::Unlocked => a.is_unlocked(),
            AchievementFilter::Locked => !a.is_unlocked(),
        })
        .map(|(ix, _)| ix)
        .collect();
    out.sort_by(|&a, &b| {
        let (a, b) = (&list[a], &list[b]);
        match (a.is_unlocked(), b.is_unlocked()) {
            (true, true) => unlock_ts(b).cmp(&unlock_ts(a)),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => next_first(a, b),
        }
    });
    out
}

/// One set of a game's achievements and how much of it is unlocked.
#[derive(Debug, Clone, PartialEq)]
pub struct SetProgress {
    /// `None` for the achievements no set holds.
    pub group: Option<AchievementGroup>,
    pub unlocked: usize,
    pub total: usize,
}

impl SetProgress {
    pub fn complete(&self) -> bool {
        self.total > 0 && self.unlocked == self.total
    }
}

/// The sets `list` falls into, in the store's order with the achievements
/// of no set last. Empty when there is at most one set, so a game without
/// sets (or with only its base game's) keeps one plain list.
pub fn sets(list: &[Achievement]) -> Vec<SetProgress> {
    let mut out: Vec<SetProgress> = Vec::new();
    for a in list {
        let ix = match out.iter().position(|s| s.group == a.group) {
            Some(ix) => ix,
            None => {
                out.push(SetProgress {
                    group: a.group.clone(),
                    unlocked: 0,
                    total: 0,
                });
                out.len() - 1
            }
        };
        out[ix].total += 1;
        out[ix].unlocked += usize::from(a.is_unlocked());
    }
    if out.len() < 2 {
        return Vec::new();
    }
    out.sort_by(|a, b| match (&a.group, &b.group) {
        (Some(a), Some(b)) => a
            .order
            .cmp(&b.order)
            .then_with(|| a.dlc_app_id.is_some().cmp(&b.dlc_app_id.is_some()))
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    out
}

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The heading of a set: an index into [`sets`].
    Set(usize),
    /// An achievement: an index into the list.
    Item(usize),
}

/// The list's lines: [`arrange`]d achievements, under a heading per set
/// when there are `sets`. A set the filter empties is left out.
pub fn rows(list: &[Achievement], filter: AchievementFilter, sets: &[SetProgress]) -> Vec<Row> {
    let order = arrange(list, filter);
    if sets.is_empty() {
        return order.into_iter().map(Row::Item).collect();
    }
    let set_of: HashMap<Option<&AchievementGroup>, usize> = sets
        .iter()
        .enumerate()
        .map(|(ix, s)| (s.group.as_ref(), ix))
        .collect();
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); sets.len()];
    for ix in order {
        if let Some(&set) = set_of.get(&list[ix].group.as_ref()) {
            buckets[set].push(ix);
        }
    }
    let mut out = Vec::with_capacity(list.len() + sets.len());
    for (set, items) in buckets.into_iter().enumerate() {
        if !items.is_empty() {
            out.push(Row::Set(set));
            out.extend(items.into_iter().map(Row::Item));
        }
    }
    out
}

/// What a game's achievements add up to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub total: usize,
    pub unlocked: usize,
    pub hardcore: usize,
    pub points_total: u64,
    pub points_unlocked: u64,
    /// Locked achievements whose names are covered.
    pub hidden: usize,
    /// Index of the rarest unlock.
    pub rarest: Option<usize>,
    /// The latest unlocks, newest first.
    pub recent: Vec<usize>,
    /// The likeliest next unlocks among those the player may see.
    pub next: Vec<usize>,
}

impl Summary {
    pub fn of(list: &[Achievement], revealed: impl Fn(&Achievement) -> bool) -> Self {
        let order = arrange(list, AchievementFilter::All);
        let unlocked: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| list[i].is_unlocked())
            .collect();
        Self {
            total: list.len(),
            unlocked: unlocked.len(),
            hardcore: list
                .iter()
                .filter(|a| a.hardcore_unlocked_at.is_some())
                .count(),
            points_total: list.iter().map(|a| a.points as u64).sum(),
            points_unlocked: unlocked.iter().map(|&i| list[i].points as u64).sum(),
            hidden: list
                .iter()
                .filter(|a| !a.is_unlocked() && !a.visible)
                .count(),
            rarest: unlocked
                .iter()
                .copied()
                .filter(|&i| list[i].rarity > 0.0)
                .min_by(|&a, &b| list[a].rarity.total_cmp(&list[b].rarity)),
            recent: unlocked.iter().copied().take(STRIP).collect(),
            next: order
                .iter()
                .copied()
                .filter(|&i| {
                    let a = &list[i];
                    !a.is_unlocked() && (a.visible || revealed(a))
                })
                .take(STRIP)
                .collect(),
        }
    }

    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.unlocked as f32 / self.total as f32 * 100.0
        }
    }
}

/// How rare an achievement is, in words: "4.2% of players".
fn rarity_text(a: &Achievement) -> Option<String> {
    match (a.rarity_label.is_empty(), a.rarity > 0.0) {
        (false, true) => Some(format!("{} · {:.1}%", a.rarity_label, a.rarity)),
        (false, false) => Some(a.rarity_label.clone()),
        (true, true) => Some(
            t_args(
                "game.achievements.rarity_players",
                &[("percent", &format!("{:.1}", a.rarity))],
            )
            .to_string(),
        ),
        (true, false) => None,
    }
}

/// Arranged rows and their measurements, rebuilt only when the list, the
/// filter or the rem size change.
pub(crate) struct RowsCache {
    key: Option<(AchievementFilter, usize, Pixels)>,
    sets: Rc<Vec<SetProgress>>,
    rows: Rc<Vec<Row>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    /// Height of the rows shown before the list scrolls, in rems.
    visible_rems: f32,
    scroll: VirtualListScrollHandle,
}

impl Default for RowsCache {
    fn default() -> Self {
        Self {
            key: None,
            sets: Rc::default(),
            rows: Rc::default(),
            sizes: Rc::default(),
            visible_rems: 0.0,
            scroll: VirtualListScrollHandle::new(),
        }
    }
}

impl RowsCache {
    fn sync(&mut self, list: &Arc<Vec<Achievement>>, filter: AchievementFilter, rem: Pixels) {
        let key = (filter, Arc::as_ptr(list) as usize, rem);
        if self.key == Some(key) {
            return;
        }
        let filter_changed = self.key.is_some_and(|k| k.0 != filter);
        let list_changed = self.key.is_none_or(|k| k.1 != key.1);
        if list_changed {
            self.sets = Rc::new(sets(list));
        }
        let rows = rows(list, filter, &self.sets);
        let height = |row: &Row| match row {
            Row::Set(_) => SET_REMS,
            Row::Item(_) => ROW_REMS,
        };
        self.visible_rems = rows.iter().take(VISIBLE_ROWS).map(height).sum();
        self.sizes = Rc::new(rows.iter().map(|r| size(px(0.), rem * height(r))).collect());
        self.rows = Rc::new(rows);
        self.key = Some(key);
        if filter_changed {
            self.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        }
    }
}

/// Which hidden achievements the user chose to see.
#[derive(Debug, Default)]
pub(crate) struct Reveal {
    pub all: bool,
    pub ids: HashSet<String>,
}

impl Reveal {
    fn shows(&self, a: &Achievement) -> bool {
        a.visible || a.is_unlocked() || self.all || self.ids.contains(&a.id)
    }
}

impl GamePage {
    pub(super) fn render_achievements(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(list) = self.achievements.ready().cloned() else {
            return div().into_any_element();
        };
        self.achievement_view
            .sync(&list, self.achievement_filter, window.rem_size());
        let rows_total = self.achievement_view.rows.len();
        let list_rems = self.achievement_view.visible_rems;
        let summary = Summary::of(&list, |a| self.achievement_reveal.shows(a));
        let filter_ix = AchievementFilter::ALL
            .iter()
            .position(|f| *f == self.achievement_filter)
            .unwrap_or(0);
        let icon_px = icon_px(window, 2.5);
        let rows = v_virtual_list(
            cx.entity(),
            "achievement-rows",
            self.achievement_view.sizes.clone(),
            |this, range, window, cx| this.render_achievement_rows(range, window, cx),
        )
        .track_scroll(&self.achievement_view.scroll)
        .size_full();
        let reveal_all = self.achievement_reveal.all;
        let toolbar = h_flex()
            .gap_4()
            .flex_wrap()
            .items_center()
            .child(
                TabBar::new("achievement-filter")
                    .segmented()
                    .small()
                    .selected_index(filter_ix)
                    .children(
                        AchievementFilter::ALL
                            .iter()
                            .map(|f| Tab::new().label(f.label())),
                    )
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        if let Some(f) = AchievementFilter::ALL.get(*ix) {
                            this.achievement_filter = *f;
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .when(summary.hidden > 0, |bar| {
                bar.child(
                    Button::new("achievements-reveal")
                        .small()
                        .ghost()
                        .icon(Icon::new(if reveal_all {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        }))
                        .label(if reveal_all {
                            t("game.achievements.cover_hidden")
                        } else {
                            t_args(
                                "game.achievements.reveal_hidden",
                                &[("count", &summary.hidden.to_string())],
                            )
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.achievement_reveal.all = !this.achievement_reveal.all;
                            if !this.achievement_reveal.all {
                                this.achievement_reveal.ids.clear();
                            }
                            cx.notify();
                        })),
                )
            });
        v_flex()
            .gap_5()
            .child(self.render_achievement_summary(&list, &summary, icon_px, cx))
            .when(
                !summary.recent.is_empty() || !summary.next.is_empty(),
                |col| {
                    col.child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .items_start()
                            .gap_5()
                            .when(!summary.recent.is_empty(), |r| {
                                r.child(self.render_strip(
                                    "achievements-recent",
                                    t("game.achievements.recent"),
                                    &list,
                                    &summary.recent,
                                    icon_px,
                                    cx,
                                ))
                            })
                            .when(!summary.next.is_empty(), |r| {
                                r.child(self.render_strip(
                                    "achievements-next",
                                    t("game.achievements.up_next"),
                                    &list,
                                    &summary.next,
                                    icon_px,
                                    cx,
                                ))
                            }),
                    )
                },
            )
            .child(toolbar)
            .when(rows_total == 0, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("game.achievements.empty")),
                )
            })
            .when(rows_total > 0, |col| {
                col.child(
                    div()
                        .id("achievement-list")
                        .w_full()
                        .h(rems(list_rems))
                        .rounded(cx.theme().radius_lg)
                        .border_1()
                        .border_color(cx.theme().border)
                        .overflow_hidden()
                        .child(rows),
                )
            })
            .into_any_element()
    }

    /// The ring of the share unlocked, with the counts beside it.
    fn render_achievement_summary(
        &self,
        list: &[Achievement],
        s: &Summary,
        icon_px: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rarest_icon = s
            .rarest
            .and_then(|i| list.get(i))
            .map(|a| self.render_icon(a, true, icon_px, 3.0, cx));
        let theme = cx.theme();
        let (muted, success) = (theme.muted_foreground, theme.success);
        let complete = s.total > 0 && s.unlocked == s.total;
        let percent = s.percent();
        let ring = ProgressCircle::new("achievements-ring")
            .size(rems(5.5))
            .color(if complete { success } else { theme.primary })
            .value(percent)
            .accessibility_label(t_args(
                "game.achievements.progress",
                &[
                    ("unlocked", &s.unlocked.to_string()),
                    ("total", &s.total.to_string()),
                ],
            ))
            .child(
                div()
                    .text_lg()
                    .font_extrabold()
                    .child(format!("{}%", percent.round() as u32)),
            );
        let mut lines: Vec<AnyElement> = Vec::new();
        if s.points_total > 0 {
            lines.push(fact(
                IconName::Star,
                t_args(
                    "game.achievements.points",
                    &[
                        ("unlocked", &s.points_unlocked.to_string()),
                        ("total", &s.points_total.to_string()),
                    ],
                ),
                muted,
            ));
        }
        if s.hardcore > 0 {
            lines.push(fact(
                IconName::Flame,
                t_args(
                    "game.achievements.hardcore",
                    &[("count", &s.hardcore.to_string())],
                ),
                muted,
            ));
        }
        if let Some(a) = s.rarest.and_then(|i| list.get(i)) {
            lines.push(fact(
                IconName::Gem,
                t_args(
                    "game.achievements.rarest",
                    &[("name", &a.name), ("percent", &format!("{:.1}", a.rarity))],
                ),
                muted,
            ));
        }
        let theme = cx.theme();
        h_flex()
            .w_full()
            .gap_5()
            .items_center()
            .p_4()
            .rounded(theme.radius_lg)
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .child(ring)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .truncate()
                            .child(if complete {
                                t("game.achievements.complete")
                            } else {
                                t_args(
                                    "game.achievements.progress",
                                    &[
                                        ("unlocked", &s.unlocked.to_string()),
                                        ("total", &s.total.to_string()),
                                    ],
                                )
                            }),
                    )
                    .children(lines),
            )
            .children(rarest_icon)
            .into_any_element()
    }

    /// A titled column of a few achievements (latest unlocks, up next).
    fn render_strip(
        &self,
        id: &'static str,
        title: SharedString,
        list: &[Achievement],
        items: &[usize],
        icon_px: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = unix_now() as i64;
        let muted = cx.theme().muted_foreground;
        let tiles: Vec<AnyElement> = items
            .iter()
            .filter_map(|&i| list.get(i))
            .map(|a| {
                let icon = self.render_icon(a, a.is_unlocked(), icon_px, 2.5, cx);
                let detail = if a.is_unlocked() {
                    a.unlock_time()
                        .filter(|d| !d.is_empty())
                        .map(|d| format_day(d, now))
                        .or_else(|| rarity_text(a))
                } else {
                    rarity_text(a)
                };
                let progress = a.progress.filter(|_| !a.is_unlocked());
                h_flex()
                    .gap_3()
                    .items_center()
                    .min_w_0()
                    .child(icon)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(a.name.clone()),
                            )
                            .children(
                                detail
                                    .map(|d| div().text_xs().text_color(muted).truncate().child(d)),
                            )
                            .children(progress.map(|p| {
                                Progress::new(SharedString::from(format!("{id}-{}", a.id)))
                                    .value(p * 100.0)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id(id)
            .flex_1()
            .min_w(rems(16.))
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .child(SharedString::from(title.to_uppercase())),
            )
            .children(tiles)
            .into_any_element()
    }

    /// An achievement's image (greyed while locked), or an icon without one.
    fn render_icon(
        &self,
        a: &Achievement,
        unlocked: bool,
        icon_px: u32,
        size_rems: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = if unlocked || a.image_locked.is_empty() {
            &a.image_unlocked
        } else {
            &a.image_locked
        };
        let image = (!url.is_empty())
            .then(|| art_img(url, icon_px, true, cx))
            .flatten();
        let theme = cx.theme();
        div()
            .size(rems(size_rems))
            .flex_shrink_0()
            .rounded(theme.radius)
            .overflow_hidden()
            .bg(theme.muted)
            .map(|d| match image {
                Some(img) => d.child(
                    img.image_cache(&self.image_cache)
                        .size_full()
                        .rounded(theme.radius)
                        .object_fit(ObjectFit::Cover)
                        .when(!unlocked, |i| i.grayscale(true)),
                ),
                None => d.flex().items_center().justify_center().child(
                    Icon::new(if unlocked {
                        IconName::Trophy
                    } else {
                        IconName::Lock
                    })
                    .text_color(theme.muted_foreground),
                ),
            })
            .into_any_element()
    }

    fn render_achievement_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(list) = self.achievements.ready().cloned() else {
            return Vec::new();
        };
        let rows = self.achievement_view.rows.clone();
        let sets = self.achievement_view.sets.clone();
        let icon_px = icon_px(window, 3.0);
        let now = unix_now() as i64;
        range
            .filter_map(|row| match *rows.get(row)? {
                Row::Set(set) => Some(render_set_heading(row, sets.get(set)?, cx)),
                Row::Item(ix) => Some(self.render_row(row, list.get(ix)?, icon_px, now, cx)),
            })
            .collect()
    }

    fn render_row(
        &self,
        row: usize,
        a: &Achievement,
        icon_px: u32,
        now: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unlocked = a.is_unlocked();
        let covered = !self.achievement_reveal.shows(a);
        let icon_slot = self.render_icon(a, unlocked, icon_px, 3.0, cx);
        let theme = cx.theme();
        let (name, description) = if covered {
            (
                t("game.achievements.hidden_name").to_string(),
                t("game.achievements.hidden_description").to_string(),
            )
        } else {
            (a.name.clone(), a.description.clone())
        };
        let when = a.unlock_time().filter(|d| !d.is_empty()).map(|w| {
            t_args(
                "game.achievements.unlocked_at",
                &[("date", &format_day(w, now))],
            )
        });
        let points = (a.points > 0).then(|| {
            t_args(
                if a.points == 1 {
                    "game.achievements.worth_one"
                } else {
                    "game.achievements.worth_other"
                },
                &[("points", &a.points.to_string())],
            )
        });
        let progress = a.progress.filter(|_| !unlocked && !covered);
        let hardcore = a.hardcore_unlocked_at.is_some();
        let (success, hover) = (theme.success, theme.list_hover);
        let id = a.id.clone();
        h_flex()
            .id(("achievement", row))
            .w_full()
            .h(rems(ROW_REMS))
            .gap_3()
            .px_3()
            .items_center()
            .when(row > 0, |r| r.border_t_1().border_color(theme.border))
            .when(covered, |r| {
                r.keyboard_focusable(cx)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.achievement_reveal.ids.insert(id.clone());
                        cx.notify();
                    }))
            })
            .child(icon_slot)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .truncate()
                                    .when(!unlocked, |d| d.text_color(theme.muted_foreground))
                                    .child(name),
                            )
                            .when(hardcore, |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(success)
                                        .child(t("game.achievements.hardcore_badge")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(description),
                    )
                    .children(progress.map(|p| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(rems(8.)).child(
                                Progress::new(("achievement-progress", row)).value(p * 100.0),
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}%", (p * 100.0).round() as u32)),
                            )
                    })),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .max_w(rems(12.))
                    .items_end()
                    .gap_0p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .children(points.map(|p| div().truncate().font_medium().child(p)))
                    .children(rarity_text(a).map(|r| div().truncate().child(r)))
                    .children(when.map(|w| div().truncate().child(w))),
            )
            .into_any_element()
    }
}

/// The heading of a set: its name, whether it is a DLC's, and its progress.
fn render_set_heading(row: usize, set: &SetProgress, cx: &mut Context<GamePage>) -> AnyElement {
    let theme = cx.theme();
    let name: SharedString = match &set.group {
        Some(g) if !g.name.trim().is_empty() => g.name.clone().into(),
        _ => t("game.achievements.set_other"),
    };
    let is_dlc = set.group.as_ref().is_some_and(|g| g.dlc_app_id.is_some());
    let complete = set.complete();
    let percent = if set.total == 0 {
        0.0
    } else {
        set.unlocked as f32 / set.total as f32 * 100.0
    };
    let progress = t_args(
        "game.achievements.progress",
        &[
            ("unlocked", &set.unlocked.to_string()),
            ("total", &set.total.to_string()),
        ],
    );
    let mut bar = Progress::new(("achievement-set-progress", row))
        .value(percent)
        .accessibility_label(progress.clone());
    if complete {
        bar = bar.color(theme.success);
    }
    h_flex()
        .id(("achievement-set", row))
        .w_full()
        .h(rems(SET_REMS))
        .gap_3()
        .px_3()
        .items_center()
        .bg(theme.secondary)
        .when(row > 0, |r| r.border_t_1().border_color(theme.border))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .text_sm()
                        .font_semibold()
                        .truncate()
                        .child(name),
                )
                .when(is_dlc, |r| {
                    r.child(
                        div()
                            .flex_shrink_0()
                            .px_1p5()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .font_medium()
                            .text_color(theme.muted_foreground)
                            .child(t("game.achievements.set_dlc")),
                    )
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .max_w(rems(14.))
                .truncate()
                .text_xs()
                .text_color(if complete {
                    theme.success
                } else {
                    theme.muted_foreground
                })
                .child(progress),
        )
        .child(div().flex_shrink_0().w(rems(6.)).child(bar))
        .into_any_element()
}

/// Device pixels of an icon `rems` wide.
fn icon_px(window: &Window, rems: f32) -> u32 {
    let px: f32 = (window.rem_size() * rems * window.scale_factor())
        .ceil()
        .into();
    px as u32
}

/// One line of the summary: a small icon and a text.
fn fact(icon: IconName, text: SharedString, muted: gpui_kit::Hsla) -> AnyElement {
    h_flex()
        .gap_2()
        .min_w_0()
        .items_center()
        .text_sm()
        .text_color(muted)
        .child(Icon::new(icon).small())
        .child(div().min_w_0().truncate().child(text))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(id: &str, rarity: f32, unlocked: Option<&str>) -> Achievement {
        Achievement {
            id: id.into(),
            rarity,
            visible: true,
            unlocked_at: unlocked.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn order_and_filter() {
        let mut close = a("close", 1.0, None);
        close.progress = Some(0.9);
        let list = vec![
            a("rare", 1.0, None),
            a("old", 50.0, Some("2020-01-01T00:00:00Z")),
            a("common", 80.0, None),
            a("new", 10.0, Some("2024-01-01T00:00:00Z")),
            close,
        ];
        let ids = |f| {
            arrange(&list, f)
                .into_iter()
                .map(|ix| list[ix].id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(AchievementFilter::All),
            ["new", "old", "close", "common", "rare"]
        );
        assert_eq!(ids(AchievementFilter::Unlocked), ["new", "old"]);
        assert_eq!(ids(AchievementFilter::Locked), ["close", "common", "rare"]);
    }

    #[test]
    fn summary_counts_points_hidden_and_picks_strips() {
        let mut hidden = a("hidden", 90.0, None);
        hidden.visible = false;
        let mut hard = a("hard", 2.0, None);
        hard.hardcore_unlocked_at = Some("2024-02-01T00:00:00Z".into());
        hard.points = 25;
        let mut easy = a("easy", 60.0, Some("2023-01-01T00:00:00Z"));
        easy.points = 5;
        let mut locked = a("locked", 30.0, None);
        locked.points = 10;
        let list = vec![hidden, hard, easy, locked];
        let s = Summary::of(&list, |_| false);
        assert_eq!((s.total, s.unlocked, s.hardcore, s.hidden), (4, 2, 1, 1));
        assert_eq!((s.points_unlocked, s.points_total), (30, 40));
        assert_eq!(s.rarest, Some(1));
        assert_eq!(s.recent, [1, 2]);
        // The hidden one is likelier but stays out until revealed.
        assert_eq!(s.next, [3]);
        assert_eq!(Summary::of(&list, |_| true).next, [0, 3]);
        assert_eq!(s.percent(), 50.0);
    }

    fn in_set(
        mut a: Achievement,
        id: &str,
        name: &str,
        dlc: Option<&str>,
        order: u32,
    ) -> Achievement {
        a.group = Some(AchievementGroup {
            id: id.into(),
            name: name.into(),
            dlc_app_id: dlc.map(String::from),
            order,
        });
        a
    }

    #[test]
    fn groups_rows_by_set_with_progress() {
        let list = vec![
            in_set(
                a("dlc-locked", 5.0, None),
                "2",
                "The Island",
                Some("1500"),
                1,
            ),
            in_set(
                a("base-old", 50.0, Some("2020-01-01T00:00:00Z")),
                "1",
                "Base game",
                None,
                0,
            ),
            in_set(
                a("dlc-done", 9.0, Some("2024-01-01T00:00:00Z")),
                "2",
                "The Island",
                Some("1500"),
                1,
            ),
            in_set(a("base-locked", 70.0, None), "1", "Base game", None, 0),
            a("loose", 1.0, None),
        ];
        let sets = sets(&list);
        let summary: Vec<_> = sets
            .iter()
            .map(|s| {
                (
                    s.group.as_ref().map(|g| g.name.as_str()),
                    s.unlocked,
                    s.total,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some("Base game"), 1, 2),
                (Some("The Island"), 1, 2),
                (None, 0, 1)
            ]
        );
        let ids = |f| {
            rows(&list, f, &sets)
                .into_iter()
                .map(|r| match r {
                    Row::Set(s) => format!("#{s}"),
                    Row::Item(ix) => list[ix].id.clone(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(AchievementFilter::All),
            [
                "#0",
                "base-old",
                "base-locked",
                "#1",
                "dlc-done",
                "dlc-locked",
                "#2",
                "loose"
            ]
        );
        // A set the filter empties loses its heading too.
        assert_eq!(
            ids(AchievementFilter::Unlocked),
            ["#0", "base-old", "#1", "dlc-done"]
        );
        assert!(!sets[0].complete());
    }

    #[test]
    fn one_set_or_none_keeps_a_plain_list() {
        let plain = vec![a("x", 1.0, None), a("y", 2.0, Some("2024-01-01T00:00:00Z"))];
        assert!(sets(&plain).is_empty());
        assert_eq!(
            rows(&plain, AchievementFilter::All, &[]),
            [Row::Item(1), Row::Item(0)]
        );
        let base_only: Vec<_> = plain
            .into_iter()
            .map(|x| in_set(x, "1", "Base game", None, 0))
            .collect();
        assert!(sets(&base_only).is_empty());
    }
}
