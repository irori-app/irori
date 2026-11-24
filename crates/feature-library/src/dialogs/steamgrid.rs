//! "Find art online": search SteamGridDB (or, with `iroriIgdb` on, IGDB)
//! for a game and pick a cover, square or background image. Embedded in the
//! "Add game", "Edit app" and "Edit details" dialogs in place of their form
//! while open.
//!
//! Without an API key the picker explains how to get one and where to enter
//! it. Nothing is searched until the user opens the picker (Heroic looked up
//! art on a third-party server whenever the title field lost focus). IGDB
//! has covers, artworks and screenshots but no square art or logos.
//!
//! With both sources on, one search asks both, each taking the game its
//! search means, and pills show one source's pictures. Every picture says
//! its size: SteamGridDB gives it, the others' is read from their first
//! bytes ([`irori_extras::image_size`]).
//!
//! A ROM's cover opens on LaunchBox's box art instead: every region's box of
//! the game the ROM library matched, its own region first, each labelled
//! with its region; other searches look through the system's LaunchBox
//! games, and "All sources" puts its boxes first. Without LaunchBox's game
//! list yet, it offers to download it, which turns `iroriLaunchBox` on (its
//! text says so).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown};
use gpui_kit::component::link::Link;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels,
    Rems, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, Window, canvas, div, rems,
};
use irori_core::{GameId, Runner};
use irori_emulation::LaunchBoxStatus;
use irori_emulation::launchbox::LaunchBoxGame;
use irori_extras::igdb::{IgdbGame, ImageSize, image_url};
use irori_extras::steamgrid::{self, ApiKey, ImageFilter, SgdbGame, SgdbImage};
use irori_ui::art_picker::{ArtSlot, OnPicked};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

/// Where SteamGridDB users create an API key.
const API_KEY_PAGE: &str = "https://www.steamgriddb.com/profile/preferences/api";
/// Images shown per game; the rest are rarely useful and cost downloads.
const MAX_IMAGES: usize = 60;
/// Pictures whose size is read at once.
const SIZE_READS: usize = 4;

/// Which artwork to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtKind {
    /// Vertical 2:3 library cover (SteamGridDB "grids").
    Cover,
    /// Square artwork (square "grids").
    Square,
    /// Wide background (SteamGridDB "heroes").
    Background,
    /// The game's name as artwork, usually transparent (SteamGridDB "logos").
    Logo,
}

impl ArtKind {
    /// Listing filter for this kind.
    pub fn filter(self) -> ImageFilter {
        let dimensions: &[&str] = match self {
            ArtKind::Cover => &["600x900", "342x482", "660x930"],
            ArtKind::Square => &["512x512", "1024x1024"],
            ArtKind::Background | ArtKind::Logo => &[],
        };
        ImageFilter {
            dimensions: dimensions.iter().map(|d| d.to_string()).collect(),
            styles: Vec::new(),
        }
    }

    /// "No covers found for “Hades”."
    pub fn none_found(self, game: &str) -> SharedString {
        let key = match self {
            ArtKind::Cover => "library.art.none_found.cover",
            ArtKind::Square => "library.art.none_found.square",
            ArtKind::Background => "library.art.none_found.background",
            ArtKind::Logo => "library.art.none_found.logo",
        };
        t_args(key, &[("name", game)])
    }

    /// Dialog title while picking.
    pub fn title(self) -> SharedString {
        t(match self {
            ArtKind::Cover => "library.art.title.cover",
            ArtKind::Square => "library.art.title.square",
            ArtKind::Background => "library.art.title.background",
            ArtKind::Logo => "library.art.title.logo",
        })
    }

    /// Thumbnail width and height, in rem.
    fn thumb_rem(self) -> (f32, f32) {
        match self {
            ArtKind::Cover => (6.0, 9.0),
            ArtKind::Square => (6.0, 6.0),
            ArtKind::Background => (12.5, 4.0),
            ArtKind::Logo => (9.0, 4.0),
        }
    }
}

/// Where the picker searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtSource {
    /// Every source that can be searched, each taking its best match.
    All,
    SteamGridDb,
    Igdb,
    /// LaunchBox's box art, for a ROM's cover.
    LaunchBox,
}

impl ArtSource {
    /// The source's name, which is never translated.
    fn name(self) -> SharedString {
        match self {
            ArtSource::All => t("library.art.source_all"),
            ArtSource::SteamGridDb => "SteamGridDB".into(),
            ArtSource::Igdb => "IGDB".into(),
            ArtSource::LaunchBox => irori_extras::launchbox::SOURCE_NAME.into(),
        }
    }
}

/// A LaunchBox game's boxes as pictures, with each one's caption (its
/// region, as LaunchBox names it).
pub fn launchbox_images(game: &LaunchBoxGame) -> Vec<(Found, SharedString)> {
    game.boxes
        .iter()
        .map(|art| {
            let url = art.url();
            let found = Found {
                source: ArtSource::LaunchBox,
                image: SgdbImage {
                    id: u64::from(game.id),
                    thumb: url.clone(),
                    url,
                    width: None,
                    height: None,
                },
            };
            let caption = match (&art.region, art.reconstructed) {
                (Some(region), false) => region.clone().into(),
                (Some(region), true) => {
                    t_args("library.art.launchbox.reconstructed", &[("region", region)])
                }
                (None, false) => t("library.art.launchbox.no_region"),
                (None, true) => t("library.art.launchbox.reconstructed_no_region"),
            };
            (found, caption)
        })
        .collect()
}

/// A picture found, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub source: ArtSource,
    pub image: SgdbImage,
}

/// The game a search result means without asking: the one named like the
/// search once folded, else the first.
pub fn pick_game<'a>(games: &'a [SgdbGame], query: &str) -> Option<&'a SgdbGame> {
    let wanted = irori_services::search::fold(query);
    games
        .iter()
        .find(|g| irori_services::search::fold(&g.name) == wanted)
        .or(games.first())
}

/// How many of `found` each source holds, in the order they first appear;
/// empty when they all come from one, where filtering would show nothing new.
pub fn source_counts(found: &[Found]) -> Vec<(ArtSource, usize)> {
    let mut counts: Vec<(ArtSource, usize)> = Vec::new();
    for f in found {
        match counts.iter_mut().find(|(s, _)| *s == f.source) {
            Some((_, n)) => *n += 1,
            None => counts.push((f.source, 1)),
        }
    }
    if counts.len() < 2 {
        counts.clear();
    }
    counts
}

