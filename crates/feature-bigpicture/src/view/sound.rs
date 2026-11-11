//! The sound output from Big Picture outside a game: an entry of the quick
//! menu (Confirm and Right switch to the next output, Left to the previous)
//! and a control in the top bar, both through the output switching the
//! in-game overlay uses ([`irori_services::Backend::audio_outputs`]). The
//! outputs are read when Big Picture opens and whenever the menu opens
//! (headphones come and go), never polled.

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, rems,
};
use irori_services::audio::{AudioOutput, SWITCHES_OUTPUT};
use irori_ui::i18n::{t, t_args};

use super::{BigPictureView, Colors};
use crate::model::MenuItem;
use crate::overlay::model::next_output;

/// The sound outputs as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum Outputs {
    #[default]
    Loading,
    Ready(Vec<AudioOutput>),
    /// They couldn't be read (the reason), or this system can't switch them.
    Failed(SharedString),
}

/// The output in use, as the menu and top bar name it.
pub(super) fn output_name(outputs: &Outputs) -> SharedString {
    match outputs {
        Outputs::Loading => "…".into(),
        Outputs::Ready(list) => list
            .iter()
            .find(|o| o.current)
            .map(|o| SharedString::from(o.name.clone()))
            .unwrap_or_else(|| t("bigpicture.overlay.output_unavailable")),
        Outputs::Failed(_) => t("bigpicture.overlay.output_unavailable"),
    }
}

impl BigPictureView {
    /// Read the sound outputs again.
    pub(super) fn refresh_outputs(&mut self, cx: &mut Context<Self>) {
        if !SWITCHES_OUTPUT {
            self.outputs = Outputs::Failed(t("bigpicture.sound.unsupported"));
            return;
        }
        if cfg!(test) {
            // Tests set the outputs themselves: they never read, let alone
            // switch, the real computer's.
            return;
        }
        let b = irori_ui::backend(cx);
        let task = irori_ui::run(cx, async move { b.audio_outputs().await });
        cx.spawn(async move |this, cx| {
            let outputs = match task.await {
                Ok(list) => Outputs::Ready(list),
                Err(e) => Outputs::Failed(format!("{e:#}").into()),
            };
            let _ = this.update(cx, |this, cx| {
                if this.outputs != outputs {
                    this.outputs = outputs;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The quick menu's label for `item`: the sound output names the one in
    /// use.
    pub(super) fn menu_label(&self, item: MenuItem) -> SharedString {
        match item {
            MenuItem::SoundOutput => t_args(
                "bigpicture.menu.sound_output",
                &[("output", output_name(&self.outputs).as_str())],
            ),
            _ => item.label(),
        }
    }

    /// Send sound to the output `step` places from the one in use, then read
    /// the outputs again. Says why when there is nothing to switch to.
    pub(super) fn switch_output(&mut self, step: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.switching_output {
            return;
        }
        let next = match &self.outputs {
            Outputs::Loading => return,
            Outputs::Failed(reason) => {
                window.push_notification(
                    Notification::warning(reason.clone()).title(t("bigpicture.error.sound_output")),
                    cx,
                );
                return;
            }
            Outputs::Ready(list) => match next_output(list, step) {
                Some(next) => next.id.clone(),
                None => {
                    window
                        .push_notification(Notification::info(t("bigpicture.sound.only_one")), cx);
                    return;
                }
            },
        };
        self.switching_output = true;
        cx.notify();
        let b = irori_ui::backend(cx);
        let task = irori_ui::run(cx, async move {
            b.set_system_audio_output(&next).await?;
            b.audio_outputs().await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.switching_output = false;
                match result {
                    Ok(list) => this.outputs = Outputs::Ready(list),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("bigpicture.error.sound_output")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The top bar's sound output: the one in use; a click switches to the
    /// next. Left out where outputs can't be switched or weren't read (the
    /// quick menu's entry says why).
    pub(super) fn render_output_control(
        &self,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Outputs::Ready(list) = &self.outputs else {
            return None;
        };
        if list.is_empty() {
            return None;
        }
        let several = list.len() > 1;
        Some(
            h_flex()
                .id("bp-sound-output")
                .role(Role::Button)
                .aria_label(self.menu_label(MenuItem::SoundOutput))
                .min_w_0()
                .max_w(rems(16.))
                .gap_2()
                .text_color(c.muted_foreground)
                .child(Icon::new(IconName::Volume2))
                .child(div().min_w_0().truncate().child(output_name(&self.outputs)))
                .when(several, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.text_color(c.foreground))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_output(1, window, cx);
                            this.focus.focus(window, cx);
                        }))
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(id: &str, current: bool) -> AudioOutput {
        AudioOutput {
            id: id.into(),
            name: format!("{id} speakers"),
            current,
        }
    }

    #[test]
    fn names_the_output_in_use() {
        crate::register_catalogs();
        let ready = Outputs::Ready(vec![out("a", false), out("b", true)]);
        assert_eq!(output_name(&ready).as_str(), "b speakers");
        let none_current = Outputs::Ready(vec![out("a", false)]);
        assert_eq!(
            output_name(&none_current),
            t("bigpicture.overlay.output_unavailable")
        );
        assert_eq!(
            output_name(&Outputs::Failed("pactl is missing".into())),
            t("bigpicture.overlay.output_unavailable")
        );
        assert_eq!(output_name(&Outputs::Loading).as_str(), "…");
    }

    #[test]
    fn switches_where_the_overlay_does() {
        assert_eq!(
            SWITCHES_OUTPUT,
            crate::overlay::support::Support::current().available,
            "Big Picture offers the overlay's output switching on the same systems"
        );
    }
}
