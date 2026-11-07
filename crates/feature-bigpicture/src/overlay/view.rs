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
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, Window, div, rems,
};
use irori_core::{Achievement, GameId};
use irori_services::{GameAudio, JournalEntry, Screenshot, Volume, VolumeTarget};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::journal::shot_url;
use irori_ui::models;

use super::model::{
    Action, Choice, HomeRow, Nav, SHOT_COLUMNS, Tab, next_up, progress, unlocked_since,
};
use crate::gamepad::{self, ControllerKind, GamepadInput};

/// How long one unlock notice stays in the corner.
const TOAST_TIME: Duration = Duration::from_secs(4);
/// How many locked achievements the Achievements tab suggests.
const NEXT_UP: usize = 6;
/// Thumbnail width of a screenshot, in rem.
const SHOT_REM: f32 = 15.;
/// Achievement icon size, in rem.
const ICON_REM: f32 = 3.;
/// Time for the overlay's surface to leave the screen before a screenshot.
const SCREENSHOT_DELAY: Duration = Duration::from_millis(300);

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
    shots: Load<Vec<Screenshot>>,
    /// The screenshot shown large, by its place in `shots`.
    preview: Option<usize>,
    achievements: Load<Arc<Vec<Achievement>>>,
    image_cache: Entity<LruImageCache>,
    _toast_timer: Option<Task<()>>,
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
        let subscriptions = vec![
            cx.observe(&art_store(cx), |_, _, cx| cx.notify()),
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
        let mut view = Self {
            game,
            title,
            started,
            nav: Nav::default(),
            focus,
            audio: Load::Loading,
            audio_error: None,
            shots: Load::Loading,
            preview: None,
            achievements: Load::Loading,
            image_cache: LruImageCache::new(24, cx),
            _toast_timer: None,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
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
                this.audio = loaded(result);
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
                    let started = this.started;
                    let queued = super::toasts(cx).offer(&key, list, started);
                    if queued > 0 && this._toast_timer.is_none() {
                        this.schedule_toast(cx);
                    }
                }
                this.achievements = loaded(result);
                cx.notify();
            });
        }));
    }

    /// Show the next unlock notice after the current one's time.
    fn schedule_toast(&mut self, cx: &mut Context<Self>) {
        self._toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TIME).await;
            let _ = this.update(cx, |this, cx| {
                let more = super::toasts(cx).advance();
                this._toast_timer = None;
                if more {
                    this.schedule_toast(cx);
                }
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

    /// How many rows the shown tab has.
    fn rows(&self) -> usize {
        match self.nav.tab() {
            Tab::Home => HomeRow::ALL.len(),
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
            Tab::Journal => 1,
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
        match self.nav.input(input, rows) {
            Action::None => return,
            Action::Moved => {}
            Action::Close => super::close(cx),
            Action::Activate(tab, row) => self.activate(tab, row, window, cx),
            Action::Adjust(row, step) => {
                let target = if row == HomeRow::SystemVolume {
                    VolumeTarget::System
                } else {
                    VolumeTarget::Game
                };
                self.change_audio(
                    target,
                    Some(step * irori_services::audio::VOLUME_STEP),
                    window,
                    cx,
                );
            }
            Action::Quit => self.quit(window, cx),
        }
        cx.notify();
    }

    fn activate(&mut self, tab: Tab, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            Tab::Home => match HomeRow::at(row) {
                Some(HomeRow::SystemVolume) => {
                    self.change_audio(VolumeTarget::System, None, window, cx)
                }
                Some(HomeRow::GameVolume) => {
                    self.change_audio(VolumeTarget::Game, None, window, cx)
                }
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
            Tab::Journal => {
                let today = chrono::Local::now().date_naive();
                let date = crate::text::relative_day(today, today);
                irori_ui::journal::open_editor(
                    self.game.clone(),
                    today,
                    &self.title,
                    &date,
                    window,
                    cx,
                );
            }
        }
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
                        this.audio = Load::Ready(audio);
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
        let kind = self.kind(cx);
        let body = match self.nav.tab() {
            Tab::Home => self.render_home(cx),
            Tab::Screenshots => self.render_shots(window, cx),
            Tab::Achievements => self.render_achievements(window, cx),
            Tab::Journal => self.render_journal(cx),
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
                    }));
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
        let danger = tab == Tab::Home && HomeRow::at(ix) == Some(HomeRow::QuitGame);
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
                    d.border_color(gpui_kit::transparent_black())
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

    fn render_home(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.kind(cx);
        let (system, game, error) = match &self.audio {
            Load::Loading => (None, None, None),
            Load::Ready(audio) => (audio.system, audio.game, None),
            Load::Failed(e) => (None, None, Some(e.clone())),
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
        let system_text = volume(system, t("bigpicture.overlay.volume_unavailable"), cx);
        let game_text = volume(game, t("bigpicture.overlay.no_game_sound"), cx);
        let rows = vec![
            self.row(
                Tab::Home,
                0,
                muted_icon(system),
                t("bigpicture.overlay.system_volume"),
                Some(system_text),
                cx,
            ),
            self.row(
                Tab::Home,
                1,
                muted_icon(game),
                t("bigpicture.overlay.game_volume"),
                Some(game_text),
                cx,
            ),
            self.row(
                Tab::Home,
                2,
                IconName::Camera,
                t("bigpicture.overlay.screenshot"),
                None,
                cx,
            ),
            self.row(
                Tab::Home,
                3,
                IconName::House,
                t("bigpicture.overlay.back_to_irori"),
                None,
                cx,
            ),
            self.row(
                Tab::Home,
                4,
                IconName::Power,
                t("bigpicture.overlay.quit"),
                None,
                cx,
            ),
        ];
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

    fn render_journal(&self, cx: &mut Context<Self>) -> AnyElement {
        // Read on every draw (the data is in memory), so a note saved in
        // the editor shows as soon as it closes.
        let note: Option<JournalEntry> = irori_ui::backend(cx)
            .userdata
            .snapshot()
            .journal_entry(&self.game.key(), chrono::Local::now().date_naive())
            .filter(|n| !n.is_empty())
            .cloned();
        let label = if note.is_some() {
            t("bigpicture.overlay.journal_edit")
        } else {
            t("bigpicture.overlay.journal_write")
        };
        let row = self.row(Tab::Journal, 0, IconName::NotebookPen, label, None, cx);
        let theme = cx.theme();
        v_flex()
            .id("overlay-journal")
            .size_full()
            .gap_3()
            .overflow_y_scroll()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(
                        t("bigpicture.overlay.journal_today").to_uppercase(),
                    )),
            )
            .map(|d| match &note {
                Some(note) => d
                    .when(!note.title.is_empty(), |d| {
                        d.child(div().text_lg().font_semibold().child(note.title.clone()))
                    })
                    .child(div().child(note.text.clone())),
                None => d.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t("bigpicture.overlay.journal_empty")),
                ),
            })
            .child(row)
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
        let px = irori_ui::components::physical_width(rems(ICON_REM), window);
        let image = (!toast.image.is_empty())
            .then(|| art_img(&toast.image, px, true, cx))
            .flatten();
        let theme = cx.theme();
        Some(
            h_flex()
                .id("overlay-toast")
                .role(Role::Status)
                .aria_label(t("bigpicture.overlay.unlocked"))
                .absolute()
                .top_4()
                .right_4()
                .max_w(rems(24.))
                .p_3()
                .gap_3()
                .items_center()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_lg()
                .child(
                    div()
                        .size(rems(ICON_REM))
                        .flex_shrink_0()
                        .rounded(theme.radius)
                        .overflow_hidden()
                        .bg(theme.muted)
                        .map(|d| match image {
                            Some(img) => d.child(img.size_full().object_fit(ObjectFit::Cover)),
                            None => d
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::Trophy)),
                        }),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(t("bigpicture.overlay.unlocked")),
                        )
                        .child(div().font_semibold().truncate().child(toast.name))
                        .when(waiting > 0, |d| {
                            d.child(div().text_xs().text_color(theme.muted_foreground).child(
                                if waiting == 1 {
                                    t("bigpicture.overlay.more_unlocks_one")
                                } else {
                                    t_args(
                                        "bigpicture.overlay.more_unlocks_other",
                                        &[("count", waiting.to_string().as_str())],
                                    )
                                },
                            ))
                        }),
                )
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
    }
}
