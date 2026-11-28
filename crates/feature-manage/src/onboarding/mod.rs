//! First-run onboarding: welcome (brand lockup, live language picker, OS-aware
//! feature highlights), unified game sources (launchers, Steam, stores, opt-in
//! stores, ROMs), install folder with detected drives, and completion leading
//! into the library, the guided tour, or Getting started ([`hub`]).

mod folder;
pub(crate) mod found;
pub(crate) mod hub;
pub(crate) mod launchers;
mod sources;
mod welcome;

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::{Paths, json_store};
use irori_ui::hooks::{self, ImportSummary};
use irori_ui::i18n::{LANGUAGES, Language, t, t_args};
use irori_ui::{Route, backend, models, navigate};

use crate::accounts::{StoreCard, store_cards};
use crate::common::{backend_op, card};
pub(crate) use folder::DriveChoice;
use launchers::{Imports, Launcher};

fn marker_file() -> PathBuf {
    Paths::get().state_dir().join("onboarding-done")
}

/// Whether the shell should show [`OnboardingView`] instead of the library:
/// no store is logged in and onboarding was never completed or skipped.
pub fn should_show_onboarding(cx: &App) -> bool {
    !models(cx).accounts.read(cx).any_logged_in() && !marker_file().exists()
}

/// Record that onboarding was completed or skipped, so it is not shown again.
pub fn mark_onboarding_done() -> anyhow::Result<()> {
    json_store::write_bytes(&marker_file(), b"done\n")
}

/// Emitted by [`OnboardingView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingEvent {
    /// The user finished or skipped onboarding; the marker is written and the
    /// navigator points at the library.
    Finished,
}

/// One page of the fixed 4-step desktop onboarding flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnboardingStep {
    Welcome,
    Sources,
    InstallFolder,
    Done,
}

/// Pages in fixed order so background detection never shifts the step count.
pub(crate) const STEPS: [OnboardingStep; 4] = [
    OnboardingStep::Welcome,
    OnboardingStep::Sources,
    OnboardingStep::InstallFolder,
    OnboardingStep::Done,
];

pub(super) enum ImportState {
    Idle,
    Running,
    Done(ImportSummary),
    Failed(SharedString),
}

/// One line per kind of data the import brought over; empty when nothing
/// needed importing.
pub(super) fn import_summary_lines(s: &ImportSummary) -> Vec<SharedString> {
    let mut lines = Vec::new();
    if !s.stores.is_empty() {
        lines.push(t_args(
            "manage.onboarding.import.summary.stores",
            &[("stores", &s.stores.join(", "))],
        ));
    }
    let count = |n: usize| n.to_string();
    match s.games_with_settings {
        0 => {}
        1 => lines.push(t("manage.onboarding.import.summary.settings_one")),
        n => lines.push(t_args(
            "manage.onboarding.import.summary.settings_other",
            &[("count", &count(n))],
        )),
    }
    match s.favourites {
        0 => {}
        1 => lines.push(t("manage.onboarding.import.summary.favourites_one")),
        n => lines.push(t_args(
            "manage.onboarding.import.summary.favourites_other",
            &[("count", &count(n))],
        )),
    }
    match s.categories {
        0 => {}
        1 => lines.push(t("manage.onboarding.import.summary.categories_one")),
        n => lines.push(t_args(
            "manage.onboarding.import.summary.categories_other",
            &[("count", &count(n))],
        )),
    }
    lines
}

/// An interface language in the welcome step's picker.
#[derive(Debug, Clone, Copy)]
struct LanguageChoice(&'static Language);

impl SelectItem for LanguageChoice {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.0.native_name.into()
    }

    fn value(&self) -> &&'static str {
        &self.0.code
    }
}

type LanguageSelect = SelectState<Vec<LanguageChoice>>;

/// Index of `code` in [`LANGUAGES`]; unknown codes select English.
fn language_index(code: &str) -> usize {
    let resolved = irori_ui::i18n::resolve_locale(code);
    LANGUAGES
        .iter()
        .position(|l| l.code == resolved)
        .unwrap_or(0)
}

