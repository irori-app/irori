//! Game page News tab: developer patch notes and official Steam announcements
//! for any Steam game or auto-linked non-Steam game. Loads lazily when opened
//! and renders BBCode/HTML natively through `irori_ui::rich_text`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, IntoElement, ObjectFit, ParentElement as _, SharedString,
    Styled as _, StyledImage as _, Window, div, rems,
};
use irori_core::GameInfo;
use irori_services::{GameNewsResult, NewsKind, SteamNewsItem};
use irori_ui::components::art_img;
use irori_ui::dates::calendar_date;
use irori_ui::i18n::{t, t_args};
use irori_ui::rich_text::{prepare_steam_news, rich_text_keyed};
use irori_ui::style::panel;
use irori_ui::{backend, models};

use super::GamePage;
use crate::load::Load;

fn now_unix_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn format_news_date(ts: u64) -> String {
    let ts_i64 = i64::try_from(ts).unwrap_or(0);
    let now = now_unix_i64();
    let days = (now - ts_i64).div_euclid(86_400);
    match days {
        0 => t("game.date.today").to_string(),
        1 => t("game.date.yesterday").to_string(),
        2..=6 => t_args("game.date.days_ago", &[("count", &days.to_string())]).to_string(),
        _ => match (
            chrono::DateTime::from_timestamp(ts_i64, 0),
            chrono::DateTime::from_timestamp(now, 0),
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

impl GamePage {
    /// Whether this game should show the `News` tab (setting on and a Steam app
    /// is resolved on this game or a sibling copy).
    pub(super) fn has_news_tab(&self, cx: &gpui_kit::App) -> bool {
        if !models(cx).settings.read(cx).app.news {
            return false;
        }
        self.game
            .as_ref()
            .is_some_and(|g| backend(cx).resolved_steam_app_id(g).is_some())
    }

    /// Fetch official Steam announcements lazily when the News tab opens, or
    /// force a refresh when requested by the user.
    pub(super) fn load_news(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !force && !matches!(self.news, Load::Idle | Load::Failed(_)) {
            return;
        }
        let Some(game) = self.game.clone() else {
            return;
        };
        self.news = Load::Loading;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.news_for_game(&game, force).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.news = Load::from_result(result);
                if let Load::Ready(Some(res)) = &this.news
                    && this.news_expanded.is_empty()
                    && let Some(first) = res.items.first()
                {
                    this.news_expanded.insert(first.gid.clone());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_news(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let followed = backend(cx).is_news_followed(game);
        let game_id = game.id();
        let filter = self.news_filter;

        let filter_btn = |id: &'static str, label: SharedString, target: Option<NewsKind>| {
            let active = filter == target;
            let btn = Button::new(id).small().label(label);
            let btn = if active { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                if this.news_filter != target {
                    this.news_filter = target;
                    cx.notify();
                }
            }))
        };

        let toolbar = h_flex()
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
                    .child(filter_btn("game-news-all", t("game.news.filter_all"), None))
                    .child(filter_btn(
                        "game-news-patches",
                        t("game.news.filter_patches"),
                        Some(NewsKind::PatchNotes),
                    ))
                    .child(filter_btn(
                        "game-news-general",
                        t("game.news.filter_news"),
                        Some(NewsKind::News),
                    )),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child({
                        let label = if followed {
                            t("game.news.following")
                        } else {
                            t("game.news.follow")
                        };
                        let btn = Button::new("game-news-follow").small().label(label);
                        let btn = if followed { btn.outline() } else { btn.ghost() };
                        btn.on_click(cx.listener(move |_, _, _, cx| {
                            let b = backend(cx);
                            if followed {
                                b.mute_news(&game_id, true);
                            } else {
                                b.follow_news(&game_id, true);
                            }
                            cx.notify();
                        }))
                    })
                    .child(
                        Button::new("game-news-refresh")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .tooltip(t("game.news.refresh"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_news(true, window, cx);
                            })),
                    ),
            );

        let body: AnyElement = match &self.news {
            Load::Idle | Load::Loading => v_flex()
                .gap_3()
                .child(Skeleton::new().h(rems(7.)).w_full())
                .child(Skeleton::new().h(rems(5.)).w_full())
                .into_any_element(),
            Load::Failed(err) => v_flex()
                .gap_2()
                .items_start()
                .child(div().text_sm().child(t("game.news.load_failed")))
                .child(div().text_xs().text_color(muted).child(err.clone()))
                .child(
                    Button::new("game-news-retry")
                        .small()
                        .outline()
                        .label(t("game.news.retry"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.load_news(true, window, cx);
                        })),
                )
                .into_any_element(),
            Load::Ready(None) => div()
                .text_sm()
                .text_color(muted)
                .child(t("game.news.empty"))
                .into_any_element(),
            Load::Ready(Some(res)) => self.render_news_items(res, window, cx),
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    fn render_news_items(
        &self,
        res: &GameNewsResult,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let filtered: Vec<&SteamNewsItem> = res
            .items
            .iter()
            .filter(|item| self.news_filter.is_none_or(|wanted| wanted == item.kind))
            .collect();

        let notice = if res.rate_limited {
            Some(t("game.news.rate_limited"))
        } else if res.offline {
            Some(t("game.news.offline_cached"))
        } else {
            None
        };

        if filtered.is_empty() {
            return v_flex()
                .gap_2()
                .when_some(notice, |v, msg| {
                    v.child(div().text_xs().text_color(muted).child(msg))
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("game.news.empty")),
                )
                .into_any_element();
        }

        let source_label = if res.via_steam {
            t("game.news.via_steam")
        } else {
            t("game.news.steam")
        };

        v_flex()
            .w_full()
            .gap_3()
            .when_some(notice, |v, msg| {
                v.child(div().text_xs().text_color(muted).child(msg))
            })
            .children(
                filtered
                    .into_iter()
                    .map(|item| self.render_news_card(item, source_label.clone(), window, cx)),
            )
            .into_any_element()
    }

    fn render_news_card(
        &self,
        item: &SteamNewsItem,
        source_label: SharedString,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let expanded = self.news_expanded.contains(&item.gid);
        let gid = item.gid.clone();
        let prep = prepare_steam_news(&item.contents);
        let date = format_news_date(item.date);
        let meta = if !item.author.trim().is_empty() && !date.is_empty() {
            format!("{date} · {}", item.author.trim())
        } else if !date.is_empty() {
            date
        } else {
            item.author.trim().to_string()
        };
        let open_url = irori_ui::web_url(&item.url);

        let badge = match item.kind {
            NewsKind::PatchNotes => Tag::success().xsmall().child(t("game.news.kind_patch")),
            NewsKind::News => Tag::info().xsmall().child(t("game.news.kind_news")),
        };

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(badge)
                    .when(!meta.is_empty(), |r| {
                        r.child(div().text_xs().text_color(muted).truncate().child(meta))
                    }),
            )
            .when_some(open_url, |r, url| {
                r.child(
                    Button::new(ElementId::Name(format!("news-open-{}", item.gid).into()))
                        .xsmall()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label(t("game.news.open_original"))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            });

        let banner = expanded
            .then_some(prep.banner_url.as_deref())
            .flatten()
            .and_then(|url| art_img(url, 800, false, cx))
            .map(|img| {
                img.w_full()
                    .max_h(rems(13.))
                    .rounded_md()
                    .object_fit(ObjectFit::Cover)
            });

        let content: AnyElement = if expanded {
            v_flex()
                .w_full()
                .gap_3()
                .children(banner)
                .child(rich_text_keyed(
                    format!("game-news:{}:{}", self.id.key(), item.gid),
                    &prep.markdown,
                ))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(muted)
                .child(prep.excerpt)
                .into_any_element()
        };

        let toggle_label = if expanded {
            t("game.news.collapse")
        } else {
            t("game.news.read_more")
        };

        panel(cx)
            .w_full()
            .p_4()
            .gap_2p5()
            .child(header)
            .child(div().text_base().font_semibold().child(item.title.clone()))
            .child(content)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .pt_1()
                    .child(
                        Button::new(ElementId::Name(format!("news-toggle-{}", item.gid).into()))
                            .xsmall()
                            .ghost()
                            .label(toggle_label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.news_expanded.remove(&gid) {
                                    this.news_expanded.insert(gid.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().text_color(muted).child(source_label)),
            )
            .into_any_element()
    }
}
