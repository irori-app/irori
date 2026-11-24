//! "Change VNDB match": search VNDB by name and pick the visual novel whose
//! details fill the page (each result with its year, another of its names
//! and its cover when VNDB flags it safe), say it isn't on VNDB, or go
//! back to finding one by title. The choice is the per-game setting
//! `iroriVndbId`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Subscription, Window, div, rems,
};
use irori_core::GameId;
use irori_extras::vndb::{VNDB_NONE, VndbNovel};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, run};

pub use crate::igdb_match::OnChosen;

/// Covers shown at once: one per result.
const THUMBS: usize = 16;

/// Open "Change VNDB match" for `id`, its search filled with `title`.
pub fn open_vndb_match_dialog(
    id: GameId,
    title: String,
    on_chosen: OnChosen,
    window: &mut Window,
    cx: &mut App,
) {
    crate::i18n::register();
    let dialog = cx.new(|cx| VndbMatch::new(id, title.clone(), on_chosen, window, cx));
    let width = window.rem_size() * 34.;
    let heading = t_args("game.vndb.match_title", &[("title", &title)]);
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

/// How a search result reads: its title, with the release year when known.
fn result_label(novel: &VndbNovel) -> SharedString {
    match novel.release_year() {
        Some(year) => t_args(
            "game.igdb.result_year",
            &[("title", &novel.title), ("year", &year.to_string())],
        ),
        None => novel.title.clone().into(),
    }
}

struct VndbMatch {
    id: GameId,
    search: Entity<InputState>,
    searching: bool,
    results: Option<Vec<VndbNovel>>,
    chosen: bool,
    on_chosen: OnChosen,
    image_cache: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl VndbMatch {
    fn new(
        id: GameId,
        title: String,
        on_chosen: OnChosen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.vndb.search_placeholder"))
                .default_value(title)
        });
        let art = art_store(cx);
        let subscriptions = vec![
            cx.observe(&art, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.run_search(window, cx);
                }
            }),
        ];
        let chosen = !backend(cx).settings.game(&id).vndb_id.trim().is_empty();
        let mut this = Self {
            id,
            search,
            searching: false,
            results: None,
            chosen,
            on_chosen,
            image_cache: LruImageCache::new(THUMBS, cx),
            _subscriptions: subscriptions,
        };
        this.run_search(window, cx);
        this
    }

    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.search.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.searching = true;
        cx.notify();
        let b = backend(cx);
        let task = run(cx, async move { b.search_vndb(&text).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.searching = false;
                match result {
                    Ok(found) => this.results = Some(found),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("game.vndb.search_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the choice (off the UI thread), close and reload the page.
    fn choose(&mut self, vndb_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (b, id) = (backend(cx), self.id.clone());
        let task = run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || saver.set_vndb_match(&id, vndb_id))
                .await
                .map_err(anyhow::Error::from)
        });
        let on_chosen = self.on_chosen.clone();
        cx.spawn_in(window, async move |_, cx| {
            let _ = task.await;
            let _ = cx.update(|window, cx| {
                window.close_dialog(cx);
                on_chosen(window, cx);
            });
        })
        .detach();
    }

    fn render_result(
        &self,
        ix: usize,
        novel: &VndbNovel,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = rems(2.5);
        let thumb = novel
            .cover
            .as_ref()
            .and_then(|c| art_img(&c.thumbnail, physical_width(width, window), false, cx));
        let theme = cx.theme();
        let (radius, muted_bg, muted) = (theme.radius, theme.muted, theme.muted_foreground);
        let frame = div()
            .flex_shrink_0()
            .w(width)
            .h(rems(3.5))
            .overflow_hidden()
            .rounded(radius)
            .bg(muted_bg)
            .map(|el| match thumb {
                Some(img) => el.child(img.size_full().object_fit(ObjectFit::Cover)),
                None => el.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .child(Icon::new(IconName::Image).small()),
                ),
            });
        let other = novel.other_titles.first().cloned();
        let vndb_id = novel.id.clone();
        Button::new(("vndb-result", ix))
            .ghost()
            .small()
            .w_full()
            .h_auto()
            .p_1()
            .justify_start()
            .accessibility_label(result_label(novel))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(frame)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .child(result_label(novel)),
                            )
                            .children(other.map(|name| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(name)
                            })),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose(Some(vndb_id.clone()), window, cx)
            }))
            .into_any_element()
    }

    fn render_results(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let results = self.results.clone().unwrap_or_default();
        if self.searching && results.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("game.vndb.searching"))
                .into_any_element();
        }
        if self.results.is_some() && results.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("game.vndb.no_results"))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = results
            .iter()
            .enumerate()
            .map(|(ix, novel)| self.render_result(ix, novel, window, cx))
            .collect();
        div()
            .image_cache(self.image_cache.clone())
            .id("vndb-results")
            .max_h(rems(24.))
            .overflow_y_scroll()
            .child(v_flex().gap_0p5().children(rows))
            .into_any_element()
    }
}

impl Render for VndbMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).small()),
                    )
                    .child(
                        Button::new("vndb-search")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t("game.igdb.search"))
                            .loading(self.searching)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run_search(window, cx)),
                            ),
                    ),
            )
            .child(self.render_results(window, cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("vndb-none")
                            .ghost()
                            .small()
                            .icon(IconName::EyeOff)
                            .label(t("game.vndb.no_match"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose(Some(VNDB_NONE.to_owned()), window, cx)
                            })),
                    )
                    .when(self.chosen, |row| {
                        row.child(
                            Button::new("vndb-automatic")
                                .ghost()
                                .small()
                                .icon(IconName::Undo2)
                                .label(t("game.igdb.automatic"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.choose(None, window, cx)
                                    }),
                                ),
                        )
                    }),
            )
    }
}
