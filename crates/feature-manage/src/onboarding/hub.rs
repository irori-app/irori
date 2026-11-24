//! "Getting started": a checklist to come back to after onboarding, from
//! the command palette, onboarding's last page and the empty library.
//! Three steps count towards its progress, each ticking itself from what
//! the library and settings already say:
//!
//! - **Your games**: the library has a game. Store logins, the other
//!   launchers found here and the usual ROM folders are offered in place.
//! - **Your look**: the layout, density, accent or theme differs from the
//!   defaults, or the user said the defaults suit them.
//! - **While you play**: one of the switches is on (in-game overlay, replay
//!   buffer, save backups after playing, staying in the tray), or the user
//!   said none is wanted.
//!
//! Tips follow and don't count. Every switch writes the setting at once,
//! as Settings does. What the user said is kept in `getting-started.json`
//! in the state folder.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::settings::{AppSettings, Density, LibraryLayout, UiPrefs};
use irori_core::{Paths, json_store};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models, navigate};
use serde::{Deserialize, Serialize};

use super::launchers::{self, Imports, Launcher, import_status};
use crate::common::{backend_op, card};

/// What the user said in the checklist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HubState {
    /// The look is as wanted, defaults included.
    #[serde(default)]
    pub look: bool,
    /// No switch for playing is wanted.
    #[serde(default)]
    pub play: bool,
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("getting-started.json")
}

/// Whether the look differs from what a new Irori shows.
pub(crate) fn look_changed(ui: &UiPrefs) -> bool {
    let defaults = UiPrefs::default();
    ui.library_layout != defaults.library_layout
        || ui.density != defaults.density
        || ui.accent_color != defaults.accent_color
        || ui.theme_name != defaults.theme_name
        || ui.theme != defaults.theme
}

/// Whether one of the switches for playing is on.
pub(crate) fn play_switched(app: &AppSettings) -> bool {
    app.overlay.enabled
        || (cfg!(target_os = "linux") && app.game.replay.enabled)
        || app.game.ludusavi_auto_backup
        || app.exit_to_tray
}

/// Steps done and steps in all.
pub(crate) fn progress(has_games: bool, app: &AppSettings, state: HubState) -> (usize, usize) {
    let done = [
        has_games,
        state.look || look_changed(&app.ui),
        state.play || play_switched(app),
    ];
    (done.iter().filter(|d| **d).count(), done.len())
}

/// Open the checklist.
pub fn open_getting_started(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let view = cx.new(|cx| GettingStarted::new(window, cx));
    let width = window.rem_size() * 40.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("manage.hub.title"))
            .width(width)
            .child(view.clone())
    });
}

struct GettingStarted {
    state: HubState,
    launchers: Imports,
    _subscriptions: Vec<Subscription>,
}

