//! The stats page, in three tabs over the art of the period's game:
//!
//! - **Overview**: play time in a week, month, year or all time, against the
//!   period before; days played, streak and sessions; a chart stacked by
//!   game; insights (games played side by side, the backlog goal, a game
//!   picked up again, new games); the most played games as covers.
//! - **Backlog**: the games beaten and abandoned in a year on a timeline, the
//!   yearly goal and its pace, and the library grouped by play status.
//! - **Album**: the screenshots the user took in the period, by game.
//! - **Journal**: the notes the user wrote on days in the period.
//!
//! The Year in review opens from the controls (and is offered on the
//! overview through December and January); it is its own full-window route.
//!
//! The overview ends with the library's achievements: completion, perfect
//! games, points, the rarest and the latest unlocks.
//!
//! Below both, the storage section shows the installed games by drive.
//!
//! The session history and the screenshots are read off the UI thread when a
//! session ends; the numbers are derived from them when they, the library or
//! the chosen period change, never while drawing.

mod achievements_view;
pub(crate) mod album;
mod album_view;
pub(crate) mod backlog;
mod backlog_view;
mod hero;
pub(crate) mod journal;
mod journal_view;
pub(crate) mod model;
pub(crate) mod overview;
mod overview_view;
pub(crate) mod period;
pub(crate) mod storage;
mod storage_view;
mod text;
pub(crate) mod year;
mod year_card;
mod year_stories;
pub(crate) mod year_view;

use std::rc::Rc;
use std::sync::Arc;

use chrono::{Datelike as _, Local, NaiveDate};
use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{VirtualListScrollHandle, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, Size, Styled as _, Subscription, Window, div, px,
};
use irori_core::GameId;
use irori_services::{ScreenshotIndex, UserData};
use irori_ui::art::LruImageCache;
use irori_ui::components::EmptyState;
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use album::{Album, Shot};
use album_view::{AlbumItem, GridLayout};
use backlog::Backlog;
use model::History;
use overview::Overview;
use period::{Period, Span};
use storage::{DriveUsage, InstalledGame};

/// Which half of the page is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Backlog,
    Album,
    Journal,
}

pub struct StatsView {
    tab: Tab,
    span: Span,
    /// The period picked with ‹ ›; `None` shows the current one of `span`,
    /// which moves on by itself as days pass.
    picked: Option<Period>,
    /// The backlog's year; `None` is the current one.
    year: Option<i32>,
    /// `None` until the history is first read.
    history: Option<Arc<History>>,
    overview: Option<Overview>,
    backlog: Option<Backlog>,
    /// The user's screenshots; `None` until first read.
    screenshots: Option<Arc<ScreenshotIndex>>,
    album: Option<Album>,
    /// The game the album shows alone.
    album_game: Option<GameId>,
    /// The screenshots the album's grid shows.
    album_shown: Arc<Vec<Shot>>,
    /// What the album's list holds, and each entry's size.
    album_items: Rc<Vec<AlbumItem>>,
    album_sizes: Rc<Vec<Size<Pixels>>>,
    album_layout: GridLayout,
    /// The album's width, measured while drawing.
    album_width: Pixels,
    album_scroll: VirtualListScrollHandle,
    /// The journal tab's notes; `None` until the history is first read.
    journal: Option<Arc<journal::JournalPage>>,
    journal_sizes: Rc<Vec<Size<Pixels>>>,
    journal_scroll: VirtualListScrollHandle,
    /// Years with play, newest first, for the Year in review.
    years: Vec<i32>,
    /// Installed games by drive; `None` until first read.
    storage: Option<Vec<DriveUsage>>,
    /// Every game's achievements, summed up; `None` until first read.
    achievements: Option<Arc<irori_services::AchievementIndex>>,
    /// "Check played games" is asking the stores.
    achievements_scanning: bool,
    /// Library revision and user data the numbers were derived from.
    seen: Option<(u64, Arc<UserData>)>,
    images: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl StatsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sessions = models(cx).sessions.clone();
        let library = models(cx).library.clone();
        let subscriptions = vec![
            // A finished session adds to the history.
            cx.observe_in(&sessions, window, |this, _, _, cx| this.reload(cx)),
            // A library refresh (which games count), playtime synced from the
            // stores and play statuses change the numbers too; the model
            // notifies for much else.
            cx.observe_in(&library, window, |this, library, _, cx| {
                let l = library.read(cx);
                let same = this.seen.as_ref().is_some_and(|(revision, user)| {
                    *revision == l.snapshot.revision && Arc::ptr_eq(user, &l.user)
                });
                if !same {
                    this.derive(cx);
                    this.reload_storage(cx);
                }
            }),
        ];
        let mut view = Self {
            tab: Tab::Overview,
            span: Span::Month,
            picked: None,
            year: None,
            history: None,
            overview: None,
            backlog: None,
            screenshots: None,
            album: None,
            album_game: None,
            album_shown: Arc::default(),
            album_items: Rc::default(),
            album_sizes: Rc::default(),
            album_layout: GridLayout::for_width(0.),
            album_width: px(0.),
            album_scroll: VirtualListScrollHandle::new(),
            journal: None,
            journal_sizes: Rc::default(),
            journal_scroll: VirtualListScrollHandle::new(),
            years: Vec::new(),
            storage: None,
            achievements: None,
            achievements_scanning: false,
            seen: None,
            images: LruImageCache::new(48, cx),
            _subscriptions: subscriptions,
        };
        view.reload(cx);
        view.reload_storage(cx);
        view
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    /// The period the overview shows.
    fn period(&self) -> Period {
        self.picked.unwrap_or_else(|| {
            let first = self.history.as_ref().and_then(|h| h.first_day());
            Period::containing(self.span, Self::today(), first)
        })
    }

