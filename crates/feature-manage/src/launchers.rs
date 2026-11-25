//! "Other launchers" on the Stores page: Battle.net, the EA app, Ubisoft
//! Connect and the Rockstar Games Launcher. One switch shows their games in
//! the library; a card per launcher sets it up in a prefix of its own with
//! the vendor's installer (natively on Windows) or adopts a prefix the user
//! already has, opens it so the user signs in and installs games there,
//! scans it for games, closes it, and removes it from Irori (its prefix and
//! games stay on disk).
//!
//! An installed launcher's card also adds (or removes) desktop and menu
//! shortcuts that open it through Irori ([`launcher_shortcut`]), which the
//! command palette offers too.
//!
//! A last card is the Xbox app's: nothing to set up, since Windows lists its
//! games; it opens the Xbox app and looks for games again, and elsewhere
//! says it is Windows-only. On every system it also signs in to an Xbox
//! (Microsoft) account, which adds playtime, achievements and art, and the
//! PC games the account played.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{Select, SelectItem, SelectState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px, rems,
};
use irori_core::{InstallProgress, OpContext, Runner, WineInstallation};
use irori_launchers::{
    LauncherKind as Launcher, LauncherSignIn, LauncherStatus, LaunchersStore, XboxAccountStatus,
    XboxStatus,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, card, notify_error, notify_success, wrapped_tooltip};

struct LaunchersHandle(Arc<LaunchersStore>);
impl Global for LaunchersHandle {}

/// Give the management screens the launchers store (the backend exposes
/// stores only as `dyn Store`).
pub fn install_launchers_store(store: Arc<LaunchersStore>, cx: &mut App) {
    cx.set_global(LaunchersHandle(store));
}

pub(crate) fn launchers_store(cx: &App) -> Option<Arc<LaunchersStore>> {
    cx.try_global::<LaunchersHandle>().map(|h| h.0.clone())
}

/// The launchers set up in a prefix of their own, for the command palette.
pub fn wine_launchers(cx: &App) -> Vec<Launcher> {
    launchers_store(cx)
        .map(|s| s.wine_launchers())
        .unwrap_or_default()
}

/// The launchers Irori set up or found games of, for the command palette.
pub fn known_launchers(cx: &App) -> Vec<Launcher> {
    launchers_store(cx)
        .map(|s| s.known_launchers())
        .unwrap_or_default()
}

/// Start a launcher, reporting a failure.
pub fn open_launcher(launcher: Launcher, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(store) = launchers_store(cx) else {
        return;
    };
    let task = irori_ui::run(cx, async move { store.open(launcher).await });
    window
        .spawn(cx, async move |cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    notify_error(
                        t_args(
                            "manage.launchers.open_failed",
                            &[("launcher", launcher.display_name())],
                        ),
                        &e,
                        window,
                        cx,
                    )
                });
            }
        })
        .detach();
}

/// Add (or with `add` false remove) the desktop and application menu
/// shortcuts that open `launcher` through Irori, with its program's icon.
pub fn launcher_shortcut(launcher: Launcher, add: bool, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(store) = launchers_store(cx) else {
        return;
    };
    let app = backend(cx).settings.app();
    let name = launcher.display_name();
    let task = irori_ui::run(cx, async move {
        use irori_extras::shortcuts;
        let program = store.program(launcher);
        let client = shortcuts::launcher_client(launcher.id(), name, program.as_deref());
        if add {
            let targets = shortcuts::ShortcutTargets {
                desktop: true,
                app_menu: true,
                steam: false,
            };
            shortcuts::add(&client, &app, targets).await.map(|_| ())
        } else {
            shortcuts::remove(&client, &app).await
        }
    });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => notify_success(
                    if add {
                        t_args("manage.launchers.shortcut_added", &[("launcher", name)])
                    } else {
                        t_args("manage.launchers.shortcut_removed", &[("launcher", name)])
                    },
                    window,
                    cx,
                ),
                Err(e) => notify_error(
                    t_args("manage.launchers.shortcut_failed", &[("launcher", name)]),
                    &e,
                    window,
                    cx,
                ),
            });
        })
        .detach();
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WineChoice {
    pub value: SharedString,
    pub title: SharedString,
}

impl SelectItem for WineChoice {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &SharedString {
        &self.value
    }
}

/// Where the Xbox app comes from when it isn't installed.
const XBOX_APP_PAGE: &str = "https://www.xbox.com/apps/xbox-app-for-pc";