/// The first-run flow. Emits [`OnboardingEvent::Finished`] when done.
pub struct OnboardingView {
    focus: FocusHandle,
    ix: usize,
    pub(super) import: ImportState,
    /// Heroic data exists (its import row shows in Sources).
    pub(super) heroic: bool,
    /// Other launchers with data here, each with its 1-click import.
    pub(super) launchers: Imports,
    pub(super) stores: Vec<StoreCard>,
    /// Whether the collapsible "More stores" section is expanded.
    pub(super) more_stores_open: bool,
    /// Addable ROM systems detected in common folders.
    pub(super) rom_systems: usize,
    pub(super) install_path: String,
    pub(super) drives: Vec<DriveChoice>,
    language: Entity<LanguageSelect>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OnboardingEvent> for OnboardingView {}

impl Focusable for OnboardingView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl OnboardingView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = Self::with_heroic(hooks::heroic_detected(), window, cx);
        let default_path = this.install_path.clone();
        // Probe launchers, common ROM folders, and writable drives off the UI
        // thread while the Welcome screen shows.
        let found_launchers = launchers::detect_in_background(cx);
        let b = backend(cx);
        let probe = irori_ui::run(cx, async move {
            let rom_systems = b.rom_sources().await.1.add.len();
            let drives = b
                .spawn_blocking(move || folder::probe_drives(default_path))
                .await
                .unwrap_or_default();
            Ok((rom_systems, drives))
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = found_launchers.await.unwrap_or_default();
            let (rom_systems, drives) = probe.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.launchers = Imports::new(&found);
                this.rom_systems = rom_systems;
                this.drives = drives;
                cx.notify();
            });
        })
        .detach();
        this
    }

    /// Construct with an explicit Heroic availability (tests).
    pub(crate) fn with_heroic(heroic: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let m = models(cx).clone();
        let current = language_index(&m.settings.read(cx).app.ui_language);
        let choices: Vec<LanguageChoice> = LANGUAGES.iter().map(LanguageChoice).collect();
        let language = cx.new(|cx| {
            SelectState::new(choices, Some(IndexPath::default().row(current)), window, cx)
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &language,
                window,
                |_, _, e: &SelectEvent<Vec<LanguageChoice>>, _, cx| {
                    let SelectEvent::Confirm(Some(code)) = e else {
                        return;
                    };
                    let code = code.to_string();
                    // The shell applies `ui_language` live when settings change.
                    backend_op(cx, move |b| b.settings.update_app(|s| s.ui_language = code));
                },
            ),
            cx.observe(&m.accounts, |this, _, cx| {
                this.stores = store_cards(cx);
                cx.notify();
            }),
            cx.observe(&m.library, |this, _, cx| {
                this.stores = store_cards(cx);
                cx.notify();
            }),
            cx.observe(&m.settings, |this, settings, cx| {
                this.install_path = settings.read(cx).app.default_install_path.clone();
                this.stores = store_cards(cx);
                cx.notify();
            }),
        ];
        Self {
            focus: cx.focus_handle(),
            ix: 0,
            import: ImportState::Idle,
            heroic,
            launchers: Imports::default(),
            stores: store_cards(cx),
            more_stores_open: false,
            rom_systems: 0,
            install_path: m.settings.read(cx).app.default_install_path.clone(),
            drives: Vec::new(),
            language,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_drives(&mut self, drives: Vec<DriveChoice>) {
        self.drives = drives;
    }

    pub(crate) fn step(&self) -> OnboardingStep {
        STEPS[self.ix]
    }

    pub(crate) fn next(&mut self, cx: &mut Context<Self>) {
        if self.ix + 1 < STEPS.len() {
            self.ix += 1;
            cx.notify();
        } else {
            self.finish(cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.ix > 0 {
            self.ix -= 1;
            cx.notify();
        }
    }

    pub(crate) fn finish(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = mark_onboarding_done() {
            tracing::warn!("saving onboarding state: {e:#}");
        }
        cx.emit(OnboardingEvent::Finished);
        navigate(Route::Library, cx);
    }

    pub(super) fn run_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.import, ImportState::Running) {
            return;
        }
        self.import = ImportState::Running;
        cx.notify();
        let task = hooks::import_from_heroic(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.import = match result {
                    Ok(summary) => {
                        irori_ui::models::refresh_library(false, cx);
                        ImportState::Done(summary)
                    }
                    Err(e) => ImportState::Failed(format!("{e:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn run_launcher_import(
        &mut self,
        launcher: Launcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("manage.onboarding.folder.prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            let _ = this.update_in(cx, |this, _, cx| {
                this.install_path = path.clone();
                cx.notify();
                backend_op(cx, move |b| {
                    b.settings.update_app(|s| s.default_install_path = path)
                });
            });
        })
        .detach();
    }

    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key.as_str() == "enter"
            && matches!(self.step(), OnboardingStep::Welcome | OnboardingStep::Done)
        {
            self.next(cx);
            cx.stop_propagation();
        }
    }

    fn render_done(&self, cx: &mut Context<Self>) -> AnyElement {
        let signed_in: Vec<&str> = self
            .stores
            .iter()
            .filter(|s| s.user.is_some())
            .map(|s| s.runner.store_name())
            .collect();
        let text = if signed_in.is_empty() {
            t("manage.onboarding.done.no_stores")
        } else {
            t_args(
                "manage.onboarding.done.stores",
                &[("stores", &signed_in.join(", "))],
            )
        };
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(t("manage.onboarding.done.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(text),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("onboarding-take-tour")
                            .outline()
                            .icon(IconName::Compass)
                            .label(t("manage.onboarding.done.take_tour"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish(cx);
                                irori_ui::tour::start(window, cx);
                            })),
                    )
                    .child(
                        Button::new("onboarding-getting-started")
                            .ghost()
                            .icon(IconName::Sparkles)
                            .label(t("manage.onboarding.done.getting_started"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish(cx);
                                hub::open_getting_started(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step();
        let content = match step {
            OnboardingStep::Welcome => self.render_welcome(cx),
            OnboardingStep::Sources => self.render_sources(window, cx),
            OnboardingStep::InstallFolder => self.render_folder(cx),
            OnboardingStep::Done => self.render_done(cx),
        };
        let last = self.ix + 1 == STEPS.len();
        let next_label = match step {
            OnboardingStep::Welcome => t("manage.onboarding.get_started"),
            OnboardingStep::Done => t("manage.onboarding.finish"),
            _ => t("manage.onboarding.continue"),
        };
        let theme = cx.theme();
        let step_bar = h_flex().gap_1p5().children((0..STEPS.len()).map(|i| {
            div().flex_1().h_1().rounded_full().bg(if i <= self.ix {
                theme.primary
            } else {
                theme.muted
            })
        }));
        div()
            .id("onboarding")
            .track_focus(&self.focus)
            .key_context("Onboarding")
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .overflow_y_scrollbar()
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(42.))
                    .px_6()
                    .py_10()
                    .gap_5()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                t_args(
                                    "manage.onboarding.step",
                                    &[
                                        ("current", &(self.ix + 1).to_string()),
                                        ("total", &STEPS.len().to_string()),
                                    ],
                                ),
                            ))
                            .child(step_bar),
                    )
                    .child(card(cx).p_6().gap_4().child(content))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("onboarding-skip")
                                    .ghost()
                                    .label(t("manage.onboarding.skip"))
                                    .when(last, |b| b.disabled(true))
                                    .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                            )
                            .child(div().flex_1())
                            .when(self.ix > 0, |this| {
                                this.child(
                                    Button::new("onboarding-back")
                                        .label(t("manage.onboarding.back"))
                                        .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                                )
                            })
                            .child(
                                Button::new("onboarding-next")
                                    .debug_selector(|| "onboarding-next".into())
                                    .primary()
                                    .label(next_label)
                                    .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_always_has_four_fixed_steps() {
        assert_eq!(
            STEPS,
            [
                OnboardingStep::Welcome,
                OnboardingStep::Sources,
                OnboardingStep::InstallFolder,
                OnboardingStep::Done,
            ]
        );
    }

    #[test]
    fn import_summary_wording() {
        crate::i18n::register();
        let s = ImportSummary {
            stores: vec!["GOG".into(), "Epic Games".into()],
            games_with_settings: 3,
            favourites: 0,
            categories: 1,
        };
        assert_eq!(
            import_summary_lines(&s),
            vec![
                SharedString::from("Accounts and installed games: GOG, Epic Games"),
                SharedString::from("Settings for 3 games"),
                SharedString::from("1 category"),
            ]
        );
        assert!(import_summary_lines(&ImportSummary::default()).is_empty());
    }

    #[test]
    fn language_index_falls_back() {
        assert_eq!(LANGUAGES[language_index("pt_BR")].code, "pt_BR");
        assert_eq!(LANGUAGES[language_index("pt-BR")].code, "pt_BR");
        assert_eq!(LANGUAGES[language_index("de_AT")].code, "de");
        assert_eq!(language_index("xx"), 0);
    }
}
