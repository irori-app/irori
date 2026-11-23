//! The overlay's panel: a header with the game, the tabs, the shown tab's
//! rows, the question before quitting, unlock notices in the corner and the
//! controller hints. Focus is virtual (the model's), driven by controller
//! inputs and by the keyboard through [`super::OVERLAY_CONTEXT`]; rows also
//! answer clicks.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, Role, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Task, Window,
    div, rems,
};
use irori_core::{Achievement, GameId};
use irori_services::audio::AudioOutput;
use irori_services::{GameAudio, JournalEntry, Screenshot, Volume, VolumeTarget};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::journal::shot_url;
use irori_ui::models;

use super::model::{
    Action, Choice, HomeContext, HomeRow, Nav, SHOT_COLUMNS, Tab, Toast, home_rows, next_output,
    next_status, next_up, progress, unlocked_since,
};
use crate::gamepad::{self, ControllerInfo, ControllerKind, GamepadInput};
use crate::layout::home::Dir;
use crate::layout::wheel::{WheelTravel, WheelView};
use irori_core::achievement_sets::{SetProgress, sets};

mod controllers;
mod home;

/// How many locked achievements the Achievements tab suggests.
const NEXT_UP: usize = 6;
/// Thumbnail width of a screenshot, in rem.
const SHOT_REM: f32 = 15.;
/// Achievement icon size, in rem.
const ICON_REM: f32 = 3.;
/// Time for the overlay's surface to leave the screen before a screenshot.
pub(super) const SCREENSHOT_DELAY: Duration = Duration::from_millis(300);
/// Rows of the Journal tab before today's notes: write a note, where I
/// left off.
const JOURNAL_ACTIONS: usize = 2;

