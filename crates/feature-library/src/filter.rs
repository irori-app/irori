//! Library view criteria, their persistence, and the cached filtered list.
//!
//! [`ViewFilter`] is what the toolbar and the Filters popover edit. It maps onto
//! [`irori_services::LibraryFilter`] for the heavy lifting and adds what the
//! service filter cannot express (several categories at once, an
//! "Uncategorized" choice, play statuses, hiding games whose files are
//! missing). The result
//! is cached by library revision, user-data identity, update-set identity,
//! availability identity and filter, so rendering never refilters an
//! unchanged library. Grouping by play status or game mode reorders that
//! result into sections ([`StatusGroup`]) in the same pass.
//!
//! Saved views and the persisted view keep criteria this version doesn't
//! know ([`ViewFilter::unknown`]), so a view a newer Irori saved keeps its
//! filters when an older one rewrites the file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use irori_core::{
    GameId, GameInfo, GameMode, InstallPlatform, Paths, Runner, ShownStores, json_store,
};
use irori_services::{
    EffectiveStatus, LibraryFilter, LibrarySnapshot, LibrarySort, StoreFilter, UserData,
};
use irori_ui::i18n::{t, t_args};
use serde::{Deserialize, Serialize};

/// Which install states to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallFilter {
    #[default]
    Any,
    Installed,
    NotInstalled,
}

/// When the shown games came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseWindow {
    #[default]
    Any,
    ThisYear,
    /// This year and the four before it.
    LastFiveYears,
    /// Before the last five years.
    Older,
}

impl ReleaseWindow {
    /// Whether a game released on `date` (`YYYY-MM-DD…`) falls in the window
    /// in `year`. Games with no known date only match [`ReleaseWindow::Any`].
    pub fn contains(self, date: Option<&str>, year: i32) -> bool {
        let released = date.and_then(|d| d.get(..4)?.parse::<i32>().ok());
        match (self, released) {
            (ReleaseWindow::Any, _) => true,
            (_, None) => false,
            (ReleaseWindow::ThisYear, Some(y)) => y == year,
            (ReleaseWindow::LastFiveYears, Some(y)) => (year - 4..=year).contains(&y),
            (ReleaseWindow::Older, Some(y)) => y < year - 4,
        }
    }

    pub fn label(self) -> String {
        t(match self {
            ReleaseWindow::Any => "library.filters.released.any",
            ReleaseWindow::ThisYear => "library.filters.released.this_year",
            ReleaseWindow::LastFiveYears => "library.filters.released.last_five_years",
            ReleaseWindow::Older => "library.filters.released.older",
        })
        .to_string()
    }
}

/// How the shown games are split into sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    /// One list in the chosen order.
    #[default]
    None,
    /// A section per play status, in [`GROUP_ORDER`]; the chosen order holds
    /// within each.
    Status,
    /// A section per game mode, in [`MODE_GROUP_ORDER`]: each game in the
    /// section of its most specific mode ([`mode_section`]).
    Mode,
}

/// Order of the game-mode sections: playing together on one screen first,
/// then co-op, network play and MMOs, single player only, and games whose
/// modes nobody knows (`None`) last.
pub const MODE_GROUP_ORDER: [Option<GameMode>; 6] = [
    Some(GameMode::Local),
    Some(GameMode::Coop),
    Some(GameMode::Online),
    Some(GameMode::Mmo),
    Some(GameMode::Single),
    None,
];

/// The section a game with `modes` goes in when grouped by game mode. A
/// game plays several ways, but sits in one section, so the list keeps each
/// game once: an MMO is an MMO before it is online, a split-screen game is
/// couch play before co-op, co-op is named before network play, and single
/// player is what's left.
pub fn mode_section(modes: &[GameMode]) -> Option<GameMode> {
    [
        GameMode::Mmo,
        GameMode::Local,
        GameMode::Coop,
        GameMode::Online,
        GameMode::Single,
    ]
    .into_iter()
    .find(|m| modes.contains(m))
}

/// Order of the play-status sections: what to pick up next first (games
/// being played, paused, planned), then the rest of the backlog, finished
/// and dropped games last. Stats charts use their own order
/// ([`EffectiveStatus::ALL`]).
pub const GROUP_ORDER: [EffectiveStatus; 7] = [
    EffectiveStatus::Playing,
    EffectiveStatus::OnHold,
    EffectiveStatus::PlanToPlay,
    EffectiveStatus::Played,
    EffectiveStatus::NotPlayed,
    EffectiveStatus::Beaten,
    EffectiveStatus::Abandoned,
];

/// What a section of a grouped list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Status(EffectiveStatus),
    /// Games of this [`mode_section`]; `None` for modes unknown.
    Mode(Option<GameMode>),
}

/// One section of a grouped list: the games at `range` of the shown list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusGroup {
    pub kind: GroupKind,
    pub range: std::ops::Range<usize>,
}

/// Split `games`, already ordered so equal kinds sit together, into
/// sections.
fn sections(games: &[Arc<GameInfo>], kind: impl Fn(&GameInfo) -> GroupKind) -> Vec<StatusGroup> {
    let mut groups: Vec<StatusGroup> = Vec::new();
    for (ix, game) in games.iter().enumerate() {
        let kind = kind(game);
        match groups.last_mut() {
            Some(group) if group.kind == kind => group.range.end = ix + 1,
            _ => groups.push(StatusGroup {
                kind,
                range: ix..ix + 1,
            }),
        }
    }
    groups
}

/// Reorder `games` into game-mode sections (stable, so the chosen order
/// holds within each) and return the non-empty sections.
pub fn group_by_mode(games: &mut [Arc<GameInfo>]) -> Vec<StatusGroup> {
    let rank = |g: &Arc<GameInfo>| {
        let section = mode_section(&g.game_modes);
        MODE_GROUP_ORDER
            .iter()
            .position(|s| *s == section)
            .unwrap_or(MODE_GROUP_ORDER.len())
    };
    games.sort_by_cached_key(rank);
    sections(games, |g| GroupKind::Mode(mode_section(&g.game_modes)))
}