/// The pictures of `found` the source filter shows (`None` for all).
pub fn shown(found: &[Found], filter: Option<ArtSource>) -> impl Iterator<Item = &Found> {
    found
        .iter()
        .filter(move |f| filter.is_none_or(|s| s == f.source))
}

/// IGDB's images of `game` for `kind`: its cover, or its artworks then
/// screenshots as backgrounds. IGDB has no square art or logos.
pub fn igdb_images(kind: ArtKind, game: &IgdbGame) -> Vec<SgdbImage> {
    let image = |id: &str, thumb: ImageSize, full: ImageSize| {
        Some(SgdbImage {
            id: game.id,
            url: image_url(id, full)?,
            thumb: image_url(id, thumb)?,
            width: None,
            height: None,
        })
    };
    match kind {
        ArtKind::Cover => game
            .cover
            .iter()
            .filter_map(|id| image(id, ImageSize::CoverBig, ImageSize::Original))
            .collect(),
        ArtKind::Background => game
            .artworks
            .iter()
            .chain(&game.screenshots)
            .filter_map(|id| image(id, ImageSize::ScreenshotMed, ImageSize::FullHD))
            .collect(),
        ArtKind::Square | ArtKind::Logo => Vec::new(),
    }
}

/// Key context of the picker, where the arrows move between pictures.
pub const ART_PICKER_CONTEXT: &str = "ArtPicker";

/// A direction the picker's cursor moves in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

/// Where the cursor goes from `cursor` in `len` pictures laid out `columns`
/// to a row: `Some` picture, or `None` to leave the pictures for the search
/// field above (up from the first row). With no cursor yet, any step but up
/// lands on the first picture.
pub fn step_cursor(cursor: Option<usize>, len: usize, columns: usize, step: Step) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let columns = columns.max(1);
    let Some(at) = cursor.map(|c| c.min(len - 1)) else {
        return (step != Step::Up).then_some(0);
    };
    match step {
        Step::Left => Some(at.saturating_sub(1)),
        Step::Right => Some((at + 1).min(len - 1)),
        Step::Up => at.checked_sub(columns),
        // The last row may be short: land on its last picture.
        Step::Down if at / columns == (len - 1) / columns => Some(at),
        Step::Down => Some((at + columns).min(len - 1)),
    }
}

/// The item after (or before) `current` in `items`, wrapping around; the
/// first when `current` isn't one of them.
pub fn cycle<T: PartialEq + Copy>(items: &[T], current: &T, forward: bool) -> T {
    let at = items.iter().position(|i| i == current);
    let n = items.len();
    match (at, n) {
        (_, 0) => *current,
        (None, _) => items[0],
        (Some(i), _) if forward => items[(i + 1) % n],
        (Some(i), _) => items[(i + n - 1) % n],
    }
}

/// What to do after a search returned `games`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchOutcome {
    NoMatch,
    /// Exactly one match: show its images right away.
    Single(SgdbGame),
    Choose(Vec<SgdbGame>),
}

pub fn search_outcome(mut games: Vec<SgdbGame>) -> SearchOutcome {
    match games.len() {
        0 => SearchOutcome::NoMatch,
        1 => SearchOutcome::Single(games.remove(0)),
        _ => SearchOutcome::Choose(games),
    }
}

/// Picker results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtPickerEvent {
    /// The full-size image URL the user picked.
    Picked(String),
    Cancelled,
}

enum Phase {
    /// No SteamGridDB API key in settings.
    NoKey,
    /// LaunchBox's game list isn't downloaded yet.
    NoLaunchBox,
    /// LaunchBox's game list is being downloaded.
    Downloading,
    Idle,
    Searching,
    Games,
    Loading(SharedString),
    Images(SharedString, Vec<Found>),
    Message(SharedString),
}

/// The SteamGridDB (or IGDB) search and image grid.
pub struct ArtPicker {
    kind: ArtKind,
    key: Option<ApiKey>,
    /// `iroriIgdb` is on: IGDB can be searched too.
    igdb_on: bool,
    source: ArtSource,
    query: Entity<InputState>,
    phase: Phase,
    games: Vec<SgdbGame>,
    /// IGDB's records of `games`, which already hold their images.
    igdb_found: Vec<IgdbGame>,
    /// The ROM library game whose cover is picked: LaunchBox is offered.
    rom: Option<String>,
    /// The title the picker opened with: the ROM's own match answers it.
    first_title: String,
    /// LaunchBox's records of `games`, which already hold their boxes.
    launchbox_found: Vec<LaunchBoxGame>,
    /// Labels under pictures (LaunchBox's regions), by full-size address.
    captions: HashMap<String, SharedString>,
    _task: Option<Task<()>>,
    /// The one source whose pictures show, when several found some.
    filter: Option<ArtSource>,
    /// Pictures' sizes by full-size address, from their source or read
    /// from their first bytes.
    sizes: HashMap<String, (u32, u32)>,
    _sizes_task: Option<Task<()>>,
    /// The picture the arrows reached, among those shown.
    cursor: Option<usize>,
    /// Holds the keyboard while the arrows move between pictures.
    focus: FocusHandle,
    results: ScrollHandle,
    /// Width of the pictures' area as last drawn, for the arrows' rows.
    results_width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ArtPickerEvent> for ArtPicker {}

impl ArtPicker {
    /// A picker for `kind`, searching for `title` right away when a key is set.
    pub fn new(kind: ArtKind, title: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::for_game(kind, None, title, window, cx)
    }

    /// A picker for `game`'s `kind`: a ROM's cover starts on LaunchBox's
    /// box art.
    pub fn for_game(
        kind: ArtKind,
        game: Option<&GameId>,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app = &models(cx).settings.read(cx).app;
        let (key, igdb_on) = (ApiKey::from_settings(app), app.igdb);
        let rom = game
            .filter(|id| {
                id.runner == Runner::Emulation
                    && kind == ArtKind::Cover
                    && crate::emulation_store(cx).is_some()
            })
            .map(|id| id.app_name.to_string());
        Self::build(kind, title, key, igdb_on, rom, window, cx)
    }

    /// A picker with SteamGridDB's `key` and IGDB on or off, whatever the
    /// settings say.
    #[cfg(test)]
    pub(crate) fn with_sources(
        kind: ArtKind,
        title: &str,
        key: Option<ApiKey>,
        igdb_on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(kind, title, key, igdb_on, None, window, cx)
    }