/// Something read in the background: on its way, there, or failed.
enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub(super) struct OverlayView {
    game: GameId,
    title: SharedString,
    /// Unix time the session started.
    started: i64,
    nav: Nav,
    focus: FocusHandle,
    audio: Load<GameAudio>,
    /// Why the last volume change failed; the volumes read before stay.
    audio_error: Option<SharedString>,
    /// The sound outputs, the one in use marked.
    outputs: Load<Vec<AudioOutput>>,
    /// The microphones, the one in use marked.
    inputs: Load<Vec<AudioOutput>>,
    /// The connected controllers, as the reader last listed them.
    controllers: Vec<ControllerInfo>,
    /// The controller row whose "Turn off" was pressed once: a second press
    /// turns it off.
    armed: Option<usize>,
    /// Why turning a controller off failed.
    controller_error: Option<SharedString>,
    /// The replay buffer records this session.
    replay: bool,
    /// MangoHud in this session, when Irori can change it.
    mangohud: Option<irori_services::mangohud::MangoState>,
    /// Why the last change to MangoHud failed.
    mangohud_error: Option<SharedString>,
    shots: Load<Vec<Screenshot>>,
    /// The screenshot shown large, by its place in `shots`.
    preview: Option<usize>,
    achievements: Load<Arc<Vec<Achievement>>>,
    /// Today's notes on the game, in the day's order; read on every draw.
    journal_today: Vec<JournalEntry>,
    image_cache: Entity<LruImageCache>,
    /// Wheel travel not yet turned into a row (as in Big Picture).
    wheel: WheelTravel,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl OverlayView {
    pub(super) fn new(game: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::register_catalogs();
        let backend = irori_ui::backend(cx);
        let title = backend
            .game(&game)
            .map(|g| SharedString::from(g.title.clone()))
            .unwrap_or_else(|| SharedString::from(game.app_name.to_string()));
        let elapsed = backend.playing_for(&game).unwrap_or_default();
        let started = chrono::Utc::now().timestamp() - elapsed.as_secs() as i64;
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let statuses = models(cx).statuses.clone();
        let settings = models(cx).settings.clone();
        let watched = game.clone();
        let watched_replay = game.clone();
        let library = models(cx).library.clone();
        let mut subscriptions = vec![
            cx.observe(&art_store(cx), |_, _, cx| cx.notify()),
            // The play status row follows the user data.
            cx.observe(&library, |_, _, cx| cx.notify()),
            // The game exited, or the overlay was turned off: nothing to show.
            cx.observe(&statuses, move |_, statuses, cx| {
                if !statuses.read(cx).is_playing(&watched) {
                    super::close(cx);
                }
            }),
            cx.observe(&settings, |_, _, cx| {
                if !super::enabled(cx) {
                    super::close(cx);
                }
            }),
        ];
        if let Some(model) = gamepad::model(cx) {
            subscriptions.push(cx.observe(&model, |this, _, cx| {
                let now = gamepad::controllers();
                if now != this.controllers {
                    // A controller went away: what was armed is something else.
                    this.armed = None;
                    this.controllers = now;
                    cx.notify();
                }
            }));
        }
        let mut view = Self {
            game,
            title,
            started,
            nav: Nav::default(),
            focus,
            audio: Load::Loading,
            audio_error: None,
            outputs: Load::Loading,
            inputs: Load::Loading,
            controllers: gamepad::controllers(),
            armed: None,
            controller_error: None,
            replay: backend.replay().is_some_and(|r| r.game == watched_replay),
            mangohud: None,
            mangohud_error: None,
            shots: Load::Loading,
            preview: None,
            achievements: Load::Loading,
            journal_today: Vec::new(),
            image_cache: LruImageCache::new(24, cx),
            wheel: WheelTravel::default(),
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        // The rows that need nothing read yet.
        view.set_audio(Load::Loading);
        view.load(window, cx);
        view
    }

    /// Read the sound, the session's screenshots and the achievements.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let b = backend.clone();
        let audio = irori_ui::run(cx, async move { b.audio(&id).await });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = audio.await;
            let _ = this.update(cx, |this, cx| {
                this.set_audio(loaded(result));
                cx.notify();
            });
        }));

        let b = backend.clone();
        let outputs = irori_ui::run(cx, async move { b.audio_outputs().await });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = outputs.await;
            let _ = this.update(cx, |this, cx| {
                this.outputs = loaded(result);
                cx.notify();
            });
        }));

        let b = backend.clone();
        let inputs = irori_ui::run(cx, async move { b.audio_inputs().await });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = inputs.await;
            let _ = this.update(cx, |this, cx| {
                this.inputs = loaded(result);
                cx.notify();
            });
        }));

        let b = backend.clone();
        let id = self.game.clone();
        let mangohud = irori_ui::state::run_infallible(cx, async move { b.mangohud(&id).await });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let state = mangohud.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.mangohud = state;
                this.refresh_home();
                cx.notify();
            });
        }));

        let b = backend.clone();
        let key = self.game.key();
        let since = self.started - 5;
        let shots = irori_ui::run(cx, async move {
            let index = b.spawn_blocking({
                let b = b.clone();
                move || b.screenshots()
            });
            let index = index.await?;
            Ok(index
                .for_game(&key)
                .filter(|s| s.taken >= since)
                .cloned()
                .collect::<Vec<_>>())
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = shots.await;
            let _ = this.update(cx, |this, cx| {
                this.shots = loaded(result);
                cx.notify();
            });
        }));

        let b = backend;
        let id = self.game.clone();
        let language = models(cx).settings.read(cx).app.ui_language.clone();
        let refresh = super::should_refresh(&self.game.key(), cx);
        let achievements = irori_ui::run(cx, async move {
            // Unlocks happen while playing: ask the store again rather
            // than show its answer from before the session.
            if refresh && let Some(store) = b.store(id.runner) {
                store.forget_achievements(&id.app_name).await;
            }
            b.achievements(&id, &language).await
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = achievements.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(list) = &result {
                    let key = this.game.key();
                    let (started, title) = (this.started, this.title.to_string());
                    let prefs = models(cx).settings.read(cx).app.achievement_toast.clone();
                    let now = Some(progress(list));
                    super::toasts(cx)
                        .offer(&key, list, started, |a| Toast::new(a, &title, now, &prefs));
                    cx.defer(super::start_toasts);
                }
                this.achievements = loaded(result);
                cx.notify();
            });
        }));
    }

    #[cfg(test)]
    pub(super) fn nav(&self) -> &Nav {
        &self.nav
    }

    /// Whether everything the view reads in the background has arrived.
    #[cfg(test)]
    pub(super) fn loaded(&self) -> bool {
        !matches!(self.audio, Load::Loading)
            && !matches!(self.shots, Load::Loading)
            && !matches!(self.achievements, Load::Loading)
    }

    /// Take the sound as read, and the Home rows it calls for.
    fn set_audio(&mut self, audio: Load<GameAudio>) {
        self.audio = audio;
        self.refresh_home();
    }

    /// The Home rows for what is known now.
    fn refresh_home(&mut self) {
        let microphone = matches!(&self.audio, Load::Ready(a) if a.microphone.is_some());
        self.nav.set_home(home_rows(HomeContext {
            microphone,
            replay: self.replay,
            mangohud: self.mangohud,
        }));
    }

    /// How many rows the shown tab has.
    fn rows(&self) -> usize {
        match self.nav.tab() {
            Tab::Home => self.nav.home().len(),
            Tab::Controllers => self.controllers.len(),
            Tab::Screenshots => match &self.shots {
                Load::Ready(shots) => shots.len(),
                _ => 0,
            },
            Tab::Achievements => match &self.achievements {
                Load::Ready(list) => {
                    unlocked_since(list, self.started).len() + next_up(list, NEXT_UP).len()
                }
                _ => 0,
            },
            Tab::Journal => JOURNAL_ACTIONS + self.journal_today.len(),
        }
    }

    /// One controller or keyboard input.
    pub(super) fn handle(
        &mut self,
        input: GamepadInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            // Back from the journal editor: the keyboard comes back here.
            self.focus.focus(window, cx);
        }
        if let Some(ix) = self.preview {
            let count = self.rows();
            match input {
                GamepadInput::Back | GamepadInput::Confirm => self.preview = None,
                GamepadInput::Left if ix > 0 => self.preview = Some(ix - 1),
                GamepadInput::Right if ix + 1 < count => self.preview = Some(ix + 1),
                GamepadInput::Guide => super::close(cx),
                _ => return,
            }
            if let Some(ix) = self.preview {
                self.nav.set_focus(ix);
            }
            cx.notify();
            return;
        }
        let rows = self.rows();
        let action = self.nav.input(input, rows);
        if action != Action::None && !matches!(action, Action::Activate(Tab::Controllers, _)) {
            // Turning a controller off takes two presses in a row.
            self.armed = None;
        }
        match action {
            Action::None => return,
            Action::Moved => {}
            Action::Close => super::close(cx),
            Action::Activate(tab, row) => self.activate(tab, row, window, cx),
            Action::Adjust(HomeRow::GameVolume, _)
                if !super::support::Support::current().game_volume =>
            {
                return;
            }
            Action::Adjust(HomeRow::Output, step) => self.switch_output(step, window, cx),
            Action::Adjust(HomeRow::Input, step) => self.switch_input(step, window, cx),
            Action::Adjust(HomeRow::PlayStatus, step) => self.step_status(step, cx),
            Action::Adjust(HomeRow::FrameLimit, step) => self.step_frame_limit(step, window, cx),
            Action::Adjust(row, step) => {
                let target = match row {
                    HomeRow::SystemVolume => VolumeTarget::System,
                    HomeRow::Microphone => VolumeTarget::Microphone,
                    _ => VolumeTarget::Game,
                };
                self.change_audio(target, Some(step * super::volume_step(cx)), window, cx);
            }
            Action::Quit => self.quit(window, cx),
        }
        cx.notify();
    }

    fn activate(&mut self, tab: Tab, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            Tab::Home => match self.nav.home_row(row) {
                Some(HomeRow::SystemVolume) => {
                    self.change_audio(VolumeTarget::System, None, window, cx)
                }
                Some(HomeRow::GameVolume) if super::support::Support::current().game_volume => {
                    self.change_audio(VolumeTarget::Game, None, window, cx)
                }
                Some(HomeRow::GameVolume) => {}
                Some(HomeRow::Output) => self.switch_output(1, window, cx),
                Some(HomeRow::Microphone) => {
                    self.change_audio(VolumeTarget::Microphone, None, window, cx)
                }
                Some(HomeRow::Input) => self.switch_input(1, window, cx),
                Some(HomeRow::Screenshot) => {
                    let backend = irori_ui::backend(cx);
                    let id = self.game.clone();
                    super::close(cx);
                    let b = backend.clone();
                    backend.spawn(async move {
                        tokio::time::sleep(SCREENSHOT_DELAY).await;
                        b.take_screenshot(Some(id));
                    });
                }
                Some(HomeRow::SaveReplay) => {
                    super::close(cx);
                    irori_ui::backend(cx).save_clip();
                }
                Some(HomeRow::MarkMoment) => {
                    let id = self.game.clone();
                    super::close(cx);
                    super::mark_moment(id, true, cx);
                }
                Some(HomeRow::PlayStatus) => self.step_status(1, cx),
                Some(HomeRow::MangoHud) => self.toggle_mangohud(window, cx),
                Some(HomeRow::FrameLimit) => self.step_frame_limit(1, window, cx),
                Some(HomeRow::BackToGame) => self.back_to_game(cx),
                Some(HomeRow::BackToIrori) => super::back_to_irori(cx),
                Some(HomeRow::QuitGame) => self.nav.ask_quit(),
                None => {}
            },
            Tab::Screenshots => {
                if matches!(&self.shots, Load::Ready(shots) if row < shots.len()) {
                    self.preview = Some(row);
                }
            }
            Tab::Achievements => {}
            Tab::Controllers => self.controller_pressed(row, window, cx),
            Tab::Journal if row == 1 => {
                irori_ui::journal::open_left_off_editor(self.game.clone(), &self.title, window, cx);
            }
            Tab::Journal => {
                // Row 0 writes a new note; the rows after the actions open
                // today's notes, newest first.
                let note = row
                    .checked_sub(JOURNAL_ACTIONS)
                    .and_then(|ix| self.journal_today.iter().rev().nth(ix))
                    .map(|n| n.id.clone());
                let today = chrono::Local::now().date_naive();
                let date = crate::text::relative_day(today, today);
                irori_ui::journal::open_editor(
                    self.game.clone(),
                    today,
                    note,
                    &self.title,
                    &date,
                    window,
                    cx,
                );
            }
        }
    }

    /// Wheel or touchpad travel: vertical moves between rows; sideways
    /// moves along the screenshots' grid (elsewhere it would change a
    /// volume, which the wheel shouldn't do by accident).
    fn on_wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview.is_some() || self.nav.confirm().is_some() {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height());
        let view = if self.nav.tab() == Tab::Screenshots {
            WheelView::Grid
        } else {
            WheelView::Rows
        };
        for dir in self
            .wheel
            .feed(view, f32::from(delta.x), f32::from(delta.y))
        {
            let input = match dir {
                Dir::Up => GamepadInput::Up,
                Dir::Down => GamepadInput::Down,
                Dir::Left => GamepadInput::Left,
                Dir::Right => GamepadInput::Right,
            };
            self.handle(input, window, cx);
        }
    }

    /// Send sound to the output `step` places from the current one, then
    /// read the outputs and volumes again (the new output has its own).
    fn switch_output(&mut self, step: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(outputs) = &self.outputs else {
            return;
        };
        let Some(next) = next_output(outputs, step) else {
            return;
        };
        let output = next.id.clone();
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let task = irori_ui::run(cx, async move {
            backend.set_audio_output(&id, &output).await?;
            let outputs = backend.audio_outputs().await?;
            let audio = backend.audio(&id).await.ok();
            Ok((outputs, audio))
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((outputs, audio)) => {
                        this.outputs = Load::Ready(outputs);
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

    /// Move a volume by `step` percent, or mute it (`None`).
    fn change_audio(
        &mut self,
        target: VolumeTarget,
        step: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let task = irori_ui::run(cx, async move {
            match step {
                Some(step) => backend.change_volume(&id, target, step).await,
                None => backend.toggle_mute(&id, target).await,
            }
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(audio) => {
                        this.set_audio(Load::Ready(audio));
                        this.audio_error = None;
                    }
                    Err(e) => this.audio_error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = irori_ui::backend(cx);
        let id = self.game.clone();
        let task = irori_ui::run(cx, async move { backend.stop(&id).await });
        // Stopping takes a few seconds; the overlay closes once the game has
        // exited (the statuses say so). A failure is shown here.
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            if let Err(e) = task.await {
                let _ = this.update_in(cx, |_, window, cx| {
                    window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("bigpicture.overlay.stop_failed")),
                        cx,
                    );
                });
            }
        }));
    }
}

