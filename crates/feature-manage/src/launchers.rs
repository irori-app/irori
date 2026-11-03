//! "Other launchers" on the Stores page: Battle.net, the EA app, Ubisoft
//! Connect and the Rockstar Games Launcher. One switch shows their games in
//! the library; a card per launcher sets it up in a prefix of its own with
//! the vendor's installer (natively on Windows) or adopts a prefix the user
//! already has, opens it so the user signs in and installs games there,
//! scans it for games, closes it, and removes it from Irori (its prefix and
//! games stay on disk).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{Select, SelectItem, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, rems,
};
use irori_core::{InstallProgress, OpContext, Runner, WineInstallation};
use irori_launchers::{LauncherKind as Launcher, LauncherSignIn, LauncherStatus, LaunchersStore};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, card, notify_error};

struct LaunchersHandle(Arc<LaunchersStore>);
impl Global for LaunchersHandle {}

/// Give the management screens the launchers store (the backend exposes
/// stores only as `dyn Store`).
pub fn install_launchers_store(store: Arc<LaunchersStore>, cx: &mut App) {
    cx.set_global(LaunchersHandle(store));
}

fn launchers_store(cx: &App) -> Option<Arc<LaunchersStore>> {
    cx.try_global::<LaunchersHandle>().map(|h| h.0.clone())
}

#[derive(Debug, Clone, PartialEq)]
struct WineChoice {
    value: SharedString,
    title: SharedString,
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
    wines: Vec<WineInstallation>,
    wine_select: Entity<SelectState<Vec<WineChoice>>>,
    running: HashMap<Launcher, Arc<Mutex<Live>>>,
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
        ];
        let mut this = Self {
            statuses: Vec::new(),
            wines: Vec::new(),
            wine_select,
            running: HashMap::new(),
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
        }
        cx.notify();
    }

    fn load_wines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations(&b.settings.app()).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(wines) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let default = backend(cx).settings.app().game.wine_version.bin.clone();
                let items: Vec<WineChoice> = wines
                    .iter()
                    .map(|w| WineChoice {
                        value: w.bin.clone().into(),
                        title: w.name.clone().into(),
                    })
                    .collect();
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
                .small()
                .label(t("manage.launchers.remove"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.act(launcher, Action::Forget, window, cx)
                }))
        });
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
                        progress.percent.is_none() || live.step.as_deref() == Some("Installing"),
                    ),
                )
                .child(div().text_xs().text_color(muted).child(
                    if live.step.as_deref() == Some("Installing") {
                        t_args("manage.launchers.installing", &[("launcher", name)])
                    } else {
                        t("manage.launchers.downloading")
                    },
                ))
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
                        .small()
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
                        .small()
                        .icon(IconName::Power)
                        .label(t("manage.launchers.close"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.close(launcher, window, cx)
                            }),
                        ),
                )
                .child(
                    Button::new("launcher-open")
                        .small()
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
                            .small()
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
                        .small()
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
            .w(rems(22.))
            .flex_grow(1.)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .truncate()
                            .child(name),
                    )
                    .children(account)
                    .child(state),
            )
            .child(div().text_xs().text_color(muted).child(match launcher {
                Launcher::BattleNet => t("manage.launchers.about.battlenet"),
                Launcher::EaApp => t("manage.launchers.about.ea"),
                Launcher::Ubisoft => t("manage.launchers.about.ubisoft"),
                Launcher::Rockstar => t("manage.launchers.about.rockstar"),
            }))
            .child(h_flex().justify_end().child(actions))
            .into_any_element()
    }
}

impl Render for LaunchersSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let enabled = models(cx).settings.read(cx).app.third_party_launchers;
        let cards: Vec<AnyElement> = if enabled {
            self.statuses
                .clone()
                .iter()
                .map(|s| self.render_card(s, cx))
                .collect()
        } else {
            Vec::new()
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().font_semibold().child(t("manage.launchers.title")))
                            .child(div().text_sm().text_color(muted).child(if cfg!(windows) {
                                t("manage.launchers.description_windows")
                            } else {
                                t("manage.launchers.description")
                            })),
                    )
                    .child(
                        Switch::new("launchers-enabled")
                            .label(t("manage.launchers.enable"))
                            .checked(enabled)
                            .on_change(|on: &bool, _, cx| {
                                let on = *on;
                                backend_op(cx, move |b| {
                                    b.settings.update_app(|s| s.third_party_launchers = on);
                                    b.bus.emit(irori_core::Event::LibraryChanged(
                                        irori_core::Runner::Launchers,
                                    ));
                                });
                            }),
                    ),
            )
            .when(enabled && !cfg!(windows), |d| {
                d.child(
                    Alert::warning("launchers-anticheat", t("manage.launchers.anticheat"))
                        .title(t("manage.launchers.anticheat_title")),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(div().text_sm().child(t("manage.launchers.wine")))
                        .child(
                            div()
                                .w(rems(18.))
                                .child(Select::new(&self.wine_select).small()),
                        ),
                )
            })
            .when(enabled, |d| {
                d.child(h_flex().flex_wrap().gap_4().items_start().children(cards))
            })
    }
}
