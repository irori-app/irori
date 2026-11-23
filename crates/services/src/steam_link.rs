//! Each game's Steam app ("Steam link"), which Steam's tags, its store
//! page (screenshots, trailers, description, genres, date: one fetch per
//! linked app, cached a week), ProtonDB and the Steam Deck rating follow for
//! games of every store. It never changes how a game
//! starts: a store game keeps its own id.
//!
//! In order of trust: a Steam game is its own app; the user's choice
//! (`iroriSteamAppId`, `none` for no app, made in "Link to Steam" or the
//! game's settings); the app GamesDB or PCGamingWiki name, learned when the
//! game's page reads them; with `iroriSteamTitleMatch` (or the library's
//! "Find on Steam by title"), the app whose title is the game's exact title.
//!
//! What was found, and a copy of the user's choices, are kept in
//! `<cache>/steam-links.json`, so building the library reads no game's own
//! settings file. Title searches are kept a month (a week when nothing
//! matched) for the title searched, so renaming a game searches again, and
//! are made at most every [`TITLE_SPACING`], [`TITLE_ROUND`] games a round.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::{ExtraInfo, GameId, GameInfo, Paths, Runner, Video, json_store, util};
use irori_steam_api::{Client, Endpoints, StoreHit, app_id_in};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::Backend;

/// The choice of a game the user said Steam doesn't sell.
pub const STEAM_NONE: &str = irori_extras::wiki::STEAM_NONE;

/// The per-game setting holding the user's choice.
pub const LINK_KEY: &str = "iroriSteamAppId";

const TITLE_FRESH_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const TITLE_MISS_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Store details read for a linked game are kept this long.
const DETAILS_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Time between two title searches in the background.
pub const TITLE_SPACING: Duration = Duration::from_millis(1500);
/// Games searched by title per round in the background.
pub const TITLE_ROUND: usize = 25;
/// Candidates whose release year is read to tell same-named games apart.
const YEAR_CHECKS: usize = 3;

/// Where a game's Steam app comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamLink {
    /// A Steam game: its own app.
    Own(u32),
    /// The user chose this app.
    Chosen(u32),
    /// The user said Steam doesn't sell the game.
    NoneChosen,
    /// GamesDB or PCGamingWiki name this app.
    Wiki(u32),
    /// The app with the game's exact title.
    Title(u32),
    /// Nothing known.
    Unknown,
}

impl SteamLink {
    /// The app, when there is one.
    pub fn app_id(self) -> Option<u32> {
        match self {
            Self::Own(id) | Self::Chosen(id) | Self::Wiki(id) | Self::Title(id) => Some(id),
            Self::NoneChosen | Self::Unknown => None,
        }
    }

    /// Whether the user decided it (an app or none).
    pub fn is_chosen(self) -> bool {
        matches!(self, Self::Chosen(_) | Self::NoneChosen)
    }
}

/// What one title search found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct TitleMatch {
    /// The folded title that was searched for.
    title: String,
    app: Option<u32>,
    /// Unix time of the search.
    fetched: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Links {
    /// The user's choices by game key, as `iroriSteamAppId` holds them.
    chosen: HashMap<String, String>,
    /// Every game's choice has been copied here once (choices made before
    /// this file existed).
    seeded: bool,
    /// The apps GamesDB or PCGamingWiki name, by game key.
    wiki: HashMap<String, u32>,
    /// Title searches by game key.
    titles: HashMap<String, TitleMatch>,
}

pub(crate) struct SteamLinkService {
    links: RwLock<Links>,
    busy: AtomicBool,
    /// Steam's servers (a test server's in tests).
    endpoints: parking_lot::Mutex<Endpoints>,
}

fn file() -> PathBuf {
    Paths::get().cache.join("steam-links.json")
}

/// The app a choice names: `Some(None)` for "none", `None` for no choice.
fn parse_choice(choice: &str) -> Option<Option<u32>> {
    let choice = choice.trim();
    if choice.eq_ignore_ascii_case(STEAM_NONE) {
        return Some(None);
    }
    app_id_in(choice).map(Some)
}

impl SteamLinkService {
    pub fn load() -> Self {
        Self {
            links: RwLock::new(json_store::read_or_default(&file())),
            busy: AtomicBool::new(false),
            endpoints: parking_lot::Mutex::new(Endpoints::default()),
        }
    }