/// What a card's buttons ask of the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Open,
    Scan,
    Close,
    Forget,
}

#[derive(Default, Clone)]
struct Live {
    progress: Option<InstallProgress>,
    step: Option<String>,
}

/// The section of the Stores page.
pub struct LaunchersSection {
    statuses: Vec<LauncherStatus>,
    xbox: Option<XboxStatus>,
    xbox_account: XboxAccountStatus,
    signing_out: bool,
    wines: Vec<WineInstallation>,
    wine_select: Entity<SelectState<Vec<WineChoice>>>,
    running: HashMap<Launcher, Arc<Mutex<Live>>>,
    grid_width: Rc<Cell<Pixels>>,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LaunchersSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wine_select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let m = models(cx).clone();
        let subscriptions = vec![
            cx.observe(&m.library, |this, _, cx| {
                this.refresh_statuses(cx);
            }),
            cx.observe(&m.settings, |_, _, cx| cx.notify()),
            cx.observe(&m.accounts, |this, _, cx| this.refresh_statuses(cx)),
        ];
        let mut this = Self {
            statuses: Vec::new(),
            xbox: None,
            xbox_account: XboxAccountStatus::default(),
            signing_out: false,
            wines: Vec::new(),
            wine_select,
            running: HashMap::new(),
            grid_width: Rc::new(Cell::new(px(0.))),
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.refresh_statuses(cx);
        if !cfg!(windows) {
            this.load_wines(window, cx);
        }
        this
    }

    fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = launchers_store(cx) {
            self.statuses = store.statuses();
            self.xbox = Some(store.xbox_status());
            self.xbox_account = store.xbox_account_status();
        }
        cx.notify();
    }

    fn load_wines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations_with_sources(&b.settings.app()).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(found) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let default = backend(cx).settings.app().game.wine_version.bin.clone();
                let items: Vec<WineChoice> = found
                    .iter()
                    .map(|(w, source)| WineChoice {
                        value: w.bin.clone().into(),
                        title: irori_ui::wine_choice::build_label(w, *source),
                    })
                    .collect();
                let wines: Vec<_> = found.into_iter().map(|(w, _)| w).collect();
                let ix = wines
                    .iter()
                    .position(|w| w.bin == default)
                    .or((!wines.is_empty()).then_some(0))
                    .map(|row| IndexPath::default().row(row));
                this.wine_select.update(cx, |s, cx| {
                    s.set_items(items, window, cx);
                    s.set_selected_index(ix, window, cx);
                });
                this.wines = wines;
                cx.notify();
            });
        })
        .detach();
    }

    fn chosen_wine(&self, cx: &App) -> Option<WineInstallation> {
        let bin = self.wine_select.read(cx).selected_value()?.to_string();
        self.wines.iter().find(|w| w.bin == bin).cloned()
    }

    fn set_up(&mut self, launcher: Launcher, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        if self.running.contains_key(&launcher) {
            return;
        }
        ensure_third_party_launchers_enabled(cx);
        let wine = self.chosen_wine(cx);
        let live = Arc::new(Mutex::new(Live::default()));
        let (progress_live, step_live) = (live.clone(), live.clone());
        let ctx = OpContext {
            progress: Arc::new(move |p| {
                if let Ok(mut l) = progress_live.lock() {
                    l.progress = Some(p);
                }
            }),
            step: Arc::new(move |s| {
                if let Ok(mut l) = step_live.lock() {
                    l.step = Some(s.to_string());
                }
            }),
            ..OpContext::detached()
        };
        self.running.insert(launcher, live);
        self.ensure_ticker(window, cx);
        cx.notify();
        let task = irori_ui::run(cx, async move { store.set_up(launcher, wine, &ctx).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.running.remove(&launcher);
                if let Err(e) = result {
                    notify_error(
                        t_args(
                            "manage.launchers.setup_failed",
                            &[("launcher", launcher.display_name())],
                        ),
                        &e,
                        window,
                        cx,
                    );
                }
                this.refresh_statuses(cx);
            });
        })
        .detach();
    }

    fn ensure_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._ticker.is_some() {
            return;
        }
        self._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let Ok(busy) = this.update(cx, |this, cx| {
                    cx.notify();
                    !this.running.is_empty()
                }) else {
                    return;
                };
                if !busy {
                    let _ = this.update(cx, |this, _| this._ticker = None);
                    return;
                }
            }
        }));
    }

    fn act(
        &mut self,
        launcher: Launcher,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        let task = irori_ui::run(cx, async move {
            match action {
                Action::Open => store.open(launcher).await,
                Action::Scan => irori_core::Store::refresh(store.as_ref(), false).await,
                Action::Close => store.close(launcher).await,
                Action::Forget => store.forget(launcher).await,
            }
        });
        let name = launcher.display_name();
        let failed = move || match action {
            Action::Open | Action::Scan => {
                t_args("manage.launchers.open_failed", &[("launcher", name)])
            }
            Action::Close => t_args("manage.launchers.close_failed", &[("launcher", name)]),
            Action::Forget => t_args("manage.launchers.remove_failed", &[("launcher", name)]),
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(failed(), &e, window, cx);
                }
                this.refresh_statuses(cx);
            });
        })
        .detach();
    }

    /// Close the launcher, first asking when one of its games is running,
    /// since that game stops too.
    fn close(&mut self, launcher: Launcher, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let prefix = format!("{}:", launcher.id());
        let playing = b
            .running_games()
            .into_iter()
            .filter(|id| id.runner == Runner::Launchers && id.app_name.starts_with(&prefix))
            .find_map(|id| b.game(&id))
            .map(|g| g.title.clone());
        let Some(title) = playing else {
            self.act(launcher, Action::Close, window, cx);
            return;
        };
        let view = cx.entity().downgrade();
        let name = launcher.display_name();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .confirm()
                .title(t_args(
                    "manage.launchers.close_confirm.title",
                    &[("launcher", name)],
                ))
                .description(t_args(
                    "manage.launchers.close_confirm.description",
                    &[("title", &title)],
                ))
                .ok_text(t("manage.launchers.close"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let _ =
                        view.update(cx, |this, cx| this.act(launcher, Action::Close, window, cx));
                    true
                })
        });
    }

    /// Ask for a prefix that already has the launcher, and use it with the
    /// chosen Wine build.
    fn adopt(&mut self, launcher: Launcher, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        let wine = self.chosen_wine(cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t_args(
                "manage.launchers.adopt_prompt",
                &[("launcher", launcher.display_name())],
            )),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(prefix) = paths.into_iter().next() else {
                return;
            };
            let Ok(task) = this.update(cx, |_, cx| {
                ensure_third_party_launchers_enabled(cx);
                irori_ui::run(cx, async move { store.adopt(launcher, prefix, wine).await })
            }) else {
                return;
            };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(
                        t_args(
                            "manage.launchers.adopt_failed",
                            &[("launcher", launcher.display_name())],
                        ),
                        &e,
                        window,
                        cx,
                    );
                }
                this.refresh_statuses(cx);
            });
        })
        .detach();
    }

    /// Open the Xbox app, or look for its games again.
    fn xbox_act(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        ensure_third_party_launchers_enabled(cx);
        let task = irori_ui::run(cx, async move {
            if open {
                store.open_xbox_app().await
            } else {
                irori_core::Store::refresh(store.as_ref(), false).await
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(
                        t_args(
                            "manage.launchers.open_failed",
                            &[("launcher", irori_launchers::XBOX_APP)],
                        ),
                        &e,
                        window,
                        cx,
                    );
                }
                this.refresh_statuses(cx);
            });
        })
        .detach();
    }

    /// Sign out of the Xbox account.
    fn xbox_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        self.signing_out = true;
        cx.notify();
        let task = irori_ui::run(cx, async move {
            irori_core::Store::logout(store.as_ref()).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.signing_out = false;
                if let Err(e) = result {
                    notify_error(
                        t("manage.launchers.xbox.account.sign_out_failed"),
                        &e,
                        window,
                        cx,
                    );
                }
                this.refresh_statuses(cx);
            });
        })
        .detach();
    }

    /// The Xbox account's part of the card: sign in, who is signed in, or
    /// that Microsoft no longer accepts the sign-in.
    fn render_xbox_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = &self.xbox_account;
        if !account.available {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let sign_in = |label: SharedString| {
            Button::new("launcher-xbox-sign-in")
                .debug_selector(|| "launcher-xbox-sign-in".to_string())
                .icon(IconName::LogIn)
                .label(label)
                .on_click(|_, window, cx| {
                    ensure_third_party_launchers_enabled(cx);
                    crate::login::open_login_dialog(Runner::Launchers, window, cx)
                })
        };
        let body: AnyElement = match (&account.gamertag, &account.expired) {
            (None, _) => v_flex()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(muted)
                        .child(t("manage.launchers.xbox.account.about")),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .child(sign_in(t("manage.launchers.xbox.account.sign_in"))),
                )
                .into_any_element(),
            (Some(_), Some(_)) => v_flex()
                .gap_2()
                .child(Alert::warning(
                    "launcher-xbox-expired",
                    t("manage.launchers.xbox.account.expired"),
                ))
                .child(
                    h_flex()
                        .justify_end()
                        .child(sign_in(t("manage.launchers.xbox.account.sign_in_again"))),
                )
                .into_any_element(),
            (Some(name), None) => {
                let played = match account.titles {
                    1 => t("manage.launchers.xbox.account.titles_one"),
                    n => t_args(
                        "manage.launchers.xbox.account.titles_other",
                        &[("count", &n.to_string())],
                    ),
                };
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .debug_selector(|| "launcher-xbox-gamertag".to_string())
                                    .child(t_args(
                                        "manage.launchers.xbox.account.signed_in",
                                        &[("name", name)],
                                    )),
                            )
                            .child(div().text_xs().text_color(muted).truncate().child(played)),
                    )
                    .child(
                        Button::new("launcher-xbox-friends")
                            .debug_selector(|| "launcher-xbox-friends".to_string())
                            .ghost()
                            .small()
                            .icon(IconName::Users)
                            .label(t("manage.friends.button"))
                            .on_click(|_, window, cx| {
                                crate::friends::open_friends_dialog(
                                    crate::friends::FriendsSource::Xbox,
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .child(
                        Button::new("launcher-xbox-sign-out")
                            .ghost()
                            .small()
                            .icon(IconName::LogOut)
                            .label(t("manage.launchers.xbox.account.sign_out"))
                            .loading(self.signing_out)
                            .disabled(self.signing_out)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.xbox_sign_out(window, cx)),
                            ),
                    )
                    .into_any_element()
            }
        };
        Some(
            v_flex()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(body)
                .into_any_element(),
        )
    }

    /// The Xbox app's card: its games on Windows, a note elsewhere.
    fn render_xbox_card(&self, status: XboxStatus, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let name = irori_launchers::XBOX_APP;
        let state = match (status.supported, status.app_installed, status.games) {
            (false, _, _) => Tag::secondary()
                .xsmall()
                .child(t("manage.launchers.state.windows_only")),
            (true, _, 1) => Tag::success()
                .xsmall()
                .child(t("manage.launchers.state.games_one")),
            (true, _, n) if n > 1 => Tag::success().xsmall().child(t_args(
                "manage.launchers.state.games_other",
                &[("count", &n.to_string())],
            )),
            (true, true, _) => Tag::success()
                .xsmall()
                .child(t("manage.launchers.state.ready")),
            (true, false, _) => Tag::secondary()
                .xsmall()
                .child(t("manage.launchers.state.no_xbox_app")),
        };
        // The note sits straight in the card, so it wraps at the card's
        // width; inside the buttons' row it kept its own length and ran
        // out of the card.
        let footer: AnyElement = if !status.supported {
            div()
                .debug_selector(|| "launcher-xbox-windows-only".to_string())
                .w_full()
                .min_w_0()
                .text_xs()
                .text_color(muted)
                .child(t("manage.launchers.xbox.windows_only"))
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .min_w_0()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("launcher-xbox-scan")
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label(t("manage.launchers.scan"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.xbox_act(false, window, cx)),
                        ),
                )
                .child(if status.app_installed {
                    Button::new("launcher-xbox-open")
                        .debug_selector(|| "launcher-xbox-open".to_string())
                        .icon(IconName::ExternalLink)
                        .label(t_args("manage.launchers.open", &[("launcher", name)]))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.xbox_act(true, window, cx)),
                        )
                } else {
                    Button::new("launcher-xbox-get")
                        .primary()
                        .icon(IconName::ExternalLink)
                        .label(t("manage.launchers.xbox.get_app"))
                        .on_click(|_, _, cx| cx.open_url(XBOX_APP_PAGE))
                })
                .into_any_element()
        };
        card(cx)
            .id("launcher-xbox")
            .debug_selector(|| "launcher-xbox-card".to_string())
            .p_5()
            .gap_4()
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_base()
                            .font_semibold()
                            .truncate()
                            .child(name),
                    )
                    .child(state),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.launchers.about.xbox")),
            )
            .child(footer)
            .children(self.render_xbox_account(cx))
            .into_any_element()
    }

    fn render_card(&self, status: &LauncherStatus, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let launcher = status.launcher;
        let name = launcher.display_name();
        let live = self
            .running
            .get(&launcher)
            .and_then(|l| l.lock().ok().map(|l| l.clone()));
        let state: AnyElement = match (&live, status.installed, status.games) {
            (Some(_), _, _) => Tag::info()
                .xsmall()
                .child(t("manage.launchers.state.setting_up"))
                .into_any_element(),
            (None, true, 0) => Tag::success()
                .xsmall()
                .child(t("manage.launchers.state.ready"))
                .into_any_element(),
            (None, true, n) => Tag::success()
                .xsmall()
                .child(if n == 1 {
                    t("manage.launchers.state.games_one")
                } else {
                    t_args(
                        "manage.launchers.state.games_other",
                        &[("count", &n.to_string())],
                    )
                })
                .into_any_element(),
            (None, false, _) => Tag::secondary()
                .xsmall()
                .child(t("manage.launchers.state.not_set_up"))
                .into_any_element(),
        };
        let account: Option<AnyElement> = match (status.installed, status.sign_in) {
            (true, LauncherSignIn::SignedIn) => Some(
                div()
                    .debug_selector(move || format!("launcher-{}-signed-in", launcher.id()))
                    .child(
                        Tag::success()
                            .xsmall()
                            .child(t("manage.launchers.state.signed_in")),
                    )
                    .into_any_element(),
            ),
            (true, LauncherSignIn::SignedOut) => Some(
                div()
                    .debug_selector(move || format!("launcher-{}-signed-out", launcher.id()))
                    .child(
                        Tag::warning()
                            .xsmall()
                            .child(t("manage.launchers.state.signed_out")),
                    )
                    .into_any_element(),
            ),
            _ => None,
        };
        let remove = (!cfg!(windows) && status.setup.is_some()).then(|| {
            Button::new("launcher-remove")
                .debug_selector(move || format!("launcher-{}-remove", launcher.id()))
                .ghost()
                .label(t("manage.launchers.remove"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.act(launcher, Action::Forget, window, cx)
                }))
        });
        // Windows runs the launchers natively.
        let wine_settings = !cfg!(windows) && status.setup.is_some();
        let actions: AnyElement = if let Some(live) = &live {
            let progress = live.progress.unwrap_or_default();
            v_flex()
                .w(rems(14.))
                .gap_1()
                .child(
                    Progress::new(ElementId::Name(
                        format!("launcher-{}-progress", launcher.id()).into(),
                    ))
                    .value(progress.percent.unwrap_or(0.))
                    .loading(
                        progress.percent.is_none() || live.step.as_deref() != Some("Downloading"),
                    ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(match live.step.as_deref() {
                            Some("Installing") => {
                                t_args("manage.launchers.installing", &[("launcher", name)])
                            }
                            Some("Creating Wine prefix") => t("manage.launchers.creating_prefix"),
                            _ => t("manage.launchers.downloading"),
                        }),
                )
                .into_any_element()
        } else if status.installed {
            h_flex()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .children(remove)
                .child(
                    Button::new("launcher-scan")
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label(t("manage.launchers.scan"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.act(launcher, Action::Scan, window, cx)
                        })),
                )
                .child(
                    Button::new("launcher-close")
                        .debug_selector(move || format!("launcher-{}-close", launcher.id()))
                        .ghost()
                        .icon(IconName::Power)
                        .label(t("manage.launchers.close"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.close(launcher, window, cx)
                            }),
                        ),
                )
                .child(
                    Button::new("launcher-more")
                        .debug_selector(move || format!("launcher-{}-more", launcher.id()))
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label(t("manage.launchers.more"))
                        .tooltip(t("manage.launchers.more"))
                        .dropdown_menu(move |menu, _, _| {
                            let menu = if wine_settings {
                                menu.item(
                                    PopupMenuItem::new(t("manage.launchers.wine_settings"))
                                        .icon(IconName::Wine)
                                        .on_click(move |_, window, cx| {
                                            crate::launcher_wine::open_launcher_wine_dialog(
                                                launcher, window, cx,
                                            )
                                        }),
                                )
                            } else {
                                menu
                            };
                            menu.item(
                                PopupMenuItem::new(t("manage.launchers.add_shortcut"))
                                    .icon(IconName::Link)
                                    .on_click(move |_, window, cx| {
                                        launcher_shortcut(launcher, true, window, cx)
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(t("manage.launchers.remove_shortcut")).on_click(
                                    move |_, window, cx| {
                                        launcher_shortcut(launcher, false, window, cx)
                                    },
                                ),
                            )
                        }),
                )
                .child(
                    Button::new("launcher-open")
                        .icon(IconName::ExternalLink)
                        .label(t_args("manage.launchers.open", &[("launcher", name)]))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.act(launcher, Action::Open, window, cx)
                        })),
                )
                .into_any_element()
        } else {
            let no_wine = !cfg!(windows) && self.wines.is_empty();
            h_flex()
                .flex_wrap()
                .justify_end()
                .gap_2()
                .children(remove)
                .when(!cfg!(windows), |d| {
                    d.child(
                        Button::new("launcher-adopt")
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .label(t("manage.launchers.adopt"))
                            .disabled(no_wine)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.adopt(launcher, window, cx)
                            })),
                    )
                })
                .child(
                    Button::new("launcher-setup")
                        .debug_selector(move || format!("launcher-{}-set-up", launcher.id()))
                        .primary()
                        .icon(IconName::Download)
                        .label(t_args("manage.launchers.set_up", &[("launcher", name)]))
                        .disabled(no_wine)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_up(launcher, window, cx)
                        })),
                )
                .into_any_element()
        };
        card(cx)
            .id(ElementId::Name(
                format!("launcher-{}", launcher.id()).into(),
            ))
            .p_5()
            .gap_4()
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .children(
                        irori_ui::components::StoreLogo::of_launcher(launcher.id())
                            .map(|logo| logo.icon().size_5().text_color(muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_base()
                            .font_semibold()
                            .truncate()
                            .child(name),
                    )
                    .children(account)
                    .child(state),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_1p5()
                    .child(div().text_sm().text_color(muted).child(match launcher {
                        Launcher::BattleNet => t("manage.launchers.about.battlenet"),
                        Launcher::EaApp => t("manage.launchers.about.ea"),
                        Launcher::Ubisoft => t("manage.launchers.about.ubisoft"),
                        Launcher::Rockstar => t("manage.launchers.about.rockstar"),
                    }))
                    // Concise one-line note with detailed encryption explanation in hover tooltip.
                    .when(launcher == Launcher::EaApp, |body| {
                        let tip = t("manage.launchers.ea.installed_only_tooltip");
                        body.child(
                            h_flex()
                                .id("launcher-ea-installed-only-tip")
                                .debug_selector(|| "launcher-ea-installed-only".to_string())
                                .gap_1()
                                .items_center()
                                .text_xs()
                                .text_color(muted)
                                .tooltip(move |window, cx| wrapped_tooltip(tip.clone(), window, cx))
                                .child(t("manage.launchers.ea.installed_only"))
                                .child(Icon::new(IconName::Info).xsmall().flex_shrink_0()),
                        )
                    }),
            )
            .child(h_flex().justify_end().child(actions))
            .into_any_element()
    }
}

