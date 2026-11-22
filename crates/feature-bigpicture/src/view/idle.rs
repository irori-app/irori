//! Big Picture's idle screen (`iroriBigPicture.idleMinutes`, `idleScreen`):
//! after a while without input, the library's art goes by one game at a
//! time with the clock, or the screen dims, so a TV left on Big Picture
//! doesn't show the same picture for hours. Any input, mouse movement or
//! click brings Big Picture back; that input does nothing else.
//!
//! One timer task waits for the moment the screen is due and goes to sleep
//! again for what is left when input came meanwhile, so input costs nothing
//! but a time stamp. The dimmed screen needs no timer; the slideshow wakes
//! every [`SLIDE`].

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Styled as _, StyledImage as _, Task, div, linear_color_stop, linear_gradient, rems,
};
use irori_core::big_picture_prefs::{BigPicturePrefs, IdleScreen};
use irori_ui::components::art_img;
use irori_ui::models;

use super::{BigPictureView, Colors, clock_text};

/// How long each picture of the slideshow stays.
pub(super) const SLIDE: Duration = Duration::from_secs(10);
/// Width the slideshow's art is asked for, in pixels.
const ART_WIDTH: u32 = 1920;

/// The idle screen's state.
pub(super) struct Idle {
    last_input: Instant,
    showing: bool,
    /// The slideshow's picture, counted up from when the screen showed.
    slide: usize,
    task: Option<Task<()>>,
}

impl Idle {
    pub(super) fn new() -> Self {
        Self {
            last_input: Instant::now(),
            showing: false,
            slide: 0,
            task: None,
        }
    }
}

/// What the timer does next: show the screen now, or wait this long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Next {
    Show,
    Wait(Duration),
    /// The screen is showing and nothing changes on it, or it is off.
    Stop,
    /// Show the next picture.
    Slide,
}

/// The timer's next step (pure, see the module docs).
pub(super) fn next(
    after: Option<Duration>,
    showing: bool,
    screen: IdleScreen,
    last_input: Instant,
    now: Instant,
) -> Next {
    let Some(after) = after else {
        return Next::Stop;
    };
    if showing {
        return match screen {
            IdleScreen::Slideshow => Next::Slide,
            IdleScreen::Dim => Next::Stop,
        };
    }
    let due = last_input + after;
    if now >= due {
        Next::Show
    } else {
        Next::Wait(due - now)
    }
}

impl BigPictureView {
    fn idle_prefs(cx: &gpui_kit::App) -> BigPicturePrefs {
        models(cx).settings.read(cx).app.big_picture.clone()
    }

    /// Note input. Returns whether it woke Big Picture from the idle screen
    /// (the input is then spent).
    pub(super) fn note_input(&mut self, cx: &mut Context<Self>) -> bool {
        self.idle.last_input = Instant::now();
        let woke = std::mem::take(&mut self.idle.showing);
        if woke {
            cx.notify();
        }
        if woke || self.idle.task.is_none() {
            self.schedule_idle(cx);
        }
        woke
    }

    /// Start the timer, or stop it when the idle screen is off.
    pub(super) fn schedule_idle(&mut self, cx: &mut Context<Self>) {
        if Self::idle_prefs(cx).idle_after().is_none() {
            self.idle.task = None;
            if self.idle.showing {
                self.idle.showing = false;
                cx.notify();
            }
            return;
        }
        self.idle.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let step = this.update(cx, |this, cx| {
                    let prefs = Self::idle_prefs(cx);
                    let step = next(
                        prefs.idle_after(),
                        this.idle.showing,
                        prefs.idle_screen,
                        this.idle.last_input,
                        Instant::now(),
                    );
                    match step {
                        Next::Show => {
                            this.idle.showing = true;
                            this.idle.slide = 0;
                            cx.notify();
                        }
                        Next::Slide => {
                            this.idle.slide = this.idle.slide.wrapping_add(1);
                            cx.notify();
                        }
                        Next::Stop | Next::Wait(_) => {}
                    }
                    step
                });
                let wait = match step {
                    Ok(Next::Wait(wait)) => wait,
                    Ok(Next::Show) | Ok(Next::Slide) => SLIDE,
                    Ok(Next::Stop) | Err(_) => break,
                };
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    /// The idle screen over everything, while it shows.
    pub(super) fn render_idle(&self, c: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.idle.showing {
            return None;
        }
        let screen = Self::idle_prefs(cx).idle_screen;
        let games: Vec<_> = match screen {
            IdleScreen::Slideshow => self
                .rows
                .iter()
                .flat_map(|row| row.games.iter())
                .filter(|g| g.hero_art().is_some())
                .collect(),
            IdleScreen::Dim => Vec::new(),
        };
        let game = (!games.is_empty()).then(|| games[self.idle.slide % games.len()].clone());
        let art = game
            .as_ref()
            .and_then(|g| g.hero_art())
            .and_then(|url| art_img(url, ART_WIDTH, false, cx));
        let caption = div()
            .absolute()
            .left(rems(4.))
            .bottom(rems(3.5))
            .right(rems(4.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_3xl()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child(clock_text()),
            )
            .when_some(game.as_ref(), |d, g| {
                d.child(
                    div()
                        .text_xl()
                        .text_color(c.muted_foreground)
                        .truncate()
                        .child(g.title.clone()),
                )
            });
        Some(
            div()
                .id("big-picture-idle")
                .absolute()
                .inset_0()
                .size_full()
                .occlude()
                .bg(c.background.opacity(if art.is_some() { 1. } else { 0.92 }))
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.note_input(cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, _: &gpui_kit::MouseMoveEvent, _, cx| {
                    this.note_input(cx);
                }))
                .when_some(art, |d, img| {
                    d.child(
                        img.absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .opacity(0.8),
                    )
                })
                // The clock and title read over the art's bottom.
                .child(div().absolute().inset_0().size_full().bg(linear_gradient(
                    0.,
                    linear_color_stop(c.background.opacity(0.9), 0.),
                    linear_color_stop(c.background.opacity(0.), 0.5),
                )))
                .child(caption)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_waits_for_what_is_left_and_shows_once_due() {
        let t0 = Instant::now();
        let five = Some(Duration::from_secs(300));
        let at = |s| t0 + Duration::from_secs(s);
        assert_eq!(
            next(five, false, IdleScreen::Slideshow, t0, at(100)),
            Next::Wait(Duration::from_secs(200))
        );
        assert_eq!(next(five, false, IdleScreen::Dim, t0, at(300)), Next::Show);
        assert_eq!(
            next(five, true, IdleScreen::Slideshow, t0, at(400)),
            Next::Slide
        );
        assert_eq!(next(five, true, IdleScreen::Dim, t0, at(400)), Next::Stop);
        assert_eq!(
            next(None, false, IdleScreen::Dim, t0, at(999)),
            Next::Stop,
            "off"
        );
    }
}