    fn build(
        kind: ArtKind,
        title: &str,
        key: Option<ApiKey>,
        igdb_on: bool,
        rom: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A ROM's boxes first; else both sources at once when both can be
        // searched.
        let source = match (rom.is_some(), key.is_some(), igdb_on) {
            (true, _, _) => ArtSource::LaunchBox,
            (_, true, true) => ArtSource::All,
            (_, false, true) => ArtSource::Igdb,
            _ => ArtSource::SteamGridDb,
        };
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("library.art.placeholder"))
                .default_value(title.trim().to_string())
        });
        let art = irori_ui::art::art_store(cx);
        let subscriptions = vec![
            cx.observe(&art, |_, _, cx| cx.notify()),
            cx.subscribe_in(&query, window, |this, _, event, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.search(cx);
                }
            }),
        ];
        let mut this = Self {
            phase: if key.is_some() || source != ArtSource::SteamGridDb {
                Phase::Idle
            } else {
                Phase::NoKey
            },
            kind,
            key,
            igdb_on,
            source,
            query,
            games: Vec::new(),
            igdb_found: Vec::new(),
            rom,
            first_title: title.trim().to_string(),
            launchbox_found: Vec::new(),
            captions: HashMap::new(),
            _task: None,
            filter: None,
            sizes: HashMap::new(),
            _sizes_task: None,
            cursor: None,
            focus: cx.focus_handle(),
            results: ScrollHandle::new(),
            results_width: Rc::default(),
            _subscriptions: subscriptions,
        };
        if !title.trim().is_empty() || this.source == ArtSource::LaunchBox {
            this.search(cx);
        }
        this
    }

    /// The sources that can be searched now: SteamGridDB with its key, IGDB
    /// while it is on, LaunchBox for a ROM's cover.
    fn searchable(&self) -> Vec<ArtSource> {
        let mut sources = Vec::new();
        if self.rom.is_some() {
            sources.push(ArtSource::LaunchBox);
        }
        if self.key.is_some() {
            sources.push(ArtSource::SteamGridDb);
        }
        if self.igdb_on {
            sources.push(ArtSource::Igdb);
        }
        sources
    }

    /// The sources the switcher offers: every one that applies (SteamGridDB
    /// even without its key, which it then asks for), and all of them at
    /// once when two or more can be searched.
    fn offered(&self) -> Vec<ArtSource> {
        let mut sources = Vec::new();
        if self.searchable().len() >= 2 {
            sources.push(ArtSource::All);
        }
        if self.rom.is_some() {
            sources.push(ArtSource::LaunchBox);
        }
        sources.push(ArtSource::SteamGridDb);
        if self.igdb_on {
            sources.push(ArtSource::Igdb);
        }
        sources
    }

    pub fn kind(&self) -> ArtKind {
        self.kind
    }

    /// Focus the search field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.focus_handle(cx).focus(window, cx);
    }

    /// Show `found` as if a search had found them (tests).
    #[cfg(test)]
    pub(crate) fn show_for_test(&mut self, found: Vec<Found>, cx: &mut Context<Self>) {
        self.phase = Phase::Images("Test".into(), found);
        self.cursor = None;
        cx.notify();
    }

    /// The picture the arrows reached (tests).
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Whether the cursor holds the keyboard (tests).
    #[cfg(test)]
    pub(crate) fn holds_keyboard(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// The search field's focus (tests).
    #[cfg(test)]
    pub(crate) fn query_focus(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }

    /// Where it searches, and whether it asks for a SteamGridDB key (tests).
    #[cfg(test)]
    pub(crate) fn source(&self) -> (ArtSource, bool) {
        (self.source, matches!(self.phase, Phase::NoKey))
    }

    /// The captions of the pictures shown, and whether it offers to
    /// download LaunchBox's game list (tests).
    #[cfg(test)]
    pub(crate) fn launchbox_view(&self) -> (Vec<SharedString>, bool) {
        let shown = match &self.phase {
            Phase::Images(_, found) => shown(found, self.filter)
                .filter_map(|f| self.captions.get(&f.image.url).cloned())
                .collect(),
            _ => Vec::new(),
        };
        (shown, matches!(self.phase, Phase::NoLaunchBox))
    }

    /// Search the other source.
    pub fn set_source(&mut self, source: ArtSource, cx: &mut Context<Self>) {
        if source == self.source {
            return;
        }
        self.source = source;
        self._task = None;
        self._sizes_task = None;
        self.filter = None;
        self.cursor = None;
        self.games.clear();
        self.igdb_found.clear();
        self.launchbox_found.clear();
        // SteamGridDB needs its key, and so does "All sources" when it would
        // otherwise search one source.
        let needs_key = source == ArtSource::SteamGridDb
            || (source == ArtSource::All && self.searchable().len() < 2);
        if needs_key && self.key.is_none() {
            self.phase = Phase::NoKey;
            cx.notify();
        } else {
            self.phase = Phase::Idle;
            self.search(cx);
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let key = match self.source {
            ArtSource::All => return self.search_all(cx),
            ArtSource::SteamGridDb => match self.key.clone() {
                Some(key) => Some(key),
                None => return,
            },
            ArtSource::Igdb => None,
            ArtSource::LaunchBox => return self.search_launchbox(cx),
        };
        let query = self.query.read(cx).value().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.filter = None;
        self.cursor = None;
        self.phase = Phase::Searching;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            match key {
                Some(key) => Ok((steamgrid::search_games(&key, &query).await?, Vec::new())),
                None => {
                    let found = b.search_igdb(&query).await?;
                    let games = found
                        .iter()
                        .map(|g| SgdbGame {
                            id: g.id,
                            name: g.name.clone(),
                        })
                        .collect();
                    Ok((games, found))
                }
            }
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let result = result.map(|(games, found)| {
                    this.igdb_found = found;
                    games
                });
                match result.map(search_outcome) {
                    Ok(SearchOutcome::NoMatch) => {
                        this.games.clear();
                        this.phase = Phase::Message(t("library.art.no_games"));
                    }
                    Ok(SearchOutcome::Single(game)) => {
                        this.games = vec![game.clone()];
                        this.load_images(game, cx);
                    }
                    Ok(SearchOutcome::Choose(games)) => {
                        this.games = games;
                        this.phase = Phase::Games;
                    }
                    Err(e) => this.phase = Phase::Message(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Search every source at once: each takes the game its search means
    /// ([`pick_game`]; a ROM's own LaunchBox match for its title), and their
    /// pictures show together, LaunchBox's boxes first. A source that fails
    /// leaves the others' pictures; only all failing says why.
    fn search_all(&mut self, cx: &mut Context<Self>) {
        let searchable = self.searchable();
        if searchable.len() < 2 {
            return;
        }
        let query = self.query.read(cx).value().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.filter = None;
        self.cursor = None;
        self.games.clear();
        self.igdb_found.clear();
        self.launchbox_found.clear();
        self.phase = Phase::Searching;
        cx.notify();
        let (b, kind) = (backend(cx), self.kind);
        let (key, igdb_on) = (self.key.clone(), self.igdb_on);
        // LaunchBox takes part once its game list is here; its pill offers
        // the download otherwise.
        let launchbox = self
            .rom
            .clone()
            .zip(crate::emulation_store(cx))
            .filter(|(_, store)| store.launchbox_status() == LaunchBoxStatus::Ready);
        let own = launchbox
            .as_ref()
            .filter(|_| query == self.first_title)
            .and_then(|(app, store)| store.launchbox_game(app));
        let attempted = searchable
            .iter()
            .filter(|s| **s != ArtSource::LaunchBox || launchbox.is_some())
            .count();
        let task = irori_ui::run(cx, async move {
            let sgdb = async {
                let Some(key) = key else {
                    return anyhow::Ok(None);
                };
                let games = steamgrid::search_games(&key, &query).await?;
                let Some(game) = pick_game(&games, &query).cloned() else {
                    return anyhow::Ok(None);
                };
                let filter = kind.filter();
                let images = match kind {
                    ArtKind::Background => steamgrid::heroes(&key, game.id, &filter).await?,
                    ArtKind::Logo => steamgrid::logos(&key, game.id, &filter).await?,
                    ArtKind::Cover | ArtKind::Square => {
                        steamgrid::grids(&key, game.id, &filter).await?
                    }
                };
                let found = images
                    .into_iter()
                    .take(MAX_IMAGES)
                    .map(|image| Found {
                        source: ArtSource::SteamGridDb,
                        image,
                    })
                    .collect::<Vec<_>>();
                Ok(Some((game.name, found, Vec::new())))
            };
            let igdb = async {
                if !igdb_on {
                    return anyhow::Ok(None);
                }
                let found = b.search_igdb(&query).await?;
                let games: Vec<SgdbGame> = found
                    .iter()
                    .map(|g| SgdbGame {
                        id: g.id,
                        name: g.name.clone(),
                    })
                    .collect();
                let Some(game) = pick_game(&games, &query) else {
                    return anyhow::Ok(None);
                };
                let record = found.iter().find(|g| g.id == game.id);
                Ok(record.map(|g| {
                    let found = igdb_images(kind, g)
                        .into_iter()
                        .take(MAX_IMAGES)
                        .map(|image| Found {
                            source: ArtSource::Igdb,
                            image,
                        })
                        .collect::<Vec<_>>();
                    (g.name.clone(), found, Vec::new())
                }))
            };
            let boxes = async {
                let game = match (own, launchbox) {
                    (Some(own), _) => Some(own),
                    (None, Some((app, store))) => {
                        let games = store.search_launchbox(&app, &query).await?;
                        let named: Vec<SgdbGame> = games
                            .iter()
                            .map(|g| SgdbGame {
                                id: u64::from(g.id),
                                name: g.name.clone(),
                            })
                            .collect();
                        pick_game(&named, &query)
                            .and_then(|p| games.iter().find(|g| u64::from(g.id) == p.id))
                            .cloned()
                    }
                    _ => None,
                };
                Ok(game.map(|g| {
                    let (found, captions): (Vec<_>, Vec<_>) =
                        launchbox_images(&g).into_iter().unzip();
                    (g.name, found, captions)
                }))
            };
            let (sgdb, igdb, boxes) = futures::join!(sgdb, igdb, boxes);
            Ok([boxes, sgdb, igdb].map(|answer| answer.map_err(|e| format!("{e:#}"))))
        });
        let query: SharedString = self.query.read(cx).value().trim().to_string().into();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.phase = match result {
                    Ok(answers) => {
                        let mut found = Vec::new();
                        let mut name = None;
                        let mut errors = Vec::new();
                        for answer in answers {
                            match answer {
                                Ok(Some((game, images, captions))) => {
                                    name.get_or_insert(game);
                                    for (f, c) in images.iter().zip(captions) {
                                        this.captions.insert(f.image.url.clone(), c);
                                    }
                                    found.extend(images);
                                }
                                Ok(None) => {}
                                Err(e) => errors.push(e),
                            }
                        }
                        if !found.is_empty() {
                            Phase::Images(name.map(Into::into).unwrap_or(query), found)
                        } else if errors.len() == attempted {
                            Phase::Message(errors.join("\n").into())
                        } else {
                            Phase::Message(kind.none_found(&query))
                        }
                    }
                    Err(e) => Phase::Message(format!("{e:#}").into()),
                };
                this.read_sizes(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Show the boxes of the game the ROM library matched, or search the
    /// system's LaunchBox games for what the user typed.
    fn search_launchbox(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(app)) = (crate::emulation_store(cx), self.rom.clone()) else {
            return;
        };
        match store.launchbox_status() {
            LaunchBoxStatus::Missing => {
                self.phase = Phase::NoLaunchBox;
                cx.notify();
                return;
            }
            // A download already runs: wait for it, then search.
            LaunchBoxStatus::Downloading => return self.download_launchbox(cx),
            LaunchBoxStatus::Ready => {}
        }
        let query = self.query.read(cx).value().trim().to_string();
        let own = (query.is_empty() || query == self.first_title)
            .then(|| store.launchbox_game(&app))
            .flatten();
        if query.is_empty() && own.is_none() {
            self.phase = Phase::Idle;
            cx.notify();
            return;
        }
        self.filter = None;
        self.cursor = None;
        self.phase = Phase::Searching;
        cx.notify();
        // The ROM's own match goes straight to its boxes; anything else the
        // user typed searches the system's games.
        let task = irori_ui::run(cx, async move {
            match own {
                Some(own) => Ok(vec![own]),
                None => store.search_launchbox(&app, &query).await,
            }
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(found) => {
                        let games: Vec<SgdbGame> = found
                            .iter()
                            .map(|g| SgdbGame {
                                id: u64::from(g.id),
                                name: g.name.clone(),
                            })
                            .collect();
                        this.launchbox_found = found;
                        match search_outcome(games) {
                            SearchOutcome::NoMatch => {
                                this.games.clear();
                                this.phase = Phase::Message(t("library.art.no_games"));
                            }
                            SearchOutcome::Single(game) => {
                                this.games = vec![game.clone()];
                                this.load_images(game, cx);
                            }
                            SearchOutcome::Choose(games) => {
                                this.games = games;
                                this.phase = Phase::Games;
                            }
                        }
                    }
                    Err(e) => this.phase = Phase::Message(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Download LaunchBox's game list (the user asked, which turns
    /// `iroriLaunchBox` on, as the text above the button says), then show
    /// the boxes.
    pub(crate) fn download_launchbox(&mut self, cx: &mut Context<Self>) {
        let Some(task) = crate::launchbox::turn_on_and_download(cx) else {
            return;
        };
        self.phase = Phase::Downloading;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.phase = Phase::Idle;
                        this.search(cx);
                    }
                    Err(e) => this.phase = Phase::Message(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Note the sizes the shown pictures' sources gave, and read the others'
    /// from their first bytes, a few at a time, off the UI thread.
    fn read_sizes(&mut self, cx: &mut Context<Self>) {
        let Phase::Images(_, found) = &self.phase else {
            return;
        };
        let mut unknown = Vec::new();
        for f in found {
            match f.image.size() {
                Some(size) => {
                    self.sizes.insert(f.image.url.clone(), size);
                }
                None if !self.sizes.contains_key(&f.image.url) => unknown.push(f.image.url.clone()),
                None => {}
            }
        }
        if unknown.is_empty() {
            return;
        }
        let task = irori_ui::run(cx, async move {
            use futures::StreamExt as _;
            let sizes: Vec<(String, (u32, u32))> = futures::stream::iter(unknown)
                .map(|url| async move {
                    let size = irori_extras::image_size::fetch(&url).await;
                    size.ok().flatten().map(|s| (url, s))
                })
                .buffer_unordered(SIZE_READS)
                .filter_map(|s| async move { s })
                .collect()
                .await;
            Ok(sizes)
        });
        self._sizes_task = Some(cx.spawn(async move |this, cx| {
            let Ok(sizes) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.sizes.extend(sizes);
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_images(&mut self, game: SgdbGame, cx: &mut Context<Self>) {
        self.cursor = None;
        let name: SharedString = game.name.clone().into();
        if self.source == ArtSource::LaunchBox {
            let boxes = self
                .launchbox_found
                .iter()
                .find(|g| u64::from(g.id) == game.id)
                .map(launchbox_images)
                .unwrap_or_default();
            let mut found = Vec::with_capacity(boxes.len());
            for (f, caption) in boxes {
                self.captions.insert(f.image.url.clone(), caption);
                found.push(f);
            }
            self.phase = if found.is_empty() {
                Phase::Message(self.kind.none_found(&name))
            } else {
                Phase::Images(name, found)
            };
            self.read_sizes(cx);
            cx.notify();
            return;
        }
        if self.source == ArtSource::Igdb {
            let images: Vec<Found> = self
                .igdb_found
                .iter()
                .find(|g| g.id == game.id)
                .map(|g| igdb_images(self.kind, g))
                .unwrap_or_default()
                .into_iter()
                .map(|image| Found {
                    source: ArtSource::Igdb,
                    image,
                })
                .collect();
            self.phase = if images.is_empty() {
                Phase::Message(self.kind.none_found(&name))
            } else {
                Phase::Images(name, images)
            };
            self.read_sizes(cx);
            cx.notify();
            return;
        }
        let Some(key) = self.key.clone() else {
            return;
        };
        self.phase = Phase::Loading(name.clone());
        cx.notify();
        let kind = self.kind;
        let task = irori_ui::run(cx, async move {
            let filter = kind.filter();
            match kind {
                ArtKind::Background => steamgrid::heroes(&key, game.id, &filter).await,
                ArtKind::Logo => steamgrid::logos(&key, game.id, &filter).await,
                ArtKind::Cover | ArtKind::Square => steamgrid::grids(&key, game.id, &filter).await,
            }
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.phase = match result {
                    Ok(images) if images.is_empty() => Phase::Message(kind.none_found(&name)),
                    Ok(mut images) => {
                        images.truncate(MAX_IMAGES);
                        let found = images
                            .into_iter()
                            .map(|image| Found {
                                source: ArtSource::SteamGridDb,
                                image,
                            })
                            .collect();
                        Phase::Images(name, found)
                    }
                    Err(e) => Phase::Message(format!("{e:#}").into()),
                };
                this.read_sizes(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The pictures shown, the source filter applied.
    fn shown_count(&self) -> usize {
        match &self.phase {
            Phase::Images(_, found) => shown(found, self.filter).count(),
            _ => 0,
        }
    }

    /// Pictures to a row in the area as last drawn.
    fn columns(&self, window: &Window) -> usize {
        let rem = f32::from(window.rem_size());
        let (w, _) = self.kind.thumb_rem();
        // A tile is its picture plus the button's padding (`p_1` a side) and
        // the cursor ring's border (2 px a side), with `gap_1` between tiles.
        let (tile, gap) = (rem * (w + 0.5) + 4.0, rem * 0.25);
        let width = f32::from(self.results_width.get());
        (((width + gap) / (tile + gap)).floor() as usize).max(1)
    }

    /// Move the cursor among the pictures: off the first row, back to the
    /// search field. Without pictures the key goes on (a controller moves
    /// to the next control).
    fn move_cursor(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.shown_count();
        if len == 0 {
            cx.propagate();
            return;
        }
        match step_cursor(self.cursor, len, self.columns(window), step) {
            Some(ix) => {
                self.cursor = Some(ix);
                self.focus.focus(window, cx);
                self.results.scroll_to_item(ix);
            }
            None => {
                self.cursor = None;
                self.query.focus_handle(cx).focus(window, cx);
            }
        }
        cx.notify();
    }

    /// Use the picture the cursor is on, while the cursor holds the keyboard
    /// (Enter on a focused button stays that button's).
    fn use_cursor(&mut self, window: &Window, cx: &mut Context<Self>) {
        let url = match (&self.phase, self.cursor) {
            (Phase::Images(_, found), Some(ix)) if self.focus.is_focused(window) => {
                shown(found, self.filter)
                    .nth(ix)
                    .map(|f| f.image.url.clone())
            }
            _ => None,
        };
        match url {
            Some(url) => cx.emit(ArtPickerEvent::Picked(url)),
            None => cx.propagate(),
        }
    }

    /// Go to the search field, where a controller's system keyboard opens.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = None;
        self.query.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// The shoulders: the previous or next source's pictures when several
    /// sources found some, else the previous or next source to search.
    fn cycle_source(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Phase::Images(_, found) = &self.phase {
            let counts = source_counts(found);
            if !counts.is_empty() {
                let mut filters: Vec<Option<ArtSource>> = vec![None];
                filters.extend(counts.iter().map(|(s, _)| Some(*s)));
                self.filter = cycle(&filters, &self.filter, forward);
                self.cursor = None;
                cx.notify();
                return;
            }
        }
        let sources = self.offered();
        if sources.len() < 2 {
            cx.propagate();
            return;
        }
        let next = cycle(&sources, &self.source, forward);
        self.cursor = None;
        self.set_source(next, cx);
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self._task = None;
        self.phase = if self.games.len() > 1 {
            Phase::Games
        } else {
            Phase::Idle
        };
        cx.notify();
    }

    fn render_no_key(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .text_sm()
            .child(t("library.art.no_key.intro"))
            .child(
                v_flex()
                    .gap_1()
                    .text_color(muted)
                    .child(t("library.art.no_key.step_copy"))
                    .child(
                        Link::new("sgdb-api-page")
                            .href(API_KEY_PAGE)
                            .child(t("library.art.no_key.open_page")),
                    )
                    .child(t("library.art.no_key.step_paste")),
            )
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("sgdb-open-settings")
                        .outline()
                        .icon(IconName::Settings)
                        .label(t("library.art.no_key.open_settings"))
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            navigate(Route::Settings("advanced".into()), cx);
                        }),
                ),
            )
            .into_any_element()
    }

    fn render_no_launchbox(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_3()
            .text_sm()
            .child(t("library.art.launchbox.intro"))
            .child(
                h_flex().justify_end().child(
                    Button::new("launchbox-download")
                        .outline()
                        .icon(IconName::ArrowDown)
                        .label(t("library.art.launchbox.download"))
                        .on_click(cx.listener(|this, _, _, cx| this.download_launchbox(cx))),
                ),
            )
            .into_any_element()
    }

    fn render_games(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.games.iter().enumerate().map(|(ix, game)| {
            let chosen = game.clone();
            ListItem::new(ElementId::Name(format!("sgdb-game-{ix}").into()))
                .py_2()
                .accessibility_label(game.name.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.load_images(chosen.clone(), cx)))
                .child(div().text_sm().truncate().child(game.name.clone()))
        });
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t("library.art.choose_game")),
            )
            .children(rows)
            .into_any_element()
    }

    /// Pills that show one source's pictures, with counts, when several
    /// sources found some.
    fn render_source_filter(&self, found: &[Found], cx: &mut Context<Self>) -> Option<AnyElement> {
        let counts = source_counts(found);
        if counts.is_empty() {
            return None;
        }
        let pill = |id: &'static str, filter: Option<ArtSource>, label: SharedString| {
            Button::new(id)
                .xsmall()
                .ghost()
                .label(label)
                .selected(self.filter == filter)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = filter;
                    this.cursor = None;
                    cx.notify();
                }))
        };
        let count = |name: SharedString, n: usize| {
            t_args(
                "library.art.source_count",
                &[("source", &name), ("count", &n.to_string())],
            )
        };
        let mut row = h_flex().gap_1().flex_wrap().child(pill(
            "art-filter-all",
            None,
            count(ArtSource::All.name(), found.len()),
        ));
        for (source, n) in counts {
            let id = match source {
                ArtSource::Igdb => "art-filter-igdb",
                ArtSource::LaunchBox => "art-filter-launchbox",
                _ => "art-filter-sgdb",
            };
            row = row.child(pill(id, Some(source), count(source.name(), n)));
        }
        Some(row.into_any_element())
    }

    /// One tile per picture shown: the direct children of the scrolled area,
    /// so the cursor's tile can be scrolled to. The cursor's tile carries a
    /// ring while the picker holds the keyboard.
    fn render_images(
        &self,
        found: &[Found],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (w, h) = self.kind.thumb_rem();
        let width: Rems = rems(w);
        let pixels = physical_width(width, window);
        let cursor = self.cursor.filter(|_| self.focus.is_focused(window));
        let theme = cx.theme();
        let (radius, muted_bg, muted) = (theme.radius, theme.muted, theme.muted_foreground);
        let ring = theme.ring;
        let mut tiles = Vec::with_capacity(found.len());
        for (ix, f) in shown(found, self.filter).enumerate() {
            let image = &f.image;
            let size = self.sizes.get(&image.url).map(|(w, h)| {
                t_args(
                    "library.art.size",
                    &[("width", &w.to_string()), ("height", &h.to_string())],
                )
            });
            let caption = self.captions.get(&image.url).cloned();
            let label = match &caption {
                Some(c) => t_args(
                    "library.art.image_label_captioned",
                    &[("number", &(ix + 1).to_string()), ("caption", c)],
                ),
                None => t_args(
                    "library.art.image_label",
                    &[("number", &(ix + 1).to_string())],
                ),
            };
            // Logos keep their transparency and are shown whole.
            let logo = self.kind == ArtKind::Logo;
            let thumb = art_img(&image.thumb, pixels, logo, cx);
            let url = image.url.clone();
            let frame = div()
                .w(width)
                .h(rems(h))
                .overflow_hidden()
                .rounded(radius)
                .bg(muted_bg)
                .map(|el| match thumb {
                    Some(img) => el.child(img.size_full().rounded(radius).object_fit(if logo {
                        ObjectFit::Contain
                    } else {
                        ObjectFit::Cover
                    })),
                    None => el.child(
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .text_color(muted)
                            .child(Icon::new(IconName::Image)),
                    ),
                });
            let tile = Button::new(ElementId::Name(format!("sgdb-image-{ix}").into()))
                .ghost()
                .p_1()
                .h_auto()
                .accessibility_label(label)
                .child(
                    v_flex()
                        .w(width)
                        .gap_0p5()
                        .child(frame)
                        .when_some(caption, |el, c| {
                            el.child(
                                div()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .truncate()
                                    .child(c),
                            )
                        })
                        // The line keeps its height before the size is known,
                        // so the grid doesn't shift when it arrives.
                        .child(
                            div()
                                .h(rems(1.))
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .children(size),
                        ),
                )
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(ArtPickerEvent::Picked(url.clone()))),
                );
            tiles.push(
                div()
                    .rounded(radius)
                    .border_2()
                    .border_color(if cursor == Some(ix) {
                        ring
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .child(tile)
                    .into_any_element(),
            );
        }
        tiles
    }
}

impl Render for ArtPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let has_key = self.key.is_some() || !matches!(self.phase, Phase::NoKey);
        let source = self.source;
        let offered = self.offered();
        let busy = matches!(
            self.phase,
            Phase::Searching | Phase::Loading(_) | Phase::Downloading
        );
        let can_go_back =
            matches!(self.phase, Phase::Images(..) | Phase::Loading(_)) && self.games.len() > 1;

        // Pictures fill the scrolled area as its own children; their caption
        // and source pills stay above it.
        let mut header = None;
        let content: AnyElement = match &self.phase {
            Phase::Images(name, found) => {
                let found = found.clone();
                header = Some(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t_args("library.art.choose_image", &[("name", name)])),
                        )
                        .children(self.render_source_filter(&found, cx))
                        .into_any_element(),
                );
                let width = self.results_width.clone();
                h_flex()
                    .id("art-picker-results")
                    .relative()
                    .max_h(rems(24.))
                    .overflow_y_scroll()
                    .track_scroll(&self.results)
                    .flex_wrap()
                    .gap_1()
                    .children(self.render_images(&found, window, cx))
                    // Measures the area's width, for the arrows' rows.
                    .child(
                        canvas(
                            move |bounds, _, _| width.set(bounds.size.width),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .into_any_element()
            }
            Phase::NoKey => self.render_no_key(cx),
            Phase::NoLaunchBox => self.render_no_launchbox(cx),
            Phase::Idle => div()
                .text_sm()
                .text_color(muted)
                .child(t("library.art.idle"))
                .into_any_element(),
            Phase::Searching | Phase::Loading(_) | Phase::Downloading => h_flex()
                .gap_2()
                .py_4()
                .justify_center()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new())
                .child(match &self.phase {
                    Phase::Loading(name) => t_args("library.art.loading", &[("name", name)]),
                    Phase::Downloading => t("library.art.launchbox.downloading"),
                    _ => t("library.art.searching"),
                })
                .into_any_element(),
            Phase::Games => self.render_games(cx),
            Phase::Message(text) => div()
                .text_sm()
                .text_color(muted)
                .child(text.clone())
                .into_any_element(),
        };
        let content = match header {
            Some(_) => content,
            None => div()
                .id("art-picker-results")
                .max_h(rems(24.))
                .overflow_y_scroll()
                .child(content)
                .into_any_element(),
        };

        v_flex()
            .id("art-picker")
            .key_context(ART_PICKER_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::ArtPickerUp, window, cx| {
                this.move_cursor(Step::Up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ArtPickerDown, window, cx| {
                this.move_cursor(Step::Down, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ArtPickerLeft, window, cx| {
                this.move_cursor(Step::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ArtPickerRight, window, cx| {
                this.move_cursor(Step::Right, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &crate::UseArt, window, cx| this.use_cursor(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::SearchArt, window, cx| this.focus_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::PrevArtSource, _, cx| this.cycle_source(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::NextArtSource, _, cx| this.cycle_source(true, cx)),
            )
            // The search field keeps Down for itself, a single line or not;
            // from there Down goes to the pictures.
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.query.focus_handle(cx).is_focused(window) && this.shown_count() > 0 {
                    cx.stop_propagation();
                    this.move_cursor(Step::Down, window, cx);
                }
            }))
            .gap_3()
            .when(offered.len() > 1, |el| {
                let mut row = h_flex().gap_1();
                for option in offered {
                    let id = match option {
                        ArtSource::All => "art-source-all",
                        ArtSource::SteamGridDb => "art-source-sgdb",
                        ArtSource::Igdb => "art-source-igdb",
                        ArtSource::LaunchBox => "art-source-launchbox",
                    };
                    row = row.child(
                        Button::new(id)
                            .ghost()
                            .small()
                            .label(option.name())
                            .selected(source == option)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.set_source(option, cx)),
                            ),
                    );
                }
                el.child(row)
            })
            .when(has_key, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .when(can_go_back, |el| {
                            el.child(
                                Button::new("sgdb-back")
                                    .ghost()
                                    .icon(IconName::ArrowLeft)
                                    .tooltip(t("library.art.back_to_games"))
                                    .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                            )
                        })
                        .child(div().flex_1().min_w_0().child(
                            Input::new(&self.query).prefix(Icon::new(IconName::Search).small()),
                        ))
                        .child(
                            Button::new("sgdb-search")
                                .outline()
                                .label(t("library.art.search"))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
                        ),
                )
            })
            .children(header)
            .child(content)
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .min_w_0()
                            .when(has_key, |el| el.child(self.attribution())),
                    )
                    .child(
                        Button::new("sgdb-cancel")
                            .ghost()
                            .label(t("library.art.back_to_form"))
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ArtPickerEvent::Cancelled)),
                            ),
                    ),
            )
    }
}

impl ArtPicker {
    /// Where the pictures come from, for the line under them.
    fn attribution(&self) -> SharedString {
        match self.source {
            ArtSource::SteamGridDb => t("library.art.attribution"),
            ArtSource::Igdb => t("library.art.attribution_igdb"),
            ArtSource::LaunchBox => t("library.art.attribution_launchbox"),
            ArtSource::All if self.rom.is_none() => t("library.art.attribution_both"),
            ArtSource::All => {
                // Source names are never translated; only the sentence is.
                let names: Vec<SharedString> =
                    self.searchable().into_iter().map(ArtSource::name).collect();
                t_args(
                    "library.art.attribution_sources",
                    &[("sources", &names.join(", "))],
                )
            }
        }
    }
}

/// Open the picker in a dialog of its own (a game's settings, through
/// `irori_ui::art_picker`); `on_picked` gets the chosen image.
pub fn open_art_picker_dialog(
    slot: ArtSlot,
    title: &str,
    game: Option<&GameId>,
    on_picked: OnPicked,
    window: &mut Window,
    cx: &mut App,
) {
    let kind = match slot {
        ArtSlot::Cover => ArtKind::Cover,
        ArtSlot::Background => ArtKind::Background,
        ArtSlot::Logo => ArtKind::Logo,
    };
    let picker = cx.new(|cx| ArtPicker::for_game(kind, game, title, window, cx));
    window
        .subscribe(&picker, cx, move |_, event: &ArtPickerEvent, window, cx| {
            if let ArtPickerEvent::Picked(url) = event {
                on_picked(url.clone(), window, cx);
            }
            window.close_dialog(cx);
        })
        .detach();
    let width = super::dialog_width(window, 40.);
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            picker.update(cx, |p, cx| p.focus(window, cx));
        }
        dialog.title(kind.title()).w(width).child(picker.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: u64, name: &str) -> SgdbGame {
        SgdbGame {
            id,
            name: name.into(),
        }
    }

    #[test]
    fn search_outcomes() {
        assert_eq!(search_outcome(Vec::new()), SearchOutcome::NoMatch);
        assert_eq!(
            search_outcome(vec![game(1, "Hades")]),
            SearchOutcome::Single(game(1, "Hades"))
        );
        assert_eq!(
            search_outcome(vec![game(1, "Hades"), game(2, "Hades II")]),
            SearchOutcome::Choose(vec![game(1, "Hades"), game(2, "Hades II")])
        );
    }

    #[test]
    fn launchbox_boxes_are_captioned_by_region() {
        use irori_emulation::launchbox::BoxArt;
        let art = |file: &str, region: Option<&str>, reconstructed| BoxArt {
            file: file.into(),
            region: region.map(str::to_string),
            reconstructed,
        };
        let game = LaunchBoxGame {
            id: 7,
            name: "Mega Man".into(),
            alternate: Vec::new(),
            boxes: vec![
                art("eu.png", Some("Europe"), false),
                art("jp.png", Some("Japan"), true),
                art("x.png", None, false),
            ],
        };
        let boxes = launchbox_images(&game);
        assert_eq!(boxes.len(), 3);
        let (first, caption) = &boxes[0];
        assert_eq!(first.source, ArtSource::LaunchBox);
        assert_eq!(first.image.url, "https://images.launchbox-app.com/eu.png");
        assert_eq!(first.image.thumb, first.image.url);
        assert_eq!(first.image.id, 7);
        assert_eq!(caption.as_ref(), "Europe");
        assert!(boxes[1].1.contains("Japan"));
    }

    #[test]
    fn igdb_offers_covers_and_backgrounds_only() {
        let game = IgdbGame {
            id: 7,
            cover: Some("co1".into()),
            artworks: vec!["ar1".into()],
            screenshots: vec!["sc1".into(), "bad/id".into()],
            ..Default::default()
        };
        let covers = igdb_images(ArtKind::Cover, &game);
        assert_eq!(covers.len(), 1);
        assert!(covers[0].url.ends_with("/t_original/co1.jpg"));
        assert!(covers[0].thumb.ends_with("/t_cover_big/co1.jpg"));
        let backgrounds: Vec<String> = igdb_images(ArtKind::Background, &game)
            .into_iter()
            .map(|i| i.url)
            .collect();
        assert_eq!(backgrounds.len(), 2);
        assert!(backgrounds[0].ends_with("/t_1080p/ar1.jpg"));
        assert!(igdb_images(ArtKind::Logo, &game).is_empty());
        assert!(igdb_images(ArtKind::Square, &game).is_empty());
    }

    fn found(source: ArtSource, id: u64) -> Found {
        Found {
            source,
            image: SgdbImage {
                id,
                url: format!("https://x/{id}.png"),
                thumb: String::new(),
                width: None,
                height: None,
            },
        }
    }

    #[test]
    fn a_merged_search_takes_the_game_named_like_it() {
        let games = [game(1, "Hades II"), game(2, "HADES"), game(3, "Hades")];
        assert_eq!(pick_game(&games, "hades").map(|g| g.id), Some(2));
        // Nothing named so: the source's own first answer.
        assert_eq!(pick_game(&games, "hades 2").map(|g| g.id), Some(1));
        assert_eq!(pick_game(&[], "hades"), None);
    }

    #[test]
    fn the_cursor_moves_by_rows_and_leaves_up_to_the_search() {
        // 7 pictures, 3 to a row:
        //  0 1 2
        //  3 4 5
        //  6
        let mv = |c, s| step_cursor(c, 7, 3, s);
        assert_eq!(mv(None, Step::Down), Some(0));
        assert_eq!(mv(None, Step::Right), Some(0));
        assert_eq!(mv(None, Step::Up), None);
        assert_eq!(mv(Some(1), Step::Down), Some(4));
        assert_eq!(mv(Some(5), Step::Down), Some(6));
        assert_eq!(mv(Some(6), Step::Down), Some(6));
        assert_eq!(mv(Some(4), Step::Up), Some(1));
        assert_eq!(mv(Some(2), Step::Up), None);
        assert_eq!(mv(Some(0), Step::Left), Some(0));
        assert_eq!(mv(Some(2), Step::Right), Some(3));
        assert_eq!(mv(Some(6), Step::Right), Some(6));
        // A cursor past a shorter list (a filter) is clamped first.
        assert_eq!(step_cursor(Some(9), 2, 3, Step::Left), Some(0));
        assert_eq!(step_cursor(Some(0), 0, 3, Step::Down), None);
    }

    #[test]
    fn shoulders_cycle_and_wrap() {
        let items = [None, Some(ArtSource::SteamGridDb), Some(ArtSource::Igdb)];
        assert_eq!(cycle(&items, &None, true), Some(ArtSource::SteamGridDb));
        assert_eq!(cycle(&items, &Some(ArtSource::Igdb), true), None);
        assert_eq!(cycle(&items, &None, false), Some(ArtSource::Igdb));
        assert_eq!(cycle(&items, &Some(ArtSource::All), true), None);
    }

    #[test]
    fn source_pills_count_each_source_and_filter_to_it() {
        let one = [found(ArtSource::Igdb, 1), found(ArtSource::Igdb, 2)];
        assert!(source_counts(&one).is_empty());
        let both = [
            found(ArtSource::SteamGridDb, 1),
            found(ArtSource::SteamGridDb, 2),
            found(ArtSource::Igdb, 3),
        ];
        assert_eq!(
            source_counts(&both),
            vec![(ArtSource::SteamGridDb, 2), (ArtSource::Igdb, 1)]
        );
        let ids = |filter| shown(&both, filter).map(|f| f.image.id).collect::<Vec<_>>();
        assert_eq!(ids(None), vec![1, 2, 3]);
        assert_eq!(ids(Some(ArtSource::Igdb)), vec![3]);
    }

    #[test]
    fn filters_per_kind() {
        assert_eq!(
            ArtKind::Cover.filter().dimensions,
            vec!["600x900", "342x482", "660x930"]
        );
        assert_eq!(ArtKind::Square.filter().dimensions.len(), 2);
        assert_eq!(ArtKind::Background.filter(), ImageFilter::default());
    }
}