    #[cfg(test)]
    fn empty() -> Self {
        Self {
            links: RwLock::new(Links::default()),
            busy: AtomicBool::new(false),
            endpoints: parking_lot::Mutex::new(Endpoints::default()),
        }
    }

    pub(crate) fn client(&self) -> Result<Client> {
        irori_core::net::ensure_online()?;
        Client::with_endpoints(self.endpoints.lock().clone())
    }

    /// The game's Steam app, from what is known.
    pub fn linked(&self, game: &GameInfo) -> SteamLink {
        if game.runner() == Runner::Steam {
            return game
                .app_name
                .parse()
                .map_or(SteamLink::Unknown, SteamLink::Own);
        }
        let chosen = self
            .links
            .read()
            .chosen
            .get(&game.id().key())
            .and_then(|c| parse_choice(c));
        match chosen {
            Some(Some(id)) => SteamLink::Chosen(id),
            Some(None) => SteamLink::NoneChosen,
            None => self.found(game),
        }
    }

    /// What was found for a game of another store, whatever the user chose:
    /// the wikis' app, else the app of its exact title.
    pub fn found(&self, game: &GameInfo) -> SteamLink {
        let key = game.id().key();
        let links = self.links.read();
        if let Some(id) = links.wiki.get(&key) {
            return SteamLink::Wiki(*id);
        }
        match links.titles.get(&key) {
            Some(found) if found.title == fold_title(&game.title).join(" ") => {
                found.app.map_or(SteamLink::Unknown, SteamLink::Title)
            }
            _ => SteamLink::Unknown,
        }
    }

    fn change(&self, edit: impl FnOnce(&mut Links) -> bool) -> bool {
        let changed = edit(&mut self.links.write());
        if changed {
            let snapshot = self.links.read().clone();
            if let Err(e) = json_store::write(&file(), &snapshot) {
                tracing::warn!("saving Steam links: {e:#}");
            }
        }
        changed
    }

    /// Copy the user's choice for `key` (empty for none). Returns whether
    /// it changed.
    pub fn note_choice(&self, key: &str, choice: &str) -> bool {
        let choice = choice.trim();
        self.change(|l| {
            if choice.is_empty() {
                l.chosen.remove(key).is_some()
            } else {
                l.chosen
                    .insert(key.to_owned(), choice.to_owned())
                    .as_deref()
                    != Some(choice)
            }
        })
    }

    pub fn seeded(&self) -> bool {
        self.links.read().seeded
    }

    /// Copy every game's choice at once (`choices`: key and choice).
    /// Returns whether a choice was new.
    pub fn seed(&self, choices: Vec<(String, String)>) -> bool {
        let mut added = false;
        self.change(|l| {
            for (key, choice) in choices {
                let choice = choice.trim();
                if !choice.is_empty() {
                    added |= l.chosen.insert(key, choice.to_owned()).as_deref() != Some(choice);
                }
            }
            l.seeded = true;
            true
        });
        added
    }

    /// Remember the app the wikis name for `key`. Returns whether it changed.
    pub fn note_wiki(&self, key: &str, app: Option<u32>) -> bool {
        self.change(|l| match app {
            Some(id) => l.wiki.insert(key.to_owned(), id) != Some(id),
            None => l.wiki.remove(key).is_some(),
        })
    }

    fn note_title(&self, key: &str, title: &str, app: Option<u32>, now: u64) -> bool {
        let title = fold_title(title).join(" ");
        self.change(|l| {
            let before = l
                .titles
                .get(key)
                .and_then(|t| (t.title == title).then_some(t.app));
            l.titles.insert(
                key.to_owned(),
                TitleMatch {
                    title,
                    app,
                    fetched: now,
                },
            );
            before != Some(app)
        })
    }

