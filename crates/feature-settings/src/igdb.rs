//! The IGDB panel of Advanced → Artwork: the user's Twitch application
//! (client ID and secret), checked with Twitch before it is saved, and
//! forgetting it. The secret is kept with `irori_secrets` by services and
//! never shown again.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};
use irori_extras::igdb::{ACCOUNT_GUIDE, Credentials};
use irori_services::IgdbStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::muted;

pub(crate) struct IgdbPanel {
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
    /// `None` until read off the UI thread.
    status: Option<IgdbStatus>,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl IgdbPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let client_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.igdb.client_id")));
        let client_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.igdb.client_secret"))
                .masked(true)
        });
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            // The switch above the panel reaches the settings model.
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(
                &client_secret,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            client_id,
            client_secret,
            status: None,
            saving: false,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.igdb_status().await });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(credentials) = Credentials::new(
            &self.client_id.read(cx).value(),
            &self.client_secret.read(cx).value(),
        ) else {
            return;
        };
        self.set(Some(credentials), window, cx);
    }

    fn set(
        &mut self,
        credentials: Option<Credentials>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        self.saving = true;
        cx.notify();
        let saving = credentials.is_some();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.set_igdb_credentials(credentials).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.client_id
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        this.client_secret
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        if saving {
                            window.push_notification(
                                Notification::success(t("settings.igdb.saved")),
                                cx,
                            );
                        }
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("settings.igdb.save_failed")),
                        cx,
                    ),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn render_application(&self, status: &IgdbStatus, cx: &mut Context<Self>) -> AnyElement {
        match &status.client_id {
            Some(id) => h_flex()
                .gap_2()
                .items_center()
                .child(Tag::success().xsmall().child(t("settings.igdb.connected")))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(t_args("settings.igdb.saved_as", &[("id", id)])),
                )
                .child(
                    Button::new("igdb-forget")
                        .ghost()
                        .small()
                        .label(t("settings.igdb.forget"))
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, window, cx| this.set(None, window, cx))),
                )
                .into_any_element(),
            None => h_flex()
                .gap_2()
                .flex_wrap()
                .items_center()
                .child(div().flex_1().min_w_32().child(Input::new(&self.client_id)))
                .child(
                    div()
                        .flex_1()
                        .min_w_32()
                        .child(Input::new(&self.client_secret).mask_toggle()),
                )
                .child(
                    Button::new("igdb-save")
                        .primary()
                        .small()
                        .icon(IconName::Check)
                        .label(t("settings.igdb.save"))
                        .loading(self.saving)
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                )
                .into_any_element(),
        }
    }
}

impl Render for IgdbPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let subpanel = || {
            v_flex()
                .w_full()
                .p_3()
                .gap_2()
                .rounded(theme.radius)
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
        };
        let Some(status) = self.status.clone() else {
            return subpanel().child(muted(t("settings.igdb.looking"), cx));
        };
        let needs_key = status.client_id.is_none();
        subpanel()
            .when(needs_key, |body| {
                body.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .flex_wrap()
                                .child(Tag::warning().xsmall().child(t("settings.igdb.needs_key")))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(t("settings.igdb.description")),
                                ),
                        )
                        .child(
                            Link::new("igdb-guide")
                                .href(ACCOUNT_GUIDE)
                                .text_xs()
                                .child(t("settings.igdb.open_guide")),
                        ),
                )
            })
            .child(self.render_application(&status, cx))
            .when(!status.enabled, |body| {
                body.child(muted(t("settings.igdb.off_hint"), cx))
            })
    }
}
