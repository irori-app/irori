//! Steam account connection dialog, drawn inside [`super::LoginDialog`].
//!
//! Installed Steam games already work without connecting an account. To add
//! owned games, playtime, friends, and achievements, the user can choose
//! between two safe methods:
//!
//! 1. Valve's official web sign-in page (`store.steampowered.com/login/`) in
//!    Irori's sign-in window (`irori-sign-in --steam-account`). Irori never
//!    sees the account password or calls `IAuthenticationService` directly.
//! 2. A personal Steam Web API key (`steamcommunity.com/dev/apikey`) plus the
//!    user's `SteamID64`, creating no Steam login session at all.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div,
};
use irori_core::process::CancellationToken;
use irori_steam::signin::{
    STEAM_API_KEY_URL, STEAM_LOGIN_URL, STEAM_PRIVACY_URL, api_key_login_input,
    detected_local_steamid, parse_steamid_input, validate_api_key, web_login_input,
};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use super::window::{self, WindowOutcome};

/// Steam finished connecting: payload accepted by [`irori_core::Store::login`].
pub(crate) struct SteamSignedIn(pub String);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Web,
    ApiKey,
}

/// Guard that cancels the sign-in window helper process when dropped.
struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

enum WebWindow {
    Idle,
    Open {
        _cancel: CancelOnDrop,
        _task: Task<()>,
    },
    Closed,
}

pub(crate) struct SteamSignInView {
    mode: Mode,
    has_helper: bool,
    web_window: WebWindow,
    steamid: Entity<InputState>,
    api_key: Entity<InputState>,
    detected_steamid: bool,
    error: Option<SharedString>,
    _detect: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SteamSignedIn> for SteamSignInView {}

impl SteamSignInView {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let steamid = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("manage.steam.api.steamid_placeholder"))
        });
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t("manage.steam.api.key_placeholder"))
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe_in(&steamid, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_api_key(cx),
                InputEvent::Change => {
                    this.detected_steamid = false;
                    this.error = None;
                    cx.notify();
                }
                _ => {
                    let _ = window;
                }
            }),
        );
        subscriptions.push(
            cx.subscribe_in(
                &api_key,
                window,
                |this, _, event, _window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit_api_key(cx),
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        );

        let settings = irori_ui::models(cx).settings.read(cx).app.clone();
        let detect_handle = backend(cx).spawn_blocking(move || detected_local_steamid(&settings));
        let detect = cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(id)) = detect_handle.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.steamid.read(cx).value().trim().is_empty() {
                    this.steamid
                        .update(cx, |state, cx| state.set_value(id, window, cx));
                    this.detected_steamid = true;
                    cx.notify();
                }
            });
        });

        Self {
            mode: Mode::Web,
            has_helper: window::helper().is_some(),
            web_window: WebWindow::Idle,
            steamid,
            api_key,
            detected_steamid: false,
            error: None,
            _detect: detect,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn close_window(&mut self) {
        if matches!(self.web_window, WebWindow::Open { .. }) {
            self.web_window = WebWindow::Closed;
        }
    }

    pub(crate) fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.close_window();
        self.mode = mode;
        self.error = None;
        if mode == Mode::ApiKey {
            if self.steamid.read(cx).value().trim().is_empty() {
                self.steamid.read(cx).focus_handle(cx).focus(window, cx);
            } else {
                self.api_key.read(cx).focus_handle(cx).focus(window, cx);
            }
        }
        cx.notify();
    }

    fn open_steam_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(helper) = window::helper() else {
            self.has_helper = false;
            self.error = None;
            cx.notify();
            return;
        };
        let cancel = CancellationToken::new();
        let title = t_args("manage.login.title", &[("store", "Steam")]).to_string();
        let task = irori_ui::run(
            cx,
            window::sign_in_steam(helper, STEAM_LOGIN_URL.into(), title, cancel.clone()),
        );
        self.error = None;
        let poll = cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if !matches!(this.web_window, WebWindow::Open { .. }) {
                    return;
                }
                match outcome {
                    Ok(WindowOutcome::SignedIn(payload)) => {
                        this.web_window = WebWindow::Idle;
                        match web_login_input(&payload) {
                            Ok(input) => cx.emit(SteamSignedIn(input)),
                            Err(e) => this.error = Some(format!("{e:#}").into()),
                        }
                    }
                    Ok(WindowOutcome::Closed) => {
                        this.web_window = WebWindow::Closed;
                    }
                    Err(e) => {
                        tracing::warn!("Steam sign-in window failed: {e:#}");
                        this.web_window = WebWindow::Closed;
                        this.error = Some(format!("{e:#}").into());
                    }
                }
                cx.notify();
            });
        });
        self.web_window = WebWindow::Open {
            _cancel: CancelOnDrop(cancel),
            _task: poll,
        };
        cx.notify();
    }

    fn submit_api_key(&mut self, cx: &mut Context<Self>) {
        let raw_id = self.steamid.read(cx).value().to_string();
        let raw_key = self.api_key.read(cx).value().to_string();
        let steamid = match parse_steamid_input(&raw_id) {
            Ok(id) => id,
            Err(_) => {
                self.error = Some(t("manage.steam.api.error_steamid"));
                cx.notify();
                return;
            }
        };
        let key = match validate_api_key(&raw_key) {
            Ok(key) => key,
            Err(_) => {
                self.error = Some(t("manage.steam.api.error_key"));
                cx.notify();
                return;
            }
        };
        self.error = None;
        cx.emit(SteamSignedIn(api_key_login_input(steamid, &key)));
        cx.notify();
    }

    fn render_web_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let controls: AnyElement = if !self.has_helper {
            v_flex()
                .gap_3()
                .items_start()
                .child(Alert::info(
                    "steam-web-no-window",
                    t("manage.steam.web.no_window"),
                ))
                .child(
                    Button::new("steam-web-use-api-key")
                        .debug_selector(|| "steam-web-use-api-key".into())
                        .primary()
                        .label(t("manage.steam.web.use_api_key"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_mode(Mode::ApiKey, window, cx)
                        })),
                )
                .into_any_element()
        } else {
            match &self.web_window {
                WebWindow::Idle => h_flex()
                    .child(
                        Button::new("steam-web-open")
                            .debug_selector(|| "steam-web-open".into())
                            .primary()
                            .icon(IconName::LogIn)
                            .label(t("manage.steam.web.open"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_steam_window(window, cx)
                            })),
                    )
                    .into_any_element(),
                WebWindow::Open { .. } => h_flex()
                    .gap_3()
                    .items_start()
                    .child(div().pt_0p5().child(Spinner::new().small()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(t("manage.steam.web.waiting")),
                    )
                    .into_any_element(),
                WebWindow::Closed => v_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(t("manage.steam.web.closed")),
                    )
                    .child(
                        Button::new("steam-web-reopen")
                            .primary()
                            .icon(IconName::LogIn)
                            .label(t("manage.steam.web.reopen"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_steam_window(window, cx)
                            })),
                    )
                    .into_any_element(),
            }
        };

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.steam.web.summary")),
            )
            .child(Alert::info(
                "steam-web-vpn-notice",
                t("manage.steam.web.vpn_notice"),
            ))
            .child(controls)
            .into_any_element()
    }

    fn render_api_key_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.steam.api.summary")),
            )
            .child(step(
                1,
                t("manage.steam.api.step1_title"),
                v_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(t("manage.steam.api.step1_body")),
                    )
                    .child(
                        Button::new("steam-api-open-key-page")
                            .debug_selector(|| "steam-api-open-key-page".into())
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label(t("manage.steam.api.open_key_page"))
                            .on_click(|_, _, cx| cx.open_url(STEAM_API_KEY_URL)),
                    ),
                cx,
            ))
            .child(step(
                2,
                t("manage.steam.api.step2_title"),
                v_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(t("manage.steam.api.step2_body")),
                    )
                    .child(
                        Button::new("steam-api-open-privacy-page")
                            .debug_selector(|| "steam-api-open-privacy-page".into())
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label(t("manage.steam.api.open_privacy_page"))
                            .on_click(|_, _, cx| cx.open_url(STEAM_PRIVACY_URL)),
                    ),
                cx,
            ))
            .child(step(
                3,
                t("manage.steam.api.step3_title"),
                v_flex()
                    .gap_2()
                    .child(Input::new(&self.steamid))
                    .when(self.detected_steamid, |this| {
                        this.child(
                            div()
                                .id("steam-api-detected-hint")
                                .text_xs()
                                .text_color(muted)
                                .child(t("manage.steam.api.steamid_detected")),
                        )
                    })
                    .child(Input::new(&self.api_key).mask_toggle())
                    .child(
                        h_flex().pt_1().justify_end().child(
                            Button::new("steam-api-connect")
                                .debug_selector(|| "steam-api-connect".into())
                                .primary()
                                .icon(IconName::LogIn)
                                .label(t("manage.steam.api.connect"))
                                .on_click(cx.listener(|this, _, _, cx| this.submit_api_key(cx))),
                        ),
                    ),
                cx,
            ))
            .into_any_element()
    }
}