    /// Games a background round may search by title: not Steam's, not ROMs
    /// (console games), with no choice, no wiki app and no fresh search for
    /// the title they have now.
    fn wanting_title(&self, games: &[Arc<GameInfo>], now: u64) -> Vec<Arc<GameInfo>> {
        let links = self.links.read();
        games
            .iter()
            .filter(|g| !matches!(g.runner(), Runner::Steam | Runner::Emulation))
            .filter(|g| !g.title.trim().is_empty())
            .filter(|g| {
                let key = g.id().key();
                !links.chosen.contains_key(&key)
                    && !links.wiki.contains_key(&key)
                    && links.titles.get(&key).is_none_or(|t| {
                        let fresh = if t.app.is_some() {
                            TITLE_FRESH_FOR
                        } else {
                            TITLE_MISS_FRESH_FOR
                        };
                        t.title != fold_title(&g.title).join(" ")
                            || now.saturating_sub(t.fetched) >= fresh.as_secs()
                    })
            })
            .take(TITLE_ROUND)
            .cloned()
            .collect()
    }
}

/// A title's words: lower case letters and digits, `&` read as "and",
/// apostrophes joining a word ("Assassin's" is "assassins"), Roman numerals
/// II–X as digits, so "DOOM II: Hell on Earth" is "doom 2 hell on earth".
pub(crate) fn fold_title(title: &str) -> Vec<String> {
    let title = title.replace('&', " and ");
    title
        .replace(['\'', '’', '`'], "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            let digit = match lower.as_str() {
                "ii" => "2",
                "iii" => "3",
                "iv" => "4",
                "v" => "5",
                "vi" => "6",
                "vii" => "7",
                "viii" => "8",
                "ix" => "9",
                "x" => "10",
                _ => return lower,
            };
            digit.to_owned()
        })
        .collect()
}

/// Words that may come before "Edition" in an edition's name.
const EDITION_WORDS: &[&str] = &[
    "game",
    "of",
    "the",
    "year",
    "goty",
    "deluxe",
    "digital",
    "definitive",
    "complete",
    "gold",
    "ultimate",
    "enhanced",
    "special",
    "standard",
    "premium",
    "collectors",
    "anniversary",
    "legendary",
    "extended",
    "platinum",
];

/// `words` without a last edition name: "… Deluxe Edition", "… Game of the
/// Year Edition", "… GOTY", "… Game of the Year", "… Director's Cut".
fn without_edition(words: &[String]) -> &[String] {
    let ends = |tail: &[&str]| {
        words.len() > tail.len() && words[words.len() - tail.len()..].iter().eq(tail.iter())
    };
    if ends(&["game", "of", "the", "year"]) {
        return &words[..words.len() - 4];
    }
    if ends(&["directors", "cut"]) {
        return &words[..words.len() - 2];
    }
    if ends(&["goty"]) {
        return &words[..words.len() - 1];
    }
    if ends(&["edition"]) {
        let mut end = words.len() - 1;
        while end > 1 && EDITION_WORDS.contains(&words[end - 1].as_str()) {
            end -= 1;
        }
        return &words[..end];
    }
    words
}

/// The search hits that are the game titled `title`: those with the same
/// title (case, punctuation and Roman numerals aside), else those whose
/// title is the same once an edition's name is left out on either side.
/// Hits that only look alike never are.
pub(crate) fn title_candidates(title: &str, hits: &[StoreHit]) -> Vec<u32> {
    let wanted = fold_title(title);
    if wanted.is_empty() {
        return Vec::new();
    }
    let folded: Vec<(u32, Vec<String>)> =
        hits.iter().map(|h| (h.id, fold_title(&h.name))).collect();
    let exact: Vec<u32> = folded
        .iter()
        .filter(|(_, words)| *words == wanted)
        .map(|(id, _)| *id)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let base = without_edition(&wanted);
    let mut same: Vec<u32> = folded
        .iter()
        .filter(|(_, words)| without_edition(words) == base)
        .map(|(id, _)| *id)
        .collect();
    same.dedup();
    same
}

/// The candidate that is the game, given each one's release year as Steam
/// gives it: with the game's `year` known, the one released within a year
/// of it when exactly one is; without, the only candidate.
pub(crate) fn pick_by_year(year: Option<u32>, candidates: &[(u32, Option<u32>)]) -> Option<u32> {
    match year {
        None => match candidates {
            [(id, _)] => Some(*id),
            _ => None,
        },
        Some(year) => {
            let fits: Vec<u32> = candidates
                .iter()
                .filter(|(_, y)| y.is_some_and(|y| y.abs_diff(year) <= 1))
                .map(|(id, _)| *id)
                .collect();
            match (fits.as_slice(), candidates) {
                ([id], _) => Some(*id),
                // Steam gives the only candidate no date ("Coming soon").
                ([], [(id, None)]) => Some(*id),
                _ => None,
            }
        }
    }
}

