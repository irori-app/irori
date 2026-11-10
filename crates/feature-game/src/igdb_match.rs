//! "Change match" for IGDB: search IGDB by name and pick the game whose
//! details fill the page, say it isn't on IGDB, or go back to finding one
//! by title. The choice is the per-game setting `iroriIgdbId`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_extras::igdb::{IGDB_NONE, IgdbGame};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, run};

/// Called once the user chose, so the page reads the game's details again.
pub type OnChosen = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Open "Change match" for `id`, its search filled with `title`.
pub fn open_igdb_match_dialog(
    id: GameId,
    title: String,
    on_chosen: OnChosen,
    window: &mut Window,
    cx: &mut App,
) {
    crate::i18n::register();
    let dialog = cx.new(|cx| IgdbMatch::new(id, title.clone(), on_chosen, window, cx));
    let width = window.rem_size() * 32.;
    let heading = t_args("game.igdb.match_title", &[("title", &title)]);
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

/// How a search result reads: its name, with the release year when known.
pub(crate) fn result_label(game: &IgdbGame) -> SharedString {
    match game.release_year() {
        Some(year) => t_args(
            "game.igdb.result_year",
            &[("title", &game.name), ("year", &year.to_string())],
        ),
        None => game.name.clone().into(),
    }
}

struct IgdbMatch {
    id: GameId,
    search: Entity<InputState>,
    searching: bool,
    results: Option<Vec<IgdbGame>>,
    chosen: bool,
    on_chosen: OnChosen,
    _subscriptions: Vec<Subscription>,
}

impl IgdbMatch {
    fn new(
        id: GameId,
        title: String,
        on_chosen: OnChosen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.igdb.search_placeholder"))
                .default_value(title)
        });
        let subscriptions =
            vec![
                cx.subscribe_in(&search, window, |this, _, e: &InputEvent, window, cx| {
                    if matches!(e, InputEvent::PressEnter { .. }) {
                        this.run_search(window, cx);
                    }
                }),
            ];
        let chosen = !backend(cx).settings.game(&id).igdb_id.trim().is_empty();
        let mut this = Self {
            id,
            search,
            searching: false,
            results: None,
            chosen,
            on_chosen,
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
        let task = run(cx, async move { b.search_igdb(&text).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.searching = false;
                match result {
                    Ok(found) => this.results = Some(found),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("game.igdb.search_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the choice (off the UI thread), close and reload the page.
    fn choose(&mut self, igdb_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (b, id) = (backend(cx), self.id.clone());
        let task = run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || saver.set_igdb_match(&id, igdb_id))
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

    fn render_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let results = self.results.clone().unwrap_or_default();
        if self.searching && results.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("game.igdb.searching"))
                .into_any_element();
        }
        if self.results.is_some() && results.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("game.igdb.no_results"))
                .into_any_element();
        }
        v_flex()
            .gap_0p5()
            .children(results.into_iter().enumerate().map(|(i, found)| {
                let igdb_id = found.id.to_string();
                Button::new(("igdb-result", i))
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .label(result_label(&found))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(Some(igdb_id.clone()), window, cx)
                    }))
            }))
            .into_any_element()
    }
}

impl Render for IgdbMatch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        Button::new("igdb-search")
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
            .child(self.render_results(cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("igdb-none")
                            .ghost()
                            .small()
                            .icon(IconName::EyeOff)
                            .label(t("game.igdb.no_match"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose(Some(IGDB_NONE.to_owned()), window, cx)
                            })),
                    )
                    .when(self.chosen, |row| {
                        row.child(
                            Button::new("igdb-automatic")
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
