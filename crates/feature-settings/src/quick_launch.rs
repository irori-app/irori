//! The quick launcher's row in General → Tray and window: whether its
//! global shortcut works and with which keys, and, where Irori can't listen
//! for keys itself, the lines that bind `irori --quick-launch` in the user's
//! compositor, or the command to bind in the desktop's keyboard settings.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, v_flex};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div,
};
use irori_services::QuickLaunchShortcut;
use irori_services::replay::bindings::{Binding, Program, QUICK_LAUNCH_FLAG};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::muted;

pub(crate) struct QuickLaunchPanel {
    state: QuickLaunchShortcut,
    /// The desktop is being asked for the shortcut.
    asking: bool,
    /// The lines were just copied.
    copied: bool,
    _subscriptions: Vec<Subscription>,
}

impl QuickLaunchPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            state: backend(cx).quick_launch_shortcut(),
            asking: false,
            copied: false,
            _subscriptions: vec![cx.observe(&settings, |this, _, cx| this.follow(cx))],
        };
        this.follow(cx);
        this
    }

    /// Ask for the shortcut once the setting is on, and show what the
    /// desktop answered.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let wanted = models(cx).settings.read(cx).app.ui.quick_launch_shortcut;
        self.state = backend(cx).quick_launch_shortcut();
        let asked = self.state != QuickLaunchShortcut::Off;
        if !wanted || asked || self.asking || !cfg!(target_os = "linux") {
            cx.notify();
            return;
        }
        self.asking = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            b.listen_for_clip_shortcut().await;
            b.quick_launch_shortcut()
        });
        cx.spawn(async move |this, cx| {
            let state = task.await.unwrap_or(QuickLaunchShortcut::Unavailable);
            let _ = this.update(cx, |this, cx| {
                this.state = state;
                this.asking = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn copy(&mut self, lines: String, window: &mut Window, cx: &mut Context<Self>) {
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

    #[cfg(test)]
    pub(crate) fn set_state(&mut self, state: QuickLaunchShortcut, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }

    /// The lines in a code box, with a button that copies them.
    fn lines(&self, lines: String, cx: &mut Context<Self>) -> AnyElement {
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
                lines
                    .lines()
                    .map(|line| div().min_w_0().child(line.to_string())),
            );
        v_flex()
            .gap_1()
            .items_start()
            .child(code)
            .child(
                Button::new("quick-launch-copy")
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
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.copy(lines.clone(), window, cx)
                        }),
                    ),
            )
            .into_any_element()
    }
}

impl Render for QuickLaunchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let command = Program::current().shell(QUICK_LAUNCH_FLAG);
        let wanted = models(cx).settings.read(cx).app.ui.quick_launch_shortcut;
        let (text, lines) = match self.state.clone() {
            _ if self.asking => (t("settings.quick_launch.asking"), None),
            QuickLaunchShortcut::Keys(Some(keys)) if !keys.is_empty() => (
                t_args("settings.quick_launch.keys", &[("keys", &keys)]),
                None,
            ),
            QuickLaunchShortcut::Keys(Some(_)) => (t("settings.quick_launch.ready"), None),
            QuickLaunchShortcut::Keys(None) => (t("settings.quick_launch.unassigned"), None),
            QuickLaunchShortcut::Commands(Binding {
                compositor: Some(c),
                lines,
            }) => (
                t_args(
                    "settings.quick_launch.commands_file",
                    &[("desktop", c.name()), ("file", c.config_file())],
                ),
                Some(lines),
            ),
            QuickLaunchShortcut::Commands(Binding { lines, .. }) => {
                (t("settings.quick_launch.command"), Some(lines))
            }
            QuickLaunchShortcut::Unavailable | QuickLaunchShortcut::Off if wanted => {
                (t("settings.quick_launch.command"), Some(command))
            }
            QuickLaunchShortcut::Unavailable | QuickLaunchShortcut::Off => {
                (t("settings.quick_launch.off"), Some(command))
            }
        };
        v_flex()
            .w_full()
            .gap_1()
            .child(muted(text, cx))
            .children(lines.map(|lines| self.lines(lines, cx)))
    }
}
