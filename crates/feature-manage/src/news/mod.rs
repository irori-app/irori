//! Stores -> News tab: unified My games patch notes and announcements feed.
//! Loads lazily on first visit, reads disk cache off the UI thread first, and
//! fetches stale apps in paced batches without background polling.

mod my_games;
mod reader;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
    Styled as _, Subscription, Window, px,
};
use irori_services::{NewsFeedSnapshot, NewsScope};
use irori_ui::art::LruImageCache;
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use self::my_games::MyGamesState;

/// Image cache capacity for Steam CDN banners and covers inside News.
const NEWS_IMAGE_CACHE_CAPACITY: usize = 64;

/// Current Unix timestamp in seconds.
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Show or hide the Stores page's News tab (`iroriNews`).
pub fn set_news_shown(shown: bool, window: &mut Window, cx: &mut App) {
    crate::common::backend_op(cx, move |b| b.settings.update_app(|s| s.news = shown));
    let message = if shown {
        t("manage.news.shown_notice")
    } else {
        t("manage.news.hidden_notice")
    };
    window.push_notification(Notification::info(message), cx);
}

/// Unified News hub mounted inside `StoresView` when `iroriNews` is enabled.
pub struct NewsSection {
    pub(crate) my_games: MyGamesState,
    pub(crate) search: Entity<InputState>,
    pub(crate) body_width: Rc<Cell<Pixels>>,
    pub(crate) images: Entity<LruImageCache>,
    load_gen: u64,
    visit_marked: bool,
    _subscriptions: Vec<Subscription>,
}

impl NewsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.news.filter_games")));
        let m = models(cx).clone();
        let subscriptions = vec![
            cx.observe_in(&m.library, window, |this, _, window, cx| {
                this.reload_cached_my_games(window, cx);
            }),
            cx.observe_in(&m.news, window, |this, _, window, cx| {
                this.reload_cached_my_games(window, cx);
            }),
            cx.observe(&m.statuses, |_, _, cx| cx.notify()),
            cx.observe(&m.accounts, |_, _, cx| cx.notify()),
            cx.observe(&m.settings, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.my_games.query = input.read(cx).value().to_string();
                    this.my_games.sync_selection();
                    cx.notify();
                }
            }),
        ];

        let mut section = Self {
            my_games: MyGamesState::default(),
            search,
            body_width: Rc::new(Cell::new(px(0.))),
            images: LruImageCache::new(NEWS_IMAGE_CACHE_CAPACITY, cx),
            load_gen: 0,
            visit_marked: false,
            _subscriptions: subscriptions,
        };
        section.load_my_games(false, window, cx);
        section
    }

    /// Switch the library scope dropdown and reload cached + fresh posts.
    pub(crate) fn set_scope(
        &mut self,
        scope: NewsScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.my_games.scope == scope {
            return;
        }
        self.my_games.scope = scope;
        self.my_games.selected_gid = None;
        self.my_games.expanded_groups.clear();
        self.load_my_games(false, window, cx);
    }

    /// Reload only the disk-cached feed snapshot off the UI thread (no network).
    pub(crate) fn reload_cached_my_games(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self.my_games.scope.clone();
        let categories = Arc::clone(&models(cx).library.read(cx).categories);
        let b = backend(cx);
        let worker = b.clone();
        let task = irori_ui::run(cx, async move {
            b.spawn_blocking(move || worker.cached_news_feed(&scope, &categories))
                .await
                .map_err(anyhow::Error::from)
        });
        let expected_scope = self.my_games.scope.clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(snap) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.my_games.scope == expected_scope {
                        this.apply_my_games_snapshot(snap, false);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Load cached posts immediately off the UI thread, then refresh stale apps.
    pub(crate) fn load_my_games(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_gen = self.load_gen.wrapping_add(1);
        let generation = self.load_gen;
        let scope = self.my_games.scope.clone();
        let categories = Arc::clone(&models(cx).library.read(cx).categories);
        let should_mark_visit = !self.visit_marked;
        self.visit_marked = true;
        self.my_games.refreshing = true;
        self.my_games.error = None;
        if !self.my_games.loaded {
            self.my_games.initial_loading = true;
        }
        cx.notify();

        let b_cache = backend(cx);
        let cache_scope = scope.clone();
        let cache_cats = Arc::clone(&categories);
        let cache_task = irori_ui::run(cx, async move {
            let worker = b_cache.clone();
            b_cache
                .spawn_blocking(move || {
                    let prev_visit = if should_mark_visit {
                        Some(worker.mark_news_visited())
                    } else {
                        None
                    };
                    let snap = worker.cached_news_feed(&cache_scope, &cache_cats);
                    (snap, prev_visit)
                })
                .await
                .map_err(anyhow::Error::from)
        });

        let b_refresh = backend(cx);
        let refresh_task = irori_ui::run(cx, async move {
            b_refresh
                .refresh_news_feed(&scope, &categories, force)
                .await
        });

        cx.spawn_in(window, async move |this, cx| {
            if let Ok((cached, prev_visit)) = cache_task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.load_gen != generation {
                        return;
                    }
                    if let Some(prev) = prev_visit {
                        this.my_games.session_last_visited_at = prev;
                    }
                    this.apply_my_games_snapshot(cached, true);
                    cx.notify();
                });
            }

            let refreshed = refresh_task.await;
            let _ = this.update(cx, |this, cx| {
                if this.load_gen != generation {
                    return;
                }
                this.my_games.refreshing = false;
                this.my_games.initial_loading = false;
                match refreshed {
                    Ok(snap) => {
                        this.my_games.error = None;
                        this.apply_my_games_snapshot(snap, true);
                    }
                    Err(e) => {
                        this.my_games.error = Some(format!("{e:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_my_games_snapshot(&mut self, snap: NewsFeedSnapshot, keep_session_visit: bool) {
        if !keep_session_visit && !self.my_games.loaded {
            self.my_games.session_last_visited_at = snap.last_visited_at;
        }
        self.my_games.loaded = true;
        self.my_games.snapshot = snap;
        self.my_games.sync_selection();
    }

    /// Mark every currently visible post read by advancing the session watermark.
    pub(crate) fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.my_games.session_last_visited_at = now_unix();
        crate::common::backend_op(cx, |b| {
            let _ = b.mark_news_visited();
        });
        cx.notify();
    }
}

impl Render for NewsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_4()
            .image_cache(self.images.clone())
            .child(self.render_my_games(window, cx))
    }
}