fn loaded<T>(result: anyhow::Result<T>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(e) => Load::Failed(format!("{e:#}").into()),
    }
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read on every draw (the data is in memory), so a note saved in
        // the editor shows as soon as it closes.
        self.journal_today = irori_ui::backend(cx)
            .userdata
            .snapshot()
            .journal_day(&self.game.key(), chrono::Local::now().date_naive())
            .to_vec();
        let kind = self.kind(cx);
        let body = match self.nav.tab() {
            Tab::Home => self.render_home(cx),
            Tab::Screenshots => self.render_shots(window, cx),
            Tab::Achievements => self.render_achievements(window, cx),
            Tab::Journal => self.render_journal(cx),
            Tab::Controllers => self.render_controllers(cx),
        };
        // While the journal editor is open its keys are its own: the panel
        // neither binds them nor handles the actions they stand for.
        let dialog = window.has_active_dialog(cx);
        let mut root = div()
            .image_cache(self.image_cache.clone())
            .id("irori-overlay")
            .role(Role::Dialog)
            .aria_label(t("bigpicture.overlay.label"))
            .track_focus(&self.focus);
        if !dialog {
            root =
                root.key_context(super::OVERLAY_CONTEXT)
                    .on_action(cx.listener(|this, _: &gamepad::Up, w, cx| {
                        this.handle(GamepadInput::Up, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Down, w, cx| {
                        this.handle(GamepadInput::Down, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Left, w, cx| {
                        this.handle(GamepadInput::Left, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Right, w, cx| {
                        this.handle(GamepadInput::Right, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Confirm, w, cx| {
                        this.handle(GamepadInput::Confirm, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Back, w, cx| {
                        this.handle(GamepadInput::Back, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::PrevPage, w, cx| {
                        this.handle(GamepadInput::PrevPage, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::NextPage, w, cx| {
                        this.handle(GamepadInput::NextPage, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Guide, w, cx| {
                        this.handle(GamepadInput::Guide, w, cx)
                    }))
                    .on_scroll_wheel(
                        cx.listener(|this, e: &ScrollWheelEvent, w, cx| this.on_wheel(e, w, cx)),
                    );
        }
        let theme = cx.theme();
        root.size_full()
            .relative()
            .overflow_hidden()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                v_flex()
                    .size_full()
                    .p_6()
                    .gap_4()
                    .child(self.render_header(cx))
                    .child(self.render_tabs(kind, cx))
                    .child(div().flex_1().min_h_0().w_full().child(body))
                    .child(self.render_hints(kind, cx)),
            )
            .when_some(self.render_preview(window, cx), |d, p| d.child(p))
            .when_some(self.render_confirm(cx), |d, c| d.child(c))
            .when_some(self.render_toast(window, cx), |d, toast| d.child(toast))
    }
}

impl OverlayView {
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let minutes = irori_ui::backend(cx)
            .playing_for(&self.game)
            .map(|d| d.as_secs() / 60)
            .unwrap_or_default();
        let theme = cx.theme();
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .child(Icon::new(IconName::Gamepad2).text_color(theme.muted_foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .truncate()
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(t_args(
                                "bigpicture.overlay.playing_for",
                                &[("time", crate::text::playtime(minutes).as_ref())],
                            )),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_0p5()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t("bigpicture.overlay.beta")),
            )
            .into_any_element()
    }

    fn render_tabs(&self, kind: ControllerKind, cx: &mut Context<Self>) -> AnyElement {
        let shown = self.nav.tab();
        let theme = cx.theme();
        let (primary, on_primary, muted) = (
            theme.primary,
            theme.primary_foreground,
            theme.muted_foreground,
        );
        let glyph = |input| {
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(kind.glyph(input))
        };
        h_flex()
            .id("overlay-tabs")
            .w_full()
            .gap_2()
            .items_center()
            .role(Role::TabList)
            .child(glyph(GamepadInput::PrevPage))
            .children(Tab::ALL.into_iter().map(|tab| {
                let selected = tab == shown;
                div()
                    .id(SharedString::from(format!("overlay-tab-{tab:?}")))
                    .role(Role::Tab)
                    .aria_selected(selected)
                    .min_w_0()
                    .px_3()
                    .py_1()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .truncate()
                    .map(|d| {
                        if selected {
                            d.bg(primary).text_color(on_primary)
                        } else {
                            d.text_color(muted)
                        }
                    })
                    .child(tab_label(tab))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav.show(tab);
                        this.preview = None;
                        cx.notify();
                    }))
            }))
            .child(glyph(GamepadInput::NextPage))
            .into_any_element()
    }

    /// A row of a tab's list, highlighted when focused; a click focuses
    /// and runs it.
    fn row(
        &self,
        tab: Tab,
        ix: usize,
        icon: IconName,
        label: SharedString,
        trailing: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let danger = tab == Tab::Home && self.nav.home_row(ix) == Some(HomeRow::QuitGame);
        let focused = self.nav.tab() == tab && self.nav.focus(tab, self.rows()) == ix;
        let theme = cx.theme();
        let color = if danger {
            theme.danger
        } else {
            theme.foreground
        };
        h_flex()
            .id(SharedString::from(format!("overlay-row-{tab:?}-{ix}")))
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .w_full()
            .min_h(rems(3.25))
            .px_4()
            .gap_3()
            .items_center()
            .rounded(theme.radius_lg)
            .border_2()
            .cursor_pointer()
            .text_color(color)
            .map(|d| {
                if focused {
                    d.bg(theme.secondary).border_color(theme.ring)
                } else {
                    // The row under the pointer shows it will take a click.
                    let hover = theme.secondary.opacity(0.6);
                    d.border_color(gpui_kit::transparent_black())
                        .hover(move |s| s.bg(hover))
                }
            })
            .child(Icon::new(icon))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .children(trailing)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.nav.set_focus(ix);
                this.activate(tab, ix, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The family of the controller in use, for button names.
    fn kind(&self, cx: &mut Context<Self>) -> ControllerKind {
        gamepad::model(cx)
            .and_then(|m| m.read(cx).active_kind())
            .unwrap_or_default()
    }

    fn empty(
        &self,
        icon: IconName,
        title: SharedString,
        hint: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).size(rems(2.)))
            .child(div().text_lg().text_center().child(title))
            .children(hint.map(|h| div().text_sm().text_center().child(h)))
            .into_any_element()
    }

    fn render_shots(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let shots = match &self.shots {
            Load::Loading => {
                return self.empty(IconName::Images, t("bigpicture.overlay.loading"), None, cx);
            }
            Load::Failed(e) => {
                return self.empty(
                    IconName::ImageOff,
                    t("bigpicture.overlay.shots_failed"),
                    Some(e.clone()),
                    cx,
                );
            }
            Load::Ready(shots) if shots.is_empty() => {
                return self.empty(
                    IconName::Images,
                    t("bigpicture.overlay.no_screenshots"),
                    Some(t_args(
                        "bigpicture.overlay.no_screenshots_hint",
                        &[("button", self.kind(cx).glyph(GamepadInput::NextPage))],
                    )),
                    cx,
                );
            }
            Load::Ready(shots) => shots.clone(),
        };
        let focused = self.nav.focus(Tab::Screenshots, shots.len());
        let width = irori_ui::components::physical_width(rems(SHOT_REM), window);
        // Rows of SHOT_COLUMNS tiles; a session holds a handful of shots, so
        // the grid needs no virtual list.
        let mut grid = v_flex()
            .id("overlay-shots")
            .size_full()
            .gap_3()
            .overflow_y_scroll();
        let mut tiles = shots
            .iter()
            .enumerate()
            .map(|(ix, shot)| self.shot_tile(ix, shot, ix == focused, width, cx))
            .collect::<Vec<_>>()
            .into_iter()
            .peekable();
        while tiles.peek().is_some() {
            let row: Vec<AnyElement> = tiles.by_ref().take(SHOT_COLUMNS).collect();
            grid = grid.child(h_flex().w_full().gap_3().children(row));
        }
        grid.into_any_element()
    }

    fn shot_tile(
        &self,
        ix: usize,
        shot: &Screenshot,
        focused: bool,
        width: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = shot_url(&shot.path.to_string_lossy());
        let image = (!shot.video)
            .then(|| art_img(&url, width, false, cx))
            .flatten();
        let theme = cx.theme();
        div()
            .id(("overlay-shot", ix))
            .role(Role::Image)
            .aria_selected(focused)
            .flex_1()
            .min_w_0()
            .h(rems(SHOT_REM * 9. / 16.))
            .rounded(theme.radius_lg)
            .overflow_hidden()
            .border_2()
            .border_color(if focused {
                theme.ring
            } else {
                gpui_kit::transparent_black()
            })
            .bg(theme.muted)
            .cursor_pointer()
            .map(|d| match image {
                Some(img) => d.child(img.size_full().object_fit(ObjectFit::Cover)),
                None => d.flex().items_center().justify_center().child(
                    Icon::new(if shot.video {
                        IconName::Video
                    } else {
                        IconName::Image
                    })
                    .text_color(theme.muted_foreground),
                ),
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.nav.set_focus(ix);
                this.activate(Tab::Screenshots, ix, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_preview(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ix = self.preview?;
        let Load::Ready(shots) = &self.shots else {
            return None;
        };
        let shot = shots.get(ix)?;
        let width = irori_ui::components::physical_width(rems(56.), window);
        let image = (!shot.video)
            .then(|| art_img(&shot_url(&shot.path.to_string_lossy()), width, false, cx))
            .flatten();
        let theme = cx.theme();
        Some(
            div()
                .id("overlay-preview")
                .absolute()
                .inset_0()
                .bg(theme.background)
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.preview = None;
                    cx.notify();
                }))
                .map(|d| match image {
                    Some(img) => d.child(img.size_full().object_fit(ObjectFit::Contain)),
                    None => d.child(Icon::new(IconName::Video).size(rems(3.))),
                })
                .into_any_element(),
        )
    }

    fn achievement_row(
        &self,
        ix: usize,
        a: &Achievement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.nav.focus(Tab::Achievements, self.rows()) == ix;
        let unlocked = a.is_unlocked();
        let url = if unlocked || a.image_locked.is_empty() {
            &a.image_unlocked
        } else {
            &a.image_locked
        };
        let px = irori_ui::components::physical_width(rems(ICON_REM), window);
        let image = (!url.is_empty())
            .then(|| art_img(url, px, true, cx))
            .flatten();
        let (name, description): (SharedString, SharedString) = if a.visible || unlocked {
            (a.name.clone().into(), a.description.clone().into())
        } else {
            (
                t("bigpicture.overlay.hidden_achievement"),
                SharedString::default(),
            )
        };
        let progress = (!unlocked)
            .then_some(a.progress)
            .flatten()
            .map(|p| SharedString::from(format!("{:.0}%", p * 100.)));
        let theme = cx.theme();
        h_flex()
            .id(("overlay-achievement", ix))
            .role(Role::ListItem)
            .aria_label(name.clone())
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .w_full()
            .p_2()
            .gap_3()
            .items_center()
            .rounded(theme.radius_lg)
            .border_2()
            .map(|d| {
                if focused {
                    d.bg(theme.secondary).border_color(theme.ring)
                } else {
                    d.border_color(gpui_kit::transparent_black())
                }
            })
            .child(
                div()
                    .size(rems(ICON_REM))
                    .flex_shrink_0()
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .map(|d| match image {
                        Some(img) => d.child(
                            img.size_full()
                                .object_fit(ObjectFit::Cover)
                                .when(!unlocked, |i| i.grayscale(true)),
                        ),
                        None => d.flex().items_center().justify_center().child(
                            Icon::new(if unlocked {
                                IconName::Trophy
                            } else {
                                IconName::Lock
                            })
                            .text_color(theme.muted_foreground),
                        ),
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_semibold().truncate().child(name))
                    .when(!description.is_empty(), |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .line_clamp(2)
                                .child(description),
                        )
                    }),
            )
            .children(progress.map(|p| {
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(p)
            }))
            .into_any_element()
    }

    fn render_achievements(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let list = match &self.achievements {
            Load::Loading => {
                return self.empty(IconName::Trophy, t("bigpicture.overlay.loading"), None, cx);
            }
            Load::Failed(e) => {
                return self.empty(
                    IconName::Trophy,
                    t("bigpicture.overlay.achievements_failed"),
                    Some(e.clone()),
                    cx,
                );
            }
            Load::Ready(list) if list.is_empty() => {
                return self.empty(
                    IconName::Trophy,
                    t("bigpicture.overlay.no_achievements"),
                    None,
                    cx,
                );
            }
            Load::Ready(list) => list.clone(),
        };
        let (unlocked, total) = progress(&list);
        let session = unlocked_since(&list, self.started);
        let next = next_up(&list, NEXT_UP);
        let set_rows: Vec<AnyElement> = sets(&list)
            .iter()
            .enumerate()
            .map(|(ix, set)| self.set_row(ix, set, cx))
            .collect();
        let mut ix = 0;
        let mut session_rows = Vec::new();
        for a in &session {
            session_rows.push(self.achievement_row(ix, a, window, cx));
            ix += 1;
        }
        let mut next_rows = Vec::new();
        for a in &next {
            next_rows.push(self.achievement_row(ix, a, window, cx));
            ix += 1;
        }
        let theme = cx.theme();
        let fraction = if total == 0 {
            0.
        } else {
            unlocked as f32 / total as f32
        };
        let heading = |key: SharedString| {
            div()
                .pt_2()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(key.to_uppercase()))
        };
        v_flex()
            .id("overlay-achievements")
            .size_full()
            .gap_1()
            .overflow_y_scroll()
            .child(div().child(t_args(
                "bigpicture.overlay.achievements_progress",
                &[
                    ("unlocked", unlocked.to_string().as_str()),
                    ("total", total.to_string().as_str()),
                ],
            )))
            .child(
                div()
                    .w_full()
                    .h(rems(0.5))
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(fraction))
                            .rounded(theme.radius)
                            .bg(theme.primary),
                    ),
            )
            .when(!set_rows.is_empty(), |d| {
                d.child(heading(t("bigpicture.overlay.sets")))
                    .children(set_rows)
            })
            .when(!session_rows.is_empty(), |d| {
                d.child(heading(t("bigpicture.overlay.this_session")))
                    .children(session_rows)
            })
            .when(!next_rows.is_empty(), |d| {
                d.child(heading(t("bigpicture.overlay.next_up")))
                    .children(next_rows)
            })
            .into_any_element()
    }

    /// One set of the game's achievements (the base game's, a DLC's): its
    /// name, how much of it is unlocked and whether the player can earn it.
    /// Not a stop of the focus: it says, it doesn't do.
    fn set_row(&self, ix: usize, set: &SetProgress, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let name: SharedString = match &set.group {
            Some(g) if !g.name.trim().is_empty() => g.name.clone().into(),
            _ => t("bigpicture.overlay.set_other"),
        };
        let status = if set.unearnable() {
            t("bigpicture.overlay.set_not_owned")
        } else if set.complete() {
            t("bigpicture.overlay.set_complete")
        } else {
            t_args(
                "bigpicture.overlay.achievements_progress",
                &[
                    ("unlocked", set.unlocked.to_string().as_str()),
                    ("total", set.total.to_string().as_str()),
                ],
            )
        };
        let fraction = if set.total == 0 {
            0.
        } else {
            set.unlocked as f32 / set.total as f32
        };
        let fill = if set.complete() {
            theme.success
        } else if set.unearnable() {
            theme.muted_foreground
        } else {
            theme.primary
        };
        h_flex()
            .id(("overlay-achievement-set", ix))
            .role(Role::ListItem)
            .aria_label(SharedString::from(format!("{name}: {status}")))
            .w_full()
            .px_2()
            .py_1()
            .gap_3()
            .items_center()
            .text_sm()
            .child(div().flex_1().min_w_0().truncate().child(name))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(rems(12.))
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(status),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(rems(6.))
                    .h(rems(0.375))
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(fraction))
                            .rounded(theme.radius)
                            .bg(fill),
                    ),
            )
            .into_any_element()
    }

    fn render_journal(&self, cx: &mut Context<Self>) -> AnyElement {
        let write = self.row(
            Tab::Journal,
            0,
            IconName::NotebookPen,
            t("bigpicture.overlay.journal_write"),
            None,
            cx,
        );
        let left_off = irori_ui::models(cx)
            .library
            .read(cx)
            .user
            .left_off
            .get(&self.game.key())
            .map(|l| {
                div()
                    .text_sm()
                    .line_clamp(1)
                    .child(l.text.clone())
                    .into_any_element()
            });
        let left_off = self.row(
            Tab::Journal,
            1,
            IconName::Bookmark,
            t("bigpicture.overlay.left_off"),
            left_off,
            cx,
        );
        let notes: Vec<AnyElement> = self
            .journal_today
            .iter()
            .rev()
            .enumerate()
            .map(|(ix, note)| {
                let label = note_label(note);
                let icon = if note.is_waiting() {
                    IconName::Bookmark
                } else {
                    IconName::StickyNote
                };
                let time = irori_ui::journal::time_label(note.time)
                    .map(|t| div().text_sm().child(t).into_any_element());
                self.row(Tab::Journal, ix + JOURNAL_ACTIONS, icon, label, time, cx)
            })
            .collect();
        let theme = cx.theme();
        v_flex()
            .id("overlay-journal")
            .size_full()
            .gap_3()
            .overflow_y_scroll()
            .child(write)
            .child(left_off)
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(
                        t("bigpicture.overlay.journal_notes_today").to_uppercase(),
                    )),
            )
            .when(notes.is_empty(), |d| {
                d.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t("bigpicture.overlay.journal_none_today")),
                )
            })
            .children(notes)
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("bigpicture.overlay.journal_keyboard")),
            )
            .into_any_element()
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let choice = self.nav.confirm()?;
        let theme = cx.theme();
        let button = |id: &'static str, label: SharedString, focused: bool, danger: bool| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .aria_selected(focused)
                .px_4()
                .py_2()
                .min_w_0()
                .truncate()
                .rounded(theme.radius)
                .border_2()
                .cursor_pointer()
                .map(|d| match (focused, danger) {
                    (true, true) => d
                        .bg(theme.danger)
                        .text_color(theme.background)
                        .border_color(theme.ring),
                    (true, false) => d.bg(theme.secondary).border_color(theme.ring),
                    (false, true) => d.text_color(theme.danger).border_color(theme.border),
                    (false, false) => d.border_color(theme.border),
                })
                .child(label)
        };
        let cancel = button(
            "overlay-quit-cancel",
            t("bigpicture.overlay.cancel"),
            choice == Choice::Cancel,
            false,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.nav.cancel_quit();
            cx.notify();
        }));
        let quit = button(
            "overlay-quit-confirm",
            t("bigpicture.overlay.quit_confirm"),
            choice == Choice::Quit,
            true,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.nav.cancel_quit();
            this.quit(window, cx);
            cx.notify();
        }));
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.background.opacity(0.85))
                .child(
                    v_flex()
                        .id("overlay-quit")
                        .role(Role::AlertDialog)
                        .aria_label(t_args(
                            "bigpicture.overlay.quit_title",
                            &[("title", self.title.as_ref())],
                        ))
                        .max_w(rems(28.))
                        .p_6()
                        .gap_4()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .child(div().text_lg().font_semibold().child(t_args(
                            "bigpicture.overlay.quit_title",
                            &[("title", self.title.as_ref())],
                        )))
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(t("bigpicture.overlay.quit_description")),
                        )
                        .child(h_flex().gap_2().justify_end().child(cancel).child(quit)),
                )
                .into_any_element(),
        )
    }

    fn render_toast(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let toast = super::toasts(cx).current().cloned()?;
        let waiting = super::toasts(cx).waiting();
        Some(
            super::notice::card("overlay-toast", &toast, waiting, window, cx)
                .absolute()
                .top_4()
                .right_4()
                .w(rems(24.))
                .into_any_element(),
        )
    }

    fn render_hints(&self, kind: ControllerKind, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let hint = |input: GamepadInput, label: SharedString| {
            h_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .flex_shrink_0()
                        .px_1p5()
                        .rounded(theme.radius)
                        .bg(theme.muted)
                        .text_color(theme.foreground)
                        .child(kind.glyph(input)),
                )
                .child(div().min_w_0().truncate().child(label))
        };
        h_flex()
            .w_full()
            .gap_4()
            .justify_end()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(hint(GamepadInput::Confirm, t("bigpicture.overlay.select")))
            .child(hint(GamepadInput::Back, t("bigpicture.overlay.close")))
            .child(hint(GamepadInput::Guide, t("bigpicture.overlay.close")))
            .into_any_element()
    }
}

fn tab_label(tab: Tab) -> SharedString {
    match tab {
        Tab::Home => t("bigpicture.overlay.tab.home"),
        Tab::Screenshots => t("bigpicture.overlay.tab.screenshots"),
        Tab::Achievements => t("bigpicture.overlay.tab.achievements"),
        Tab::Journal => t("bigpicture.overlay.tab.journal"),
        Tab::Controllers => t("bigpicture.overlay.tab.controllers"),
    }
}

/// How a note reads as an overlay row: its title, else the start of its
/// text, else that it is a moment marked to write later.
fn note_label(note: &JournalEntry) -> SharedString {
    let words = if note.title.trim().is_empty() {
        note.text.lines().next().unwrap_or_default().trim()
    } else {
        note.title.trim()
    };
    if words.is_empty() {
        t("bigpicture.overlay.journal_marked")
    } else {
        SharedString::from(words.to_string())
    }
}
