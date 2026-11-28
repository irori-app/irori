//! The Emulators page's Achievements tab: the ROM library's
//! RetroAchievements account (a username and Web API key, checked with
//! RetroAchievements before it is kept), and signing the installed
//! emulators in to the same account with the password, which is used once
//! and not kept.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, rems,
};
use irori_core::Runner;
use irori_emulation::retro::{self, EmulatorSignIn};
use irori_engines::cheevos::{self, Outcome};
use irori_engines::{Host, systems};
use irori_services::EngineStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::ops::{EngineOps, engine_key, engine_ops};
use super::{
    install_engine_target, muted_xs, open, open_engine, preview_names, row_between, start_install,
};
use crate::common::{card, notify_success, plain_tooltip};

#[derive(Debug, Clone)]
struct SupportedEngine {
    id: String,
    name: String,
    systems_hint: String,
    installed: bool,
    downloadable: bool,
    can_open: bool,
    manual_login: bool,
}

/// The RetroAchievements account and emulator sign-in.
pub struct RetroView {
    /// The signed-in username.
    account: Option<String>,
    engines: Vec<SupportedEngine>,
    username: Entity<InputState>,
    api_key: Entity<InputState>,
    password: Entity<InputState>,
    hardcore: bool,
    signing_in: bool,
    signing_out: bool,
    signing_emulators: bool,
    error: Option<SharedString>,
    emulator_error: Option<SharedString>,
    results: HashMap<String, EmulatorSignIn>,
    ops: Entity<EngineOps>,
    _subscriptions: Vec<Subscription>,
}

fn current_account(cx: &App) -> Option<String> {
    backend(cx)
        .store(Runner::Emulation)?
        .accounts()
        .into_iter()
        .find(|a| a.slot == retro::SLOT)
        .map(|a| a.display_name)
}

fn build_supported_engines(statuses: Option<&[EngineStatus]>) -> Vec<SupportedEngine> {
    let host = Host::current();
    cheevos::SUPPORTED
        .iter()
        .filter_map(|id| {
            let status = statuses.and_then(|list| list.iter().find(|s| s.manifest.id == *id));
            let m = status
                .map(|s| s.manifest.clone())
                .or_else(|| irori_engines::engine(id))?;
            let shorts: Vec<String> = m
                .systems
                .iter()
                .filter_map(|s| systems::get(s).map(|sys| sys.short.clone()))
                .collect();
            Some(SupportedEngine {
                id: m.id.clone(),
                name: m.name.clone(),
                systems_hint: preview_names(&shorts, 3),
                installed: status.is_some_and(EngineStatus::is_installed),
                downloadable: m.downloadable_on(host),
                can_open: irori_engines::open::can_open(&m),
                manual_login: *id == "duckstation",
            })
        })
        .collect()
}

fn inset_box(cx: &App) -> Div {
    let t = cx.theme();
    div()
        .rounded(t.radius)
        .bg(t.background)
        .border_1()
        .border_color(t.border)
}

fn tip_wrap(id: String, tip: SharedString, child: impl IntoElement) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .tooltip(move |w, cx| plain_tooltip(tip.clone(), w, cx))
        .child(child)
        .into_any_element()
}

fn labeled(label: SharedString, field: impl IntoElement) -> Div {
    v_flex()
        .gap_1()
        .child(div().text_xs().font_medium().child(label))
        .child(field)
}

