//! "Link to Steam": search Steam's store by name (or paste an app id or
//! store address) and pick the app whose tags, store details and ProtonDB
//! rating the game takes, say Steam doesn't sell it, or go back to finding
//! it on its own. The choice is the per-game setting `iroriSteamAppId`; the
//! game itself still starts as before.

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
use irori_services::{STEAM_NONE, SteamHit, SteamLink};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models, run};

/// What the game's Steam link is, in a sentence.
pub(crate) fn describe(link: SteamLink) -> SharedString {
    let app = |id: u32| id.to_string();
    match link {
        SteamLink::Own(id) | SteamLink::Chosen(id) => {
            t_args("game.steam_link.chosen", &[("id", &app(id))])
        }
        SteamLink::Wiki(id) => t_args("game.steam_link.from_wiki", &[("id", &app(id))]),
        SteamLink::Title(id) => t_args("game.steam_link.from_title", &[("id", &app(id))]),
        SteamLink::NoneChosen => t("game.steam_link.none_chosen"),
        SteamLink::Unknown => t("game.steam_link.unknown"),
    }
}

/// Open "Link to Steam" for `id`, its search filled with the game's title.
/// The game page follows the change on its own.
pub fn open_steam_link_dialog(id: GameId, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(game) = models(cx).library.read(cx).snapshot.get(&id).cloned() else {
        return;
    };
    let title = game.title.clone();
    let link = backend(cx).steam_link(&game);
    let dialog = cx.new(|cx| SteamMatch::new(id, title.clone(), link, window, cx));
    let width = window.rem_size() * 32.;
    let heading = t_args("game.steam_link.title", &[("title", &title)]);
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

struct SteamMatch {
    id: GameId,
    link: SteamLink,
    search: Entity<InputState>,
    searching: bool,
    results: Option<Vec<SteamHit>>,
    _subscriptions: Vec<Subscription>,
}

impl SteamMatch {
    fn new(
        id: GameId,
        title: String,
        link: SteamLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.steam_link.search_placeholder"))
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
        let mut this = Self {
            id,
            link,
            search,
            searching: false,
            results: None,
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
        let task = run(cx, async move { b.search_steam(&text).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.searching = false;
                match result {
                    Ok(found) => this.results = Some(found),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("game.steam_link.search_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the choice (off the UI thread) and close; the page reads the
    /// game's details again when its link changes.
    fn choose(&mut self, choice: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (b, id) = (backend(cx), self.id.clone());
        let task = run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || saver.set_steam_link(&id, choice))
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                window.close_dialog(cx);
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("game.steam_link.failed")),
                        cx,
                    );
                }
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
                .child(t("game.steam_link.searching"))
                .into_any_element();
        }
        if self.results.is_some() && results.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("game.steam_link.no_results"))
                .into_any_element();
        }
        let current = self.link.app_id();
        v_flex()
            .gap_0p5()
            .children(results.into_iter().take(12).enumerate().map(|(i, hit)| {
                let app = hit.id;
                Button::new(("steam-link-result", i))
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .when(current == Some(app), |b| b.icon(IconName::Check))
                    .label(format!("{} · {app}", hit.name))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(Some(app.to_string()), window, cx)
                    }))
            }))
            .into_any_element()
    }
}

impl Render for SteamMatch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .child(div().child(describe(self.link)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("game.steam_link.explain")),
                    ),
            )
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
                        Button::new("steam-link-search")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t("game.steam_link.search"))
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
                        Button::new("steam-link-none")
                            .ghost()
                            .small()
                            .icon(IconName::EyeOff)
                            .label(t("game.steam_link.none"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose(Some(STEAM_NONE.to_owned()), window, cx)
                            })),
                    )
                    .when(self.link.is_chosen(), |row| {
                        row.child(
                            Button::new("steam-link-automatic")
                                .ghost()
                                .small()
                                .icon(IconName::Undo2)
                                .label(t("game.steam_link.automatic"))
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