impl GettingStarted {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let b = irori_ui::backend(cx);
        let read = irori_ui::run(cx, async move {
            Ok(
                b.spawn_blocking(|| json_store::read_or_default::<HubState>(&state_file()))
                    .await?,
            )
        });
        let found = launchers::detect_in_background(cx);
        cx.spawn_in(window, async move |this, cx| {
            let state = read.await.unwrap_or_default();
            let found = found.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.state = state;
                this.launchers = Imports::new(&found);
                cx.notify();
            });
        })
        .detach();
        Self {
            state: HubState::default(),
            launchers: Imports::default(),
            _subscriptions: vec![
                cx.observe(&m.library, |_, _, cx| cx.notify()),
                cx.observe(&m.settings, |_, _, cx| cx.notify()),
                cx.observe(&m.accounts, |_, _, cx| cx.notify()),
            ],
        }
    }

    fn remember(&mut self, change: impl FnOnce(&mut HubState), cx: &mut Context<Self>) {
        change(&mut self.state);
        let state = self.state;
        backend_op(cx, move |_| {
            if let Err(e) = json_store::write(&state_file(), &state) {
                tracing::warn!("saving Getting started: {e:#}");
            }
        });
        cx.notify();
    }

    fn run_import(&mut self, launcher: Launcher, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.launchers.start(launcher, cx) else {
            return;
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Ok(summary) = &result {
                    irori_ui::models::refresh_library(false, cx);
                    irori_ui::lutris_summary::open_lutris_summary(summary.clone(), window, cx);
                }
                this.launchers.finish(launcher, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn step(
        &self,
        id: &'static str,
        done: bool,
        title: SharedString,
        description: SharedString,
        body: AnyElement,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        card(cx)
            .id(id)
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        Icon::new(if done {
                            IconName::CircleCheck
                        } else {
                            IconName::Circle
                        })
                        .text_color(if done {
                            theme.success
                        } else {
                            theme.muted_foreground
                        }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().font_semibold().child(title))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(description),
                            ),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    fn render_games(&self, done: bool, cx: &mut Context<Self>) -> AnyElement {
        let any_logged_in = models(cx).accounts.read(cx).any_logged_in();
        let found: Vec<(Launcher, launchers::ImportState)> = self.launchers.entries.clone();
        let imports: Vec<AnyElement> = found
            .into_iter()
            .map(|(launcher, state)| {
                let view = cx.entity().downgrade();
                h_flex()
                    .gap_3()
                    .child(div().flex_1().min_w_0().text_sm().child(t_args(
                        "manage.onboarding.launcher.title",
                        &[("launcher", launcher.name())],
                    )))
                    .child(import_status(launcher, &state, move |window, cx| {
                        let _ = view.update(cx, |this, cx| this.run_import(launcher, window, cx));
                    }))
                    .into_any_element()
            })
            .collect();
        let body = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("hub-accounts")
                            .small()
                            .when(!any_logged_in, |b| b.primary())
                            .icon(IconName::LogIn)
                            .label(t("manage.hub.games.log_in"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::Accounts, cx);
                            }),
                    )
                    .child(
                        Button::new("hub-roms")
                            .small()
                            .icon(IconName::Joystick)
                            .label(t("manage.hub.games.roms"))
                            .on_click(|_, window, cx| {
                                crate::emulators::open_rom_sources_dialog(window, cx)
                            }),
                    ),
            )
            .children(imports)
            .into_any_element();
        self.step(
            "hub-games",
            done,
            t("manage.hub.games.title"),
            t("manage.hub.games.description"),
            body,
            cx,
        )
    }

    fn render_look(&self, done: bool, cx: &mut Context<Self>) -> AnyElement {
        let ui = models(cx).settings.read(cx).app.ui.clone();
        let layout = h_flex().gap_1().children(
            [
                (
                    LibraryLayout::Grid,
                    t("manage.hub.look.grid"),
                    IconName::LayoutGrid,
                ),
                (
                    LibraryLayout::List,
                    t("manage.hub.look.list"),
                    IconName::List,
                ),
                (
                    LibraryLayout::Focus,
                    t("manage.hub.look.focus"),
                    IconName::PanelLeft,
                ),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (value, text, icon))| {
                Button::new(("hub-layout", ix))
                    .small()
                    .icon(icon)
                    .label(text)
                    .when(ui.library_layout == value, |b| b.primary())
                    .when(ui.library_layout != value, |b| b.ghost())
                    .on_click(move |_, _, cx| {
                        backend_op(cx, move |b| {
                            b.settings.update_app(|s| s.ui.library_layout = value)
                        })
                    })
            }),
        );
        let density = h_flex().gap_1().children(
            [
                (Density::Compact, t("manage.hub.look.compact")),
                (Density::Comfortable, t("manage.hub.look.comfortable")),
                (Density::Spacious, t("manage.hub.look.spacious")),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (value, text))| {
                Button::new(("hub-density", ix))
                    .small()
                    .label(text)
                    .when(ui.density == value, |b| b.primary())
                    .when(ui.density != value, |b| b.ghost())
                    .on_click(move |_, _, cx| {
                        backend_op(cx, move |b| b.settings.update_app(|s| s.ui.density = value))
                    })
            }),
        );
        let muted = cx.theme().muted_foreground;
        let label = |text: SharedString| {
            div()
                .min_w(rems(7.))
                .text_sm()
                .text_color(muted)
                .child(text)
        };
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(label(t("manage.hub.look.layout")))
                    .child(layout),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(label(t("manage.hub.look.density")))
                    .child(density),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("hub-appearance")
                            .small()
                            .icon(IconName::Palette)
                            .label(t("manage.hub.look.appearance"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::Settings("appearance".into()), cx);
                            }),
                    )
                    .when(!done, |d| {
                        d.child(
                            Button::new("hub-look-done")
                                .small()
                                .ghost()
                                .icon(IconName::Check)
                                .label(t("manage.hub.look.keep"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remember(|s| s.look = true, cx)
                                })),
                        )
                    }),
            )
            .into_any_element();
        self.step(
            "hub-look",
            done,
            t("manage.hub.look.title"),
            t("manage.hub.look.description"),
            body,
            cx,
        )
    }

    fn render_play(&self, done: bool, cx: &mut Context<Self>) -> AnyElement {
        let app = models(cx).settings.read(cx).app.clone();
        let mut switches = v_flex().gap_2().child(
            Switch::new("hub-overlay")
                .label(t("manage.hub.play.overlay"))
                .checked(app.overlay.enabled)
                .on_change(|on: &bool, _, cx| {
                    let on = *on;
                    backend_op(cx, move |b| {
                        b.settings.update_app(|s| s.overlay.enabled = on)
                    })
                }),
        );
        if cfg!(target_os = "linux") {
            switches = switches.child(
                Switch::new("hub-replay")
                    .label(t("manage.hub.play.replay"))
                    .checked(app.game.replay.enabled)
                    .on_change(|on: &bool, _, cx| {
                        let on = *on;
                        backend_op(cx, move |b| {
                            b.settings.update_app(|s| s.game.replay.enabled = on)
                        })
                    }),
            );
        }
        switches = switches
            .child(
                Switch::new("hub-backups")
                    .label(t("manage.hub.play.backups"))
                    .checked(app.game.ludusavi_auto_backup)
                    .on_change(|on: &bool, _, cx| {
                        let on = *on;
                        backend_op(cx, move |b| {
                            b.settings.update_app(|s| s.game.ludusavi_auto_backup = on)
                        })
                    }),
            )
            .child(
                Switch::new("hub-tray")
                    .label(t("manage.hub.play.tray"))
                    .checked(app.exit_to_tray)
                    .on_change(|on: &bool, _, cx| {
                        let on = *on;
                        backend_op(cx, move |b| b.settings.update_app(|s| s.exit_to_tray = on))
                    }),
            );
        let body = v_flex()
            .gap_3()
            .child(switches)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("hub-saves")
                            .small()
                            .label(t("manage.hub.play.saves_settings"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::Settings("saves".into()), cx);
                            }),
                    )
                    .when(!done, |d| {
                        d.child(
                            Button::new("hub-play-done")
                                .small()
                                .ghost()
                                .icon(IconName::Check)
                                .label(t("manage.hub.play.none"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remember(|s| s.play = true, cx)
                                })),
                        )
                    }),
            )
            .into_any_element();
        self.step(
            "hub-play",
            done,
            t("manage.hub.play.title"),
            t("manage.hub.play.description"),
            body,
            cx,
        )
    }

    fn render_tips(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let palette = if cfg!(target_os = "macos") {
            "⌘K"
        } else {
            "Ctrl+K"
        };
        let tips = [
            (
                IconName::Search,
                t_args("manage.hub.tips.palette", &[("keys", palette)]),
            ),
            (IconName::Gamepad2, t("manage.hub.tips.big_picture")),
            (IconName::Flag, t("manage.hub.tips.status")),
            (IconName::NotebookPen, t("manage.hub.tips.journal")),
            (IconName::ChartColumn, t("manage.hub.tips.stats")),
        ];
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(div().font_semibold().child(t("manage.hub.tips.title")))
                    .child(
                        Button::new("hub-tour")
                            .ghost()
                            .small()
                            .icon(IconName::Compass)
                            .label(t("manage.hub.tour"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                irori_ui::tour::start(window, cx);
                            }),
                    ),
            )
            .children(tips.into_iter().map(|(icon, text)| {
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(Icon::new(icon).small().text_color(muted))
                    .child(div().flex_1().min_w_0().text_sm().child(text))
            }))
            .into_any_element()
    }
}

