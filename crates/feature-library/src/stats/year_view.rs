//! The Year in review: a year's play told as full-window stories over the
//! art of the games each is about, stepped through with the arrow keys,
//! Space, a click (the left third goes back) or a controller, and closed
//! with Escape. The last story sums the year up and saves it as a PNG to
//! share. Any year with play can be picked.
//!
//! The history is read and the numbers derived off the UI thread; each
//! story arrives once, with one transition, and not at all while the user
//! reduces motion.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Local, NaiveDateTime};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, relative, rems,
};
use irori_core::GameId;
use irori_services::{LibrarySnapshot, UserData};
use irori_ui::art::{Art, LruImageCache, art_store};
use irori_ui::components::{ArtBackdrop, BackdropInk, EmptyState};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, nav};

use super::text;
use super::year::{self, Story, Timed, YearReview};
use super::year_card::{self, CardColors, CardGame, CardInput};
use super::year_stories::{percent, title};

/// Key context of the review, so its keys stay its own.
pub const YEAR_REVIEW_CONTEXT: &str = "YearReview";
/// Most games the card shows.
const CARD_GAMES: usize = 5;
/// How far a story's content rises as it arrives, in pixels.
const RISE_PX: f32 = 18.;

/// The history, read once.
struct Loaded {
    sessions: Arc<Vec<Timed>>,
    /// Years with play, newest first.
    years: Vec<i32>,
}