fn circle_badge(
    size: f32,
    bg: gpui_kit::Hsla,
    fg: gpui_kit::Hsla,
    text: impl Into<SharedString>,
) -> Div {
    div()
        .size(rems(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_bold()
        .flex_shrink_0()
        .bg(bg)
        .text_color(fg)
        .child(text.into())
}

fn step_header(
    num: &'static str,
    title: SharedString,
    active: bool,
    done: bool,
    trailing: Option<AnyElement>,
    cx: &App,
) -> Div {
    let t = cx.theme();
    let (bg, fg) = if done {
        (t.success.opacity(0.18), t.success)
    } else if active {
        (t.primary, t.primary_foreground)
    } else {
        (t.secondary, t.muted_foreground)
    };
    row_between()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(circle_badge(1.35, bg, fg, num))
                .child(div().text_sm().font_bold().child(title)),
        )
        .children(trailing)
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
        let ops = engine_ops(cx);
        let openable = open::openable(cx);
        let mut subscriptions = vec![
            cx.observe(&accounts, |this, _, cx| {
                let now = current_account(cx);
                if now != this.account {
                    this.account = now;
                    cx.notify();
                }
            }),
            cx.observe(&ops, |_, _, cx| cx.notify()),
            cx.observe(&openable, |_, _, cx| cx.notify()),
        ];
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
            engines: build_supported_engines(None),
            username,
            api_key,
            password,
            hardcore: false,
            signing_in: false,
            signing_out: false,
            signing_emulators: false,
            error: None,
            emulator_error: None,
            results: HashMap::new(),
            ops,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub(super) fn set_engines(&mut self, statuses: &[EngineStatus], cx: &mut Context<Self>) {
        self.engines = build_supported_engines(Some(statuses));
        cx.notify();
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
                    Ok(results) => {
                        this.results = results.into_iter().map(|r| (r.engine.clone(), r)).collect();
                    }
                    Err(e) => this.emulator_error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let (fg, border, secondary) = (
            cx.theme().foreground,
            cx.theme().border,
            cx.theme().secondary,
        );
        if let Some(name) = &self.account {
            let initials: String = name.chars().take(2).collect::<String>().to_uppercase();
            return v_flex()
                .gap_3()
                .child(
                    inset_box(cx).p_3().child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                circle_badge(2.25, secondary, fg, initials)
                                    .border_1()
                                    .border_color(border),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .truncate()
                                            .child(name.clone()),
                                    )
                                    .child(
                                        muted_xs(t("manage.retro.syncing_progress"), cx).truncate(),
                                    ),
                            )
                            .child(
                                Button::new("retro-sign-out")
                                    .small()
                                    .outline()
                                    .icon(IconName::LogOut)
                                    .label(t("manage.retro.sign_out"))
                                    .loading(self.signing_out)
                                    .disabled(self.signing_out)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.sign_out(window, cx)
                                        }),
                                    ),
                            ),
                    ),
                )
                .child(muted_xs(t("manage.retro.description"), cx))
                .into_any_element();
        }
        let api_url = irori_retroachievements_api_url();
        v_flex()
            .gap_3()
            .child(muted_xs(t("manage.retro.description"), cx))
            .child(
                inset_box(cx).p_3().child(
                    row_between()
                        .child(
                            muted_xs(t("manage.retro.key_hint"), cx)
                                .flex_1()
                                .min_w(rems(12.)),
                        )
                        .child(
                            Button::new("retro-open-settings")
                                .outline()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label(t("manage.retro.key_link"))
                                .on_click(move |_, _, cx| cx.open_url(&api_url)),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        labeled(
                            t("manage.retro.username"),
                            Input::new(&self.username).disabled(self.signing_in),
                        )
                        .flex_1()
                        .min_w(rems(11.)),
                    )
                    .child(
                        labeled(
                            t("manage.retro.api_key"),
                            Input::new(&self.api_key)
                                .mask_toggle()
                                .disabled(self.signing_in),
                        )
                        .flex_1()
                        .min_w(rems(13.)),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| Alert::error("retro-error", e).into_any_element()),
            )
            .child(
                row_between()
                    .pt_2()
                    .border_t_1()
                    .border_color(border)
                    .child(muted_xs(t("manage.retro.keyring_note"), cx))
                    .child(
                        Button::new("retro-sign-in")
                            .primary()
                            .small()
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
        let (fg, muted, border, success, info) = {
            let t = cx.theme();
            (
                t.foreground,
                t.muted_foreground,
                t.border,
                t.success,
                t.info,
            )
        };
        let connected = self.account.is_some();
        let openable = open::openable(cx);
        let running = &self.ops.read(cx).running;
        let opening_set = &openable.read(cx).opening;
        let installed_count = self.engines.iter().filter(|e| e.installed).count();
        let rows: Vec<AnyElement> = self
            .engines
            .iter()
            .enumerate()
            .map(|(ix, eng)| {
                let sel = format!("retro-engine-{}", eng.id);
                let status_el: AnyElement = if running.contains_key(&engine_key(&eng.id)) {
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(Spinner::new().xsmall())
                        .child(muted_xs(t("manage.emulators.progress.starting"), cx))
                        .into_any_element()
                } else if let Some(res) = self.results.get(&eng.id) {
                    match &res.result {
                        Ok(Outcome::SignedIn(path)) => tip_wrap(
                            format!("ra-ok-{}", eng.id),
                            t_args(
                                "manage.retro.emulator_signed_in",
                                &[("engine", &res.name), ("path", &path.display().to_string())],
                            ),
                            Tag::success()
                                .xsmall()
                                .child(t("manage.retro.status.signed_in")),
                        ),
                        Ok(Outcome::Enabled(_)) => {
                            let (id, name) = (eng.id.clone(), SharedString::from(eng.name.clone()));
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(tip_wrap(
                                    format!("ra-en-{}", eng.id),
                                    t_args(
                                        "manage.retro.emulator_enabled",
                                        &[("engine", &res.name)],
                                    ),
                                    Tag::info().xsmall().child(t("manage.retro.status.enabled")),
                                ))
                                .when(eng.can_open, |row| {
                                    row.child(
                                        Button::new(ElementId::Name(
                                            format!("ra-open-{}", eng.id).into(),
                                        ))
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::Play)
                                        .label(t("manage.emulators.open.button"))
                                        .loading(opening_set.contains(&eng.id))
                                        .on_click(
                                            move |_, w, cx| {
                                                open_engine(id.clone(), name.clone(), w, cx)
                                            },
                                        ),
                                    )
                                })
                                .into_any_element()
                        }
                        Err(e) => tip_wrap(
                            format!("ra-err-{}", eng.id),
                            t_args(
                                "manage.retro.emulator_failed",
                                &[("engine", &res.name), ("error", e)],
                            ),
                            Tag::danger()
                                .xsmall()
                                .child(t("manage.retro.status.failed")),
                        ),
                    }
                } else if eng.installed && eng.manual_login {
                    tip_wrap(
                        format!("ra-man-{}", eng.id),
                        t_args("manage.retro.emulator_enabled", &[("engine", &eng.name)]),
                        Tag::secondary()
                            .xsmall()
                            .child(t("manage.retro.manual_login")),
                    )
                } else if eng.installed {
                    Tag::secondary()
                        .xsmall()
                        .child(t("manage.retro.ready_to_sync"))
                        .into_any_element()
                } else if eng.downloadable {
                    let id = eng.id.clone();
                    Button::new(ElementId::Name(format!("ra-install-{}", eng.id).into()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Download)
                        .label(t("manage.emulators.install"))
                        .on_click(move |_, _, cx| {
                            start_install(install_engine_target(id.clone()), cx)
                        })
                        .into_any_element()
                } else {
                    muted_xs(t("manage.retro.not_installed"), cx).into_any_element()
                };
                h_flex()
                    .id(ElementId::Name(sel.clone().into()))
                    .debug_selector(move || sel.clone())
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .when(ix > 0, |r| r.border_t_1().border_color(border))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .size(rems(0.5))
                                    .rounded_full()
                                    .flex_shrink_0()
                                    .bg(if eng.installed { success } else { border }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if eng.installed { fg } else { muted })
                                    .truncate()
                                    .child(eng.name.clone()),
                            )
                            .child(muted_xs(eng.systems_hint.clone(), cx).truncate()),
                    )
                    .child(div().flex_shrink_0().child(status_el))
                    .into_any_element()
            })
            .collect();

        let busy = self.signing_emulators;
        let sync_label = match installed_count {
            0 => t("manage.retro.sign_in_emulators"),
            1 => t("manage.retro.sign_in_count_one"),
            n => t_args(
                "manage.retro.sign_in_count_other",
                &[("count", &n.to_string())],
            ),
        };
        let col = v_flex()
            .gap_3()
            .child(muted_xs(t("manage.retro.emulators_intro"), cx))
            .child(
                inset_box(cx)
                    .w_full()
                    .overflow_hidden()
                    .child(v_flex().children(rows)),
            );
        if !connected {
            return col
                .child(
                    inset_box(cx)
                        .p_3()
                        .child(muted_xs(t("manage.retro.connect_first"), cx)),
                )
                .into_any_element();
        }
        col.child(
            h_flex()
                .pt_2()
                .border_t_1()
                .border_color(border)
                .gap_2()
                .items_start()
                .child(Icon::new(IconName::ShieldCheck).small().text_color(info))
                .child(
                    muted_xs(t("manage.retro.emulators_hint"), cx)
                        .flex_1()
                        .min_w_0(),
                ),
        )
        .child(labeled(
            t("manage.retro.password"),
            Input::new(&self.password).mask_toggle().disabled(busy),
        ))
        .child(
            inset_box(cx).p_3().child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .child(t("manage.retro.hardcore")),
                            )
                            .child(muted_xs(t("manage.retro.hardcore_help"), cx)),
                    )
                    .child(
                        Switch::new("retro-hardcore")
                            .checked(self.hardcore)
                            .disabled(busy)
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.hardcore = *on;
                                cx.notify();
                            })),
                    ),
            ),
        )
        .children(
            self.emulator_error
                .clone()
                .map(|e| Alert::error("retro-emulator-error", e).into_any_element()),
        )
        .child(
            row_between()
                .child(muted_xs(
                    t_args(
                        "manage.retro.installed_summary",
                        &[
                            ("installed", &installed_count.to_string()),
                            ("total", &self.engines.len().to_string()),
                        ],
                    ),
                    cx,
                ))
                .child(
                    Button::new("retro-sign-in-emulators")
                        .primary()
                        .small()
                        .icon(IconName::Gamepad2)
                        .label(sync_label)
                        .loading(busy)
                        .disabled(busy || installed_count == 0)
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
        let connected = self.account.is_some();
        let muted = cx.theme().muted_foreground;
        let account_card = card(cx)
            .flex_1()
            .min_w(rems(24.))
            .gap_3()
            .child(step_header(
                if connected { "✓" } else { "1" },
                t("manage.retro.account_title"),
                !connected,
                connected,
                Some(if connected {
                    Tag::success()
                        .xsmall()
                        .child(t("manage.retro.connected"))
                        .into_any_element()
                } else {
                    Icon::new(IconName::Trophy)
                        .small()
                        .text_color(muted)
                        .into_any_element()
                }),
                cx,
            ))
            .child(self.render_account(cx));

        let emulators_card = card(cx)
            .flex_1()
            .min_w(rems(24.))
            .gap_3()
            .child(step_header(
                "2",
                t("manage.retro.emulators_title"),
                connected,
                false,
                None,
                cx,
            ))
            .child(self.render_emulators(cx));

        h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .gap_4()
            .child(account_card)
            .child(emulators_card)
    }
}