fn release_year(game: &GameInfo) -> Option<u32> {
    game.release_date
        .as_deref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse().ok())
}

/// What a linked game's page takes from its Steam app's store page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SteamPage {
    fetched: u64,
    description: String,
    genres: Vec<String>,
    release_date: String,
    screenshots: Vec<String>,
    /// Trailers, highlighted first.
    videos: Vec<Video>,
    /// The Metacritic score the page shows.
    metacritic: Option<u8>,
}

fn details_file(app: u32, language: &str) -> PathBuf {
    Paths::get().cache.join("steam-details").join(format!(
        "{app}-{}.json",
        irori_core::paths::sanitize(language)
    ))
}

/// Fill what `info` lacks from a Steam store page. Returns whether
/// anything was added.
fn fill_extra(info: &mut ExtraInfo, page: &SteamPage) -> bool {
    let mut filled = false;
    let empty = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    if empty(&info.description) && empty(&info.short_description) && !page.description.is_empty() {
        info.description = Some(page.description.clone());
        filled = true;
    }
    if info.genres.is_empty() && !page.genres.is_empty() {
        info.genres = page.genres.clone();
        filled = true;
    }
    if empty(&info.release_date) && !page.release_date.is_empty() {
        info.release_date = Some(page.release_date.clone());
        filled = true;
    }
    if info.screenshots.is_empty() && !page.screenshots.is_empty() {
        info.screenshots = page.screenshots.clone();
        filled = true;
    }
    // Trailers are media, not details: they don't make the page "from Steam".
    if info.videos.is_empty() {
        info.videos = page.videos.clone();
    }
    // Nor does a score, which names its own source.
    if info.score.is_none() {
        info.score = page
            .metacritic
            .and_then(|s| irori_core::CriticScore::new("Metacritic", f64::from(s)));
    }
    if filled && info.details_from.is_none() {
        info.details_from = Some("Steam".into());
    }
    filled
}

/// Whether a store page lacks something a Steam page could give.
fn lacks_any(info: &ExtraInfo) -> bool {
    let empty = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    info.screenshots.is_empty()
        || info.videos.is_empty()
        || info.genres.is_empty()
        || empty(&info.release_date)
        || (empty(&info.description) && empty(&info.short_description))
        || info.score.is_none()
}

/// How "Find on Steam by title" went for a set of games.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleLinkReport {
    /// Games now linked by their title.
    pub linked: usize,
    /// Games Steam has no app of that title for (or several).
    pub not_found: usize,
    /// Games skipped: Steam's own, ROMs, or linked by the user or a wiki.
    pub skipped: usize,
    /// Why searching stopped, when Steam couldn't be reached.
    pub failed: Option<String>,
}

impl Backend {
    /// Talk to other servers than Steam's (tests).
    #[cfg(test)]
    pub(crate) fn set_steam_link_endpoints(&self, endpoints: Endpoints) {
        *self.steam_links.endpoints.lock() = endpoints;
    }

    /// The game's Steam app and where it comes from. The game's own
    /// setting decides a choice, so one made a moment ago (in the settings
    /// form) counts before the kept copy follows it.
    pub fn steam_link(&self, game: &GameInfo) -> SteamLink {
        if game.runner() != Runner::Steam {
            match parse_choice(&self.settings.game(&game.id()).steam_app_id) {
                Some(Some(app)) => return SteamLink::Chosen(app),
                Some(None) => return SteamLink::NoneChosen,
                None => {}
            }
        }
        match self.steam_links.linked(game) {
            // The setting was cleared and the copy hasn't followed yet.
            SteamLink::Chosen(_) | SteamLink::NoneChosen => self.steam_links.found(game),
            link => link,
        }
    }

    /// Link `id` to a Steam app: an app id (or a store address naming one),
    /// [`STEAM_NONE`] for none, or `None` to find it again on its own.
    pub fn set_steam_link(&self, id: &GameId, choice: Option<String>) {
        let choice = choice
            .map(|c| match parse_choice(&c) {
                Some(Some(app)) => app.to_string(),
                _ => STEAM_NONE.to_owned(),
            })
            .filter(|c| !c.is_empty());
        self.settings
            .set_game_value(id, LINK_KEY, choice.clone().map(serde_json::Value::String));
        if self
            .steam_links
            .note_choice(&id.key(), choice.as_deref().unwrap_or(""))
        {
            self.rebuild_library();
        }
    }

