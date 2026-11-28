//! `My games` sub-view inside `Stores -> News`: scope picker, kind filters,
//! time-grouped feed list with automatic hotfix collapsing, and native reader
//! pane with live game actions and one-click mute with undo.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px, rems,
};
use irori_core::{GameId, Runner};
use irori_services::{CategoryRef, GameNewsEntry, NewsFeedSnapshot, NewsKind, NewsScope};
use irori_ui::categories::{CategoryEntry, entries as category_entries};
use irori_ui::dates::calendar_date;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::style::panel;

use super::{NewsSection, now_unix, set_news_shown};
use crate::common::backend_op;

/// Minimum width (in rem) for side-by-side list + reader layout.
const SPLIT_MIN_REM: f32 = 56.;
/// Fixed width (in rem) of the left feed list column in split mode.
const LIST_COLUMN_REM: f32 = 22.;

/// Chronological time bucket for grouping feed cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TimeBucket {
    Today,
    Yesterday,
    ThisWeek,
    Earlier,
}

impl TimeBucket {
    pub(crate) fn from_timestamps(item_ts: u64, now_ts: u64) -> Self {
        let item_i64 = i64::try_from(item_ts).unwrap_or(0);
        let now_i64 = i64::try_from(now_ts).unwrap_or(0);
        let days = (now_i64 - item_i64).div_euclid(86_400);
        match days {
            i64::MIN..=0 => Self::Today,
            1 => Self::Yesterday,
            2..=6 => Self::ThisWeek,
            _ => Self::Earlier,
        }
    }

    pub(crate) fn label(self) -> SharedString {
        match self {
            Self::Today => t("manage.news.bucket.today"),
            Self::Yesterday => t("manage.news.bucket.yesterday"),
            Self::ThisWeek => t("manage.news.bucket.this_week"),
            Self::Earlier => t("manage.news.bucket.earlier"),
        }
    }
}

/// One visible entry row in the feed list, with optional collapsed hotfixes
/// from the same game directly beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedRow {
    pub entry_idx: usize,
    pub bucket: TimeBucket,
    pub group_key: String,
    pub collapsed_siblings: Vec<usize>,
    pub expanded: bool,
}

/// Filter entries by optional kind and case-insensitive game/post title search.
pub(crate) fn filter_entries(
    entries: &[GameNewsEntry],
    kind_filter: Option<NewsKind>,
    query: &str,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| kind_filter.is_none_or(|wanted| e.item.kind == wanted))
        .filter(|(_, e)| {
            q.is_empty()
                || e.title.to_lowercase().contains(&q)
                || e.item.title.to_lowercase().contains(&q)
        })
        .map(|(idx, _)| idx)
        .collect()
}

/// Group filtered entry indices into chronological buckets and collapse
/// consecutive posts from the same game within the same bucket.
pub(crate) fn group_rows(
    entries: &[GameNewsEntry],
    filtered: &[usize],
    now_ts: u64,
    expanded_groups: &HashSet<String>,
) -> Vec<FeedRow> {
    let mut rows: Vec<FeedRow> = Vec::with_capacity(filtered.len());
    let mut pos = 0;
    while pos < filtered.len() {
        let Some(&first_idx) = filtered.get(pos) else {
            break;
        };
        let Some(first) = entries.get(first_idx) else {
            pos += 1;
            continue;
        };
        let bucket = TimeBucket::from_timestamps(first.item.date, now_ts);
        let mut run_end = pos + 1;
        while let Some(&next_idx) = filtered.get(run_end) {
            let Some(next) = entries.get(next_idx) else {
                break;
            };
            if next.game_id == first.game_id
                && TimeBucket::from_timestamps(next.item.date, now_ts) == bucket
            {
                run_end += 1;
            } else {
                break;
            }
        }
        let group_key = format!("{:?}:{}:{}", bucket, first.game_id.key(), first.item.gid);
        let siblings: Vec<usize> = filtered
            .get(pos + 1..run_end)
            .map(<[usize]>::to_vec)
            .unwrap_or_default();
        let expanded = !siblings.is_empty() && expanded_groups.contains(&group_key);
        rows.push(FeedRow {
            entry_idx: first_idx,
            bucket,
            group_key,
            collapsed_siblings: siblings,
            expanded,
        });
        pos = run_end;
    }
    rows
}

