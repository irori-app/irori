//! `Releases` sub-view inside `Stores -> News`: keyless Steam New this week and
//! Coming soon cards with platform/genre filters, library ownership cross-check,
//! and direct Steam Store links.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, rems,
};
use irori_core::{GameId, Runner};
use irori_services::{ReleasesSnapshot, StoreReleaseItem};
use irori_ui::components::art_img;
use irori_ui::dates::calendar_date;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::panel;
use irori_ui::{Route, backend, models, navigate};

use super::{NewsSection, now_unix};
use crate::common::{card, card_grid_capped};

/// Minimum card width (in rem) for release cards.
const MIN_RELEASE_CARD_REM: f32 = 18.5;
/// Maximum columns in release grids on wide screens.
const MAX_RELEASE_COLUMNS: u16 = 4;

/// Platform filter pills on the Releases sub-tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PlatformFilter {
    #[default]
    All,
    Windows,
    Linux,
    MacOs,
}

impl PlatformFilter {
    fn matches(self, item: &StoreReleaseItem) -> bool {
        match self {
            Self::All => true,
            Self::Windows => item.windows,
            Self::Linux => item.linux,
            Self::MacOs => item.macos,
        }
    }
}

/// UI state for the `Releases` sub-view.
#[derive(Default)]
pub(crate) struct ReleasesState {
    pub snapshot: ReleasesSnapshot,
    pub platform: PlatformFilter,
    pub genre: Option<String>,
    pub query: String,
    pub loaded: bool,
    pub initial_loading: bool,
    pub refreshing: bool,
    pub error: Option<SharedString>,
    load_gen: u64,
}

/// Filter release items by platform, optional genre, and case-insensitive search.
pub(crate) fn filter_releases(
    items: &[StoreReleaseItem],
    platform: PlatformFilter,
    genre: Option<&str>,
    query: &str,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| platform.matches(item))
        .filter(|(_, item)| {
            genre.is_none_or(|wanted| item.genres.iter().any(|g| g.eq_ignore_ascii_case(wanted)))
        })
        .filter(|(_, item)| {
            q.is_empty()
                || item.name.to_lowercase().contains(&q)
                || item.developer.to_lowercase().contains(&q)
                || item.short_description.to_lowercase().contains(&q)
        })
        .map(|(idx, _)| idx)
        .collect()
}

fn format_release_date(item: &StoreReleaseItem, now_ts: u64) -> String {
    if let Some(custom) = item.release_label.as_deref()
        && !custom.is_empty()
    {
        return custom.to_string();
    }
    let Some(ts) = item.release_date else {
        return String::new();
    };
    let ts_i64 = i64::try_from(ts).unwrap_or(0);
    let now_i64 = i64::try_from(now_ts).unwrap_or(0);
    match (
        chrono::DateTime::from_timestamp(ts_i64, 0),
        chrono::DateTime::from_timestamp(now_i64, 0),
    ) {
        (Some(dt), Some(now_dt)) => {
            use chrono::Datelike as _;
            let date = dt.date_naive();
            calendar_date(date, date.year() != now_dt.date_naive().year())
        }
        _ => String::new(),
    }
}

fn top_genres(snap: &ReleasesSnapshot) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in snap.new_this_week.iter().chain(snap.coming_soon.iter()) {
        for g in &item.genres {
            *counts.entry(g.as_str()).or_default() += 1;
        }
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked
        .into_iter()
        .take(7)
        .map(|(g, _)| g.to_string())
        .collect()
}

fn owned_steam_map(cx: &gpui_kit::App) -> HashMap<u32, GameId> {
    let lib = models(cx).library.read(cx);
    let b = backend(cx);
    let mut map = HashMap::new();
    for game in &lib.snapshot.games {
        if let Some((app_id, _)) = b.resolved_steam_app_id(game) {
            map.entry(app_id).or_insert_with(|| game.id());
        } else if game.runner() == Runner::Steam
            && let Ok(app_id) = game.app_name.parse::<u32>()
        {
            map.entry(app_id).or_insert_with(|| game.id());
        }
    }
    map
}

