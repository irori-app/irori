//! The Emulators page's Achievements tab: the ROM library's
//! RetroAchievements account (a username and Web API key, checked with
//! RetroAchievements before it is kept), and signing the installed
//! emulators in to the same account with the password, which is used once
//! and not kept.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::Runner;
use irori_emulation::retro::{self, EmulatorSignIn};
use irori_engines::cheevos::Outcome;
use irori_ui::components::SectionTitle;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{card, notify_success};

/// The RetroAchievements account and emulator sign-in.
pub struct RetroView {
    /// The signed-in username.
    account: Option<String>,
    username: Entity<InputState>,
    api_key: Entity<InputState>,
    password: Entity<InputState>,
    hardcore: bool,
    signing_in: bool,
    signing_out: bool,
    signing_emulators: bool,
    error: Option<SharedString>,
    emulator_error: Option<SharedString>,
    results: Vec<EmulatorSignIn>,
    _subscriptions: Vec<Subscription>,
}

fn current_account(cx: &gpui_kit::App) -> Option<String> {
    backend(cx)
        .store(Runner::Emulation)?
        .accounts()
        .into_iter()
        .find(|a| a.slot == retro::SLOT)
        .map(|a| a.display_name)
}

impl RetroView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.retro.username")));
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t("manage.retro.api_key"))
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t("manage.retro.password"))
        });
        let accounts = models(cx).accounts.clone();
        let mut subscriptions = vec![cx.observe(&accounts, |this, _, cx| {
            let now = current_account(cx);
            if now != this.account {
                this.account = now;
                cx.notify();
            }
        })];
        for input in [&username, &api_key] {
            subscriptions.push(cx.subscribe_in(input, window, |this, _, e, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.sign_in(window, cx);
                }
            }));
        }
        subscriptions.push(
            cx.subscribe_in(&password, window, |this, _, e, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.sign_in_emulators(window, cx);
                }
            }),
        );
        Self {
            account: current_account(cx),
            username,
            api_key,
            password,
            hardcore: false,
            signing_in: false,
            signing_out: false,
            signing_emulators: false,
            error: None,
            emulator_error: None,
            results: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.signing_in {
            return;
        }
        let username = self.username.read(cx).value().to_string();
        let key = self.api_key.read(cx).value().to_string();
        if username.trim().is_empty() || key.trim().is_empty() {
            self.error = Some(t("manage.retro.missing"));
            cx.notify();
            return;
        }
        self.signing_in = true;
        self.error = None;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(Runner::Emulation)
                .ok_or_else(|| anyhow::anyhow!("the ROM library is not available"))?;
            store.login(&retro::login_input(&username, &key)).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.signing_in = false;
                match result {
                    Ok(user) => {
                        this.account = Some(user.display_name.clone());
                        this.api_key.update(cx, |s, cx| s.set_value("", window, cx));
                        notify_success(
                            t_args("manage.retro.signed_in", &[("name", &user.display_name)]),
                            window,
                            cx,
                        );
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.signing_out {
            return;
        }
        self.signing_out = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(Runner::Emulation)
                .ok_or_else(|| anyhow::anyhow!("the ROM library is not available"))?;
            store.logout().await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.signing_out = false;
                match result {
                    Ok(()) => {
                        this.account = None;
                        this.results.clear();
                    }
                    Err(e) => crate::common::notify_error(
                        t("manage.retro.sign_out_failed"),
                        &e,
                        window,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in_emulators(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(username) = self.account.clone() else {
            return;
        };
        if self.signing_emulators {
            return;
        }
        let password = self.password.read(cx).value().to_string();
        if password.is_empty() {
            self.emulator_error = Some(t("manage.retro.password_missing"));
            cx.notify();
            return;
        }
        self.signing_emulators = true;
        self.emulator_error = None;
        cx.notify();
        let hardcore = self.hardcore;
        let task = irori_ui::run(cx, async move {
            retro::sign_in_emulators(&username, &password, hardcore).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.signing_emulators = false;
                this.password
                    .update(cx, |s, cx| s.set_value("", window, cx));
                match result {
                    Ok(results) if results.is_empty() => {
                        this.emulator_error = Some(t("manage.retro.no_emulators"));
                        this.results.clear();
                    }
                    Ok(results) => this.results = results,
                    Err(e) => this.emulator_error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if let Some(name) = &self.account {
            return h_flex()
                .gap_3()
                .items_center()
                .child(Icon::new(IconName::CircleCheck).text_color(cx.theme().success))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t_args("manage.retro.signed_in_as", &[("name", name)])),
                )
                .child(
                    Button::new("retro-sign-out")
                        .small()
                        .outline()
                        .icon(IconName::LogOut)
                        .label(t("manage.retro.sign_out"))
                        .loading(self.signing_out)
                        .disabled(self.signing_out)
                        .on_click(cx.listener(|this, _, window, cx| this.sign_out(window, cx))),
                )
                .into_any_element();
        }
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.retro.key_hint")),
            )
            .child(
                Link::new("retro-key-link")
                    .href(irori_retroachievements_api_url())
                    .child(t("manage.retro.key_link")),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(gpui_kit::rems(12.))
                            .child(Input::new(&self.username).disabled(self.signing_in)),
                    )
                    .child(
                        div().flex_1().min_w(gpui_kit::rems(16.)).child(
                            Input::new(&self.api_key)
                                .mask_toggle()
                                .disabled(self.signing_in),
                        ),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| Alert::error("retro-error", e).into_any_element()),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("retro-sign-in")
                        .primary()
                        .icon(IconName::LogIn)
                        .label(t("manage.retro.sign_in"))
                        .loading(self.signing_in)
                        .disabled(self.signing_in)
                        .on_click(cx.listener(|this, _, window, cx| this.sign_in(window, cx))),
                ),
            )
            .into_any_element()
    }

    fn render_emulators(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, success, danger) = (theme.muted_foreground, theme.success, theme.danger);
        let results: Vec<AnyElement> = self
            .results
            .iter()
            .map(|r| {
                let (icon, color, text) = match &r.result {
                    Ok(Outcome::SignedIn(path)) => (
                        IconName::CircleCheck,
                        success,
                        t_args(
                            "manage.retro.emulator_signed_in",
                            &[("engine", &r.name), ("path", &path.display().to_string())],
                        ),
                    ),
                    Ok(Outcome::Enabled(_)) => (
                        IconName::Info,
                        muted,
                        t_args("manage.retro.emulator_enabled", &[("engine", &r.name)]),
                    ),
                    Err(e) => (
                        IconName::CircleX,
                        danger,
                        t_args(
                            "manage.retro.emulator_failed",
                            &[("engine", &r.name), ("error", e)],
                        ),
                    ),
                };
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_sm()
                    .child(Icon::new(icon).small().text_color(color))
                    .child(div().flex_1().min_w_0().child(text))
                    .into_any_element()
            })
            .collect();
        let busy = self.signing_emulators;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.retro.emulators_hint")),
            )
            .child(Input::new(&self.password).mask_toggle().disabled(busy))
            .child(
                Switch::new("retro-hardcore")
                    .label(t("manage.retro.hardcore"))
                    .checked(self.hardcore)
                    .disabled(busy)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.hardcore = *on;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.retro.hardcore_help")),
            )
            .children(
                self.emulator_error
                    .clone()
                    .map(|e| Alert::error("retro-emulator-error", e).into_any_element()),
            )
            .children(results)
            .child(
                h_flex().justify_end().child(
                    Button::new("retro-sign-in-emulators")
                        .icon(IconName::Gamepad2)
                        .label(t("manage.retro.sign_in_emulators"))
                        .loading(busy)
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.sign_in_emulators(window, cx)),
                        ),
                ),
            )
            .into_any_element()
    }
}

fn irori_retroachievements_api_url() -> SharedString {
    SharedString::new_static(irori_retroachievements_api::API_KEY_URL)
}

impl Render for RetroView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let account = self.render_account(cx);
        let emulators = self.account.is_some().then(|| self.render_emulators(cx));
        v_flex()
            .gap_4()
            .child(
                card(cx)
                    .gap_3()
                    .child(SectionTitle::new("RetroAchievements"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("manage.retro.description")),
                    )
                    .child(account),
            )
            .when_some(emulators, |col, emulators| {
                col.child(
                    card(cx)
                        .gap_3()
                        .child(SectionTitle::new(t("manage.retro.emulators_title")))
                        .child(emulators),
                )
            })
    }
}