/// Reorder `games` into play-status sections (stable, so the chosen order
/// holds within each) and return the non-empty sections.
pub fn group_by_status(games: &mut [Arc<GameInfo>], user: &UserData) -> Vec<StatusGroup> {
    let rank = |status: EffectiveStatus| {
        GROUP_ORDER
            .iter()
            .position(|s| *s == status)
            .unwrap_or(GROUP_ORDER.len())
    };
    games.sort_by_cached_key(|g| rank(user.effective_status(&g.id().key())));
    sections(games, |g| {
        GroupKind::Status(user.effective_status(&g.id().key()))
    })
}

/// How hidden games are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenMode {
    /// Hidden games are left out (the default).
    #[default]
    Hide,
    /// Hidden games are shown next to the others.
    Show,
    /// Only hidden games are shown (the "Hidden" view).
    Only,
}

/// One selectable entry of the category filter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryFilter {
    /// Games that belong to no category.
    Uncategorized,
    Named(String),
}

impl CategoryFilter {
    pub fn label(&self) -> String {
        match self {
            CategoryFilter::Uncategorized => t("library.filter.uncategorized").to_string(),
            CategoryFilter::Named(n) => n.clone(),
        }
    }
}

/// Selected categories; empty means every game. Accepts the single-category
/// shape written by earlier versions (`"all"`, `"uncategorized"`,
/// `{"named": …}`).
fn deserialize_categories<'de, D>(d: D) -> Result<BTreeSet<CategoryFilter>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Legacy {
        All,
        Uncategorized,
        Named(String),
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Set(BTreeSet<CategoryFilter>),
        Legacy(Legacy),
    }
    Ok(match Repr::deserialize(d)? {
        Repr::Set(set) => set,
        Repr::Legacy(Legacy::All) => BTreeSet::new(),
        Repr::Legacy(Legacy::Uncategorized) => [CategoryFilter::Uncategorized].into(),
        Repr::Legacy(Legacy::Named(n)) => [CategoryFilter::Named(n)].into(),
    })
}

/// One account of a store, picked in the library's account switcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFilter {
    pub runner: Runner,
    pub account_id: String,
}

/// Everything that decides which games the library shows and in which order.
/// Persisted between sessions except for the search query.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewFilter {
    #[serde(skip)]
    pub query: String,
    pub store: StoreFilter,
    /// Only the games this account owns, while its store is the one shown.
    pub account: Option<AccountFilter>,
    pub platform: Option<InstallPlatform>,
    pub install: InstallFilter,
    pub favourites_only: bool,
    pub hidden: HiddenMode,
    pub offline_only: bool,
    pub hide_third_party: bool,
    pub updates_only: bool,
    /// Hide installed games whose files are missing (unmounted drive,
    /// deleted folder).
    pub hide_unavailable: bool,
    /// Games in any of these categories are shown; empty shows every game.
    #[serde(alias = "category", deserialize_with = "deserialize_categories")]
    pub categories: BTreeSet<CategoryFilter>,
    pub released: ReleaseWindow,
    /// Emulated games of these systems only (systems catalog ids); empty
    /// shows every game.
    pub systems: BTreeSet<String>,
    /// Games in any of these play statuses (as stats count them: a game
    /// without one is played or not played); empty shows every game.
    pub statuses: BTreeSet<EffectiveStatus>,
    /// Only games every board's rating allows at this age (see
    /// [`passes_age_limit`]); `None` shows every game.
    pub max_age: Option<u8>,
    /// With an age limit, also show games no board rated.
    pub include_unrated: bool,
    /// Games with any of these modes; empty shows every game.
    #[serde(deserialize_with = "deserialize_modes")]
    pub modes: BTreeSet<GameMode>,
    pub sort: LibrarySort,
    pub descending: bool,
    /// Sections the list is split into. Like sorting, it orders rather than
    /// narrows, so filter resets keep it.
    #[serde(deserialize_with = "deserialize_grouping")]
    pub group: Grouping,
    /// Criteria a newer Irori saved that this one doesn't know, kept as
    /// they were so rewriting the file doesn't drop them.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// Selected game modes; modes this version doesn't know are left out.
fn deserialize_modes<'de, D>(d: D) -> Result<BTreeSet<GameMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(irori_core::game_mode::lenient(d)?.into_iter().collect())
}

/// A grouping; one this version doesn't know shows one list.
fn deserialize_grouping<'de, D>(d: D) -> Result<Grouping, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Known(Grouping),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Repr::deserialize(d)? {
        Repr::Known(g) => g,
        Repr::Other(_) => Grouping::None,
    })
}

