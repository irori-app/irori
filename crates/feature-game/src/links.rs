//! A game's own links ("Links…"): web pages the user keeps with it (a wiki,
//! a mod site, a guide), added with a name and an address and removed one
//! by one. They are kept with the game's overrides
//! (`UserDataService::set_links`) and travel with sync; the page's Links row
//! opens them through `irori_ui::web_url`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};
use irori_core::{GameId, GameLink};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

/// The user's own links of `id`, as the library knows them.
pub(crate) fn of(id: &GameId, cx: &App) -> Vec<GameLink> {
    models(cx)
        .library
        .read(cx)
        .user
        .overrides
        .get(&id.key())
        .map(|o| o.links.clone())
        .unwrap_or_default()
}

/// Open "Links…" for `id`.
pub fn open_links_dialog(id: GameId, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let title = models(cx)
        .library
        .read(cx)
        .snapshot
        .get(&id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let dialog = cx.new(|cx| LinksDialog::new(id, window, cx));
    let width = window.rem_size() * 32.;
    let heading = t_args("game.links.dialog_title", &[("title", &title)]);
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

struct LinksDialog {
    id: GameId,
    links: Vec<GameLink>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    invalid: bool,
    _subscriptions: Vec<Subscription>,
}

impl LinksDialog {
    fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("game.links.name_placeholder")));
        let url =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("game.links.url_placeholder")));
        let enter =
            |this: &mut Self, e: &InputEvent, window: &mut Window, cx: &mut Context<Self>| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.add(window, cx);
                }
            };
        let subscriptions = vec![
            cx.subscribe_in(&name, window, move |this, _, e, window, cx| {
                enter(this, e, window, cx)
            }),
            cx.subscribe_in(&url, window, move |this, _, e, window, cx| {
                enter(this, e, window, cx)
            }),
        ];
        url.focus_handle(cx).focus(window, cx);
        Self {
            links: of(&id, cx),
            id,
            name,
            url,
            invalid: false,
            _subscriptions: subscriptions,
        }
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let url = self.url.read(cx).value().to_string();
        let Some(link) = GameLink::new(&name, &url) else {
            self.invalid = true;
            cx.notify();
            return;
        };
        self.invalid = false;
        if !self.links.iter().any(|l| l.url == link.url) {
            self.links.push(link);
            self.save(window, cx);
        }
        self.name.update(cx, |i, cx| i.set_value("", window, cx));
        self.url.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn remove(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.links.len() {
            self.links.remove(index);
            self.save(window, cx);
            cx.notify();
        }
    }

    /// Keep the list (off the UI thread: the library file is written).
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (b, id, links) = (backend(cx), self.id.clone(), self.links.clone());
        let task = irori_ui::run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || saver.userdata.set_links(&id, links))
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("game.links.failed")),
                        cx,
                    )
                });
            }
        })
        .detach();
    }
}

impl Render for LinksDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows = self.links.iter().enumerate().map(|(i, link)| {
            let label = link.label();
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(label.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(link.url.clone()),
                        ),
                )
                .child(
                    Button::new(("link-remove", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Trash)
                        .tooltip(t_args("game.links.remove", &[("name", &label)]))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.remove(i, window, cx)),
                        ),
                )
        });
        v_flex()
            .gap_3()
            .when(self.links.is_empty(), |c| {
                c.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("game.links.empty")),
                )
            })
            .child(v_flex().gap_1().children(rows))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w_1_3().child(Input::new(&self.name).small()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.url).small()),
                    )
                    .child(
                        Button::new("link-add")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label(t("game.links.add"))
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
            .when(self.invalid, |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(t("game.links.invalid")),
                )
            })
    }
}