/// One numbered step in the guided API key setup.
fn step(n: usize, title: SharedString, body: impl IntoElement, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .flex_shrink_0()
                .size_6()
                .rounded_full()
                .bg(theme.muted)
                .text_xs()
                .font_semibold()
                .flex()
                .items_center()
                .justify_center()
                .child(n.to_string()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().font_semibold().child(title))
                .child(body),
        )
}

#[cfg(test)]
impl SteamSignInView {
    pub(crate) fn set_api_inputs_for_test(
        &mut self,
        steamid: &str,
        api_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.steamid
            .update(cx, |s, cx| s.set_value(steamid, window, cx));
        self.api_key
            .update(cx, |s, cx| s.set_value(api_key, window, cx));
    }

    pub(crate) fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
}

impl Render for SteamSignInView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.steam.intro")),
            )
            .child(
                div().debug_selector(|| "steam-sign-in-mode".into()).child(
                    TabBar::new("steam-sign-in-mode")
                        .segmented()
                        .small()
                        .selected_index(match mode {
                            Mode::Web => 0,
                            Mode::ApiKey => 1,
                        })
                        .child(Tab::new().label(t("manage.steam.tab_web")))
                        .child(Tab::new().label(t("manage.steam.tab_api_key")))
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            let mode = if *ix == 0 { Mode::Web } else { Mode::ApiKey };
                            this.set_mode(mode, window, cx);
                        })),
                ),
            )
            .child(match mode {
                Mode::Web => self.render_web_tab(cx),
                Mode::ApiKey => self.render_api_key_tab(cx),
            })
            .when_some(self.error.clone(), |this, message| {
                this.child(Alert::error("steam-error", message))
            })
    }
}
