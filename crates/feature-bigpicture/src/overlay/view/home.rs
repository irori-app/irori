//! The Home tab: the volumes, the sound output and microphone, then the
//! actions, in the order [`super::super::model::home_rows`] gives them.

use super::*;
use irori_services::mangohud::MangoState;

use super::super::model::next_limit;

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
                    HomeRow::MangoHud => {
                        let on = self.mangohud.is_some_and(|m| m.visible);
                        let text = if on {
                            t("bigpicture.overlay.on")
                        } else {
                            t("bigpicture.overlay.off")
                        };
                        let trailing = div()
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(text)
                            .into_any_element();
                        let icon = if on { IconName::Eye } else { IconName::EyeOff };
                        (icon, t("bigpicture.overlay.mangohud_show"), Some(trailing))
                    }
                    HomeRow::FrameLimit => {
                        let text = match self.mangohud.and_then(|m| m.fps_limit) {
                            Some(fps) => {
                                t_args("bigpicture.overlay.fps", &[("fps", &fps.to_string())])
                            }
                            None => t("bigpicture.overlay.no_limit"),
                        };
                        let trailing = h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .items_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::ChevronLeft))
                            .child(div().min_w(rems(4.)).text_center().child(text))
                            .child(Icon::new(IconName::ChevronRight))
                            .into_any_element();
                        (
                            IconName::Gauge,
                            t("bigpicture.overlay.frame_limit"),
                            Some(trailing),
                        )
                    }
                    HomeRow::Screenshot => {
                        (IconName::Camera, t("bigpicture.overlay.screenshot"), None)
                    }
                    HomeRow::SaveReplay => (
                        IconName::Clapperboard,
                        t("bigpicture.overlay.save_replay"),
                        None,
                    ),
                    HomeRow::PlayStatus => {
                        let status = irori_ui::play_status::of(&self.game, cx);
                        let text = match status {
                            Some(s) => irori_ui::play_status::label(s),
                            None => t("bigpicture.overlay.no_status"),
                        };
                        let icon = status
                            .map(irori_ui::play_status::icon)
                            .unwrap_or(IconName::CircleDashed);
                        let trailing = h_flex()
                            .flex_shrink_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::ChevronLeft))
                            .child(div().min_w_0().truncate().child(text))
                            .child(Icon::new(IconName::ChevronRight))
                            .into_any_element();
                        (icon, irori_ui::play_status::title(), Some(trailing))
                    }
                    HomeRow::BackToGame => (
                        IconName::AppWindow,
                        t("bigpicture.overlay.back_to_game"),
                        None,
                    ),
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
            .when_some(self.mangohud_error.clone(), |d, e| {
                d.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .into_any_element()
    }

    /// Show MangoHud when it is hidden, hide it when it shows.
    pub(super) fn toggle_mangohud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.mangohud {
            let visible = !state.visible;
            self.change_mangohud(MangoState { visible, ..state }, window, cx);
        }
    }

    /// Move MangoHud's frame limit `step` places along the presets
    /// ([`super::super::model::FRAME_LIMITS`]).
    pub(super) fn step_frame_limit(
        &mut self,
        step: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.mangohud.filter(|m| m.limits) {
            let fps_limit = next_limit(state.fps_limit, step);
            self.change_mangohud(MangoState { fps_limit, ..state }, window, cx);
        }
    }

    /// Write MangoHud's new state, which it applies at once; the row shows
    /// it right away and the file's answer when it comes.
    fn change_mangohud(&mut self, state: MangoState, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.mangohud;
        self.mangohud = Some(state);
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let task = irori_ui::run(cx, async move { backend.set_mangohud(&id, state).await });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(now) => {
                        this.mangohud = Some(now);
                        this.mangohud_error = None;
                    }
                    Err(e) => {
                        this.mangohud = before;
                        this.mangohud_error = Some(t_args(
                            "bigpicture.overlay.mangohud_failed",
                            &[("error", format!("{e:#}").as_str())],
                        ));
                    }
                }
                this.refresh_home();
                cx.notify();
            });
        }));
    }

    /// Set the game's play status `step` places from the current one.
    pub(super) fn step_status(&mut self, step: i32, cx: &mut Context<Self>) {
        let next = next_status(irori_ui::play_status::of(&self.game, cx), step);
        irori_ui::play_status::set(&self.game, Some(next), cx);
    }

    /// Close the overlay and bring the game's window to the front. Where
    /// the desktop doesn't allow that, closing is all there is to do: the
    /// game is under the overlay.
    pub(super) fn back_to_game(&mut self, cx: &mut Context<Self>) {
        super::super::close(cx);
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let b = backend.clone();
        backend.spawn(async move {
            if let Err(e) = b.focus_game_window(&id).await {
                tracing::info!("bringing {id}'s window to the front: {e:#}");
            }
        });
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
