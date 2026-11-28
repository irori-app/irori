//! `Gaming news` sub-view inside `Stores -> News`: visual RSS/Atom card grid,
//! source filters, in-app native article reader with on-demand Reader Mode, and
//! a source management dialog for built-in presets plus custom RSS/Atom URLs.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window,
    div, rems,
};
use irori_services::{ArticleContent, FeedArticle, NewsFeedsConfig, PRESET_FEEDS};
use irori_ui::backend;
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::rich_text::rich_text_keyed;
use irori_ui::style::panel;

use super::my_games::format_date;
use super::{NewsSection, now_unix};
use crate::common::{card, card_grid_capped};

/// Minimum card width (in rem) for the Gaming news grid.
const MIN_ARTICLE_CARD_REM: f32 = 18.5;
/// Maximum columns in the Gaming news grid on wide screens.
const MAX_ARTICLE_COLUMNS: u16 = 4;

/// UI state for the `Gaming news` sub-view.
#[derive(Default)]
pub(crate) struct GamingNewsState {
    pub config: NewsFeedsConfig,
    pub articles: Vec<FeedArticle>,
    pub failed_sources: Vec<String>,
    pub selected_source: Option<String>,
    pub query: String,
    pub selected_article: Option<FeedArticle>,
    pub reader_content: Option<ArticleContent>,
    pub reader_loading: bool,
    pub loaded: bool,
    pub initial_loading: bool,
    pub refreshing: bool,
    load_gen: u64,
    reader_gen: u64,
}