fn ensure_third_party_launchers_enabled(cx: &mut App) {
    if !models(cx).settings.read(cx).app.third_party_launchers {
        backend_op(cx, |b| {
            b.settings.update_app(|s| s.third_party_launchers = true);
            b.bus
                .emit(irori_core::Event::LibraryChanged(Runner::Launchers));
        });
    }
}

impl Render for LaunchersSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mut cards: Vec<AnyElement> = self
            .statuses
            .clone()
            .iter()
            .map(|s| self.render_card(s, cx))
            .collect();
        cards.extend(self.xbox.map(|x| self.render_xbox_card(x, cx)));
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .gap_6()
                    .flex_wrap()
                    .items_center()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .child(t("manage.launchers.title")),
                            )
                            .child(div().text_sm().text_color(muted).child(if cfg!(windows) {
                                t("manage.launchers.description_windows")
                            } else {
                                t("manage.launchers.description")
                            })),
                    )
                    .when(!cfg!(windows), |bar| {
                        bar.child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    div()
                                        .id("launchers-wine-label")
                                        .text_sm()
                                        .text_color(muted)
                                        .tooltip(|window, cx| {
                                            crate::common::wrapped_tooltip(
                                                t("manage.launchers.wine_tooltip"),
                                                window,
                                                cx,
                                            )
                                        })
                                        .child(t("manage.launchers.wine")),
                                )
                                .child(div().w(rems(16.)).child(Select::new(&self.wine_select))),
                        )
                    }),
            )
            .child(crate::common::card_grid_capped(
                &self.grid_width,
                20.,
                5,
                cards,
                window,
            ))
    }
}
