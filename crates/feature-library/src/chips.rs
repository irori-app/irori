//! The library's tab bar helpers: built-in presets (`All`, `Installed`,
//! `Favorites`, `Recently added`) and dynamic collections (smart categories).
//!
//! Active tab matching ignores the search query, sort order, sort direction
//! and grouping (since those are remembered independently per tab in
//! [`crate::views::TabViewPrefs`]). Tab counts use the exact same filtering
//! pipeline as the cover grid and are cached until the library, user data,
//! stores, availability or collections change; `Recently added` shows no count.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use irori_core::{GameId, Runner};
use irori_services::{CategoryIndex, LibrarySnapshot, LibrarySort, UserData};
use irori_ui::i18n::t;

use crate::filter::{self, Grouping, InstallFilter, ViewFilter};
use crate::views::TabKey;

/// The built-in preset tabs in the library's top strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresetChip {
    All,
    Installed,
    Favourites,
    RecentlyAdded,
}

impl PresetChip {
    pub fn id(self) -> &'static str {
        match self {
            PresetChip::All => "chip-all",
            PresetChip::Installed => "chip-installed",
            PresetChip::Favourites => "chip-favourites",
            PresetChip::RecentlyAdded => "chip-recently-added",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            PresetChip::All => "library.chips.all",
            PresetChip::Installed => "library.chips.installed",
            PresetChip::Favourites => "library.chips.favourites",
            PresetChip::RecentlyAdded => "library.views.builtin.recently_added",
        })
    }

    pub fn icon(self) -> IconName {
        match self {
            PresetChip::All => IconName::LayoutGrid,
            PresetChip::Installed => IconName::HardDrive,
            PresetChip::Favourites => IconName::Star,
            PresetChip::RecentlyAdded => IconName::Clock,
        }
    }

    /// Default criteria when this preset tab is opened for the first time
    /// (before the user customizes its per-tab sort or grouping).
    pub fn filter(self) -> ViewFilter {
        match self {
            PresetChip::All => ViewFilter::default(),
            PresetChip::Installed => ViewFilter {
                install: InstallFilter::Installed,
                ..Default::default()
            },
            PresetChip::Favourites => ViewFilter {
                favourites_only: true,
                ..Default::default()
            },
            PresetChip::RecentlyAdded => ViewFilter {
                sort: LibrarySort::DateAdded,
                ..Default::default()
            },
        }
    }
}

/// Criteria applied when a dynamic collection tab is clicked (before applying
/// any per-tab saved sort or grouping).
pub fn collection_filter(id: &str) -> ViewFilter {
    let mut filter = ViewFilter::default();
    filter.smart_categories.insert(id.to_owned());
    filter
}

/// Strip search text, sort, direction and grouping so active-tab matching
/// compares only the tab's narrowing criteria.
fn narrowing_only(filter: &ViewFilter) -> ViewFilter {
    ViewFilter {
        query: String::new(),
        sort: LibrarySort::default(),
        descending: false,
        group: Grouping::None,
        ..filter.clone()
    }
}

/// Return the preset or dynamic collection tab whose narrowing criteria match
/// `filter` (ignoring search text, sort, direction and grouping), or `None`
/// when additional filters from the Filters popover are active.
///
/// Because `All` and `Recently added` both show the un-narrowed library,
/// `current` disambiguates them once the user customizes sort or grouping on
/// either tab.
pub fn active_tab_for(
    filter: &ViewFilter,
    current: Option<&TabKey>,
    smart_ids: &[String],
) -> Option<TabKey> {
    let base = narrowing_only(filter);
    if base == narrowing_only(&PresetChip::Installed.filter()) {
        return Some(TabKey::Installed);
    }
    if base == narrowing_only(&PresetChip::Favourites.filter()) {
        return Some(TabKey::Favorites);
    }
    if let Some(id) = smart_ids
        .iter()
        .find(|id| base == narrowing_only(&collection_filter(id)))
    {
        return Some(TabKey::Collection(id.clone()));
    }
    if base == ViewFilter::default() {
        if current == Some(&TabKey::RecentlyAdded) {
            return Some(TabKey::RecentlyAdded);
        }
        if current.is_none()
            && filter.sort == LibrarySort::DateAdded
            && !filter.descending
            && filter.group == Grouping::None
        {
            return Some(TabKey::RecentlyAdded);
        }
        return Some(TabKey::All);
    }
    None
}

