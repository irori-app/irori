//! The trailer player of a game page: a 16:9 stage that shows the trailer's
//! still until it plays, the frames ffmpeg decodes while it does
//! (`irori_services::media::trailer`), and controls below it (play and
//! pause, sound and its volume, the other trailers). It starts muted.
//!
//! It costs nothing while it doesn't play: pausing ends ffmpeg, and so does
//! the stage leaving the screen (scrolled away, another page shown, the
//! window hidden), which the stage notices because it isn't drawn; it plays
//! on from where it was when it is drawn again. Each frame replaces the one
//! before it in the GPU's atlas, so memory stays at two frames.
//!
//! Without ffmpeg the stage says so: elsewhere it can be installed with the
//! system's packages and looked for again; on Windows Irori can download
//! one, after asking ([`irori_services::media::ffmpeg_download`]).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ObjectFit, ParentElement as _, Pixels,
    Render, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, WeakEntity, Window, canvas, div, img, rems,
};
use irori_core::{GameId, Video};
use irori_services::media::{
    FfmpegStatus, Frame, Trailer, TrailerEvent, TrailerRequest, TrailerStop,
};

use crate::components::{ArtImage, art_image};
use crate::i18n::{t, t_args};
use crate::state::{backend, run, run_infallible};
use crate::style::KeyboardFocusable as _;

/// How long the stage may go undrawn before the trailer stops.
const UNSEEN_FOR: Duration = Duration::from_secs(1);
/// Volume when sound is first turned on.
const DEFAULT_VOLUME: f32 = 0.6;
/// Decode width until the stage was measured, in pixels.
const DEFAULT_WIDTH: u32 = 640;
/// Width the still is decoded at, in physical pixels.
const STILL_WIDTH: u32 = 960;

/// Whether a stage at `stage` is on screen inside `visible`, the clip of
/// the scroll area and window it is drawn in.
pub fn on_screen(stage: Bounds<Pixels>, visible: Bounds<Pixels>) -> bool {
    stage.intersects(&visible)
}

/// Whether a trailer last drawn at `seen` should stop by `now`.
pub fn unseen(seen: Option<Instant>, now: Instant) -> bool {
    seen.is_none_or(|s| now.saturating_duration_since(s) > UNSEEN_FOR)
}

/// The trailer `delta` away from `index` among `count`, wrapping around.
pub fn step(index: usize, count: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (index as isize + delta).rem_euclid(count as isize) as usize
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    /// The still, before the first play.
    Idle,
    Starting,
    Playing,
    Paused,
    Failed(SharedString),
    /// Fetching what the ffmpeg download would be, or downloading it.
    Downloading,
}

/// The players of the pages that exist, so the command palette can reach
/// the open game's.
#[derive(Default)]
struct Players(HashMap<GameId, WeakEntity<TrailerPlayer>>);
impl Global for Players {}

fn player_of(id: &GameId, cx: &App) -> Option<Entity<TrailerPlayer>> {
    cx.try_global::<Players>()?.0.get(id)?.upgrade()
}

/// Whether `id`'s page has a trailer to play.
pub fn has_trailer(id: &GameId, cx: &App) -> bool {
    player_of(id, cx).is_some_and(|p| !p.read(cx).is_empty())
}

/// Play or pause `id`'s trailer on its page.
pub fn toggle_trailer(id: &GameId, cx: &mut App) {
    if let Some(player) = player_of(id, cx) {
        player.update(cx, |p, cx| p.toggle(cx));
    }
}