/// Filter articles by selected source and case-insensitive title/summary search.
pub(crate) fn filter_articles(
    articles: &[FeedArticle],
    source_id: Option<&str>,
    query: &str,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    articles
        .iter()
        .enumerate()
        .filter(|(_, a)| source_id.is_none_or(|wanted| a.source_id == wanted))
        .filter(|(_, a)| {
            q.is_empty()
                || a.title.to_lowercase().contains(&q)
                || a.summary.to_lowercase().contains(&q)
                || a.source_name.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

impl NewsSection {
    /// Load cached gaming articles off-thread first, then refresh enabled RSS feeds.
    pub(crate) fn load_gaming_news(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.gaming.load_gen = self.gaming.load_gen.wrapping_add(1);
        let generation = self.gaming.load_gen;
        self.gaming.refreshing = true;
        if !self.gaming.loaded {
            self.gaming.initial_loading = true;
        }
        cx.notify();

        let b_cfg = backend(cx);
        let cfg_task = irori_ui::run(cx, async move {
            let worker = b_cfg.clone();
            let cfg = b_cfg
                .spawn_blocking(move || worker.news_feeds_config())
                .await
                .map_err(anyhow::Error::from)?;
            let cached = b_cfg.cached_gaming_news().await;
            Ok((cfg, cached))
        });

        let b_refresh = backend(cx);
        let refresh_task =
            irori_ui::run(
                cx,
                async move { Ok(b_refresh.refresh_gaming_news(force).await) },
            );

        cx.spawn_in(window, async move |this, cx| {
            if let Ok((cfg, cached)) = cfg_task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.gaming.load_gen != generation {
                        return;
                    }
                    this.gaming.config = cfg;
                    if !cached.is_empty() {
                        this.gaming.articles = cached;
                        this.gaming.loaded = true;
                    }
                    cx.notify();
                });
            }

            if let Ok(result) = refresh_task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.gaming.load_gen != generation {
                        return;
                    }
                    this.gaming.refreshing = false;
                    this.gaming.initial_loading = false;
                    this.gaming.loaded = true;
                    this.gaming.articles = result.articles;
                    this.gaming.failed_sources = result.failed_sources;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn open_gaming_article(
        &mut self,
        article: FeedArticle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.gaming.reader_gen = self.gaming.reader_gen.wrapping_add(1);
        let generation = self.gaming.reader_gen;
        self.gaming.selected_article = Some(article.clone());
        self.gaming.reader_content = None;
        self.gaming.reader_loading = true;
        cx.notify();

        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.read_gaming_article(&article).await) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(content) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.gaming.reader_gen != generation {
                        return;
                    }
                    this.gaming.reader_loading = false;
                    this.gaming.reader_content = Some(content);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn render_gaming_news(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(article) = self.gaming.selected_article.clone() {
            return self.render_gaming_reader(&article, cx);
        }

        let now_ts = now_unix();
        let filtered = filter_articles(
            &self.gaming.articles,
            self.gaming.selected_source.as_deref(),
            &self.gaming.query,
        );
        let toolbar = self.render_gaming_toolbar(cx);
        let partial_notice = (!self.gaming.failed_sources.is_empty()).then(|| {
            let joined = self.gaming.failed_sources.join(", ");
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t_args(
                    "manage.news.gaming.partial_failed",
                    &[("sources", joined.as_str())],
                ))
                .into_any_element()
        });

        let body: AnyElement = if self.gaming.initial_loading && self.gaming.articles.is_empty() {
            let skeletons: Vec<AnyElement> = (0..6)
                .map(|_| Skeleton::new().h(rems(17.)).w_full().into_any_element())
                .collect();
            card_grid_capped(
                &self.grid_width,
                MIN_ARTICLE_CARD_REM,
                MAX_ARTICLE_COLUMNS,
                skeletons,
                window,
            )
            .into_any_element()
        } else if self.gaming.articles.is_empty() {
            let muted = cx.theme().muted_foreground;
            panel(cx)
                .w_full()
                .p_6()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(t("manage.news.gaming.empty")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("manage.news.gaming.empty_hint")),
                )
                .child(
                    Button::new("gaming-empty-sources")
                        .small()
                        .outline()
                        .label(t("manage.news.gaming.sources"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_sources_dialog(window, cx);
                        })),
                )
                .into_any_element()
        } else if filtered.is_empty() {
            panel(cx)
                .w_full()
                .p_6()
                .gap_3()
                .items_start()
                .child(div().text_sm().child(t("manage.news.gaming.no_match")))
                .child(
                    Button::new("gaming-clear-filters")
                        .small()
                        .outline()
                        .label(t("manage.news.clear_filters"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.gaming.selected_source = None;
                            this.gaming.query.clear();
                            this.gaming_search.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            let cards: Vec<AnyElement> = filtered
                .into_iter()
                .filter_map(|idx| self.gaming.articles.get(idx))
                .map(|article| self.render_article_card(article, now_ts, cx))
                .collect();
            card_grid_capped(
                &self.grid_width,
                MIN_ARTICLE_CARD_REM,
                MAX_ARTICLE_COLUMNS,
                cards,
                window,
            )
            .into_any_element()
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(toolbar)
            .children(partial_notice)
            .child(body)
            .into_any_element()
    }

    fn render_gaming_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut sources: Vec<(String, String)> = Vec::new();
        for preset in PRESET_FEEDS {
            if !self
                .gaming
                .config
                .disabled_presets
                .iter()
                .any(|d| d == preset.id)
            {
                sources.push((preset.id.to_string(), preset.name.to_string()));
            }
        }
        for custom in &self.gaming.config.custom_feeds {
            sources.push((custom.id.clone(), custom.name.clone()));
        }

        let selected = self.gaming.selected_source.clone();
        let all_btn = {
            let btn = Button::new("gaming-src-all")
                .small()
                .label(t("manage.news.gaming.all_sources"));
            let btn = if selected.is_none() {
                btn.primary()
            } else {
                btn.outline()
            };
            btn.on_click(cx.listener(|this, _, _, cx| {
                if this.gaming.selected_source.is_some() {
                    this.gaming.selected_source = None;
                    cx.notify();
                }
            }))
        };

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
                    .child(all_btn)
                    .children(sources.into_iter().map(|(id, name)| {
                        let active = selected.as_deref() == Some(id.as_str());
                        let btn = Button::new(ElementId::Name(format!("gaming-src-{id}").into()))
                            .small()
                            .label(name);
                        let btn = if active { btn.primary() } else { btn.outline() };
                        btn.on_click(cx.listener(move |this, _, _, cx| {
                            if this.gaming.selected_source.as_deref() != Some(id.as_str()) {
                                this.gaming.selected_source = Some(id.clone());
                                cx.notify();
                            }
                        }))
                    })),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .w(rems(13.))
                            .child(Input::new(&self.gaming_search).small()),
                    )
                    .child(
                        Button::new("gaming-refresh")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .loading(self.gaming.refreshing)
                            .tooltip(t("manage.news.refresh"))
                            .accessibility_label(t("manage.news.refresh"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_gaming_news(true, window, cx);
                            })),
                    )
                    .child(
                        Button::new("gaming-sources-btn")
                            .small()
                            .outline()
                            .icon(IconName::ListFilter)
                            .label(t("manage.news.gaming.sources"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_sources_dialog(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_article_card(
        &self,
        article: &FeedArticle,
        now_ts: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let placeholder_bg = theme.muted.opacity(0.35);
        let date = if article.published_at > 0 {
            format_date(article.published_at, now_ts)
        } else {
            String::new()
        };
        let cover: AnyElement = article
            .image_url
            .as_deref()
            .and_then(|url| art_img(url, 600, false, cx))
            .map(|img| {
                img.w_full()
                    .h(rems(10.))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .w_full()
                    .h(rems(4.5))
                    .bg(placeholder_bg)
                    .into_any_element()
            });

        let clicked = article.clone();
        card(cx)
            .id(ElementId::Name(
                format!("gaming-card-{}", article.id).into(),
            ))
            .cursor_pointer()
            .p_0()
            .gap_0()
            .overflow_hidden()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_gaming_article(clicked.clone(), window, cx);
            }))
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
                            .child(Tag::info().xsmall().child(article.source_name.clone()))
                            .when(!date.is_empty(), |r| {
                                r.child(div().text_xs().text_color(muted).child(date))
                            }),
                    )
                    .child(div().text_sm().font_semibold().child(article.title.clone()))
                    .when(!article.summary.is_empty(), |v| {
                        v.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(article.summary.clone()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_gaming_reader(&self, article: &FeedArticle, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let now_ts = now_unix();
        let date = if article.published_at > 0 {
            format_date(article.published_at, now_ts)
        } else {
            String::new()
        };
        let meta = if !article.author.is_empty() && !date.is_empty() {
            format!("{date} · {}", article.author)
        } else if !date.is_empty() {
            date
        } else {
            article.author.clone()
        };

        let resolved_image = self
            .gaming
            .reader_content
            .as_ref()
            .and_then(|c| c.image_url.as_deref())
            .or(article.image_url.as_deref());
        let banner = resolved_image
            .and_then(|url| art_img(url, 800, false, cx))
            .map(|img| {
                img.w_full()
                    .max_h(rems(18.))
                    .rounded_md()
                    .object_fit(ObjectFit::Cover)
            });

        let open_url = irori_ui::web_url(&article.url);
        let body_html: &str = self
            .gaming
            .reader_content
            .as_ref()
            .map(|c| c.body_html.as_str())
            .unwrap_or(article.body_html.as_str());
        let show_summary_note = !self.gaming.reader_loading
            && self
                .gaming
                .reader_content
                .as_ref()
                .is_some_and(|c| !c.used_reader_mode && c.body_html.len() < 700);

        let reader_body: AnyElement = if self.gaming.reader_loading && body_html.trim().is_empty() {
            v_flex()
                .gap_2p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("manage.news.gaming.loading_article")),
                )
                .child(Skeleton::new().h(rems(8.)).w_full())
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap_3()
                .when(self.gaming.reader_loading, |v| {
                    v.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.news.gaming.loading_article")),
                    )
                })
                .child(rich_text_keyed(
                    format!(
                        "gaming-article:{}:{}",
                        article.id,
                        usize::from(self.gaming.reader_content.is_some())
                    ),
                    body_html,
                ))
                .when(show_summary_note, |v| {
                    v.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.news.gaming.reader_summary_note")),
                    )
                })
                .into_any_element()
        };

        panel(cx)
            .w_full()
            .p_5()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("gaming-reader-back")
                            .small()
                            .outline()
                            .icon(IconName::ArrowLeft)
                            .label(t("manage.news.gaming.back"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.gaming.selected_article = None;
                                this.gaming.reader_content = None;
                                this.gaming.reader_loading = false;
                                cx.notify();
                            })),
                    )
                    .when_some(open_url, |r, url| {
                        r.child(
                            Button::new("gaming-reader-open")
                                .small()
                                .primary()
                                .icon(IconName::ExternalLink)
                                .label(t("manage.news.gaming.open_browser"))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Tag::info().xsmall().child(article.source_name.clone()))
                            .when(!meta.is_empty(), |r| {
                                r.child(div().text_xs().text_color(muted).child(meta))
                            }),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .child(article.title.clone()),
                    ),
            )
            .children(banner)
            .child(reader_body)
            .into_any_element()
    }

    pub(super) fn save_config_and_reload(
        &mut self,
        cfg: NewsFeedsConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.gaming.config = cfg.clone();
        let b = backend(cx);
        let worker = b.clone();
        let task = irori_ui::run(cx, async move {
            b.spawn_blocking(move || worker.save_news_feeds_config(cfg))
                .await
                .map_err(anyhow::Error::from)?
        });
        cx.spawn_in(window, async move |this, cx| {
            if task.await.is_ok() {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.load_gaming_news(false, window, cx);
                });
            }
        })
        .detach();
        cx.notify();
    }
}