pub(super) fn format_date(ts: u64, now_ts: u64) -> String {
    let ts_i64 = i64::try_from(ts).unwrap_or(0);
    let now_i64 = i64::try_from(now_ts).unwrap_or(0);
    let days = (now_i64 - ts_i64).div_euclid(86_400);
    match days {
        i64::MIN..=0 => t("manage.news.bucket.today").to_string(),
        1 => t("manage.news.bucket.yesterday").to_string(),
        2..=6 => t_args("manage.news.days_ago", &[("count", &days.to_string())]).to_string(),
        _ => match (
            chrono::DateTime::from_timestamp(ts_i64, 0),
            chrono::DateTime::from_timestamp(now_i64, 0),
        ) {
            (Some(dt), Some(now_dt)) => {
                use chrono::Datelike as _;
                let date = dt.date_naive();
                calendar_date(date, date.year() != now_dt.date_naive().year())
            }
            _ => String::new(),
        },
    }
}

pub(super) fn store_chip_label(store: Runner, via_steam: bool) -> SharedString {
    if via_steam {
        t_args("manage.news.via_steam", &[("store", store.store_name())])
    } else {
        t("manage.news.source_steam")
    }
}

/// UI state for the `My games` sub-view.
#[derive(Default)]
pub(crate) struct MyGamesState {
    pub scope: NewsScope,
    pub kind_filter: Option<NewsKind>,
    pub query: String,
    pub snapshot: NewsFeedSnapshot,
    pub session_last_visited_at: u64,
    pub selected_gid: Option<String>,
    pub expanded_groups: HashSet<String>,
    pub loaded: bool,
    pub initial_loading: bool,
    pub refreshing: bool,
    pub error: Option<SharedString>,
}

impl MyGamesState {
    /// Ensure `selected_gid` points to a currently visible post when possible.
    pub(crate) fn sync_selection(&mut self) {
        let filtered = filter_entries(&self.snapshot.entries, self.kind_filter, &self.query);
        let still_visible = self.selected_gid.as_deref().is_some_and(|sel| {
            filtered.iter().any(|&idx| {
                self.snapshot
                    .entries
                    .get(idx)
                    .is_some_and(|e| e.item.gid == sel)
            })
        });
        if !still_visible {
            self.selected_gid = filtered
                .first()
                .and_then(|&idx| self.snapshot.entries.get(idx))
                .map(|e| e.item.gid.clone());
        }
    }
}