impl Render for GettingStarted {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_games = !models(cx).library.read(cx).snapshot.is_empty();
        let app = models(cx).settings.read(cx).app.clone();
        let (done, total) = progress(has_games, &app, self.state);
        let look = self.state.look || look_changed(&app.ui);
        let play = self.state.play || play_switched(&app);
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t_args(
                                "manage.hub.progress",
                                &[("done", &done.to_string()), ("total", &total.to_string())],
                            )),
                    )
                    .child(Progress::new("hub-progress").value(done as f32 * 100. / total as f32)),
            )
            .child(
                div()
                    .id("hub-steps")
                    .max_h(rems(32.))
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .gap_3()
                            .child(self.render_games(has_games, cx))
                            .child(self.render_look(look, cx))
                            .child(self.render_play(play, cx))
                            .child(self.render_tips(cx)),
                    ),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("hub-close")
                        .primary()
                        .label(t("manage.hub.close"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_tick_from_the_library_and_settings() {
        let mut app = AppSettings::default();
        // A new Irori with an empty library.
        assert_eq!(progress(false, &app, HubState::default()), (0, 3));
        assert_eq!(progress(true, &app, HubState::default()), (1, 3));

        app.ui.density = Density::Compact;
        assert!(look_changed(&app.ui));
        assert_eq!(progress(true, &app, HubState::default()), (2, 3));

        app.exit_to_tray = true;
        assert!(play_switched(&app));
        assert_eq!(progress(true, &app, HubState::default()), (3, 3));
    }

    #[test]
    fn saying_the_defaults_suit_counts_too() {
        let app = AppSettings::default();
        assert!(!look_changed(&app.ui));
        assert!(!play_switched(&app));
        let said = HubState {
            look: true,
            play: true,
        };
        assert_eq!(progress(false, &app, said), (2, 3));
        // The file keeps what was said, and an older or empty one reads.
        let text = serde_json::to_string(&said).unwrap();
        assert_eq!(serde_json::from_str::<HubState>(&text).unwrap(), said);
        assert_eq!(
            serde_json::from_str::<HubState>("{}").unwrap(),
            HubState::default()
        );
    }
}