pub struct YearReviewView {
    focus: FocusHandle,
    /// The year asked for (a link names one); `None` lets the review pick.
    requested: Option<i32>,
    loaded: Option<Loaded>,
    review: Option<Arc<YearReview>>,
    stories: Vec<Story>,
    index: usize,
    /// The card is being drawn or written.
    pub(super) saving: bool,
    images: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl YearReviewView {
    pub fn new(year: Option<i32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let handle = focus.clone();
        window.defer(cx, move |window, cx| handle.focus(window, cx));
        let store = art_store(cx);
        let mut view = Self {
            focus,
            requested: year,
            loaded: None,
            review: None,
            stories: Vec::new(),
            index: 0,
            saving: false,
            images: LruImageCache::new(32, cx),
            // Art arriving redraws the covers and the card's inputs.
            _subscriptions: vec![cx.observe(&store, |_, _, cx| cx.notify())],
        };
        view.load(cx);
        view
    }

    /// Read the history and derive the opening year, off the UI thread.
    fn load(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let (snapshot, user) = library(cx);
        let requested = self.requested;
        let today = Local::now().date_naive();
        let task = irori_ui::state::run_infallible(cx, async move {
            let job = b.spawn_blocking({
                let b = b.clone();
                move || {
                    let sessions = Arc::new(year::timed(b.visible_sessions(), local_time));
                    let years = year::years(&sessions);
                    let year = requested
                        .filter(|y| years.contains(y))
                        .or_else(|| year::default_year(today, &years));
                    let review = year.map(|y| Arc::new(compute(&sessions, y, &user, &snapshot)));
                    (Loaded { sessions, years }, review)
                }
            });
            job.await.ok()
        });
        cx.spawn(async move |this, cx| {
            if let Some(Some((loaded, review))) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.loaded = Some(loaded);
                    this.show(review, cx);
                });
            }
        })
        .detach();
    }

    /// Switch to `year`, deriving its numbers off the UI thread.
    fn pick_year(&mut self, year: i32, cx: &mut Context<Self>) {
        let Some(sessions) = self.loaded.as_ref().map(|l| l.sessions.clone()) else {
            return;
        };
        let (snapshot, user) = library(cx);
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            b.spawn_blocking(move || Arc::new(compute(&sessions, year, &user, &snapshot)))
                .await
                .ok()
        });
        cx.spawn(async move |this, cx| {
            if let Some(Some(review)) = task.await {
                let _ = this.update(cx, |this, cx| this.show(Some(review), cx));
            }
        })
        .detach();
    }

    fn show(&mut self, review: Option<Arc<YearReview>>, cx: &mut Context<Self>) {
        self.stories = review.as_deref().map(year::stories).unwrap_or_default();
        self.review = review;
        self.index = 0;
        cx.notify();
    }

    fn year(&self) -> Option<i32> {
        self.review.as_ref().map(|r| r.year)
    }

    /// The year before (`-1`) or after (`1`) the shown one that has play.
    fn neighbour_year(&self, by: i32) -> Option<i32> {
        let (years, year) = (&self.loaded.as_ref()?.years, self.year()?);
        // Years are newest first.
        let ix = years.iter().position(|y| *y == year)?;
        if by < 0 {
            years.get(ix + 1).copied()
        } else {
            ix.checked_sub(1).and_then(|i| years.get(i).copied())
        }
    }

    /// Go to the story before (`-1`) or after (`1`). The last story stays
    /// until the review is closed, so a stray click does not lose it.
    fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.stories.is_empty() {
            return;
        }
        let last = self.stories.len() - 1;
        self.index = self.index.saturating_add_signed(by).min(last);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn go_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.index = index.min(self.stories.len().saturating_sub(1));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if nav::navigator(cx).read(cx).can_go_back() {
            nav::go_back(cx);
        } else {
            nav::navigate(Route::Stats, cx);
        }
    }

    /// Arrows, Home, End and Escape work wherever the focus is inside the
    /// review; Space and Enter only when no button has it, so they still
    /// press buttons.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Alt+Left and the like stay the shell's.
        if event.keystroke.modifiers.modified() {
            return;
        }
        let on_page = self.focus.is_focused(window);
        match event.keystroke.key.as_str() {
            "left" | "pageup" => self.step(-1, window, cx),
            "right" | "pagedown" => self.step(1, window, cx),
            "space" | "enter" if on_page => self.step(1, window, cx),
            "home" => self.go_to(0, cx),
            "end" => self.go_to(usize::MAX, cx),
            "escape" => self.close(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Ask where to save the summary card, then draw and write it off the UI
    /// thread.
    pub(super) fn save_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.review.clone() else {
            return;
        };
        let input = card_input(&review, window, cx);
        let name = t_args(
            "library.year.card.file_name",
            &[("year", &review.year.to_string())],
        );
        let dir = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{name}.png")));
        self.saving = true;
        cx.notify();
        let b = backend(cx);
        cx.spawn_in(window, async move |this, cx| {
            let path: Option<PathBuf> = match rx.await {
                Ok(Ok(path)) => path,
                _ => None,
            };
            let result = match path {
                Some(path) => {
                    let task = cx.update(|_, cx| {
                        let b = b.clone();
                        irori_ui::run(cx, async move {
                            b.spawn_blocking(move || {
                                let png = year_card::render_png(&input, irori_ui::theme::FONTS)?;
                                irori_core::json_store::write_bytes(&path, &png)?;
                                anyhow::Ok(path)
                            })
                            .await?
                        })
                    });
                    match task {
                        Ok(task) => Some(task.await),
                        Err(e) => Some(Err(e)),
                    }
                }
                None => None,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Some(Ok(path)) => {
                        let file = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        window.push_notification(
                            Notification::success(t_args(
                                "library.year.card.saved",
                                &[("file", &file)],
                            )),
                            cx,
                        );
                        cx.reveal_path(&path);
                    }
                    Some(Err(e)) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("library.year.card.save_failed")),
                        cx,
                    ),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The library and user data the numbers are derived from.
fn library(cx: &App) -> (Arc<LibrarySnapshot>, Arc<UserData>) {
    let l = models(cx).library.read(cx);
    (l.snapshot.clone(), l.user.clone())
}

fn compute(
    sessions: &[Timed],
    year: i32,
    user: &UserData,
    snapshot: &LibrarySnapshot,
) -> YearReview {
    year::compute(sessions, year, user, |id| {
        snapshot.get(id).and_then(|g| g.system.clone())
    })
}

/// An RFC 3339 time in local time.
fn local_time(time: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(time)
        .ok()
        .map(|t| t.with_timezone(&Local).naive_local())
}

/// The art a story is told over.
fn story_art(story: Story, review: &YearReview, sessions: &[Timed]) -> Option<GameId> {
    let top = review.games.first().map(|g| g.id.clone());
    match story {
        Story::Months => review
            .busiest_month()
            .and_then(|m| review.top_of_month(sessions, m).cloned())
            .or(top),
        Story::Records => review
            .longest_session
            .as_ref()
            .map(|s| s.game.clone())
            .or(top),
        Story::Comebacks => review.comebacks.first().map(|c| c.id.clone()).or(top),
        Story::NewAndBeaten => review
            .beaten
            .last()
            .map(|(id, _)| id.clone())
            .or_else(|| review.new_games.first().cloned())
            .or(top),
        Story::Numbers | Story::Rhythm | Story::Stores => {
            review.games.get(1).map(|g| g.id.clone()).or(top)
        }
        _ => top,
    }
}

/// An sRGB triple of a theme colour.
fn rgb(color: Hsla) -> [u8; 3] {
    let c = color.to_rgb();
    [c.r, c.g, c.b].map(|v| (v.clamp(0., 1.) * 255.).round() as u8)
}

/// Everything the summary card shows, gathered on the UI thread: words in
/// the interface language, theme colours and the art already on disk.
fn card_input(review: &YearReview, window: &mut Window, cx: &mut App) -> CardInput {
    let snapshot = models(cx).library.read(cx).snapshot.clone();
    let theme = cx.theme();
    let colors = CardColors {
        background: rgb(theme.background),
        foreground: rgb(theme.foreground),
        muted: rgb(theme.muted_foreground),
        accent: rgb(irori_ui::style::legible(
            theme.primary,
            &[theme.background],
            irori_ui::style::TEXT_CONTRAST,
            cx,
        )),
        surface: rgb(theme.secondary),
    };
    // The size the ranking story drew them at, so they are on disk by now.
    let cover_width = irori_ui::components::physical_width(rems(3.5), window);
    let store = art_store(cx);
    let mut art_path = |url: Option<&str>, width: u32| {
        let url = url?;
        match store.update(cx, |s, cx| s.get(url, width, false, cx)) {
            Art::Ready(path) => Some(path.to_path_buf()),
            _ => None,
        }
    };
    let games: Vec<CardGame> = review
        .games
        .iter()
        .take(CARD_GAMES)
        .map(|g| CardGame {
            title: title(&g.id, &snapshot).to_string(),
            detail: format!(
                "{} · {}",
                text::hours(g.seconds / 60),
                percent(review.share(g))
            ),
            share: review.share(g),
            cover: art_path(snapshot.get(&g.id).and_then(|g| g.cover_art()), cover_width),
        })
        .collect();
    let backdrop = review
        .games
        .first()
        .and_then(|g| snapshot.get(&g.id))
        .and_then(|g| g.hero_art())
        .and_then(|url| irori_ui::art::ambient(url, cx))
        .map(|a| a.backdrop.to_path_buf());
    let mut facts = vec![
        (
            review.days_played.to_string(),
            text::count_word(
                "library.year.word.days_played_one",
                "library.year.word.days_played_other",
                review.days_played.into(),
            )
            .to_string(),
        ),
        (
            review.games.len().to_string(),
            text::count_word(
                "library.year.word.games_one",
                "library.year.word.games_other",
                review.games.len() as u64,
            )
            .to_string(),
        ),
        (
            review.sessions.to_string(),
            text::count_word(
                "library.year.word.sessions_one",
                "library.year.word.sessions_other",
                review.sessions.into(),
            )
            .to_string(),
        ),
    ];
    if let Some(run) = review.longest_streak.filter(|r| r.days > 1) {
        facts.push((
            run.days.to_string(),
            t("library.year.word.day_streak").to_string(),
        ));
    }
    CardInput {
        colors,
        eyebrow: t("library.year.eyebrow").to_string(),
        year: review.year.to_string(),
        headline: t_args(
            "library.year.card.headline",
            &[("hours", &text::hours(review.seconds / 60))],
        )
        .to_string(),
        facts,
        games_title: t("library.year.top_games.eyebrow").to_string(),
        games,
        backdrop,
        brand: "Irori".into(),
    }
}

impl Render for YearReviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        let story = self.stories.get(self.index).copied();
        let art_game = match (&self.review, &self.loaded, story) {
            (Some(r), Some(l), Some(s)) => story_art(s, r, &l.sessions),
            _ => None,
        };
        let art = art_game
            .as_ref()
            .and_then(|id| snapshot.get(id))
            .and_then(|g| g.hero_art())
            .map(str::to_owned);
        let ink = BackdropInk::of(art.as_deref(), cx);
        let body = match (&self.loaded, self.review.clone(), story) {
            (None, _, _) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().large())
                .into_any_element(),
            (Some(_), Some(review), Some(story)) => {
                let content = self.render_story(story, &review, &snapshot, ink, cx);
                irori_ui::motion::rise_in(
                    SharedString::from(format!("year-story-{}-{}", review.year, self.index)),
                    div().size_full().child(content),
                    RISE_PX,
                )
            }
            _ => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    EmptyState::new(IconName::Sparkles, t("library.year.empty.title"))
                        .description(t("library.year.empty.description")),
                )
                .child(
                    Button::new("year-empty-close")
                        .outline()
                        .label(t("library.year.close"))
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
                .into_any_element(),
        };
        let has_stories = !self.stories.is_empty();
        div()
            .image_cache(self.images.clone())
            .id("year-review")
            .key_context(YEAR_REVIEW_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.on_key(event, window, cx)
                }),
            )
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(ink.text)
            .child(ArtBackdrop::new(art.clone()).image_cache(&self.images))
            // Click anywhere to go on; the left third goes back. Content
            // drawn over these has no pointer handlers of its own, so clicks
            // reach them; the bars and button rows over them occlude them.
            .when(has_stories, |d| {
                d.child(
                    div()
                        .id("year-zone-back")
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(0.3))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
                )
                .child(
                    div()
                        .id("year-zone-next")
                        .absolute()
                        .top_0()
                        .right_0()
                        .h_full()
                        .w(relative(0.7))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
                )
            })
            .child(
                v_flex()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(self.render_top(ink, cx))
                    .child(div().flex_1().min_h_0().px_10().py_4().child(body))
                    .child(self.render_bottom(ink, cx)),
            )
    }
}

