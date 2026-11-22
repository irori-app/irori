//! The Home tab: the volumes, the sound output and microphone, then the
//! actions, in the order [`super::super::model::home_rows`] gives them.

use super::*;

/// What a device row shows: the device in use, between the arrows that
/// switch it when there are several.
fn device_text(devices: &Load<Vec<AudioOutput>>, cx: &mut Context<OverlayView>) -> AnyElement {
    let text: SharedString = match devices {
        Load::Loading => t("bigpicture.overlay.loading"),
        Load::Ready(list) => list
            .iter()
            .find(|o| o.current)
            .map(|o| SharedString::from(o.name.clone()))
            .unwrap_or_else(|| t("bigpicture.overlay.output_unavailable")),
        Load::Failed(_) => t("bigpicture.overlay.output_unavailable"),
    };
    let several = matches!(devices, Load::Ready(list) if list.len() > 1);
    h_flex()
        .flex_shrink_1()
        .min_w_0()
        .max_w(rems(16.))
        .gap_2()
        .items_center()
        .text_color(cx.theme().muted_foreground)
        .when(several, |d| d.child(Icon::new(IconName::ChevronLeft)))
        .child(div().min_w_0().truncate().child(text))
        .when(several, |d| d.child(Icon::new(IconName::ChevronRight)))
        .into_any_element()
}

impl OverlayView {
    pub(super) fn render_home(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.kind(cx);
        let (system, game, microphone, error) = match &self.audio {
            Load::Loading => (None, None, None, None),
            Load::Ready(audio) => (audio.system, audio.game, audio.microphone, None),
            Load::Failed(e) => (None, None, None, Some(e.clone())),
        };
        let error = self.audio_error.clone().or(error);
        let loading = matches!(self.audio, Load::Loading);
        let volume = |v: Option<Volume>, empty: SharedString, cx: &mut Context<Self>| {
            let theme = cx.theme();
            let text: SharedString = match v {
                Some(v) if v.muted => t("bigpicture.overlay.muted"),
                Some(v) => format!("{}%", v.percent).into(),
                None if loading => t("bigpicture.overlay.loading"),
                None => empty,
            };
            h_flex()
                .flex_shrink_0()
                .gap_2()
                .items_center()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::Minus))
                .child(div().min_w(rems(4.)).text_center().child(text))
                .child(Icon::new(IconName::Plus))
                .into_any_element()
        };
        let muted_icon = |v: Option<Volume>| {
            if v.is_some_and(|v| v.muted) {
                IconName::VolumeX
            } else {
                IconName::Volume2
            }
        };
        // macOS has no per-program volume: say so rather than "no sound".
        let game_volume = super::super::support::Support::current().game_volume;
        let rows: Vec<AnyElement> = self
            .nav
            .home()
            .to_vec()
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let (icon, label, trailing) = match row {
                    HomeRow::SystemVolume => (
                        muted_icon(system),
                        t("bigpicture.overlay.system_volume"),
                        Some(volume(
                            system,
                            t("bigpicture.overlay.volume_unavailable"),
                            cx,
                        )),
                    ),
                    HomeRow::GameVolume => (
                        muted_icon(game),
                        t("bigpicture.overlay.game_volume"),
                        Some(if game_volume {
                            volume(game, t("bigpicture.overlay.no_game_sound"), cx)
                        } else {
                            volume(None, t("bigpicture.overlay.game_volume_unsupported"), cx)
                        }),
                    ),
                    HomeRow::Output => (
                        IconName::Speaker,
                        t("bigpicture.overlay.output"),
                        Some(device_text(&self.outputs, cx)),
                    ),
                    HomeRow::Microphone => (
                        if microphone.is_some_and(|m| m.muted) {
                            IconName::MicOff
                        } else {
                            IconName::Mic
                        },
                        t("bigpicture.overlay.microphone"),
                        Some(volume(
                            microphone,
                            t("bigpicture.overlay.output_unavailable"),
                            cx,
                        )),
                    ),
                    HomeRow::Input => (
                        IconName::Mic,
                        t("bigpicture.overlay.input"),
                        Some(device_text(&self.inputs, cx)),
                    ),
                    HomeRow::Screenshot => {
                        (IconName::Camera, t("bigpicture.overlay.screenshot"), None)
                    }
                    HomeRow::BackToIrori => {
                        (IconName::House, t("bigpicture.overlay.back_to_irori"), None)
                    }
                    HomeRow::QuitGame => (IconName::Power, t("bigpicture.overlay.quit"), None),
                };
                self.row(Tab::Home, ix, icon, label, trailing, cx)
            })
            .collect();
        let theme = cx.theme();
        v_flex()
            .id("overlay-home")
            .size_full()
            .gap_1()
            .overflow_y_scroll()
            .children(rows)
            .child(
                div()
                    .pt_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t_args(
                        "bigpicture.overlay.volume_hint",
                        &[("button", kind.glyph(GamepadInput::Confirm))],
                    )),
            )
            .when_some(error, |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(t_args(
                    "bigpicture.overlay.audio_failed",
                    &[("error", e.as_ref())],
                )))
            })
            .into_any_element()
    }

    /// Record from the microphone `step` places from the current one, then
    /// read the microphones and levels again.
    pub(super) fn switch_input(&mut self, step: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(inputs) = &self.inputs else {
            return;
        };
        let Some(next) = next_output(inputs, step) else {
            return;
        };
        let input = next.id.clone();
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let task = irori_ui::run(cx, async move {
            backend.set_audio_input(&input).await?;
            let inputs = backend.audio_inputs().await?;
            let audio = backend.audio(&id).await.ok();
            Ok((inputs, audio))
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((inputs, audio)) => {
                        this.inputs = Load::Ready(inputs);
                        if let Some(audio) = audio {
                            this.set_audio(Load::Ready(audio));
                        }
                        this.audio_error = None;
                    }
                    Err(e) => this.audio_error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        }));
    }
}