    fn year(&self) -> i32 {
        self.year.unwrap_or_else(|| Self::today().year())
    }

    /// Read the history and the screenshots (off the UI thread), then derive
    /// the numbers.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_achievements(cx);
        let b = backend(cx);
        let shots = irori_ui::state::run_infallible(cx, {
            let b = b.clone();
            async move {
                let task = b.spawn_blocking({
                    let b = b.clone();
                    move || b.screenshots()
                });
                task.await.ok().map(Arc::new)
            }
        });
        cx.spawn(async move |this, cx| {
            if let Some(Some(index)) = shots.await {
                let _ = this.update(cx, |this, cx| {
                    this.screenshots = Some(index);
                    this.derive(cx);
                });
            }
        })
        .detach();
        let task = irori_ui::state::run_infallible(cx, async move {
            Arc::new(History::new(b.sessions(), model::local_day))
        });
        cx.spawn(async move |this, cx| {
            if let Some(history) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.history = Some(history);
                    this.derive(cx);
                });
            }
        })
        .detach();
    }

    /// Derive the overview and backlog of the chosen period and year.
    fn derive(&mut self, cx: &mut Context<Self>) {
        let (snapshot, user) = {
            let library = models(cx).library.read(cx);
            (library.snapshot.clone(), library.user.clone())
        };
        self.seen = Some((snapshot.revision, user.clone()));
        let Some(history) = self.history.clone() else {
            return;
        };
        let today = Self::today();
        self.overview = Some(overview::compute(
            &history,
            self.period(),
            &user.playtime,
            |id| snapshot.get(id).is_some(),
            today,
        ));
        self.journal = Some(Arc::new(journal::compute(
            &user.journal,
            &history,
            self.period(),
            |id| snapshot.get(id).is_some(),
            today,
        )));
        let mut years: Vec<i32> = history.sessions.iter().map(|p| p.day.year()).collect();
        years.dedup();
        years.reverse();
        self.years = years;
        let keys: Vec<String> = snapshot.games.iter().map(|g| g.id().key()).collect();
        self.backlog = Some(backlog::compute(
            &history,
            &user,
            keys.iter().map(String::as_str),
            self.year(),
            today,
        ));
        if let Some(index) = &self.screenshots {
            self.album = Some(album::compute(
                index,
                self.period(),
                |id| snapshot.get(id).is_some(),
                album::local_day,
                today,
            ));
            self.refresh_album_shown();
        }
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn set_span(&mut self, span: Span, cx: &mut Context<Self>) {
        self.span = span;
        self.picked = None;
        self.derive(cx);
    }

    /// Show the period or year before (`-1`) or after (`1`).
    fn step(&mut self, by: i32, cx: &mut Context<Self>) {
        let today = Self::today();
        match self.tab {
            Tab::Overview | Tab::Album | Tab::Journal => {
                let period = self.period();
                let to = if by < 0 {
                    period.previous()
                } else {
                    period.next(today)
                };
                if let Some(to) = to {
                    self.picked = (!to.is_current(today)).then_some(to);
                }
            }
            Tab::Backlog => {
                let year = self.year() + by;
                self.year = (year != today.year()).then_some(year);
            }
        }
        self.derive(cx);
    }

    /// Whether ‹ and › lead anywhere: back while the history (or a status
    /// date, or a screenshot) reaches further, forward up to today.
    fn can_step(&self, cx: &Context<Self>) -> (bool, bool) {
        let today = Self::today();
        let mut first = self.history.as_ref().and_then(|h| h.first_day());
        if self.tab == Tab::Album {
            let oldest = self
                .screenshots
                .as_ref()
                .and_then(|i| i.shots.last())
                .and_then(|s| album::local_day(s.taken));
            first = first.into_iter().chain(oldest).min();
        }
        if self.tab == Tab::Journal {
            let user = models(cx).library.read(cx).user.clone();
            first = first
                .into_iter()
                .chain(journal::first_day(&user.journal))
                .min();
        }
        match self.tab {
            Tab::Overview | Tab::Album | Tab::Journal => {
                let period = self.period();
                let back = period
                    .previous()
                    .zip(first)
                    .is_some_and(|(before, first)| before.end(today) > first);
                let forward = period.next(today).is_some_and(|after| after.start <= today);
                (back, forward)
            }
            Tab::Backlog => {
                let user = models(cx).library.read(cx).user.clone();
                let earliest = user
                    .play_status
                    .values()
                    .filter_map(|r| model::rfc3339_day(&r.since))
                    .chain(first)
                    .map(|d| d.year())
                    .min()
                    .unwrap_or(today.year());
                (self.year() > earliest, self.year() < today.year())
            }
        }
    }

    /// Read the drives and where the installed games are (off the UI thread).
    fn reload_storage(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let installed: Vec<(irori_core::GameId, std::path::PathBuf, u64)> = models(cx)
            .library
            .read(cx)
            .snapshot
            .games
            .iter()
            .filter(|g| g.is_installed)
            .filter_map(|g| {
                let bytes = g.install.as_ref().map_or(0, |i| i.install_size_bytes);
                Some((g.id(), g.install_path()?, bytes))
            })
            .collect();
        let task = irori_ui::state::run_infallible(cx, async move {
            b.spawn_blocking(move || {
                let games: Vec<InstalledGame> = installed
                    .into_iter()
                    .map(|(id, path, bytes)| InstalledGame {
                        id,
                        path: storage::resolve(&path),
                        bytes,
                    })
                    .collect();
                storage::group(&games, &storage::read_drives())
            })
            .await
            .ok()
        });
        cx.spawn(async move |this, cx| {
            if let Some(Some(drives)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.storage = Some(drives);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Whether there is nothing to show at all: no play, no statuses, no
    /// screenshots.
    fn is_blank(&self, cx: &Context<Self>) -> bool {
        let user = &models(cx).library.read(cx).user;
        self.history.as_ref().is_some_and(|h| h.sessions.is_empty())
            && user.playtime.values().all(|r| r.total_minutes == 0)
            && user.play_status.is_empty()
            && user.journal.is_empty()
            && self.screenshots.as_ref().is_none_or(|s| s.shots.is_empty())
    }
}

impl Render for StatsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An album with screenshots is a list of its own that scrolls the
        // whole page; the storage section belongs to the other tabs.
        let album_page = self.tab == Tab::Album
            && self.history.is_some()
            && self.album.as_ref().is_some_and(|a| !a.shots.is_empty())
            && !self.is_blank(cx);
        if album_page {
            return v_flex()
                .size_full()
                .min_h_0()
                .image_cache(self.images.clone())
                .child(self.render_album_page(window, cx));
        }
        // So is a journal with notes.
        let journal_page = self.tab == Tab::Journal
            && self.history.is_some()
            && self.journal.as_ref().is_some_and(|j| !j.notes.is_empty());
        if journal_page {
            return v_flex()
                .size_full()
                .min_h_0()
                .image_cache(self.images.clone())
                .child(self.render_journal_page(window, cx));
        }
        let body: AnyElement = if self.history.is_none() {
            div()
                .flex()
                .justify_center()
                .py_12()
                .child(Spinner::new())
                .into_any_element()
        } else if self.is_blank(cx) {
            v_flex()
                .gap_6()
                .child(self.render_controls(cx))
                .child(
                    EmptyState::new(IconName::ChartColumn, t("library.stats.empty.title"))
                        .description(t("library.stats.empty.description")),
                )
                .into_any_element()
        } else {
            match self.tab {
                Tab::Overview => v_flex()
                    .gap_6()
                    .children(self.render_year_offer(cx))
                    .child(self.render_overview(window, cx))
                    .into_any_element(),
                Tab::Backlog => self.render_backlog(window, cx),
                Tab::Album => self.render_album(window, cx),
                Tab::Journal => self.render_journal(window, cx),
            }
        };
        let storage = self
            .storage
            .clone()
            .filter(|d| !d.is_empty())
            .map(|d| self.render_storage(&d, cx));
        v_flex()
            .size_full()
            .min_h_0()
            .image_cache(self.images.clone())
            .child(
                div()
                    .id("stats-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(v_flex().p_6().gap_6().child(body).children(storage)),
            )
    }
}