impl YearReviewView {
    /// The story segments, the year picker and the close button.
    fn render_top(&self, ink: BackdropInk, cx: &mut Context<Self>) -> AnyElement {
        let segments = (0..self.stories.len()).map(|ix| {
            let done = ix <= self.index;
            div()
                .id(("year-segment", ix))
                .flex_1()
                .h(px(4.))
                .rounded_full()
                .cursor_pointer()
                .bg(if done {
                    ink.text
                } else {
                    ink.text.opacity(0.25)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.go_to(ix, cx)))
        });
        let year = self.year().map(|y| y.to_string()).unwrap_or_default();
        let (before, after) = (self.neighbour_year(-1), self.neighbour_year(1));
        let picker = (self.loaded.as_ref().is_some_and(|l| l.years.len() > 1)).then(|| {
            h_flex()
                .gap_1()
                .items_center()
                .rounded_full()
                .px_1()
                .bg(ink.panel)
                .child(
                    Button::new("year-previous")
                        .ghost()
                        .xsmall()
                        .rounded_full()
                        .icon(Icon::new(IconName::ChevronLeft))
                        .accessibility_label(t("library.year.previous_year"))
                        .tooltip(t("library.year.previous_year"))
                        .disabled(before.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(y) = before {
                                this.pick_year(y, cx);
                            }
                        })),
                )
                .child(
                    div()
                        .min_w(rems(3.5))
                        .text_center()
                        .text_sm()
                        .font_semibold()
                        .child(year.clone()),
                )
                .child(
                    Button::new("year-next")
                        .ghost()
                        .xsmall()
                        .rounded_full()
                        .icon(Icon::new(IconName::ChevronRight))
                        .accessibility_label(t("library.year.next_year"))
                        .tooltip(t("library.year.next_year"))
                        .disabled(after.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(y) = after {
                                this.pick_year(y, cx);
                            }
                        })),
                )
        });
        // The bars take their own clicks: the zones behind them do not step.
        v_flex()
            .id("year-top")
            .occlude()
            .w_full()
            .px_10()
            .pt_6()
            .gap_4()
            .child(h_flex().w_full().gap_1().children(segments))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .font_semibold()
                            .text_color(ink.muted)
                            .child(Icon::new(IconName::Sparkles).small())
                            .child(div().min_w_0().truncate().child(t("library.year.eyebrow"))),
                    )
                    .children(picker)
                    .child(
                        Button::new("year-close")
                            .ghost()
                            .small()
                            .rounded_full()
                            .icon(Icon::new(IconName::X))
                            .accessibility_label(t("library.year.close"))
                            .tooltip(t("library.year.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .into_any_element()
    }

    /// How to move on, and buttons that do it.
    fn render_bottom(&self, ink: BackdropInk, cx: &mut Context<Self>) -> AnyElement {
        if self.stories.is_empty() {
            return div().into_any_element();
        }
        let last = self.index + 1 == self.stories.len();
        h_flex()
            .id("year-bottom")
            .occlude()
            .w_full()
            .px_10()
            .pb_6()
            .gap_3()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(ink.muted)
                    .child(t("library.year.hint")),
            )
            .child(div().text_xs().text_color(ink.muted).child(format!(
                "{} / {}",
                self.index + 1,
                self.stories.len()
            )))
            .child(
                Button::new("year-back")
                    .ghost()
                    .small()
                    .rounded_full()
                    .icon(Icon::new(IconName::ChevronLeft))
                    .accessibility_label(t("library.year.back"))
                    .tooltip(t("library.year.back"))
                    .disabled(self.index == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
            )
            .child(
                Button::new("year-forward")
                    .ghost()
                    .small()
                    .rounded_full()
                    .icon(Icon::new(if last {
                        IconName::X
                    } else {
                        IconName::ChevronRight
                    }))
                    .accessibility_label(t(if last {
                        "library.year.close"
                    } else {
                        "library.year.forward"
                    }))
                    .tooltip(t(if last {
                        "library.year.close"
                    } else {
                        "library.year.forward"
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if last {
                            this.close(cx)
                        } else {
                            this.step(1, window, cx)
                        }
                    })),
            )
            .into_any_element()
    }
}