/// Counts for every counted preset tab and dynamic collection.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChipCountMap {
    pub presets: [usize; 3],
    pub collections: HashMap<String, usize>,
}

impl ChipCountMap {
    pub fn preset(&self, preset: PresetChip) -> Option<usize> {
        match preset {
            PresetChip::All => Some(self.presets[0]),
            PresetChip::Installed => Some(self.presets[1]),
            PresetChip::Favourites => Some(self.presets[2]),
            PresetChip::RecentlyAdded => None,
        }
    }

    pub fn collection(&self, id: &str) -> usize {
        self.collections.get(id).copied().unwrap_or(0)
    }
}

/// Library state needed to compute and cache tab counts.
pub struct ChipCountInputs<'a> {
    pub smart_ids: &'a [String],
    pub stores: &'a [Runner],
    pub library: &'a LibrarySnapshot,
    pub user: &'a Arc<UserData>,
    pub categories: &'a Arc<CategoryIndex>,
    pub updates: &'a HashSet<GameId>,
    pub missing: &'a HashSet<GameId>,
}

/// Compute counts for all counted presets and dynamic collections using the
/// same filter criteria as the grid.
pub fn chip_counts(inputs: &ChipCountInputs<'_>) -> ChipCountMap {
    let count = |f: &ViewFilter| {
        filter::apply_in(
            f,
            inputs.stores,
            inputs.library,
            inputs.user,
            inputs.categories,
            inputs.updates,
            inputs.missing,
        )
        .len()
    };
    let presets = [
        count(&PresetChip::All.filter()),
        count(&PresetChip::Installed.filter()),
        count(&PresetChip::Favourites.filter()),
    ];
    let mut collection_counts = HashMap::new();
    for id in inputs.smart_ids {
        collection_counts.insert(id.clone(), count(&collection_filter(id)));
    }
    ChipCountMap {
        presets,
        collections: collection_counts,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    user: usize,
    categories: usize,
    stores: Vec<Runner>,
    updates: HashSet<GameId>,
    missing: HashSet<GameId>,
    smart_ids: Vec<String>,
}

/// [`chip_counts`] cached by library revision, user data, categories, stores,
/// availability, updates and collections.
#[derive(Default)]
pub struct ChipCountCache {
    key: Option<CacheKey>,
    counts: ChipCountMap,
    computes: usize,
}

impl ChipCountCache {
    pub fn get(&mut self, inputs: &ChipCountInputs<'_>) -> &ChipCountMap {
        let key = CacheKey {
            revision: inputs.library.revision,
            user: Arc::as_ptr(inputs.user) as usize,
            categories: Arc::as_ptr(inputs.categories) as usize,
            stores: inputs.stores.to_vec(),
            updates: inputs.updates.clone(),
            missing: inputs.missing.clone(),
            smart_ids: inputs.smart_ids.to_vec(),
        };
        if self.key.as_ref() != Some(&key) {
            self.counts = chip_counts(inputs);
            self.key = Some(key);
            self.computes += 1;
        }
        &self.counts
    }

    #[cfg(test)]
    pub fn computes(&self) -> usize {
        self.computes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::tests::game;
    use irori_services::StoreFilter;

    #[test]
    fn active_tab_matches_presets_and_collections_across_sort_and_group() {
        let smart = vec!["s-souls".to_string()];

        assert_eq!(
            active_tab_for(&ViewFilter::default(), None, &smart),
            Some(TabKey::All)
        );
        // Customizing sort/group/query inside All keeps All active.
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    query: "hades".into(),
                    sort: LibrarySort::Score,
                    descending: true,
                    group: Grouping::Status,
                    ..Default::default()
                },
                Some(&TabKey::All),
                &smart
            ),
            Some(TabKey::All)
        );
        // Customizing sort/group inside RecentlyAdded keeps RecentlyAdded active.
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    sort: LibrarySort::Playtime,
                    group: Grouping::Genre,
                    ..Default::default()
                },
                Some(&TabKey::RecentlyAdded),
                &smart
            ),
            Some(TabKey::RecentlyAdded)
        );
        // Cold start on default DateAdded resolves to RecentlyAdded.
        assert_eq!(
            active_tab_for(&PresetChip::RecentlyAdded.filter(), None, &smart),
            Some(TabKey::RecentlyAdded)
        );
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    query: "hades".into(),
                    install: InstallFilter::Installed,
                    sort: LibrarySort::LastPlayed,
                    ..Default::default()
                },
                Some(&TabKey::Installed),
                &smart
            ),
            Some(TabKey::Installed)
        );
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    favourites_only: true,
                    group: Grouping::Store,
                    ..Default::default()
                },
                None,
                &smart
            ),
            Some(TabKey::Favorites)
        );
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    sort: LibrarySort::Score,
                    descending: true,
                    ..collection_filter("s-souls")
                },
                Some(&TabKey::Collection("s-souls".into())),
                &smart
            ),
            Some(TabKey::Collection("s-souls".into()))
        );
        // Extra narrowing filters from the Filters popover leave no tab active.
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    store: StoreFilter::Store(Runner::Gog),
                    ..Default::default()
                },
                Some(&TabKey::All),
                &smart
            ),
            None
        );
        assert_eq!(
            active_tab_for(
                &ViewFilter {
                    install: InstallFilter::Installed,
                    favourites_only: true,
                    ..Default::default()
                },
                Some(&TabKey::Installed),
                &smart
            ),
            None
        );
    }

    #[test]
    fn chip_counts_match_grid_and_cache_by_revision() {
        let games = vec![
            game(Runner::Gog, "1", "Witcher", true),
            game(Runner::Legendary, "a", "Hades", true),
            game(Runner::Legendary, "b", "Celeste", false),
            game(Runner::Steam, "s", "Hidden Game", true),
        ];
        let snapshot = crate::filter::tests::snapshot(games);
        let mut user = UserData::default();
        user.favourites.insert("legendary:a".into());
        user.hidden.insert("steam:s".into());
        let user = Arc::new(user);
        let categories = Arc::new(CategoryIndex::default());
        let stores = vec![Runner::Gog, Runner::Legendary, Runner::Steam];
        let updates = HashSet::new();
        let missing = HashSet::new();

        let mut cache = ChipCountCache::default();
        let smart_ids = Vec::new();
        let inputs = ChipCountInputs {
            smart_ids: &smart_ids,
            stores: &stores,
            library: &snapshot,
            user: &user,
            categories: &categories,
            updates: &updates,
            missing: &missing,
        };
        let counts = cache.get(&inputs);
        assert_eq!(counts.preset(PresetChip::All), Some(3));
        assert_eq!(counts.preset(PresetChip::Installed), Some(2));
        assert_eq!(counts.preset(PresetChip::Favourites), Some(1));
        assert_eq!(counts.preset(PresetChip::RecentlyAdded), None);
        assert_eq!(cache.computes(), 1);

        // Same inputs hit cache.
        let _ = cache.get(&inputs);
        assert_eq!(cache.computes(), 1);

        // New library revision invalidates cache.
        let mut next =
            crate::filter::tests::snapshot(vec![game(Runner::Gog, "1", "Witcher", true)]);
        next.revision = 2;
        let next_inputs = ChipCountInputs {
            library: &next,
            ..inputs
        };
        let counts = cache.get(&next_inputs);
        assert_eq!(counts.preset(PresetChip::All), Some(1));
        assert_eq!(cache.computes(), 2);
    }
}
