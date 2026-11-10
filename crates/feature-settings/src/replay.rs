//! The replay buffer panel of Performance → Replay buffer: whether
//! gpu-screen-recorder is installed (and how to install it), every way to
//! save a clip with the keyboard shortcuts' state (and, where Irori can't
//! listen for keys, the lines that bind `irori --save-clip` and
//! `irori --screenshot` in the user's compositor), and the memory the buffer
//! takes with the settings below it.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_services::{Recorder, ShortcutState};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{muted, panel};

/// Where gpu-screen-recorder is published.
const FLATHUB_URL: &str = "https://flathub.org/apps/com.dec05eba.gpu_screen_recorder";

pub(crate) struct ReplayPanel {
    /// The game whose settings are shown; `None` for the defaults.
    game: Option<GameId>,
    /// `None` until looked for; then the recorder, if any.
    recorder: Option<Option<Recorder>>,
    shortcut: ShortcutState,
    asking: bool,
    /// The binding's lines were just copied.
    copied: bool,
    _subscriptions: Vec<Subscription>,
}

impl ReplayPanel {
    pub(crate) fn new(game: Option<GameId>, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            game,
            recorder: None,
            shortcut: backend(cx).clip_shortcut(),
            asking: false,
            copied: false,
            _subscriptions: vec![cx.observe(&settings, |_, _, cx| cx.notify())],
        };
        this.detect(cx);
        this
    }

    fn detect(&mut self, cx: &mut Context<Self>) {
        let task = irori_ui::state::run_infallible(cx, async { Recorder::detect() });
        cx.spawn(async move |this, cx| {
            let found = task.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.recorder = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_for_shortcut(&mut self, cx: &mut Context<Self>) {
        self.asking = true;
        cx.notify();
        let b = backend(cx);
        let task =
            irori_ui::state::run_infallible(cx, async move { b.listen_for_clip_shortcut().await });
        cx.spawn(async move |this, cx| {
            let state = task.await.unwrap_or(ShortcutState::Unavailable);
            let _ = this.update(cx, |this, cx| {
                this.shortcut = state;
                this.asking = false;
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn set_shortcut(&mut self, state: ShortcutState, cx: &mut Context<Self>) {
        self.shortcut = state;
        cx.notify();
    }

    pub(crate) fn copy_binding(
        &mut self,
        lines: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(lines));
        self.copied = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// The keyboard shortcuts' line: their keys, or how to get them.
    fn render_keyboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let keys = |keys: &Option<String>, named: &str, ready: &str, missing: &str| match keys {
            Some(k) if !k.is_empty() => t_args(named, &[("keys", k)]),
            Some(_) => t(ready),
            None => t(missing),
        };
        match &self.shortcut {
            ShortcutState::Portal { clip, screenshot } => v_flex()
                .gap_1()
                .child(muted(
                    keys(
                        clip,
                        "settings.replay.shortcut_clip_keys",
                        "settings.replay.shortcut_clip_ready",
                        "settings.replay.shortcut_clip_unassigned",
                    ),
                    cx,
                ))
                .child(muted(
                    keys(
                        screenshot,
                        "settings.replay.shortcut_screenshot_keys",
                        "settings.replay.shortcut_screenshot_ready",
                        "settings.replay.shortcut_screenshot_unassigned",
                    ),
                    cx,
                ))
                .child(muted(t("settings.replay.shortcut_portal_change"), cx))
                .into_any_element(),
            ShortcutState::Grabbed { clip, screenshot } => muted(
                t_args(
                    "settings.replay.shortcut_grabbed",
                    &[("clip", clip), ("screenshot", screenshot)],
                ),
                cx,
            )
            .into_any_element(),
            ShortcutState::Commands(binding) => {
                let intro = match binding.compositor {
                    Some(c) => t_args(
                        "settings.replay.shortcut_commands_file",
                        &[("desktop", c.name()), ("file", c.config_file())],
                    ),
                    None => t("settings.replay.shortcut_commands_other"),
                };
                let lines = binding.lines.clone();
                let theme = cx.theme();
                let code = div()
                    .w_full()
                    .min_w_0()
                    .p_2()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .children(
                        binding
                            .lines
                            .lines()
                            .map(|line| div().min_w_0().child(line.to_string())),
                    );
                v_flex()
                    .gap_1()
                    .items_start()
                    .child(muted(intro, cx))
                    .child(code)
                    .child(
                        Button::new("replay-copy-binding")
                            .outline()
                            .xsmall()
                            .icon(if self.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .label(if self.copied {
                                t("settings.common.copied")
                            } else {
                                t("settings.replay.copy_lines")
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_binding(lines.clone(), window, cx)
                            })),
                    )
                    .into_any_element()
            }
            ShortcutState::Unavailable => {
                muted(t("settings.replay.shortcut_unavailable"), cx).into_any_element()
            }
            ShortcutState::Off => v_flex()
                .gap_1()
                .items_start()
                .child(muted(t("settings.replay.shortcut_ask"), cx))
                .child(
                    Button::new("replay-shortcut")
                        .outline()
                        .xsmall()
                        .icon(IconName::Keyboard)
                        .label(t("settings.replay.set_up_shortcuts"))
                        .loading(self.asking)
                        .disabled(self.asking)
                        .on_click(cx.listener(|this, _, _, cx| this.ask_for_shortcut(cx))),
                )
                .into_any_element(),
        }
    }

    fn render_recorder(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted_fg, success, warning) = (theme.muted_foreground, theme.success, theme.warning);
        let refresh = Button::new("replay-recorder-refresh")
            .ghost()
            .xsmall()
            .icon(IconName::RefreshCw)
            .label(t("settings.common.refresh"))
            .on_click(cx.listener(|this, _, _, cx| this.detect(cx)));
        let row = h_flex().gap_3().px_3().py_2().items_start();
        match &self.recorder {
            None => row
                .child(
                    Icon::new(IconName::LoaderCircle)
                        .small()
                        .text_color(muted_fg),
                )
                .child(muted(t("settings.replay.looking"), cx))
                .into_any_element(),
            Some(Some(recorder)) => {
                let how = match recorder {
                    Recorder::Flatpak => SharedString::from("Flatpak"),
                    Recorder::Native(path) => path.display().to_string().into(),
                };
                row.items_center()
                    .child(Icon::new(IconName::CircleCheck).small().text_color(success))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(t("settings.replay.found")),
                    )
                    .child(
                        Tag::secondary()
                            .xsmall()
                            .outline()
                            .child(div().max_w_64().truncate().child(how)),
                    )
                    .child(refresh)
                    .into_any_element()
            }
            Some(None) => row
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(warning),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_sm().child(t("settings.replay.missing")))
                        .child(muted(t("settings.replay.missing_help"), cx))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .pt_1()
                                .child(
                                    Button::new("replay-flathub")
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::ExternalLink)
                                        .label(t("settings.replay.open_flathub"))
                                        .on_click(|_, _, cx| cx.open_url(FLATHUB_URL)),
                                )
                                .child(refresh),
                        ),
                )
                .into_any_element(),
        }
    }

    fn render_ways(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted_fg = cx.theme().muted_foreground;
        let keyboard = self.render_keyboard(cx);
        let line = |icon: IconName, text: AnyElement| {
            h_flex()
                .gap_3()
                .items_start()
                .child(Icon::new(icon).small().text_color(muted_fg))
                .child(div().flex_1().min_w_0().child(text))
        };
        v_flex()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(div().text_sm().child(t("settings.replay.ways")))
            .child(line(
                IconName::Gamepad2,
                muted(t("settings.replay.way_controller"), cx).into_any_element(),
            ))
            .child(line(
                IconName::Command,
                muted(t("settings.replay.way_menus"), cx).into_any_element(),
            ))
            .child(line(IconName::Keyboard, keyboard))
            .into_any_element()
    }
}

impl Render for ReplayPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let b = backend(cx);
        let settings = match &self.game {
            Some(id) => b.settings.game(id).replay,
            None => b.settings.app().game.replay.clone(),
        };
        let memory = irori_services::replay::buffer_mib(&settings).to_string();
        let seconds = settings.seconds.to_string();
        v_flex()
            .w_full()
            .gap_2()
            .child(
                panel(cx)
                    .child(self.render_recorder(cx))
                    .child(self.render_ways(cx)),
            )
            .child(muted(
                t_args(
                    "settings.replay.memory",
                    &[("size", &memory), ("seconds", &seconds)],
                ),
                cx,
            ))
            .when(settings.enabled, |d| {
                let folder = irori_services::replay::clips_root(&settings);
                d.child(muted(
                    t_args(
                        "settings.replay.clips_in",
                        &[("folder", &folder.display().to_string())],
                    ),
                    cx,
                ))
            })
    }
}
