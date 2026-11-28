//! First-run controller-first welcome flow for Big Picture / Game Mode and
//! controller-navigable empty library actions.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, rems,
};
use irori_core::{Paths, json_store};
use irori_ui::components::{BrandMark, BrandWordmark};
use irori_ui::i18n::{LANGUAGES, resolve_locale, t, t_args};
use irori_ui::{Route, backend, models, navigate};

use super::chrome::{active_kind, glyph};
use super::{BigPictureView, Colors};
use crate::gamepad::GamepadInput;
use crate::layout::Metrics;

fn marker_file() -> PathBuf {
    Paths::get().state_dir().join("bigpicture-welcome-done")
}

/// Whether Big Picture should greet the user with the 2-step controller
/// welcome flow.
pub(crate) fn should_show_welcome() -> bool {
    !marker_file().exists()
}

/// Record that Big Picture's welcome flow was completed or skipped.
pub(crate) fn mark_welcome_done() -> anyhow::Result<()> {
    json_store::write_bytes(&marker_file(), b"done\n")
}

/// Exit Big Picture into Desktop setup: onboarding if never completed and no
/// store account is connected yet, else the Accounts page.
pub(crate) fn open_desktop_setup(cx: &mut App) {
    let onboarding_done = Paths::get().state_dir().join("onboarding-done").exists();
    let any_logged_in = models(cx).accounts.read(cx).any_logged_in();
    let target = if !onboarding_done && !any_logged_in {
        Route::Onboarding
    } else {
        Route::Accounts
    };
    navigate(target, cx);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WelcomeStep {
    Intro,
    Sources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WelcomeState {
    pub step: WelcomeStep,
    /// Focused row inside the current step.
    pub ix: usize,
}

impl Default for WelcomeState {
    fn default() -> Self {
        Self {
            step: WelcomeStep::Intro,
            // Start on Continue so users whose OS language was already detected
            // move forward in one press; Up focuses the language selector.
            ix: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceAction {
    AddRoms,
    Start,
    DesktopSetup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmptyAction {
    AddRoms,
    Emulation,
    DesktopSetup,
    Exit,
}

fn language_index(code: &str) -> usize {
    let resolved = resolve_locale(code);
    LANGUAGES
        .iter()
        .position(|l| l.code == resolved)
        .unwrap_or(0)
}

fn cycle_language(current: &str, delta: isize) -> &'static str {
    let len = LANGUAGES.len() as isize;
    let ix = language_index(current) as isize;
    let next = (ix + delta).rem_euclid(len) as usize;
    LANGUAGES[next].code
}

impl BigPictureView {
    /// Probe common ROM folders off the UI thread so Welcome Step 2 and the
    /// Empty state can offer 1-click ROM import without blocking rendering.
    pub(super) fn probe_rom_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let (_, plan) = b.rom_sources().await;
            let folders: Vec<PathBuf> = plan.add.into_iter().map(|(_, path)| path).collect();
            Ok(folders)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(folders) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.found_roms = folders;
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_welcome(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = mark_welcome_done() {
            tracing::warn!("saving Big Picture welcome state: {e:#}");
        }
        self.welcome = None;
        cx.notify();
    }

    fn set_language_delta(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let current = models(cx).settings.read(cx).app.ui_language.clone();
        let next = cycle_language(&current, delta).to_string();
        irori_ui::i18n::set_language(&next);
        let b = backend(cx);
        let worker = b.clone();
        drop(b.spawn_blocking(move || {
            worker.settings.update_app(|s| s.ui_language = next);
        }));
        window.refresh();
        cx.notify();
    }

    fn add_found_roms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.adding_roms || self.found_roms.is_empty() {
            return;
        }
        let folders = self.found_roms.clone();
        let count = folders.len();
        self.adding_roms = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.add_rom_folders(folders).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.adding_roms = false;
                match result {
                    Ok(()) => {
                        this.found_roms.clear();
                        let msg = if count == 1 {
                            t("bigpicture.welcome.roms_added_one")
                        } else {
                            t_args(
                                "bigpicture.welcome.roms_added_other",
                                &[("count", &count.to_string())],
                            )
                        };
                        window.push_notification(Notification::success(msg), cx);
                    }
                    Err(e) => {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("bigpicture.welcome.roms_failed")),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn source_actions(&self, cx: &App) -> Vec<SourceAction> {
        let has_games = !models(cx).library.read(cx).snapshot.games.is_empty();
        let mut actions = Vec::with_capacity(3);
        if !self.found_roms.is_empty() {
            actions.push(SourceAction::AddRoms);
        }
        if has_games {
            actions.push(SourceAction::Start);
            actions.push(SourceAction::DesktopSetup);
        } else {
            actions.push(SourceAction::DesktopSetup);
            actions.push(SourceAction::Start);
        }
        actions
    }

    fn empty_actions(&self) -> Vec<EmptyAction> {
        let mut actions = Vec::with_capacity(3);
        if !self.found_roms.is_empty() {
            actions.push(EmptyAction::AddRoms);
        } else {
            actions.push(EmptyAction::Emulation);
        }
        actions.push(EmptyAction::DesktopSetup);
        actions.push(EmptyAction::Exit);
        actions
    }

    fn run_source_action(
        &mut self,
        action: SourceAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SourceAction::AddRoms => self.add_found_roms(window, cx),
            SourceAction::Start => self.finish_welcome(cx),
            SourceAction::DesktopSetup => {
                self.finish_welcome(cx);
                open_desktop_setup(cx);
            }
        }
    }

    fn run_empty_action(
        &mut self,
        action: EmptyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            EmptyAction::AddRoms => self.add_found_roms(window, cx),
            EmptyAction::Emulation => {
                crate::exit_big_picture(cx);
                irori_ui::hooks::open_emulation_setup(window, cx);
            }
            EmptyAction::DesktopSetup => open_desktop_setup(cx),
            EmptyAction::Exit => crate::exit_big_picture(cx),
        }
    }

    pub(super) fn welcome_input(
        &mut self,
        input: GamepadInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut state) = self.welcome else {
            return;
        };
        match state.step {
            WelcomeStep::Intro => match input {
                GamepadInput::Up => {
                    state.ix = state.ix.saturating_sub(1);
                    self.welcome = Some(state);
                    cx.notify();
                }
                GamepadInput::Down => {
                    state.ix = (state.ix + 1).min(1);
                    self.welcome = Some(state);
                    cx.notify();
                }
                GamepadInput::Left if state.ix == 0 => self.set_language_delta(-1, window, cx),
                GamepadInput::Right if state.ix == 0 => self.set_language_delta(1, window, cx),
                GamepadInput::Confirm => {
                    if state.ix == 0 {
                        state.ix = 1;
                        self.welcome = Some(state);
                        cx.notify();
                    } else {
                        self.welcome = Some(WelcomeState {
                            step: WelcomeStep::Sources,
                            ix: 0,
                        });
                        cx.notify();
                    }
                }
                GamepadInput::Back => self.finish_welcome(cx),
                _ => {}
            },
            WelcomeStep::Sources => {
                let actions = self.source_actions(cx);
                let max = actions.len().saturating_sub(1);
                state.ix = state.ix.min(max);
                match input {
                    GamepadInput::Up => {
                        state.ix = state.ix.saturating_sub(1);
                        self.welcome = Some(state);
                        cx.notify();
                    }
                    GamepadInput::Down => {
                        state.ix = (state.ix + 1).min(max);
                        self.welcome = Some(state);
                        cx.notify();
                    }
                    GamepadInput::Confirm => {
                        if let Some(&action) = actions.get(state.ix) {
                            self.run_source_action(action, window, cx);
                        }
                    }
                    GamepadInput::Back => {
                        self.welcome = Some(WelcomeState::default());
                        cx.notify();
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn empty_input(
        &mut self,
        input: GamepadInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let actions = self.empty_actions();
        let max = actions.len().saturating_sub(1);
        self.empty_ix = self.empty_ix.min(max);
        match input {
            GamepadInput::Up => {
                self.empty_ix = self.empty_ix.saturating_sub(1);
                cx.notify();
            }
            GamepadInput::Down => {
                self.empty_ix = (self.empty_ix + 1).min(max);
                cx.notify();
            }
            GamepadInput::Confirm | GamepadInput::Alt => {
                if let Some(&action) = actions.get(self.empty_ix) {
                    self.run_empty_action(action, window, cx);
                }
            }
            GamepadInput::Back => crate::exit_big_picture(cx),
            _ => {}
        }
    }

    fn control_card(
        inputs: &[GamepadInput],
        label: SharedString,
        c: Colors,
        cx: &App,
    ) -> AnyElement {
        let kind = active_kind(cx);
        h_flex()
            .flex_1()
            .min_w(rems(16.))
            .p_3()
            .gap_3()
            .items_center()
            .rounded(c.radius_lg)
            .border_1()
            .border_color(c.border)
            .bg(c.background.opacity(0.65))
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .children(inputs.iter().map(|i| glyph(*i, kind, c, cx))),
            )
            .child(div().flex_1().min_w_0().text_sm().child(label))
            .into_any_element()
    }

    fn render_welcome_intro(&self, ix: usize, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let lang_code = models(cx).settings.read(cx).app.ui_language.clone();
        let lang_name = LANGUAGES[language_index(&lang_code)].native_name;
        let lang_focused = ix == 0;
        let lang_row = h_flex()
            .id("bp-welcome-language")
            .role(Role::MenuItem)
            .aria_selected(lang_focused)
            .w_full()
            .h(rems(3.))
            .px_4()
            .gap_3()
            .items_center()
            .rounded(c.radius_lg)
            .cursor_pointer()
            .map(|d| {
                if lang_focused {
                    d.bg(c.foreground).text_color(c.background)
                } else {
                    d.border_1()
                        .border_color(c.border)
                        .bg(c.background.opacity(0.5))
                        .text_color(c.foreground)
                }
            })
            .child(Icon::new(IconName::Globe))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_medium()
                    .child(t("bigpicture.welcome.language")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("bp-welcome-lang-prev")
                            .cursor_pointer()
                            .p_1()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_language_delta(-1, window, cx);
                            }))
                            .child(Icon::new(IconName::ChevronLeft).small()),
                    )
                    .child(div().font_semibold().child(lang_name))
                    .child(
                        div()
                            .id("bp-welcome-lang-next")
                            .cursor_pointer()
                            .p_1()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_language_delta(1, window, cx);
                            }))
                            .child(Icon::new(IconName::ChevronRight).small()),
                    ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(w) = &mut this.welcome {
                    w.ix = 0;
                    cx.notify();
                }
            }));

        let controls = v_flex()
            .gap_2p5()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2p5()
                    .child(Self::control_card(
                        &[GamepadInput::PrevPage, GamepadInput::NextPage],
                        t("bigpicture.welcome.controls.tabs"),
                        c,
                        cx,
                    ))
                    .child(Self::control_card(
                        &[GamepadInput::Menu],
                        t("bigpicture.welcome.controls.menu"),
                        c,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2p5()
                    .child(Self::control_card(
                        &[GamepadInput::Guide],
                        t("bigpicture.welcome.controls.guide"),
                        c,
                        cx,
                    ))
                    .child(Self::control_card(
                        &[GamepadInput::Confirm, GamepadInput::Back],
                        t("bigpicture.welcome.controls.play"),
                        c,
                        cx,
                    )),
            );

        let next_row = Self::overlay_row(
            "bp-welcome-continue".into(),
            IconName::ArrowRight,
            t("bigpicture.welcome.continue"),
            ix == 1,
            false,
            false,
            c,
            |this, _, cx| {
                this.welcome = Some(WelcomeState {
                    step: WelcomeStep::Sources,
                    ix: 0,
                });
                cx.notify();
            },
            cx,
        );

        v_flex()
            .gap_5()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_3xl()
                            .font_extrabold()
                            .child(t("bigpicture.welcome.title")),
                    )
                    .child(
                        div()
                            .text_base()
                            .text_color(c.muted_foreground)
                            .child(t("bigpicture.welcome.intro_body")),
                    ),
            )
            .child(controls)
            .child(v_flex().gap_2().child(lang_row).child(next_row))
            .into_any_element()
    }

    fn render_welcome_sources(&self, ix: usize, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let game_count = models(cx).library.read(cx).snapshot.games.len();
        let summary = match game_count {
            0 => t("bigpicture.welcome.no_games"),
            1 => t("bigpicture.welcome.games_ready_one"),
            n => t_args(
                "bigpicture.welcome.games_ready_other",
                &[("count", &n.to_string())],
            ),
        };
        let actions = self.source_actions(cx);
        let ix = ix.min(actions.len().saturating_sub(1));
        let rom_count = self.found_roms.len();
        let rows: Vec<AnyElement> = actions
            .into_iter()
            .enumerate()
            .map(|(i, action)| {
                let (id, icon, label, loading) = match action {
                    SourceAction::AddRoms => (
                        "bp-welcome-add-roms",
                        IconName::Joystick,
                        if rom_count == 1 {
                            t("bigpicture.welcome.add_roms_one")
                        } else {
                            t_args(
                                "bigpicture.welcome.add_roms_other",
                                &[("count", &rom_count.to_string())],
                            )
                        },
                        self.adding_roms,
                    ),
                    SourceAction::Start => (
                        "bp-welcome-start",
                        IconName::Play,
                        t("bigpicture.welcome.start"),
                        false,
                    ),
                    SourceAction::DesktopSetup => (
                        "bp-welcome-desktop",
                        IconName::Monitor,
                        t("bigpicture.welcome.desktop_setup"),
                        false,
                    ),
                };
                Self::overlay_row(
                    id.into(),
                    icon,
                    label,
                    i == ix,
                    false,
                    loading,
                    c,
                    move |this, window, cx| this.run_source_action(action, window, cx),
                    cx,
                )
            })
            .collect();

        v_flex()
            .gap_5()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_3xl()
                            .font_extrabold()
                            .child(t("bigpicture.welcome.sources_title")),
                    )
                    .child(
                        div()
                            .text_base()
                            .text_color(c.muted_foreground)
                            .child(summary),
                    ),
            )
            .child(v_flex().gap_2().children(rows))
            .into_any_element()
    }

    pub(super) fn render_welcome(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.welcome?;
        let step_num = match state.step {
            WelcomeStep::Intro => "1",
            WelcomeStep::Sources => "2",
        };
        let body = match state.step {
            WelcomeStep::Intro => self.render_welcome_intro(state.ix, c, cx),
            WelcomeStep::Sources => self.render_welcome_sources(state.ix, c, cx),
        };
        let width = 46.0_f32.min(m.width - 2.0 * m.gutter);
        let card = v_flex()
            .id("bp-welcome")
            .role(Role::Dialog)
            .aria_label(t("bigpicture.welcome.title"))
            .w(rems(width))
            .p_8()
            .gap_5()
            .rounded(c.radius_lg)
            .bg(c.popover)
            .border_1()
            .border_color(c.border)
            .shadow_lg()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2p5()
                            .child(BrandMark::new(rems(2.25)))
                            .child(BrandWordmark::new(rems(1.35), c.foreground)),
                    )
                    .child(div().text_sm().text_color(c.muted_foreground).child(t_args(
                        "bigpicture.welcome.step",
                        &[("current", step_num), ("total", "2")],
                    ))),
            )
            .child(body);
        Some(
            div()
                .id("bp-welcome-layer")
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(rems(m.gutter))
                .child(irori_ui::motion::fade_in("bp-welcome-in", card))
                .into_any_element(),
        )
    }

    pub(super) fn render_empty_screen(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let refreshing = models(cx).library.read(cx).refreshing;
        let actions = self.empty_actions();
        let ix = self.empty_ix.min(actions.len().saturating_sub(1));
        let rom_count = self.found_roms.len();
        let rows: Vec<AnyElement> = actions
            .into_iter()
            .enumerate()
            .map(|(i, action)| {
                let (id, icon, label, loading) = match action {
                    EmptyAction::AddRoms => (
                        "bp-empty-roms",
                        IconName::Joystick,
                        if rom_count == 1 {
                            t("bigpicture.welcome.add_roms_one")
                        } else {
                            t_args(
                                "bigpicture.welcome.add_roms_other",
                                &[("count", &rom_count.to_string())],
                            )
                        },
                        self.adding_roms,
                    ),
                    EmptyAction::Emulation => (
                        "bp-empty-emulation",
                        IconName::Joystick,
                        t("bigpicture.menu.set_up_emulation"),
                        false,
                    ),
                    EmptyAction::DesktopSetup => (
                        "bp-empty-desktop",
                        IconName::Monitor,
                        t("bigpicture.welcome.desktop_setup"),
                        false,
                    ),
                    EmptyAction::Exit => (
                        "bp-empty-exit",
                        IconName::LogOut,
                        t("bigpicture.exit"),
                        false,
                    ),
                };
                Self::overlay_row(
                    id.into(),
                    icon,
                    label,
                    i == ix,
                    false,
                    loading,
                    c,
                    move |this, window, cx| this.run_empty_action(action, window, cx),
                    cx,
                )
            })
            .collect();

        let width = 38.0_f32.min(m.width - 2.0 * m.gutter);
        let card = v_flex()
            .id("bp-empty")
            .w(rems(width))
            .p_8()
            .gap_5()
            .rounded(c.radius_lg)
            .bg(c.popover)
            .border_1()
            .border_color(c.border)
            .shadow_lg()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when(refreshing, |d| d.child(Spinner::new()))
                            .when(!refreshing, |d| {
                                d.child(
                                    Icon::new(IconName::Gamepad2)
                                        .large()
                                        .text_color(c.muted_foreground),
                                )
                            })
                            .child(div().text_2xl().font_bold().child(if refreshing {
                                t("bigpicture.empty.loading")
                            } else {
                                t("bigpicture.empty.title")
                            })),
                    )
                    .child(
                        div()
                            .text_base()
                            .text_color(c.muted_foreground)
                            .child(t("bigpicture.empty.description")),
                    ),
            )
            .child(v_flex().gap_2().children(rows));

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .px(rems(m.gutter))
            .child(card)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_languages_in_both_directions() {
        assert_eq!(cycle_language("en", 1), LANGUAGES[1].code);
        assert_eq!(
            cycle_language("en", -1),
            LANGUAGES[LANGUAGES.len() - 1].code
        );
        assert_eq!(cycle_language("pt_BR", 0), "pt_BR");
    }
}