    /// Games that may be other copies of `id`'s entry: linked to the same
    /// Steam app as one of its copies, yet left apart ([`crate::copies::possible`]).
    /// Reads memory only.
    pub fn possible_copies(&self, id: &GameId) -> Vec<Arc<GameInfo>> {
        let library = self.library();
        let mut group: Vec<GameId> = library.copies_of(id).to_vec();
        if group.is_empty() {
            group.push(id.clone());
        }
        let apps: Vec<u32> = group
            .iter()
            .filter_map(|copy| library.get(copy))
            .filter_map(|g| self.steam_links.linked(g).app_id())
            .collect();
        if apps.is_empty() {
            return Vec::new();
        }
        let user = self.userdata.snapshot();
        let candidates = library.all_games().map(|g| crate::copies::Candidate {
            steam_app: self.steam_links.linked(g).app_id(),
            kept_apart: user
                .overrides
                .get(&g.id().key())
                .is_some_and(|o| o.copy_of.as_deref() == Some("")),
            game: g.clone(),
        });
        crate::copies::possible(&group, &apps, candidates, |copy| {
            library
                .copies_of(copy)
                .first()
                .cloned()
                .unwrap_or_else(|| copy.clone())
        })
    }

    /// Follow a change of `id`'s settings (the settings form edits the
    /// choice too).
    pub(crate) fn follow_steam_link_setting(&self, id: &GameId) {
        let choice = self.settings.game(id).steam_app_id;
        if self.steam_links.note_choice(&id.key(), &choice) {
            self.rebuild_library();
        }
    }

    /// Copy every game's choice once, for choices made before Irori kept
    /// them in one place. Reads each game's settings, so it runs on a
    /// blocking thread.
    pub(crate) fn seed_steam_links(&self, games: &[Arc<GameInfo>]) {
        if self.steam_links.seeded() {
            return;
        }
        let ids: Vec<GameId> = games.iter().map(|g| g.id()).collect();
        let b = self.clone();
        self.spawn_blocking(move || {
            let choices = ids
                .iter()
                .map(|id| (id.key(), b.settings.game(id).steam_app_id))
                .collect();
            if b.steam_links.seed(choices) {
                b.rebuild_library();
            }
        });
    }

    /// Remember the app the wikis name for `game`, unless the user chose.
    pub(crate) fn note_wiki_steam_app(&self, game: &GameInfo, app: Option<&str>) {
        if game.runner() == Runner::Steam {
            return;
        }
        let app = app.and_then(|a| a.trim().parse().ok());
        if self.steam_links.note_wiki(&game.id().key(), app) {
            self.rebuild_library();
        }
    }