impl ViewFilter {
    /// Number of criteria set in the Filters popover (store and search have
    /// their own visible controls and are not counted).
    pub fn active_count(&self) -> usize {
        [
            self.platform.is_some(),
            self.install != InstallFilter::Any,
            self.favourites_only,
            self.hidden != HiddenMode::Hide,
            self.offline_only,
            self.hide_third_party,
            self.updates_only,
            self.hide_unavailable,
            !self.categories.is_empty(),
            self.released != ReleaseWindow::Any,
            !self.systems.is_empty(),
            !self.statuses.is_empty(),
            self.max_age.is_some(),
            !self.modes.is_empty(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count()
    }

    /// Reset the Filters popover criteria to their defaults. Sorting, store and
    /// search are kept.
    pub fn reset_filters(&mut self) {
        *self = ViewFilter {
            query: std::mem::take(&mut self.query),
            store: self.store,
            account: self.account.take(),
            sort: self.sort,
            descending: self.descending,
            group: self.group,
            ..Default::default()
        };
    }

    /// Reset everything that narrows the list: search, store and filters.
    pub fn clear_all(&mut self) {
        self.reset_filters();
        self.query.clear();
        self.store = StoreFilter::All;
        self.account = None;
    }

    /// Whether anything narrows the list.
    pub fn is_narrowed(&self) -> bool {
        !self.query.trim().is_empty()
            || self.store != StoreFilter::All
            || self.account.is_some()
            || self.active_count() > 0
    }

    /// The one store the list shows: the switcher's choice, or the only
    /// store offered.
    pub fn shown_store(&self, stores: &[Runner]) -> Option<Runner> {
        match (self.store, stores) {
            (StoreFilter::Store(r), _) if stores.contains(&r) => Some(r),
            (_, [only]) => Some(*only),
            _ => None,
        }
    }

    /// Short descriptions of the active criteria, for the "no results" state.
    pub fn describe(&self) -> Vec<String> {
        let text = |key: &str| t(key).to_string();
        let mut out = Vec::new();
        if !self.query.trim().is_empty() {
            let query = self.query.trim();
            out.push(t_args("library.filter.describe.search", &[("query", query)]).to_string());
        }
        if let StoreFilter::Store(r) = self.store {
            out.push(r.store_name().to_string());
        }
        if let Some(p) = self.platform {
            out.push(platform_label(p));
        }
        match self.install {
            InstallFilter::Any => {}
            InstallFilter::Installed => out.push(text("library.filter.describe.installed")),
            InstallFilter::NotInstalled => out.push(text("library.filter.describe.not_installed")),
        }
        if self.favourites_only {
            out.push(text("library.filter.describe.favourites"));
        }
        match self.hidden {
            HiddenMode::Hide => {}
            HiddenMode::Show => out.push(text("library.filter.describe.hidden_shown")),
            HiddenMode::Only => out.push(text("library.filter.describe.hidden_only")),
        }
        if self.offline_only {
            out.push(text("library.filter.describe.offline"));
        }
        if self.hide_third_party {
            out.push(text("library.filter.describe.no_third_party"));
        }
        if self.updates_only {
            out.push(text("library.filter.describe.updates"));
        }
        if self.hide_unavailable {
            out.push(text("library.filter.describe.missing_hidden"));
        }
        if self.released != ReleaseWindow::Any {
            let when = self.released.label();
            out.push(t_args("library.filter.describe.released", &[("when", &when)]).to_string());
        }
        if !self.systems.is_empty() {
            let names: Vec<&str> = self
                .systems
                .iter()
                .map(|id| irori_engines::systems::get(id).map_or(id.as_str(), |s| s.short.as_str()))
                .collect();
            out.push(names.join(&t("library.filter.list_separator")));
        }
        if !self.statuses.is_empty() {
            let names: Vec<String> = self
                .statuses
                .iter()
                .map(|s| irori_ui::play_status::label(*s).to_string())
                .collect();
            out.push(names.join(&t("library.filter.list_separator")));
        }
        if let Some(age) = self.max_age {
            let key = if self.include_unrated {
                "library.filter.describe.max_age_unrated"
            } else {
                "library.filter.describe.max_age"
            };
            out.push(t_args(key, &[("age", &age.to_string())]).to_string());
        }
        if !self.modes.is_empty() {
            let names: Vec<String> = self
                .modes
                .iter()
                .map(|m| irori_ui::game_mode::label(*m).to_string())
                .collect();
            out.push(names.join(&t("library.filter.list_separator")));
        }
        let heading = self.category_heading();
        match self.categories.len() {
            0 => {}
            1 => out.push(
                t_args("library.filter.describe.category", &[("name", &heading)]).to_string(),
            ),
            _ => out.push(
                t_args("library.filter.describe.categories", &[("names", &heading)]).to_string(),
            ),
        }
        out
    }

    /// Add or remove one category from the selection.
    pub fn toggle_category(&mut self, category: CategoryFilter) {
        if !self.categories.remove(&category) {
            self.categories.insert(category);
        }
    }

    /// Follow a category rename in the selection.
    pub fn rename_category(&mut self, from: &str, to: &str) {
        if self
            .categories
            .remove(&CategoryFilter::Named(from.to_string()))
        {
            self.categories
                .insert(CategoryFilter::Named(to.to_string()));
        }
    }

    /// Drop a deleted category from the selection. When it was the only one
    /// selected, the library returns to every game.
    pub fn forget_category(&mut self, name: &str) {
        self.categories
            .remove(&CategoryFilter::Named(name.to_string()));
    }

    /// Title for the selected categories: the name of a single one, the
    /// names of up to three, else a count.
    pub fn category_heading(&self) -> String {
        match self.categories.len() {
            0 => String::new(),
            1..=3 => self
                .categories
                .iter()
                .map(CategoryFilter::label)
                .collect::<Vec<_>>()
                .join(&t("library.filter.list_separator")),
            n => t_args(
                "library.filter.category_count",
                &[("count", &n.to_string())],
            )
            .to_string(),
        }
    }

    /// The service filter for these criteria. `stores` are the stores the
    /// store switcher offers; a persisted store that is no longer offered
    /// falls back to all stores.
    pub fn to_library_filter(&self, stores: &[Runner]) -> LibraryFilter {
        let store = match self.store {
            StoreFilter::Store(r) if stores.contains(&r) => StoreFilter::Store(r),
            _ => StoreFilter::All,
        };
        LibraryFilter {
            query: self.query.trim().to_string(),
            store,
            platform: self.platform,
            installed_only: self.install == InstallFilter::Installed,
            not_installed_only: self.install == InstallFilter::NotInstalled,
            favourites_only: self.favourites_only,
            show_hidden: self.hidden == HiddenMode::Show,
            hidden_only: self.hidden == HiddenMode::Only,
            offline_capable_only: self.offline_only,
            hide_third_party: self.hide_third_party,
            category: None,
            updates_only: self.updates_only,
            sort: self.sort,
            descending: self.descending,
        }
    }
}

/// Display name of a platform; operating systems keep their own names.
pub fn platform_label(platform: InstallPlatform) -> String {
    match platform {
        InstallPlatform::Browser => t("library.platform.browser").to_string(),
        other => other.label().to_string(),
    }
}

/// Where the view criteria are stored.
pub fn state_file() -> PathBuf {
    Paths::get().state_dir().join("library-view.json")
}

/// Load persisted criteria (defaults when missing or unreadable).
pub fn load() -> ViewFilter {
    json_store::read_or_default(&state_file())
}

/// Persist criteria (the search query is never written).
pub fn save(filter: &ViewFilter) -> anyhow::Result<()> {
    json_store::write(&state_file(), filter)
}

/// Stores the store switcher offers: stores with games in the library, plus
/// stores the user is logged in to, leaving out stores the user hid.
/// Ordered like [`Runner::ALL`].
pub fn offered_stores(
    library: &LibrarySnapshot,
    logged_in: &dyn Fn(Runner) -> bool,
    shown: &ShownStores,
) -> Vec<Runner> {
    let mut with_games = HashSet::new();
    for g in &library.games {
        with_games.insert(g.runner());
    }
    Runner::ALL
        .into_iter()
        .filter(|r| with_games.contains(r) || (r.has_account() && logged_in(*r)))
        .filter(|r| shown.shows(*r))
        .collect()
}

/// The age limits the Filters popover offers, after "Any": PEGI's steps,
/// which ESRB's, USK's and the others' ages fall between.
pub const AGE_LIMITS: [u8; 4] = [3, 7, 12, 16];

/// Whether a game passes an age limit: every board that rated it allows
/// `limit`, so the strictest rating decides. A game no board rated (or
/// whose only rating is pending) passes only with `include_unrated`.
pub fn passes_age_limit(game: &GameInfo, limit: u8, include_unrated: bool) -> bool {
    match irori_core::age_rating::strictest_age(&game.age_ratings) {
        Some(age) => age <= limit,
        None => include_unrated,
    }
}

/// Whether `key` passes the category selection (union of the selected
/// categories; "Uncategorized" matches games in no category).
fn in_categories(selected: &BTreeSet<CategoryFilter>, user: &UserData, key: &str) -> bool {
    selected.iter().any(|c| match c {
        CategoryFilter::Uncategorized => !user.categories.values().any(|set| set.contains(key)),
        CategoryFilter::Named(name) => user.categories.get(name).is_some_and(|s| s.contains(key)),
    })
}

/// Apply the criteria to a library snapshot. `missing` holds installed games
/// whose files were not found.
pub fn apply(
    filter: &ViewFilter,
    stores: &[Runner],
    library: &LibrarySnapshot,
    user: &UserData,
    updates: &HashSet<GameId>,
    missing: &HashSet<GameId>,
) -> Vec<Arc<GameInfo>> {
    let mut games = filter
        .to_library_filter(stores)
        .apply(library, user, updates);
    if !filter.categories.is_empty() {
        games.retain(|g| in_categories(&filter.categories, user, &g.id().key()));
    }
    if let Some(account) = &filter.account
        && filter.shown_store(stores) == Some(account.runner)
        && library.owners.keys().any(|id| id.runner == account.runner)
    {
        games.retain(|g| library.owners(&g.id()).contains(&account.account_id));
    }
    if !filter.systems.is_empty() {
        games.retain(|g| {
            g.system
                .as_ref()
                .is_some_and(|s| filter.systems.contains(s))
        });
    }
    if !filter.statuses.is_empty() {
        games.retain(|g| {
            filter
                .statuses
                .contains(&user.effective_status(&g.id().key()))
        });
    }
    if filter.hide_unavailable && !missing.is_empty() {
        games.retain(|g| !missing.contains(&g.id()));
    }
    if let Some(limit) = filter.max_age {
        games.retain(|g| passes_age_limit(g, limit, filter.include_unrated));
    }
    if !filter.modes.is_empty() {
        games.retain(|g| g.game_modes.iter().any(|m| filter.modes.contains(m)));
    }
    if filter.released != ReleaseWindow::Any {
        let year = chrono::Datelike::year(&chrono::Local::now());
        games.retain(|g| filter.released.contains(g.release_date.as_deref(), year));
    }
    games
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    user: usize,
    updates: usize,
    missing: usize,
    stores: Vec<Runner>,
    filter: ViewFilter,
}

/// The filtered, sorted game list for one set of inputs, with its
/// play-status sections when grouped and an index from game identity to
/// position (keyboard focus is kept by identity).
#[derive(Default)]
pub struct FilteredGames {
    key: Option<CacheKey>,
    games: Arc<[Arc<GameInfo>]>,
    groups: Arc<[StatusGroup]>,
    positions: HashMap<GameId, usize>,
    recomputes: usize,
}

impl FilteredGames {
    /// Current list for these inputs, recomputed only when one changed.
    /// `user`, `updates` and `missing` are compared by identity: a new `Arc`
    /// is published for every change.
    pub fn get(
        &mut self,
        filter: &ViewFilter,
        stores: &[Runner],
        library: &LibrarySnapshot,
        user: &Arc<UserData>,
        updates: &Arc<HashSet<GameId>>,
        missing: &Arc<HashSet<GameId>>,
    ) -> Arc<[Arc<GameInfo>]> {
        let key = CacheKey {
            revision: library.revision,
            user: Arc::as_ptr(user) as usize,
            updates: Arc::as_ptr(updates) as usize,
            missing: Arc::as_ptr(missing) as usize,
            stores: stores.to_vec(),
            filter: filter.clone(),
        };
        if self.key.as_ref() != Some(&key) {
            let mut games = apply(filter, stores, library, user, updates, missing);
            self.groups = match filter.group {
                Grouping::None => Arc::from(Vec::new()),
                Grouping::Status => group_by_status(&mut games, user).into(),
                Grouping::Mode => group_by_mode(&mut games).into(),
            };
            let games: Arc<[Arc<GameInfo>]> = games.into();
            self.positions = games.iter().enumerate().map(|(i, g)| (g.id(), i)).collect();
            self.games = games;
            self.key = Some(key);
            self.recomputes += 1;
        }
        self.games.clone()
    }

    /// Sections of the last computed list; empty when it is not grouped.
    pub fn groups(&self) -> Arc<[StatusGroup]> {
        self.groups.clone()
    }

    /// Position of a game in the last computed list.
    pub fn position(&self, id: &GameId) -> Option<usize> {
        self.positions.get(id).copied()
    }

    /// How many times the list was rebuilt.
    #[cfg(test)]
    pub fn recomputes(&self) -> usize {
        self.recomputes
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn game(runner: Runner, app: &str, title: &str, installed: bool) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                runner: Some(runner),
                app_name: app.into(),
                title: title.into(),
                is_installed: installed,
                ..Default::default()
            }
            .finalize(),
        )
    }