impl NewsSection {
    pub(crate) fn render_my_games(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now_ts = now_unix();
        let filtered = filter_entries(
            &self.my_games.snapshot.entries,
            self.my_games.kind_filter,
            &self.my_games.query,
        );
        let rows = group_rows(
            &self.my_games.snapshot.entries,
            &filtered,
            now_ts,
            &self.my_games.expanded_groups,
        );

        let selected_idx = self
            .my_games
            .selected_gid
            .as_deref()
            .and_then(|gid| {
                filtered.iter().copied().find(|&idx| {
                    self.my_games
                        .snapshot
                        .entries
                        .get(idx)
                        .is_some_and(|e| e.item.gid == gid)
                })
            })
            .or_else(|| filtered.first().copied());

        let toolbar = self.render_my_games_toolbar(cx);
        let notice = self.render_my_games_notice(cx);

        let body: AnyElement = if self.my_games.initial_loading && filtered.is_empty() {
            h_flex()
                .w_full()
                .items_start()
                .gap_4()
                .child(
                    v_flex()
                        .w(rems(LIST_COLUMN_REM))
                        .gap_2p5()
                        .child(Skeleton::new().h(rems(5.5)).w_full())
                        .child(Skeleton::new().h(rems(5.5)).w_full())
                        .child(Skeleton::new().h(rems(5.5)).w_full()),
                )
                .child(Skeleton::new().flex_1().h(rems(20.)))
                .into_any_element()
        } else if self.my_games.snapshot.entries.is_empty() {
            self.render_my_games_empty(false, cx)
        } else if filtered.is_empty() {
            self.render_my_games_empty(true, cx)
        } else {
            let measured = self.body_width.get();
            let wide = measured <= px(0.) || (measured / window.rem_size()) >= SPLIT_MIN_REM;
            let list_el = self.render_feed_list(&rows, selected_idx, now_ts, cx);
            let reader_el = self.render_reader_pane(selected_idx, now_ts, cx);
            let width_tracker = self.body_width.clone();
            let layout: AnyElement = if wide {
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .w(rems(LIST_COLUMN_REM))
                            .flex_shrink_0()
                            .child(list_el),
                    )
                    .child(div().flex_1().min_w_0().child(reader_el))
                    .into_any_element()
            } else {
                v_flex()
                    .w_full()
                    .gap_4()
                    .child(list_el)
                    .child(reader_el)
                    .into_any_element()
            };
            div()
                .relative()
                .w_full()
                .child(
                    canvas(
                        move |bounds, window, _| {
                            if width_tracker.replace(bounds.size.width) != bounds.size.width {
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .child(layout)
                .into_any_element()
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(toolbar)
            .children(notice)
            .child(body)
            .into_any_element()
    }

    fn render_my_games_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let msg = if let Some(err) = &self.my_games.error {
            format!("{} {}", t("manage.news.load_failed"), err)
        } else if self.my_games.snapshot.rate_limited {
            t("manage.news.rate_limited").to_string()
        } else if self.my_games.snapshot.offline {
            t("manage.news.offline_cached").to_string()
        } else {
            return None;
        };
        Some(
            div()
                .text_xs()
                .text_color(muted)
                .child(msg)
                .into_any_element(),
        )
    }

    fn render_my_games_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let lib = models(cx).library.read(cx);
        let collections: Vec<CategoryEntry> = category_entries(&lib.user, &lib.categories);
        let muted_games: Vec<(GameId, String)> = lib
            .user
            .news_muted
            .iter()
            .filter_map(|key| {
                let (r, app) = key.split_once(':')?;
                let runner = Runner::from_id(r)?;
                let id = GameId::new(runner, app);
                let title = lib
                    .snapshot
                    .get(&id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| app.to_string());
                Some((id, title))
            })
            .collect();

        let scope = self.my_games.scope.clone();
        let scope_name: SharedString = match &scope {
            NewsScope::Installed => t("manage.news.scope.installed"),
            NewsScope::Favorites => t("manage.news.scope.favorites"),
            NewsScope::Collection(cat) => collections
                .iter()
                .find(|c| &c.category == cat)
                .map(|c| SharedString::from(c.name.clone()))
                .unwrap_or_else(|| match cat {
                    CategoryRef::Manual(name) => SharedString::from(name.clone()),
                    CategoryRef::Smart(id) => SharedString::from(id.clone()),
                }),
        };
        let scope_button_label = t_args(
            "manage.news.scope_button",
            &[("scope", scope_name.as_ref())],
        );

        let view = cx.entity().downgrade();
        let scope_menu_scope = scope.clone();
        let scope_button = Button::new("news-scope-dropdown")
            .small()
            .outline()
            .dropdown_caret(true)
            .label(scope_button_label)
            .dropdown_menu(move |mut menu, _, _| {
                let v_inst = view.clone();
                menu = menu.item(
                    PopupMenuItem::new(t("manage.news.scope.installed"))
                        .checked(scope_menu_scope == NewsScope::Installed)
                        .on_click(move |_, window, cx| {
                            let _ = v_inst.update(cx, |this, cx| {
                                this.set_scope(NewsScope::Installed, window, cx)
                            });
                        }),
                );
                let v_fav = view.clone();
                menu = menu.item(
                    PopupMenuItem::new(t("manage.news.scope.favorites"))
                        .checked(scope_menu_scope == NewsScope::Favorites)
                        .on_click(move |_, window, cx| {
                            let _ = v_fav.update(cx, |this, cx| {
                                this.set_scope(NewsScope::Favorites, window, cx)
                            });
                        }),
                );
                if !collections.is_empty() {
                    menu = menu.separator().label(t("manage.news.scope.collections"));
                    for col in &collections {
                        let cat = col.category.clone();
                        let checked =
                            matches!(&scope_menu_scope, NewsScope::Collection(c) if c == &cat);
                        let v_col = view.clone();
                        let label = format!("{} ({})", col.name, col.count);
                        menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                            move |_, window, cx| {
                                let target = NewsScope::Collection(cat.clone());
                                let _ =
                                    v_col.update(cx, |this, cx| this.set_scope(target, window, cx));
                            },
                        ));
                    }
                }
                menu
            });

        let kind_filter = self.my_games.kind_filter;
        let kind_btn = |id: &'static str, label: SharedString, target: Option<NewsKind>| {
            let active = kind_filter == target;
            let btn = Button::new(id).small().label(label);
            let btn = if active { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                if this.my_games.kind_filter != target {
                    this.my_games.kind_filter = target;
                    this.my_games.sync_selection();
                    cx.notify();
                }
            }))
        };

        let view_more = cx.entity().downgrade();
        let overflow_btn = Button::new("news-more-menu")
            .small()
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip(t("manage.news.more"))
            .accessibility_label(t("manage.news.more"))
            .dropdown_menu(move |mut menu, _, _| {
                let v_read = view_more.clone();
                menu = menu.item(PopupMenuItem::new(t("manage.news.mark_all_read")).on_click(
                    move |_, _, cx| {
                        let _ = v_read.update(cx, |this, cx| this.mark_all_read(cx));
                    },
                ));
                if !muted_games.is_empty() {
                    menu = menu.separator().label(t("manage.news.muted_header"));
                    for (id, title) in &muted_games {
                        let id_copy = id.clone();
                        let v_unmute = view_more.clone();
                        let label = t_args("manage.news.unmute_game", &[("game", title.as_str())]);
                        menu =
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                let id_inner = id_copy.clone();
                                backend_op(cx, move |b| b.mute_news(&id_inner, false));
                                let _ = v_unmute
                                    .update(cx, |this, cx| this.reload_cached_my_games(window, cx));
                            }));
                    }
                }
                menu =
                    menu.separator()
                        .item(PopupMenuItem::new(t("manage.news.hide_tab")).on_click(
                            move |_, window, cx| {
                                set_news_shown(false, window, cx);
                            },
                        ));
                menu
            });

        h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(scope_button)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(kind_btn("news-kind-all", t("manage.news.filter.all"), None))
                            .child(kind_btn(
                                "news-kind-patches",
                                t("manage.news.filter.patches"),
                                Some(NewsKind::PatchNotes),
                            ))
                            .child(kind_btn(
                                "news-kind-general",
                                t("manage.news.filter.news"),
                                Some(NewsKind::News),
                            )),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().w(rems(13.)).child(Input::new(&self.search).small()))
                    .child(
                        Button::new("news-refresh")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .loading(self.my_games.refreshing)
                            .tooltip(t("manage.news.refresh"))
                            .accessibility_label(t("manage.news.refresh"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_my_games(true, window, cx);
                            })),
                    )
                    .child(overflow_btn),
            )
            .into_any_element()
    }

    fn render_feed_list(
        &self,
        rows: &[FeedRow],
        selected_idx: Option<usize>,
        now_ts: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut list = v_flex().w_full().gap_2();
        let mut last_bucket: Option<TimeBucket> = None;

        for row in rows {
            if last_bucket != Some(row.bucket) {
                last_bucket = Some(row.bucket);
                list = list.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(row.bucket.label()),
                );
            }
            if let Some(entry) = self.my_games.snapshot.entries.get(row.entry_idx) {
                list = list.child(self.render_feed_card(
                    entry,
                    selected_idx == Some(row.entry_idx),
                    false,
                    now_ts,
                    cx,
                ));
            }
            if !row.collapsed_siblings.is_empty() {
                if row.expanded {
                    for &sib_idx in &row.collapsed_siblings {
                        if let Some(sib) = self.my_games.snapshot.entries.get(sib_idx) {
                            list = list.child(self.render_feed_card(
                                sib,
                                selected_idx == Some(sib_idx),
                                true,
                                now_ts,
                                cx,
                            ));
                        }
                    }
                }
                let group_key = row.group_key.clone();
                let toggle_label = if row.expanded {
                    t("manage.news.earlier_hide")
                } else {
                    t_args(
                        "manage.news.earlier_more",
                        &[("count", &row.collapsed_siblings.len().to_string())],
                    )
                };
                list = list.child(
                    div().pl_2().child(
                        Button::new(ElementId::Name(format!("news-grp-{group_key}").into()))
                            .xsmall()
                            .ghost()
                            .label(toggle_label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.my_games.expanded_groups.remove(&group_key) {
                                    this.my_games.expanded_groups.insert(group_key.clone());
                                }
                                cx.notify();
                            })),
                    ),
                );
            }
        }

        list.into_any_element()
    }

    fn render_feed_card(
        &self,
        entry: &GameNewsEntry,
        selected: bool,
        indented: bool,
        now_ts: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let bg = if selected {
            theme.accent
        } else {
            theme.group_box
        };
        let border = if selected {
            theme.primary
        } else {
            theme.border
        };
        let muted = theme.muted_foreground;
        let primary = theme.primary;
        let radius = theme.radius;
        let unread = self.my_games.session_last_visited_at > 0
            && entry.item.date > self.my_games.session_last_visited_at;
        let gid = entry.item.gid.clone();
        let date = format_date(entry.item.date, now_ts);
        let badge = match entry.item.kind {
            NewsKind::PatchNotes => Tag::success()
                .xsmall()
                .child(t("manage.news.filter.patches")),
            NewsKind::News => Tag::info().xsmall().child(t("manage.news.filter.news")),
        };

        v_flex()
            .id(ElementId::Name(
                format!("news-row-{}", entry.item.gid).into(),
            ))
            .cursor_pointer()
            .w_full()
            .when(indented, |el| el.ml_3())
            .p_3()
            .gap_1p5()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(bg)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.my_games.selected_gid = Some(gid.clone());
                cx.notify();
            }))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .when(unread, |r| {
                                r.child(
                                    div()
                                        .w(px(7.))
                                        .h(px(7.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(primary),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .font_semibold()
                                    .truncate()
                                    .child(entry.title.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(muted)
                            .child(date),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .child(entry.item.title.clone()),
            )
            .child(
                h_flex()
                    .items_center()
                    .flex_wrap()
                    .gap_1p5()
                    .child(badge)
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(store_chip_label(entry.store, entry.via_steam)),
                    ),
            )
            .into_any_element()
    }

    fn render_my_games_empty(&self, filtered_out: bool, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if filtered_out {
            return panel(cx)
                .w_full()
                .p_6()
                .gap_3()
                .items_start()
                .child(div().text_sm().child(t("manage.news.no_match")))
                .child(
                    Button::new("news-clear-filters")
                        .small()
                        .outline()
                        .label(t("manage.news.clear_filters"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.my_games.kind_filter = None;
                            this.my_games.query.clear();
                            this.search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                            this.my_games.sync_selection();
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        panel(cx)
            .w_full()
            .p_6()
            .gap_2()
            .items_start()
            .child(div().text_sm().font_medium().child(t("manage.news.empty")))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.news.empty_hint")),
            )
            .child(
                Button::new("news-empty-refresh")
                    .small()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(t("manage.news.refresh"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.load_my_games(true, window, cx);
                    })),
            )
            .into_any_element()
    }
}