/// The trailer player of one game's page.
pub struct TrailerPlayer {
    id: GameId,
    videos: Vec<Video>,
    index: usize,
    state: State,
    /// The frame shown, and the one before it, still in the GPU's atlas
    /// until the next arrives.
    frame: Option<Arc<RenderImage>>,
    retired: Option<Arc<RenderImage>>,
    position: Duration,
    muted: bool,
    volume: f32,
    ffmpeg: Option<FfmpegStatus>,
    /// Ends ffmpeg when dropped.
    stop: Option<TrailerStop>,
    pump: Option<Task<()>>,
    /// Which start the frames arriving belong to.
    generation: u64,
    /// Frames shown since the last start.
    frames: u64,
    /// When the stage was last drawn on screen.
    seen: Rc<Cell<Option<Instant>>>,
    /// The stage's width in physical pixels.
    width: Rc<Cell<u32>>,
    /// Stopped because the stage left the screen; plays on when it's back.
    suspended: bool,
    autoplayed: bool,
    focus: FocusHandle,
    slider: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl TrailerPlayer {
    pub fn new(id: GameId, videos: Vec<Video>, cx: &mut Context<Self>) -> Self {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(DEFAULT_VOLUME)
        });
        let subscriptions =
            vec![
                cx.subscribe(&slider, |this: &mut Self, _, event: &SliderEvent, cx| {
                    if let SliderEvent::Release(v) = event {
                        this.volume = v.start().clamp(0., 1.);
                        // ffmpeg's volume is set when it starts.
                        this.restart(cx);
                    }
                }),
            ];
        let me = cx.weak_entity();
        cx.default_global::<Players>()
            .0
            .insert(id.clone(), me.clone());
        cx.on_release(move |this: &mut Self, cx| {
            for image in this.frame.take().into_iter().chain(this.retired.take()) {
                cx.drop_image(image, None);
            }
            // A newer page of the same game may have taken the place.
            let players = &mut cx.default_global::<Players>().0;
            if players
                .get(&this.id)
                .is_some_and(|p| p.entity_id() == me.entity_id())
            {
                players.remove(&this.id);
            }
        })
        .detach();
        let mut player = Self {
            id,
            videos,
            index: 0,
            state: State::Idle,
            frame: None,
            retired: None,
            position: Duration::ZERO,
            muted: true,
            volume: DEFAULT_VOLUME,
            ffmpeg: None,
            stop: None,
            pump: None,
            generation: 0,
            frames: 0,
            seen: Rc::new(Cell::new(None)),
            width: Rc::new(Cell::new(DEFAULT_WIDTH)),
            suspended: false,
            autoplayed: false,
            focus: cx.focus_handle(),
            slider,
            _subscriptions: subscriptions,
        };
        player.check_ffmpeg(false, cx);
        player
    }

    /// The page's trailers changed (its store details arrived).
    pub fn set_videos(&mut self, videos: Vec<Video>, cx: &mut Context<Self>) {
        if videos == self.videos {
            return;
        }
        self.halt();
        self.videos = videos;
        self.index = 0;
        self.reset(cx);
        cx.notify();
    }

    pub fn is_empty(&self) -> bool {
        self.videos.is_empty()
    }

    /// How many trailers it has.
    pub fn len(&self) -> usize {
        self.videos.len()
    }

    fn check_ffmpeg(&mut self, look_again: bool, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = run_infallible(cx, async move { b.ffmpeg_status(look_again).await });
        cx.spawn(async move |this, cx| {
            let status = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.ffmpeg = Some(status);
                cx.notify();
            });
        })
        .detach();
    }

    fn playing(&self) -> bool {
        matches!(self.state, State::Starting | State::Playing)
    }

    /// Play, or pause what plays.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.playing() {
            self.pause(cx);
        } else {
            self.play(cx);
        }
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self.playing() || !self.ffmpeg.is_some_and(|f| f.found) || self.videos.is_empty() {
            return;
        }
        self.suspended = false;
        self.start(cx);
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.halt();
        self.suspended = false;
        self.state = State::Paused;
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if !self.ffmpeg.is_some_and(|f| f.sound) {
            return;
        }
        self.muted = !self.muted;
        self.restart(cx);
    }

    /// Show the trailer `delta` away, playing it when one played.
    pub fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let was_playing = self.playing();
        self.halt();
        self.index = step(self.index, self.videos.len(), delta);
        self.reset(cx);
        if was_playing {
            self.start(cx);
        }
        cx.notify();
    }

    /// Start again where it is, with the current sound settings.
    fn restart(&mut self, cx: &mut Context<Self>) {
        if self.playing() {
            self.start(cx);
        }
        cx.notify();
    }

    /// Back to the still of the current trailer.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.position = Duration::ZERO;
        self.state = State::Idle;
        for image in self.frame.take().into_iter().chain(self.retired.take()) {
            cx.drop_image(image, None);
        }
    }

    /// End ffmpeg and ignore whatever it still sends.
    fn halt(&mut self) {
        self.generation += 1;
        self.stop = None;
        self.pump = None;
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self.videos.get(self.index).map(|v| v.url.clone()) else {
            return;
        };
        self.halt();
        let generation = self.generation;
        self.state = State::Starting;
        self.frames = 0;
        let has_sound = self.ffmpeg.is_some_and(|f| f.sound);
        let request = TrailerRequest {
            url,
            width: self.width.get(),
            start: self.position,
            volume: (has_sound && !self.muted).then_some(self.volume),
        };
        let b = backend(cx);
        let task = run(cx, async move { b.play_trailer(request).await });
        self.pump = Some(cx.spawn(async move |this, cx| {
            let Trailer { mut events, stop } = match task.await {
                Ok(trailer) => trailer,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| this.fail(generation, format!("{e:#}"), cx));
                    return;
                }
            };
            let current = this
                .update(cx, move |this, _| {
                    let current = this.generation == generation;
                    if current {
                        this.stop = Some(stop);
                    }
                    current
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            while let Some(event) = events.recv().await {
                let more = this
                    .update(cx, |this, cx| this.on_event(generation, event, cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn fail(&mut self, generation: u64, error: String, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        tracing::info!("trailer of {}: {error}", self.id.key());
        self.halt();
        self.state = State::Failed(error.into());
        cx.notify();
    }

    /// Take one event of the trailer started as `generation`; `false` once
    /// no more are wanted.
    fn on_event(&mut self, generation: u64, event: TrailerEvent, cx: &mut Context<Self>) -> bool {
        if generation != self.generation {
            return false;
        }
        match event {
            TrailerEvent::Failed(error) => {
                self.fail(generation, error, cx);
                false
            }
            TrailerEvent::Frame(frame) => {
                self.show(frame, cx);
                // Until the first frame nothing may have been drawn (a
                // stream can take a while to start), and ffmpeg sends the
                // first half second at once: the stage has a second from
                // the first frame to be drawn, and each frame it is drawn
                // notes that it is seen.
                self.frames += 1;
                if self.frames == 1 {
                    self.seen.set(Some(Instant::now()));
                }
                if unseen(self.seen.get(), Instant::now()) {
                    // Off screen: stop until it's drawn again.
                    self.halt();
                    self.state = State::Paused;
                    self.suspended = true;
                    cx.notify();
                    return false;
                }
                true
            }
        }
    }

    fn show(&mut self, frame: Frame, cx: &mut Context<Self>) {
        self.position = frame.position;
        self.state = State::Playing;
        let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            return;
        };
        // GPUI's images are BGRA, which is what ffmpeg was asked for.
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.retired.take() {
            cx.drop_image(old, None);
        }
        self.retired = self.frame.replace(image);
        cx.notify();
    }

    /// Ask, then download ffmpeg (Windows).
    fn offer_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state = State::Downloading;
        cx.notify();
        let b = backend(cx);
        let task = run(cx, async move { b.ffmpeg_download_offer().await });
        cx.spawn_in(window, async move |this, cx| {
            let offer = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.state = State::Idle;
                match offer {
                    Ok(offer) => this.ask_download(offer, window, cx),
                    Err(e) => {
                        this.state = State::Failed(t_args(
                            "ui.trailer.download_failed",
                            &[("error", &format!("{e:#}"))],
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_download(
        &mut self,
        offer: irori_services::media::ffmpeg_download::Offer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = match offer.size {
            Some(size) => t_args(
                "ui.trailer.download_description",
                &[
                    ("file", &offer.file),
                    ("size", &irori_core::util::format_bytes(size)),
                    ("source", offer.source),
                ],
            ),
            None => t_args(
                "ui.trailer.download_description_unsized",
                &[("file", &offer.file), ("source", offer.source)],
            ),
        };
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, offer) = (this.clone(), offer.clone());
            alert
                .title(t("ui.trailer.download_title"))
                .description(description.clone())
                .confirm()
                .ok_text(t("ui.trailer.download"))
                .on_ok(move |_, _, cx| {
                    let (this, offer) = (this.clone(), offer.clone());
                    cx.defer(move |cx| this.update(cx, |p, cx| p.download(offer, cx)));
                    true
                })
        });
    }

    fn download(
        &mut self,
        offer: irori_services::media::ffmpeg_download::Offer,
        cx: &mut Context<Self>,
    ) {
        self.state = State::Downloading;
        cx.notify();
        let b = backend(cx);
        let task = run(cx, async move {
            b.download_ffmpeg(&offer).await?;
            Ok(b.ffmpeg_status(false).await)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(status) => {
                        this.ffmpeg = Some(status);
                        this.state = State::Idle;
                    }
                    Err(e) => {
                        this.state = State::Failed(t_args(
                            "ui.trailer.download_failed",
                            &[("error", &format!("{e:#}"))],
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Start on its own, once, when the settings say so and the user
    /// doesn't ask for less motion.
    fn maybe_autoplay(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.ffmpeg else {
            return;
        };
        if self.autoplayed || !status.found || self.videos.is_empty() {
            return;
        }
        self.autoplayed = true;
        let app = &crate::models(cx).settings.read(cx).app;
        if app.ui.trailer_autoplay && !crate::style::reduce_motion(cx) {
            self.play(cx);
        }
    }

    fn render_stage(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted_bg, scrim, fg, radius) = (
            theme.muted,
            theme.background.opacity(0.72),
            theme.foreground,
            theme.radius_lg,
        );
        let video = self.videos.get(self.index);
        let picture = match (&self.frame, video.and_then(|v| v.thumbnail.as_deref())) {
            (Some(frame), _) => Some(
                img(frame.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
            ),
            (None, Some(still)) => match art_image(still, STILL_WIDTH, false, cx) {
                ArtImage::Ready(img) => Some(
                    img.size_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                ),
                _ => None,
            },
            (None, None) => None,
        };
        let overlay = self.render_overlay(cx);
        let (seen, width, suspended) = (self.seen.clone(), self.width.clone(), self.suspended);
        let entity = cx.entity();
        div()
            .id("trailer-stage")
            .debug_selector(|| "trailer-stage".into())
            .track_focus(&self.focus)
            .keyboard_focusable(cx)
            .relative()
            .w_full()
            .aspect_ratio(16. / 9.)
            .rounded(radius)
            .overflow_hidden()
            .bg(muted_bg)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "m" => this.toggle_mute(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(picture)
            .when_some(overlay, |d, overlay| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(scrim)
                        .text_color(fg)
                        .child(overlay),
                )
            })
            .child(
                canvas(
                    move |bounds, window, cx| {
                        width.set((f32::from(bounds.size.width) * window.scale_factor()) as u32);
                        if on_screen(bounds, window.content_mask().bounds) {
                            seen.set(Some(Instant::now()));
                            if suspended {
                                cx.defer(move |cx| {
                                    entity.update(cx, |p, cx| {
                                        if p.suspended {
                                            p.play(cx);
                                        }
                                    })
                                });
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    /// What the stage says over its picture, if anything.
    fn render_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted_fg = cx.theme().muted_foreground;
        let Some(status) = self.ffmpeg else {
            return Some(Spinner::new().into_any_element());
        };
        if self.state == State::Downloading {
            return Some(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(Spinner::new())
                    .child(div().text_sm().child(t("ui.trailer.downloading")))
                    .into_any_element(),
            );
        }
        if !status.found {
            let hint = if status.can_download {
                t("ui.trailer.no_ffmpeg_download")
            } else {
                t("ui.trailer.no_ffmpeg_install")
            };
            return Some(
                v_flex()
                    .max_w(rems(28.))
                    .p_4()
                    .gap_3()
                    .items_center()
                    .text_center()
                    .child(div().font_medium().child(t("ui.trailer.no_ffmpeg")))
                    .child(div().text_sm().text_color(muted_fg).child(hint))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .justify_center()
                            .when(status.can_download, |row| {
                                row.child(
                                    Button::new("trailer-download")
                                        .primary()
                                        .icon(IconName::Download)
                                        .label(t("ui.trailer.download"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.offer_download(window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("trailer-look-again")
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .label(t("ui.trailer.look_again"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.ffmpeg = None;
                                        this.check_ffmpeg(true, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        match &self.state {
            State::Playing => None,
            State::Starting if self.frame.is_some() => None,
            State::Starting => Some(Spinner::new().large().into_any_element()),
            State::Failed(error) => Some(
                v_flex()
                    .max_w(rems(28.))
                    .p_4()
                    .gap_2()
                    .items_center()
                    .text_center()
                    .child(div().font_medium().child(t("ui.trailer.failed")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .line_clamp(3)
                            .child(error.clone()),
                    )
                    .child(
                        Button::new("trailer-retry")
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label(t("ui.trailer.try_again"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.play(cx)
                            })),
                    )
                    .into_any_element(),
            ),
            State::Idle | State::Paused | State::Downloading => Some(
                div()
                    .size_16()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().background.opacity(0.85))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(IconName::Play).large())
                    .into_any_element(),
            ),
        }
    }

    fn render_controls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let status = self.ffmpeg.unwrap_or_default();
        let muted_fg = cx.theme().muted_foreground;
        let playing = self.playing();
        let count = self.videos.len();
        let name: SharedString = self
            .videos
            .get(self.index)
            .map(|v| v.name.clone())
            .filter(|n| !n.is_empty())
            .map(SharedString::from)
            .unwrap_or_else(|| t("ui.trailer.untitled"));
        let (play_label, play_icon) = if playing {
            (t("ui.trailer.pause"), IconName::Pause)
        } else {
            (t("ui.trailer.play"), IconName::Play)
        };
        let (mute_label, mute_icon) = if self.muted {
            (t("ui.trailer.unmute"), IconName::VolumeX)
        } else {
            (t("ui.trailer.mute"), IconName::Volume2)
        };
        h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(
                Button::new("trailer-play")
                    .ghost()
                    .icon(play_icon)
                    .disabled(!status.found)
                    .accessibility_label(play_label.clone())
                    .tooltip(play_label)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
            )
            .when(status.found && status.sound, |row| {
                row.child(
                    Button::new("trailer-mute")
                        .ghost()
                        .icon(mute_icon)
                        .accessibility_label(mute_label.clone())
                        .tooltip(mute_label)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                )
                .when(!self.muted, |row| {
                    row.child(
                        Slider::new(&self.slider)
                            .accessibility_label(t("ui.trailer.volume"))
                            .w(rems(6.)),
                    )
                })
            })
            .child(div().flex_1().min_w_0().truncate().text_sm().child(name))
            .when(status.found && !status.sound, |row| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("ui.trailer.no_sound")),
                )
            })
            .when(count > 1, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t_args(
                            "ui.trailer.count",
                            &[
                                ("index", &(self.index + 1).to_string()),
                                ("count", &count.to_string()),
                            ],
                        )),
                )
                .child(
                    Button::new("trailer-previous")
                        .ghost()
                        .icon(IconName::SkipBack)
                        .accessibility_label(t("ui.trailer.previous"))
                        .tooltip(t("ui.trailer.previous"))
                        .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                )
                .child(
                    Button::new("trailer-next")
                        .ghost()
                        .icon(IconName::SkipForward)
                        .accessibility_label(t("ui.trailer.next"))
                        .tooltip(t("ui.trailer.next"))
                        .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                )
            })
            .into_any_element()
    }
}

impl Render for TrailerPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.videos.is_empty() {
            return div().into_any_element();
        }
        self.maybe_autoplay(cx);
        v_flex()
            .w_full()
            .gap_2()
            .child(self.render_stage(cx))
            .child(self.render_controls(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    #[test]
    fn plays_only_while_drawn_on_screen() {
        let view = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)));
        let stage = |y: f32| Bounds::new(point(px(100.), px(y)), size(px(400.), px(225.)));
        assert!(on_screen(stage(100.), view));
        // Scrolled partly out: still seen.
        assert!(on_screen(stage(-200.), view));
        assert!(!on_screen(stage(-300.), view));
        assert!(!on_screen(stage(700.), view));

        let now = Instant::now();
        assert!(!unseen(Some(now), now));
        assert!(!unseen(Some(now), now + Duration::from_millis(900)));
        assert!(unseen(Some(now), now + Duration::from_secs(2)));
        assert!(unseen(None, now));
    }

    #[test]
    fn steps_through_trailers_around() {
        assert_eq!(step(0, 3, 1), 1);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
        assert_eq!(step(0, 1, -1), 0);
    }
}
