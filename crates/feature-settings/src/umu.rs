//! The umu panel of a game's Wine section: the umu database id the game
//! runs with when umu starts it, where that id came from, and a search of
//! the database to choose one (written as the game's `iroriUmuId`).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_runtime::umu::{UmuEntry, UmuMatch, UmuSource};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{muted, panel};

/// Suggestions shown at most.
const SUGGESTIONS: usize = 8;

pub(crate) struct UmuPanel {
    id: GameId,
    /// `None` while looked for; `Some(None)` when the game has no id.
    current: Option<Option<UmuMatch>>,
    query: Entity<InputState>,
    /// The last search's results; `None` before one or while it runs.
    found: Option<Vec<UmuEntry>>,
    searching: bool,
    _subscriptions: Vec<Subscription>,
}

impl UmuPanel {
    pub(crate) fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = backend(cx)
            .game(&id)
            .map(|g| g.title.clone())
            .unwrap_or_default();
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.umu.search_placeholder"))
                .default_value(title)
        });
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            // The id field above the panel changes the setting.
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(cx);
                }
            }),
        ];
        let mut this = Self {
            id,
            current: None,
            query,
            found: None,
            searching: false,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (b, id) = (backend(cx), self.id.clone());
        let task = irori_ui::state::run_infallible(cx, async move { b.umu_id(&id).await });
        cx.spawn(async move |this, cx| {
            let current = task.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.current = Some(current);
                cx.notify();
            });
        })
        .detach();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        if self.searching {
            return;
        }
        let query = self.query.read(cx).value().to_string();
        self.searching = true;
        self.found = None;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            b.umu_suggestions(&query, SUGGESTIONS).await
        });
        cx.spawn(async move |this, cx| {
            let found = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.searching = false;
                this.found = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn choose(&mut self, umu_id: &str, cx: &mut Context<Self>) {
        backend(cx).settings.set_game_value(
            &self.id,
            "iroriUmuId",
            Some(serde_json::Value::String(umu_id.to_string())),
        );
        self.refresh(cx);
    }

    fn current_text(&self) -> SharedString {
        match &self.current {
            None => t("settings.umu.looking"),
            Some(None) => t("settings.umu.none"),
            Some(Some(found)) => {
                let source = match found.source {
                    UmuSource::Setting => t("settings.umu.source.setting"),
                    UmuSource::Store => t("settings.umu.source.store"),
                    UmuSource::SteamApp => t("settings.umu.source.steam"),
                    UmuSource::Title => t("settings.umu.source.title"),
                };
                t_args(
                    "settings.umu.current",
                    &[("id", &found.id), ("source", &source)],
                )
            }
        }
    }
}

impl Render for UmuPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let results = self.found.clone().map(|found| {
            if found.is_empty() {
                return v_flex().child(muted(t("settings.umu.no_results"), cx));
            }
            found
                .into_iter()
                .enumerate()
                .fold(v_flex().gap_1(), |list, (ix, entry)| {
                    let umu_id = entry.umu_id.clone();
                    list.child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(format!("{} · {}", entry.title, entry.umu_id)),
                            )
                            .child(
                                Button::new(ElementId::Name(format!("umu-use-{ix}").into()))
                                    .ghost()
                                    .xsmall()
                                    .label(t("settings.umu.use"))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.choose(&umu_id, cx)),
                                    ),
                            ),
                    )
                })
        });
        panel(cx)
            .p_4()
            .gap_3()
            .child(div().text_sm().child(self.current_text()))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.query)))
                    .child(
                        Button::new("umu-search")
                            .outline()
                            .small()
                            .label(t("settings.umu.search"))
                            .loading(self.searching)
                            .disabled(self.searching)
                            .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
                    ),
            )
            .when_some(results, |p, r| p.child(r))
            .child(muted(t("settings.umu.hint"), cx).text_xs())
    }
}