    pub(crate) fn snapshot(games: Vec<Arc<GameInfo>>) -> LibrarySnapshot {
        // LibrarySnapshot's index is private; a snapshot built through Default
        // and filled with games is enough for filtering, which only iterates.
        let mut s = LibrarySnapshot::default();
        s.revision = 1;
        s.games = games;
        s
    }

    fn sample() -> LibrarySnapshot {
        snapshot(vec![
            game(Runner::Gog, "1", "The Witcher 3", true),
            game(Runner::Legendary, "a", "Alan Wake", false),
            game(Runner::Legendary, "b", "Hades", true),
            game(Runner::Sideload, "x", "Itch game", true),
        ])
    }

    fn titles(v: &[Arc<GameInfo>]) -> Vec<&str> {
        v.iter().map(|g| g.title.as_str()).collect()
    }

    #[test]
    fn counts_and_resets_filters() {
        let mut f = ViewFilter {
            query: "hades".into(),
            store: StoreFilter::Store(Runner::Gog),
            platform: Some(InstallPlatform::Linux),
            install: InstallFilter::Installed,
            favourites_only: true,
            hidden: HiddenMode::Only,
            sort: LibrarySort::Playtime,
            descending: true,
            categories: [CategoryFilter::Uncategorized].into(),
            hide_unavailable: true,
            ..Default::default()
        };
        assert_eq!(f.active_count(), 6);
        assert!(f.is_narrowed());
        f.reset_filters();
        assert_eq!(f.active_count(), 0);
        // Reset keeps search, store and sorting, and never turns hidden games on.
        assert_eq!(f.query, "hades");
        assert_eq!(f.store, StoreFilter::Store(Runner::Gog));
        assert_eq!(f.sort, LibrarySort::Playtime);
        assert!(f.descending);
        assert_eq!(f.hidden, HiddenMode::Hide);
        f.clear_all();
        assert!(!f.is_narrowed());
    }