    /// Steam's apps named like `query`, or the app a pasted id or store
    /// address names ("Link to Steam").
    pub async fn search_steam(&self, query: &str) -> Result<Vec<StoreHit>> {
        let client = self.steam_links.client()?;
        let language =
            irori_steam_api::achievements::steam_language(&self.settings.app().ui_language);
        if let Some(app) = app_id_in(query) {
            let name = client
                .app_details(app, language)
                .await
                .ok()
                .flatten()
                .map(|d| d.name)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| app.to_string());
            return Ok(vec![StoreHit {
                id: app,
                name,
                image: None,
            }]);
        }
        client
            .store_search(query, language)
            .await
            .context("Couldn’t search Steam")
    }

    /// The app with `title`, if Steam has exactly one (see
    /// [`title_candidates`], [`pick_by_year`]).
    async fn find_by_title(client: &Client, title: &str, year: Option<u32>) -> Result<Option<u32>> {
        // Store titles are mostly English, and so are Steam's names there.
        let hits = client.store_search(title, "english").await?;
        let candidates = title_candidates(title, &hits);
        if candidates.is_empty() {
            return Ok(None);
        }
        if year.is_none() || candidates.len() > YEAR_CHECKS {
            return Ok(pick_by_year(
                year,
                &candidates.iter().map(|c| (*c, None)).collect::<Vec<_>>(),
            ));
        }
        let mut dated = Vec::with_capacity(candidates.len());
        for app in candidates {
            tokio::time::sleep(TITLE_SPACING).await;
            let year = client
                .app_details(app, "english")
                .await?
                .and_then(|d| d.release_date.year());
            dated.push((app, year));
        }
        Ok(pick_by_year(year, &dated))
    }

    /// Search Steam by title for games that need it, while
    /// `iroriSteamTitleMatch` is on (after a library rebuild). Returns
    /// whether a game's app changed. One round runs at a time.
    pub(crate) async fn match_steam_titles(&self, games: &[Arc<GameInfo>]) -> bool {
        if !self.settings.app().steam_title_match {
            return false;
        }
        let wanting = self.steam_links.wanting_title(games, util::unix_now());
        if wanting.is_empty() || self.steam_links.busy.swap(true, Ordering::AcqRel) {
            return false;
        }
        let report = self.link_titles(&wanting).await;
        self.steam_links.busy.store(false, Ordering::Release);
        report.linked > 0
    }

    async fn link_titles(&self, games: &[Arc<GameInfo>]) -> TitleLinkReport {
        let mut report = TitleLinkReport::default();
        let client = match self.steam_links.client() {
            Ok(c) => c,
            Err(e) => {
                report.failed = Some(format!("{e:#}"));
                return report;
            }
        };
        for (i, game) in games.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(TITLE_SPACING).await;
            }
            match Self::find_by_title(&client, &game.title, release_year(game)).await {
                Ok(app) => {
                    self.steam_links.note_title(
                        &game.id().key(),
                        &game.title,
                        app,
                        util::unix_now(),
                    );
                    if app.is_some() {
                        report.linked += 1;
                    } else {
                        report.not_found += 1;
                    }
                }
                Err(e) => {
                    tracing::info!("Steam title search: {e:#}");
                    report.failed = Some(format!("{e:#}"));
                    break;
                }
            }
        }
        report
    }

    /// Link the picked games to the Steam app of their exact title, now,
    /// whatever `iroriSteamTitleMatch` says (the library's bulk bar). The
    /// user's choices and the wikis' apps stay.
    pub async fn link_by_title(&self, ids: &[GameId]) -> TitleLinkReport {
        let library = self.library();
        let mut wanted = Vec::new();
        let mut skipped = 0;
        for id in ids {
            let Some(game) = library.get(id).cloned() else {
                skipped += 1;
                continue;
            };
            let usable = !matches!(game.runner(), Runner::Steam | Runner::Emulation)
                && matches!(
                    self.steam_link(&game),
                    SteamLink::Unknown | SteamLink::Title(_)
                );
            if usable {
                wanted.push(game);
            } else {
                skipped += 1;
            }
        }
        let mut report = self.link_titles(&wanted).await;
        report.skipped = skipped;
        if report.linked > 0 || report.not_found > 0 {
            self.rebuild_library();
        }
        report
    }

    /// Fill a store page's gaps from the store page of the game's Steam
    /// app (a game of another store linked to one). Steam being out of
    /// reach never fails the store's page.
    pub(crate) async fn fill_from_steam(&self, id: &GameId, info: &mut ExtraInfo) {
        if id.runner == Runner::Steam || !lacks_any(info) {
            return;
        }
        let Some(game) = self.library().get(id).cloned() else {
            return;
        };
        let Some(app) = self.steam_link(&game).app_id() else {
            return;
        };
        let language =
            irori_steam_api::achievements::steam_language(&self.settings.app().ui_language);
        match self.steam_page(app, language).await {
            Ok(Some(page)) => {
                fill_extra(info, &page);
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("Steam details for {}: {e:#}", id.key()),
        }
    }

    /// Forget the store page kept for `game`'s Steam app, so it is read
    /// again ("Fetch details again").
    pub(crate) async fn forget_steam_page(&self, game: &GameInfo) {
        if game.runner() == Runner::Steam {
            return;
        }
        let Some(app) = self.steam_links.linked(game).app_id() else {
            return;
        };
        let language =
            irori_steam_api::achievements::steam_language(&self.settings.app().ui_language);
        match tokio::fs::remove_file(details_file(app, language)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("forgetting Steam details: {e}");
            }
            _ => {}
        }
    }

    /// The parts of app `app`'s store page a linked game uses, cached a week.
    async fn steam_page(&self, app: u32, language: &str) -> Result<Option<SteamPage>> {
        let path = details_file(app, language);
        let read = path.clone();
        let cached: Option<SteamPage> =
            tokio::task::spawn_blocking(move || json_store::read(&read).ok().flatten())
                .await
                .ok()
                .flatten();
        let now = util::unix_now();
        if let Some(page) = cached
            .as_ref()
            .filter(|p| now.saturating_sub(p.fetched) < DETAILS_FRESH_FOR.as_secs())
        {
            return Ok(Some(page.clone()));
        }
        let fetched = match self.steam_links.client()?.app_details(app, language).await {
            Ok(found) => found,
            // An old page beats none while Steam is out of reach.
            Err(e) if cached.is_some() => {
                tracing::debug!("Steam details: {e:#}");
                return Ok(cached);
            }
            Err(e) => return Err(e),
        };
        let page = fetched.map(|d| SteamPage {
            fetched: now,
            description: d.description().trim().to_owned(),
            genres: d.genres.iter().map(|g| g.description.clone()).collect(),
            release_date: d.release_date.date.trim().to_owned(),
            screenshots: crate::media::steam::screenshots_of(&d),
            videos: crate::media::steam::videos_of(&d),
            metacritic: d
                .metacritic
                .as_ref()
                .and_then(|m| u8::try_from(m.score).ok())
                .filter(|s| (1..=100).contains(s)),
        });
        let stored = page.clone().unwrap_or(SteamPage {
            fetched: now,
            ..Default::default()
        });
        let written = tokio::task::spawn_blocking(move || json_store::write(&path, &stored)).await;
        if let Ok(Err(e)) | Err(e) = written.map_err(anyhow::Error::from) {
            tracing::warn!("caching Steam details: {e:#}");
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: u32, name: &str) -> StoreHit {
        StoreHit {
            id,
            name: name.into(),
            image: None,
        }
    }

    fn game(runner: Runner, app: &str, title: &str) -> GameInfo {
        GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn titles_fold_like_steam_spells_them() {
        assert_eq!(
            fold_title("DOOM II: Hell on Earth"),
            ["doom", "2", "hell", "on", "earth"]
        );
        assert_eq!(
            fold_title("Assassin’s Creed® & Co."),
            ["assassins", "creed", "and", "co"]
        );
    }

    #[test]
    fn only_the_same_title_matches_edition_words_aside() {
        let hits = [
            hit(1, "Hades"),
            hit(2, "Hades II"),
            hit(3, "Hades: Battle Out of Hell"),
        ];
        assert_eq!(title_candidates("HADES", &hits), [1]);
        assert_eq!(title_candidates("Hades 2", &hits), [2]);
        // An edition of the game is the game…
        assert_eq!(title_candidates("Hades - Deluxe Edition", &hits), [1]);
        assert_eq!(title_candidates("Hades GOTY", &hits), [1]);
        assert_eq!(
            title_candidates("Hades Game of the Year Edition", &hits),
            [1]
        );
        // …but a similar title never is.
        assert!(title_candidates("Hades Battle", &hits).is_empty());
        assert!(title_candidates("Hadès Online", &hits).is_empty());
        assert!(title_candidates("", &hits).is_empty());
        // The exact edition wins over the plain game.
        let skyrim = [
            hit(72850, "The Elder Scrolls V: Skyrim"),
            hit(489830, "The Elder Scrolls V: Skyrim Special Edition"),
        ];
        assert_eq!(
            title_candidates("The Elder Scrolls V: Skyrim Special Edition", &skyrim),
            [489830]
        );
        // Another edition matches both: the year decides, or nothing does.
        assert_eq!(
            title_candidates("The Elder Scrolls V: Skyrim Anniversary Edition", &skyrim),
            [72850, 489830]
        );
        // "Edition" alone is a title, not an edition's name.
        assert_eq!(without_edition(&fold_title("Edition")), ["edition"]);
    }

    #[test]
    fn the_release_year_tells_same_named_games_apart() {
        assert_eq!(pick_by_year(None, &[(1, None)]), Some(1));
        assert_eq!(pick_by_year(None, &[(1, None), (2, None)]), None);
        let remakes = [(1, Some(1998)), (2, Some(2023))];
        assert_eq!(pick_by_year(Some(2023), &remakes), Some(2));
        assert_eq!(pick_by_year(Some(1999), &remakes), Some(1));
        assert_eq!(pick_by_year(Some(2010), &remakes), None);
        assert_eq!(pick_by_year(Some(2010), &[(1, Some(2015))]), None);
        assert_eq!(pick_by_year(Some(2010), &[(1, None)]), Some(1));
    }

    #[test]
    fn links_follow_the_user_then_the_wikis_then_the_title() {
        crate::tests::isolate_paths();
        let links = SteamLinkService::empty();
        let gog = game(Runner::Gog, "1207658930", "Portal 2");
        assert_eq!(links.linked(&gog), SteamLink::Unknown);
        assert_eq!(
            links.linked(&game(Runner::Steam, "620", "Portal 2")),
            SteamLink::Own(620)
        );

        assert!(links.note_title(&gog.id().key(), "Portal 2", Some(1), 5));
        assert_eq!(links.linked(&gog), SteamLink::Title(1));
        // A renamed game's search was for another title.
        let renamed = GameInfo {
            title: "Portal Two".into(),
            ..gog.clone()
        };
        assert_eq!(links.linked(&renamed), SteamLink::Unknown);

        assert!(links.note_wiki(&gog.id().key(), Some(620)));
        assert!(!links.note_wiki(&gog.id().key(), Some(620)));
        assert_eq!(links.linked(&gog), SteamLink::Wiki(620));

        assert!(links.note_choice(&gog.id().key(), "https://store.steampowered.com/app/400/"));
        assert_eq!(links.linked(&gog), SteamLink::Chosen(400));
        assert!(links.note_choice(&gog.id().key(), "none"));
        assert_eq!(links.linked(&gog), SteamLink::NoneChosen);
        assert!(links.note_choice(&gog.id().key(), ""));
        assert_eq!(links.linked(&gog), SteamLink::Wiki(620));
    }

    #[test]
    fn background_rounds_skip_what_is_known() {
        crate::tests::isolate_paths();
        let links = SteamLinkService::empty();
        let now = 1_000_000_000;
        let games: Vec<Arc<GameInfo>> = [
            game(Runner::Steam, "620", "Portal 2"),
            game(Runner::Emulation, "rom", "Chrono Trigger"),
            game(Runner::Gog, "a", "Hades"),
            game(Runner::Gog, "b", "Celeste"),
            game(Runner::Gog, "c", "Inside"),
            game(Runner::Sideload, "d", "Tunic"),
        ]
        .into_iter()
        .map(Arc::new)
        .collect();
        links.note_choice("gog:b", "none");
        links.note_wiki("gog:c", Some(304430));
        links.note_title("sideload:d", "Tunic", None, now);
        let keys = |list: Vec<Arc<GameInfo>>| list.iter().map(|g| g.id().key()).collect::<Vec<_>>();
        assert_eq!(keys(links.wanting_title(&games, now)), ["gog:a"]);
        // A search that found nothing is asked again after a week.
        let later = now + TITLE_MISS_FRESH_FOR.as_secs();
        assert_eq!(
            keys(links.wanting_title(&games, later)),
            ["gog:a", "sideload:d"]
        );
    }

    #[test]
    fn steam_pages_fill_only_gaps() {
        let page = SteamPage {
            fetched: 1,
            description: "From Steam.".into(),
            genres: vec!["Puzzle".into()],
            release_date: "18 Apr, 2011".into(),
            screenshots: vec!["https://cdn.example/1.jpg".into()],
            videos: vec![Video {
                name: "Launch".into(),
                thumbnail: None,
                url: "https://cdn.example/l.m3u8".into(),
            }],
            metacritic: Some(88),
        };
        let mut info = ExtraInfo {
            description: Some("The store's own.".into()),
            ..Default::default()
        };
        assert!(fill_extra(&mut info, &page));
        assert_eq!(info.description.as_deref(), Some("The store's own."));
        assert_eq!(info.genres, ["Puzzle"]);
        assert_eq!(info.screenshots, ["https://cdn.example/1.jpg"]);
        assert_eq!(info.details_from.as_deref(), Some("Steam"));
        assert_eq!(info.videos.len(), 1);
        assert_eq!(info.score.as_ref().map(|s| s.score), Some(88));
        assert!(!lacks_any(&info));
        assert!(!fill_extra(&mut info, &page), "nothing left to fill");
    }
}