impl NewsSection {
    /// Load cached weekly releases off-thread first, then refresh from Steam.
    pub(crate) fn load_releases(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.releases.load_gen = self.releases.load_gen.wrapping_add(1);
        let generation = self.releases.load_gen;
        self.releases.refreshing = true;
        self.releases.error = None;
        if !self.releases.loaded {
            self.releases.initial_loading = true;
        }
        cx.notify();

        let b_cache = backend(cx);
        let cache_task = irori_ui::run(cx, async move {
            let worker = b_cache.clone();
            b_cache
                .spawn_blocking(move || worker.cached_store_releases())
                .await
                .map_err(anyhow::Error::from)
        });

        let b_refresh = backend(cx);
        let refresh_task =
            irori_ui::run(
                cx,
                async move { b_refresh.refresh_store_releases(force).await },
            );

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(cached) = cache_task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.releases.load_gen != generation {
                        return;
                    }
                    if !cached.new_this_week.is_empty() || !cached.coming_soon.is_empty() {
                        this.releases.snapshot = cached;
                        this.releases.loaded = true;
                    }
                    cx.notify();
                });
            }

            let refreshed = refresh_task.await;
            let _ = this.update(cx, |this, cx| {
                if this.releases.load_gen != generation {
                    return;
                }
                this.releases.refreshing = false;
                this.releases.initial_loading = false;
                this.releases.loaded = true;
                match refreshed {
                    Ok(snap) => {
                        this.releases.error = None;
                        this.releases.snapshot = snap;
                    }
                    Err(e) => {
                        this.releases.error = Some(format!("{e:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_releases(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now_ts = now_unix();
        let owned = owned_steam_map(cx);
        let toolbar = self.render_releases_toolbar(cx);
        let notice = self.render_releases_notice(cx);

        let new_indices = filter_releases(
            &self.releases.snapshot.new_this_week,
            self.releases.platform,
            self.releases.genre.as_deref(),
            &self.releases.query,
        );
        let soon_indices = filter_releases(
            &self.releases.snapshot.coming_soon,
            self.releases.platform,
            self.releases.genre.as_deref(),
            &self.releases.query,
        );

        let has_any = !self.releases.snapshot.new_this_week.is_empty()
            || !self.releases.snapshot.coming_soon.is_empty();

        let body: AnyElement = if self.releases.initial_loading && !has_any {
            let skeletons: Vec<AnyElement> = (0..6)
                .map(|_| Skeleton::new().h(rems(17.)).w_full().into_any_element())
                .collect();
            card_grid_capped(
                &self.releases_new_width,
                MIN_RELEASE_CARD_REM,
                MAX_RELEASE_COLUMNS,
                skeletons,
                window,
            )
            .into_any_element()
        } else if !has_any {
            panel(cx)
                .w_full()
                .p_6()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(t("manage.news.releases.empty")),
                )
                .child(
                    Button::new("releases-empty-refresh")
                        .small()
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label(t("manage.news.refresh"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.load_releases(true, window, cx);
                        })),
                )
                .into_any_element()
        } else if new_indices.is_empty() && soon_indices.is_empty() {
            panel(cx)
                .w_full()
                .p_6()
                .gap_3()
                .items_start()
                .child(div().text_sm().child(t("manage.news.releases.no_match")))
                .child(
                    Button::new("releases-clear-filters")
                        .small()
                        .outline()
                        .label(t("manage.news.clear_filters"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.releases.platform = PlatformFilter::All;
                            this.releases.genre = None;
                            this.releases.query.clear();
                            this.releases_search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            let mut sections = v_flex().w_full().gap_6();
            if !new_indices.is_empty() {
                let cards: Vec<AnyElement> = new_indices
                    .into_iter()
                    .filter_map(|i| self.releases.snapshot.new_this_week.get(i))
                    .map(|item| self.render_release_card(item, "new", &owned, now_ts, cx))
                    .collect();
                sections = sections.child(
                    v_flex()
                        .w_full()
                        .gap_3()
                        .child(
                            div()
                                .text_base()
                                .font_semibold()
                                .child(t("manage.news.releases.new_this_week")),
                        )
                        .child(card_grid_capped(
                            &self.releases_new_width,
                            MIN_RELEASE_CARD_REM,
                            MAX_RELEASE_COLUMNS,
                            cards,
                            window,
                        )),
                );
            }
            if !soon_indices.is_empty() {
                let cards: Vec<AnyElement> = soon_indices
                    .into_iter()
                    .filter_map(|i| self.releases.snapshot.coming_soon.get(i))
                    .map(|item| self.render_release_card(item, "soon", &owned, now_ts, cx))
                    .collect();
                sections = sections.child(
                    v_flex()
                        .w_full()
                        .gap_3()
                        .child(
                            div()
                                .text_base()
                                .font_semibold()
                                .child(t("manage.news.releases.coming_soon")),
                        )
                        .child(card_grid_capped(
                            &self.releases_soon_width,
                            MIN_RELEASE_CARD_REM,
                            MAX_RELEASE_COLUMNS,
                            cards,
                            window,
                        )),
                );
            }
            sections.into_any_element()
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(toolbar)
            .children(notice)
            .child(body)
            .into_any_element()
    }

    fn render_releases_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let msg = if let Some(err) = &self.releases.error {
            format!("{} {}", t("manage.news.releases.load_failed"), err)
        } else if self.releases.snapshot.rate_limited {
            t("manage.news.rate_limited").to_string()
        } else if self.releases.snapshot.offline {
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

    fn render_releases_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let platform = self.releases.platform;
        let plat_btn = |id: &'static str, label: SharedString, target: PlatformFilter| {
            let btn = Button::new(id).small().label(label);
            let btn = if platform == target {
                btn.primary()
            } else {
                btn.outline()
            };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                if this.releases.platform != target {
                    this.releases.platform = target;
                    cx.notify();
                }
            }))
        };

        let genres = top_genres(&self.releases.snapshot);
        let selected_genre = self.releases.genre.clone();

        v_flex()
            .w_full()
            .gap_2()
            .child(
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
                            .gap_1p5()
                            .child(plat_btn(
                                "rel-plat-all",
                                t("manage.news.releases.all_platforms"),
                                PlatformFilter::All,
                            ))
                            .child(plat_btn(
                                "rel-plat-win",
                                t("manage.news.releases.windows"),
                                PlatformFilter::Windows,
                            ))
                            .child(plat_btn(
                                "rel-plat-linux",
                                t("manage.news.releases.linux"),
                                PlatformFilter::Linux,
                            ))
                            .child(plat_btn(
                                "rel-plat-mac",
                                t("manage.news.releases.macos"),
                                PlatformFilter::MacOs,
                            )),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .w(rems(13.))
                                    .child(Input::new(&self.releases_search).small()),
                            )
                            .child(
                                Button::new("releases-refresh")
                                    .small()
                                    .outline()
                                    .icon(IconName::RefreshCw)
                                    .loading(self.releases.refreshing)
                                    .tooltip(t("manage.news.refresh"))
                                    .accessibility_label(t("manage.news.refresh"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.load_releases(true, window, cx);
                                    })),
                            ),
                    ),
            )
            .when(!genres.is_empty(), |v| {
                let all_genre_btn = {
                    let btn = Button::new("rel-genre-all")
                        .xsmall()
                        .label(t("manage.news.releases.all_genres"));
                    let btn = if selected_genre.is_none() {
                        btn.primary()
                    } else {
                        btn.ghost()
                    };
                    btn.on_click(cx.listener(|this, _, _, cx| {
                        if this.releases.genre.is_some() {
                            this.releases.genre = None;
                            cx.notify();
                        }
                    }))
                };
                v.child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(all_genre_btn)
                        .children(genres.into_iter().map(|g| {
                            let active = selected_genre.as_deref() == Some(g.as_str());
                            let btn = Button::new(ElementId::Name(format!("rel-genre-{g}").into()))
                                .xsmall()
                                .label(g.clone());
                            let btn = if active { btn.primary() } else { btn.ghost() };
                            btn.on_click(cx.listener(move |this, _, _, cx| {
                                if this.releases.genre.as_deref() != Some(g.as_str()) {
                                    this.releases.genre = Some(g.clone());
                                    cx.notify();
                                }
                            }))
                        })),
                )
            })
            .into_any_element()
    }

    fn render_release_card(
        &self,
        item: &StoreReleaseItem,
        section: &str,
        owned: &HashMap<u32, GameId>,
        now_ts: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let placeholder_bg = theme.muted.opacity(0.35);
        let cover: AnyElement = item
            .header_image
            .as_deref()
            .and_then(|url| art_img(url, 600, false, cx))
            .map(|img| {
                img.w_full()
                    .h(rems(9.5))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .w_full()
                    .h(rems(5.))
                    .bg(placeholder_bg)
                    .into_any_element()
            });

        let date_label = format_release_date(item, now_ts);
        let price_label: Option<SharedString> = if item.is_free {
            Some(t("manage.news.releases.free"))
        } else {
            item.price_formatted
                .as_ref()
                .map(|p| SharedString::from(p.clone()))
        };
        let owned_id = owned.get(&item.appid).cloned();
        let open_url = irori_ui::web_url(&item.store_url);
        let click_owned = owned_id.clone();
        let click_url = open_url.clone();

        let mut platforms = Vec::new();
        if item.windows {
            platforms.push("Win");
        }
        if item.linux {
            platforms.push("Linux");
        }
        if item.macos {
            platforms.push("macOS");
        }
        let plat_line = platforms.join(" · ");

        card(cx)
            .id(ElementId::Name(
                format!("release-{section}-{}", item.appid).into(),
            ))
            .cursor_pointer()
            .p_0()
            .gap_0()
            .overflow_hidden()
            .on_click(move |_, _, cx| {
                if let Some(id) = &click_owned {
                    navigate(Route::Game(id.clone()), cx);
                } else if let Some(url) = &click_url {
                    cx.open_url(url);
                }
            })
            .child(cover)
            .child(
                v_flex()
                    .flex_1()
                    .p_3p5()
                    .gap_2()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_semibold()
                                    .truncate()
                                    .child(item.name.clone()),
                            )
                            .when_some(owned_id, |r, _| {
                                r.child(
                                    Tag::success()
                                        .xsmall()
                                        .child(t("manage.news.releases.in_library")),
                                )
                            }),
                    )
                    .when(!item.short_description.is_empty(), |v| {
                        v.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(item.short_description.clone()),
                        )
                    })
                    .when(!item.genres.is_empty(), |v| {
                        v.child(
                            h_flex().flex_wrap().gap_1().children(
                                item.genres
                                    .iter()
                                    .map(|g| Tag::secondary().xsmall().child(g.clone())),
                            ),
                        )
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .pt_1()
                            .child(div().text_xs().text_color(muted).truncate().child(
                                if !date_label.is_empty() && !plat_line.is_empty() {
                                    format!("{date_label} · {plat_line}")
                                } else if !date_label.is_empty() {
                                    date_label
                                } else {
                                    plat_line
                                },
                            ))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .when_some(item.discount_pct, |r, pct| {
                                        r.child(Tag::success().xsmall().child(t_args(
                                            "manage.deals.discount",
                                            &[("percent", &pct.to_string())],
                                        )))
                                    })
                                    .when_some(price_label, |r, price| {
                                        r.child(div().text_xs().font_semibold().child(price))
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}