    fn played(app: &str, title: &str, modes: &[GameMode]) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                runner: Some(Runner::Steam),
                app_name: app.into(),
                title: title.into(),
                game_modes: modes.to_vec(),
                ..Default::default()
            }
            .finalize(),
        )
    }

    #[test]
    fn filters_and_groups_by_game_mode() {
        use GameMode::*;
        let library = snapshot(vec![
            played("1", "Celeste", &[Single]),
            played("2", "Portal 2", &[Single, Coop, Local, Online]),
            played("3", "Deep Rock Galactic", &[Single, Coop, Online]),
            played("4", "Guild Wars 2", &[Online, Mmo, Coop]),
            played("5", "Mystery", &[]),
            played("6", "Rocket League", &[Local, Online]),
        ]);
        let user = Arc::new(UserData::default());
        let none = Arc::new(HashSet::new());
        let f = ViewFilter {
            modes: [Local, Mmo].into(),
            ..Default::default()
        };
        // Any of the chosen modes.
        let shown = apply(&f, &[], &library, &user, &none, &none);
        let mut shown = titles(&shown);
        shown.sort_unstable();
        assert_eq!(shown, ["Guild Wars 2", "Portal 2", "Rocket League"]);
        assert_eq!(f.active_count(), 1);
        assert_eq!(f.describe().len(), 1);
        let mut reset = f.clone();
        reset.reset_filters();
        assert!(reset.modes.is_empty());

        let mut cache = FilteredGames::default();
        let f = ViewFilter {
            group: Grouping::Mode,
            ..Default::default()
        };
        let games = cache.get(&f, &[], &library, &user, &none, &none);
        assert_eq!(
            titles(&games),
            [
                "Portal 2",
                "Rocket League",
                "Deep Rock Galactic",
                "Guild Wars 2",
                "Celeste",
                "Mystery"
            ]
        );
        let sections: Vec<_> = cache
            .groups()
            .iter()
            .map(|g| (g.kind, g.range.clone()))
            .collect();
        assert_eq!(
            sections,
            [
                (GroupKind::Mode(Some(Local)), 0..2),
                (GroupKind::Mode(Some(Coop)), 2..3),
                (GroupKind::Mode(Some(Mmo)), 3..4),
                (GroupKind::Mode(Some(Single)), 4..5),
                (GroupKind::Mode(None), 5..6),
            ]
        );
    }

    #[test]
    fn keeps_criteria_it_does_not_know() {
        // A view a newer Irori saved: a facet and a mode this one doesn't
        // know, and a grouping it doesn't have.
        let json = r#"{"favourites_only": true, "modes": ["coop", "vr"],
            "group": "genre", "price": {"max": 10}, "tags": ["roguelike"]}"#;
        let f: ViewFilter = serde_json::from_str(json).unwrap();
        assert!(f.favourites_only);
        assert_eq!(f.modes, [GameMode::Coop].into());
        assert_eq!(f.group, Grouping::None);
        assert_eq!(f.unknown.len(), 2);
        // Writing it again keeps the unknown facets as they were.
        let written = serde_json::to_value(&f).unwrap();
        assert_eq!(written["price"], serde_json::json!({"max": 10}));
        assert_eq!(written["tags"], serde_json::json!(["roguelike"]));
        assert_eq!(written["modes"], serde_json::json!(["coop"]));
        let back: ViewFilter = serde_json::from_value(written).unwrap();
        assert_eq!(back, f);
        // Files without them read as before.
        let plain: ViewFilter = serde_json::from_str(r#"{"group": "mode"}"#).unwrap();
        assert_eq!(plain.group, Grouping::Mode);
        assert!(plain.unknown.is_empty() && plain.modes.is_empty());
    }

    #[test]
    fn filters_by_the_strictest_age_rating() {
        use irori_core::{AgeRating, RatingSystem};
        let rated = |app: &str, title: &str, ratings: &[(RatingSystem, &str)]| {
            Arc::new(
                GameInfo {
                    runner: Some(Runner::Steam),
                    app_name: app.into(),
                    title: title.into(),
                    age_ratings: ratings
                        .iter()
                        .map(|(s, r)| AgeRating::new(*s, r, Vec::new(), None).unwrap())
                        .collect(),
                    ..Default::default()
                }
                .finalize(),
            )
        };
        let library = snapshot(vec![
            rated(
                "1",
                "Cars",
                &[(RatingSystem::Pegi, "3"), (RatingSystem::Esrb, "e")],
            ),
            rated(
                "2",
                "Knights",
                &[(RatingSystem::Pegi, "7"), (RatingSystem::Esrb, "e10")],
            ),
            rated("3", "Shooter", &[(RatingSystem::Esrb, "m")]),
            rated("4", "Pending", &[(RatingSystem::Esrb, "rp")]),
            rated("5", "Unrated", &[]),
        ]);
        let user = UserData::default();
        let none = HashSet::new();
        let shown = |max_age: Option<u8>, include_unrated: bool| {
            let f = ViewFilter {
                max_age,
                include_unrated,
                ..Default::default()
            };
            titles(&apply(&f, &[], &library, &user, &none, &none))
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(None, false).len(), 5);
        // ESRB's E is for ages 6 and up, so PEGI 3 alone doesn't let it in.
        assert!(shown(Some(3), false).is_empty());
        assert_eq!(shown(Some(7), false), ["Cars"]);
        // E10+ asks for 10: the strictest board decides.
        assert_eq!(shown(Some(12), false), ["Cars", "Knights"]);
        assert_eq!(
            shown(Some(16), true),
            ["Cars", "Knights", "Pending", "Unrated"]
        );

        let f = ViewFilter {
            max_age: Some(12),
            ..Default::default()
        };
        assert_eq!(f.active_count(), 1);
        // Saved views and the persisted view keep the limit.
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<ViewFilter>(&json).unwrap(), f);
        let old: ViewFilter = serde_json::from_str(r#"{"favourites_only": true}"#).unwrap();
        assert_eq!((old.max_age, old.include_unrated), (None, false));
        let mut f = f;
        f.reset_filters();
        assert_eq!(f.max_age, None);
    }

    #[test]
    fn filters_by_play_status_counting_untouched_games_as_not_played() {
        use irori_services::PlayStatus;
        let library = sample();
        let mut user = UserData::default();
        user.play_status.insert(
            "legendary:b".into(),
            irori_services::PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2026-10-01T10:00:00Z".into(),
            },
        );
        user.playtime.insert(
            "gog:1".into(),
            irori_core::game::PlaytimeRecord {
                total_minutes: 90,
                ..Default::default()
            },
        );
        let shown = |statuses: &[EffectiveStatus]| {
            let f = ViewFilter {
                statuses: statuses.iter().copied().collect(),
                ..Default::default()
            };
            assert_eq!(f.active_count(), usize::from(!statuses.is_empty()));
            let games = apply(&f, &[], &library, &user, &HashSet::new(), &HashSet::new());
            titles(&games)
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&[EffectiveStatus::Beaten]), ["Hades"]);
        assert_eq!(shown(&[EffectiveStatus::Played]), ["The Witcher 3"]);
        assert_eq!(
            shown(&[EffectiveStatus::NotPlayed, EffectiveStatus::Beaten]),
            ["Alan Wake", "Hades", "Itch game"]
        );
        assert_eq!(shown(&[]).len(), 4);
    }

    #[test]
    fn query_is_not_persisted() {
        let f = ViewFilter {
            query: "secret".into(),
            install: InstallFilter::NotInstalled,
            categories: [
                CategoryFilter::Named("RPG".into()),
                CategoryFilter::Uncategorized,
            ]
            .into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("secret"));
        let back: ViewFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(back.install, InstallFilter::NotInstalled);
        assert_eq!(back.categories, f.categories);
        assert!(back.query.is_empty());
        // Unknown or partial files fall back to defaults field by field.
        let partial: ViewFilter = serde_json::from_str(r#"{"favourites_only":true}"#).unwrap();
        assert!(partial.favourites_only);
    }

    #[test]
    fn reads_single_category_files() {
        let read = |json: &str| serde_json::from_str::<ViewFilter>(json).unwrap().categories;
        assert!(read(r#"{"category":"all"}"#).is_empty());
        assert_eq!(
            read(r#"{"category":"uncategorized"}"#),
            [CategoryFilter::Uncategorized].into()
        );
        assert_eq!(
            read(r#"{"category":{"named":"RPG"}}"#),
            [CategoryFilter::Named("RPG".into())].into()
        );
        assert_eq!(
            read(r#"{"categories":["uncategorized",{"named":"RPG"}]}"#).len(),
            2
        );
    }

    #[test]
    fn category_selection_follows_renames_and_deletes() {
        crate::i18n::register();
        let mut f = ViewFilter::default();
        f.toggle_category(CategoryFilter::Named("RPG".into()));
        f.toggle_category(CategoryFilter::Uncategorized);
        assert_eq!(f.category_heading(), "Uncategorized, RPG");
        f.rename_category("RPG", "Role-playing");
        assert!(
            f.categories
                .contains(&CategoryFilter::Named("Role-playing".into()))
        );
        f.toggle_category(CategoryFilter::Uncategorized);
        f.forget_category("Role-playing");
        assert!(f.categories.is_empty());
        for n in ["a", "b", "c", "d"] {
            f.toggle_category(CategoryFilter::Named(n.into()));
        }
        assert_eq!(f.category_heading(), "4 categories");
    }

    #[test]
    fn unavailable_store_falls_back_to_all() {
        let f = ViewFilter {
            store: StoreFilter::Store(Runner::Nile),
            ..Default::default()
        };
        assert_eq!(f.to_library_filter(&[Runner::Gog]).store, StoreFilter::All);
        assert_eq!(
            f.to_library_filter(&[Runner::Nile]).store,
            StoreFilter::Store(Runner::Nile)
        );
    }

    #[test]
    fn uncategorized_and_named_categories() {
        // `sample()` is in snapshot (display) order; filters keep it.
        let lib = sample();
        let mut user = UserData::default();
        user.categories
            .entry("RPG".into())
            .or_default()
            .insert("gog:1".into());
        user.categories
            .entry("Roguelike".into())
            .or_default()
            .insert("legendary:b".into());
        let none = HashSet::new();
        let stores = Runner::ALL.to_vec();
        let with = |c: Vec<CategoryFilter>| ViewFilter {
            categories: c.into_iter().collect(),
            ..Default::default()
        };

        let f = with(vec![CategoryFilter::Uncategorized]);
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &none)),
            vec!["Alan Wake", "Itch game"]
        );
        let f = with(vec![CategoryFilter::Named("RPG".into())]);
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &none)),
            vec!["The Witcher 3"]
        );
        // Several categories show the union.
        let f = with(vec![
            CategoryFilter::Named("RPG".into()),
            CategoryFilter::Named("Roguelike".into()),
        ]);
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &none)),
            vec!["The Witcher 3", "Hades"]
        );
        let f = with(vec![
            CategoryFilter::Named("RPG".into()),
            CategoryFilter::Uncategorized,
        ]);
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &none)),
            vec!["The Witcher 3", "Alan Wake", "Itch game"]
        );
    }

    #[test]
    fn shows_one_accounts_games_while_its_store_is_shown() {
        let mut lib = sample();
        let id = |app: &str| GameId::new(Runner::Legendary, app);
        lib.owners = [
            (id("a"), vec!["alice".to_string(), "bob".to_string()]),
            (id("b"), vec!["alice".to_string()]),
        ]
        .into();
        let user = UserData::default();
        let none = HashSet::new();
        let stores = Runner::ALL.to_vec();
        let mut f = ViewFilter {
            store: StoreFilter::Store(Runner::Legendary),
            account: Some(AccountFilter {
                runner: Runner::Legendary,
                account_id: "bob".into(),
            }),
            ..Default::default()
        };
        assert!(f.is_narrowed());
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &none)),
            vec!["Alan Wake"]
        );
        // Another store shown: the account does not apply.
        f.store = StoreFilter::All;
        assert_eq!(apply(&f, &stores, &lib, &user, &none, &none).len(), 4);
        // The only store offered counts as shown.
        assert_eq!(
            titles(&apply(&f, &[Runner::Legendary], &lib, &user, &none, &none)),
            vec!["Alan Wake"]
        );
        // The store no longer has several accounts: nothing to narrow by.
        lib.owners.clear();
        f.store = StoreFilter::Store(Runner::Legendary);
        assert_eq!(apply(&f, &stores, &lib, &user, &none, &none).len(), 2);
        f.clear_all();
        assert_eq!(f.account, None);
    }

    #[test]
    fn hides_games_with_missing_files() {
        let lib = sample();
        let user = UserData::default();
        let none = HashSet::new();
        let stores = Runner::ALL.to_vec();
        let missing: HashSet<GameId> = [GameId::new(Runner::Legendary, "b")].into();
        let mut f = ViewFilter::default();
        assert_eq!(apply(&f, &stores, &lib, &user, &none, &missing).len(), 4);
        f.hide_unavailable = true;
        assert_eq!(
            titles(&apply(&f, &stores, &lib, &user, &none, &missing)),
            vec!["The Witcher 3", "Alan Wake", "Itch game"]
        );
    }

    #[test]
    fn offered_stores_include_logged_in_and_non_empty() {
        let lib = sample();
        let shown = ShownStores::default();
        let stores = offered_stores(&lib, &|r| r == Runner::Zoom, &shown);
        assert_eq!(
            stores,
            vec![
                Runner::Legendary,
                Runner::Gog,
                Runner::Zoom,
                Runner::Sideload
            ]
        );
        let shown = ShownStores {
            epic: false,
            ..shown
        };
        let stores = offered_stores(&lib, &|r| r != Runner::Gog, &shown);
        assert!(!stores.contains(&Runner::Legendary));
        assert!(stores.contains(&Runner::Nile));
        assert!(stores.contains(&Runner::Gog));
    }

    #[test]
    fn cache_recomputes_only_when_inputs_change() {
        let lib = sample();
        let user = Arc::new(UserData::default());
        let updates = Arc::new(HashSet::new());
        let missing = Arc::new(HashSet::new());
        let stores = Runner::ALL.to_vec();
        let mut cache = FilteredGames::default();
        let mut f = ViewFilter::default();

        let a = cache.get(&f, &stores, &lib, &user, &updates, &missing);
        let b = cache.get(&f, &stores, &lib, &user, &updates, &missing);
        assert_eq!(cache.recomputes(), 1);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(
            cache.position(&GameId::new(Runner::Legendary, "b")),
            Some(2)
        );

        // A new user-data snapshot (even with equal content) is a new input.
        let user2 = Arc::new(UserData::default());
        cache.get(&f, &stores, &lib, &user2, &updates, &missing);
        assert_eq!(cache.recomputes(), 2);

        f.install = InstallFilter::Installed;
        let c = cache.get(&f, &stores, &lib, &user2, &updates, &missing);
        assert_eq!(cache.recomputes(), 3);

        // New availability results are a new input.
        let missing2 = Arc::new(HashSet::new());
        cache.get(&f, &stores, &lib, &user2, &updates, &missing2);
        assert_eq!(cache.recomputes(), 4);
        assert_eq!(c.len(), 3);
        assert_eq!(cache.position(&GameId::new(Runner::Legendary, "a")), None);
    }

    #[test]
    fn groups_by_play_status_keeping_the_order_within_each() {
        use irori_services::{PlayStatus, PlayStatusRecord};
        let lib = snapshot(vec![
            game(Runner::Gog, "1", "Alan Wake", true),
            game(Runner::Gog, "2", "Celeste", true),
            game(Runner::Gog, "3", "Hades", true),
            game(Runner::Gog, "4", "Inside", false),
            game(Runner::Gog, "5", "Outer Wilds", true),
            game(Runner::Gog, "6", "Tunic", true),
        ]);
        let mut user = UserData::default();
        let mut set = |app: &str, status| {
            user.play_status.insert(
                format!("gog:{app}"),
                PlayStatusRecord {
                    status,
                    since: "2026-10-01T10:00:00Z".into(),
                },
            );
        };
        set("6", PlayStatus::Playing);
        set("2", PlayStatus::Playing);
        set("3", PlayStatus::Beaten);
        set("5", PlayStatus::PlanToPlay);
        user.playtime.insert(
            "gog:4".into(),
            irori_core::game::PlaytimeRecord {
                total_minutes: 30,
                ..Default::default()
            },
        );
        let user = Arc::new(user);
        let none = Arc::new(HashSet::new());
        let mut cache = FilteredGames::default();
        let mut f = ViewFilter {
            group: Grouping::Status,
            ..Default::default()
        };

        let games = cache.get(&f, &[], &lib, &user, &none, &none);
        // Playing, then Plan to play, Played and Not played, finished last;
        // titles stay A–Z inside each section and empty sections are left out.
        assert_eq!(
            titles(&games),
            [
                "Celeste",
                "Tunic",
                "Outer Wilds",
                "Inside",
                "Alan Wake",
                "Hades"
            ]
        );
        let sections: Vec<_> = cache
            .groups()
            .iter()
            .map(|g| (g.kind, g.range.clone()))
            .collect();
        let status = GroupKind::Status;
        assert_eq!(
            sections,
            [
                (status(EffectiveStatus::Playing), 0..2),
                (status(EffectiveStatus::PlanToPlay), 2..3),
                (status(EffectiveStatus::Played), 3..4),
                (status(EffectiveStatus::NotPlayed), 4..5),
                (status(EffectiveStatus::Beaten), 5..6),
            ]
        );
        // Focus follows the grouped order.
        assert_eq!(cache.position(&GameId::new(Runner::Gog, "6")), Some(1));
        assert_eq!(cache.position(&GameId::new(Runner::Gog, "3")), Some(5));

        // Grouping is derived with the list: drawing again changes nothing.
        cache.get(&f, &[], &lib, &user, &none, &none);
        assert_eq!(cache.recomputes(), 1);

        // Reversing reverses inside each section, not the sections.
        f.descending = true;
        let games = cache.get(&f, &[], &lib, &user, &none, &none);
        assert_eq!(
            titles(&games),
            [
                "Tunic",
                "Celeste",
                "Outer Wilds",
                "Inside",
                "Alan Wake",
                "Hades"
            ]
        );

        // Filters narrow the sections; resetting them keeps the grouping.
        f.install = InstallFilter::Installed;
        f.reset_filters();
        assert_eq!(f.group, Grouping::Status);
        assert_eq!(f.active_count(), 0);
        f.statuses = [EffectiveStatus::Beaten].into();
        cache.get(&f, &[], &lib, &user, &none, &none);
        assert_eq!(cache.groups().len(), 1);

        // Ungrouped lists have no sections.
        f.group = Grouping::None;
        f.statuses.clear();
        cache.get(&f, &[], &lib, &user, &none, &none);
        assert!(cache.groups().is_empty());
    }

    #[test]
    fn release_windows_count_calendar_years() {
        let (any, this, five, older) = (
            ReleaseWindow::Any,
            ReleaseWindow::ThisYear,
            ReleaseWindow::LastFiveYears,
            ReleaseWindow::Older,
        );
        assert!(any.contains(None, 2026));
        assert!(!this.contains(None, 2026));
        assert!(this.contains(Some("2026-03-01"), 2026));
        assert!(!this.contains(Some("2025-12-31"), 2026));
        assert!(five.contains(Some("2022-01-01T00:00:00+0000"), 2026));
        assert!(!five.contains(Some("2021-12-31"), 2026));
        assert!(older.contains(Some("2021-12-31"), 2026));
        assert!(!older.contains(Some("2022-01-01"), 2026));
        assert!(!older.contains(Some("garbage"), 2026));
    }
}
