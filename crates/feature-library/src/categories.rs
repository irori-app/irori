//! Categories in the library: sections per category when grouping by it,
//! and turning a saved view into a smart category. Both kinds are listed
//! together everywhere (`irori_ui::categories`); only the manual ones take
//! games by hand, the smart ones follow their rules.

use std::collections::BTreeSet;
use std::sync::Arc;

use irori_core::GameInfo;
use irori_services::{CategoryIndex, Match, Rule, SmartCategory, StoreFilter, UserData};

use crate::filter::{CategoryFilter, GroupKind, InstallFilter, StatusGroup, ViewFilter, sections};

/// Reorder `games` into category sections (stable, so the chosen order
/// holds within each): categories pinned first, then by name, the kinds
/// mixed, games in no category last. A game in several categories sits in
/// the first one's section, so the list keeps each game once.
pub fn group_by_category(
    games: &mut [Arc<GameInfo>],
    user: &UserData,
    index: &CategoryIndex,
) -> Vec<StatusGroup> {
    let order: Vec<(irori_services::CategoryRef, Arc<str>)> = user
        .categories_in_order()
        .into_iter()
        .map(|(r, name)| (r, Arc::from(name)))
        .collect();
    let first = |g: &GameInfo| {
        let id = g.id();
        order
            .iter()
            .position(|(r, _)| irori_ui::categories::contains(r, &id, user, index))
    };
    games.sort_by_cached_key(|g| first(g).unwrap_or(usize::MAX));
    sections(games, |g| {
        GroupKind::Category(first(g).map(|ix| order[ix].clone()))
    })
}

/// The smart category a saved view's criteria make, named `name`, and the
/// criteria no rule can hold, left in a filter of their own (so they can be
/// described to the user). Sorting and grouping aren't criteria and stay
/// with the view.
pub fn smart_from_view(name: &str, view: &ViewFilter) -> (SmartCategory, ViewFilter) {
    let mut rules = Vec::new();
    let mut rest = ViewFilter {
        platform: view.platform,
        hidden: view.hidden,
        offline_only: view.offline_only,
        hide_third_party: view.hide_third_party,
        updates_only: view.updates_only,
        hide_unavailable: view.hide_unavailable,
        released: view.released,
        smart_categories: view.smart_categories.clone(),
        ..Default::default()
    };
    if let StoreFilter::Store(runner) = view.store {
        rules.push(Rule::Store {
            stores: [runner].into(),
        });
    }
    match view.install {
        InstallFilter::Any => {}
        InstallFilter::Installed => rules.push(Rule::Installed { installed: true }),
        InstallFilter::NotInstalled => rules.push(Rule::Installed { installed: false }),
    }
    if view.favourites_only {
        rules.push(Rule::Favourite {});
    }
    // "Uncategorized" has no rule; the categories picked with it are its
    // alternatives, so none of the choice can come along without it.
    if view.categories.contains(&CategoryFilter::Uncategorized) {
        rest.categories = view.categories.clone();
    } else if !view.categories.is_empty() {
        let names: BTreeSet<String> = view
            .categories
            .iter()
            .filter_map(|c| match c {
                CategoryFilter::Named(n) => Some(n.clone()),
                CategoryFilter::Uncategorized => None,
            })
            .collect();
        rules.push(Rule::Category { names });
    }
    if !view.systems.is_empty() {
        rules.push(Rule::System {
            systems: view.systems.clone(),
        });
    }
    if !view.statuses.is_empty() {
        rules.push(Rule::Status {
            statuses: view.statuses.clone(),
        });
    }
    if let Some(max_age) = view.max_age {
        rules.push(Rule::AgeRating {
            max_age,
            include_unrated: view.include_unrated,
        });
    }
    if !view.modes.is_empty() {
        rules.push(Rule::Mode {
            modes: view.modes.clone(),
        });
    }
    if !view.genres.is_empty() {
        rules.push(Rule::Genre {
            genres: view.genres.iter().cloned().collect(),
        });
    }
    if !view.tags.is_empty() {
        rules.push(Rule::Tags {
            tags: view.tags.iter().cloned().collect(),
        });
    }
    (
        SmartCategory::new(name, Match::All, rules, Vec::new()),
        rest,
    )
}

/// The library's entries in a category, for tests and counts that need
/// games rather than numbers.
#[cfg(test)]
pub fn entries_in(
    category: &irori_services::CategoryRef,
    library: &irori_services::LibrarySnapshot,
    user: &UserData,
    index: &CategoryIndex,
) -> Vec<String> {
    irori_ui::categories::games_in(category, library, user, index)
        .iter()
        .map(|g| g.title.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use irori_core::{GameMode, Runner};
    use irori_services::{
        CategoryLook, CategoryRef, EffectiveStatus, LibrarySnapshot, smart_categories::IndexKey,
    };

    use super::*;
    use crate::filter::{FilteredGames, Grouping, HiddenMode, ReleaseWindow, tests::game};

    fn library() -> LibrarySnapshot {
        let games = vec![
            game(Runner::Gog, "1", "Witcher 3", true),
            game(Runner::Legendary, "a", "Alan Wake", false),
            game(Runner::Legendary, "b", "Hades", true),
            game(Runner::Steam, "c", "Celeste", false),
        ];
        LibrarySnapshot::new(
            1,
            irori_services::copies::Grouped {
                entries: games,
                ..Default::default()
            },
            HashMap::new(),
        )
    }

    fn index(library: &LibrarySnapshot, user: &UserData) -> Arc<CategoryIndex> {
        Arc::new(CategoryIndex::build(
            IndexKey::default(),
            library,
            user,
            &HashMap::new(),
            &chrono::Local,
        ))
    }

    fn user() -> UserData {
        let mut user = UserData::default();
        user.categories
            .insert("Story".into(), ["legendary:a".to_string()].into());
        user.smart_categories.insert(
            "s-inst".into(),
            SmartCategory::new(
                "Installed",
                Match::All,
                vec![Rule::Installed { installed: true }],
                vec![],
            ),
        );
        user
    }

    fn titles(games: &[Arc<GameInfo>]) -> Vec<&str> {
        games.iter().map(|g| g.title.as_str()).collect()
    }

    #[test]
    fn smart_and_manual_categories_filter_together() {
        let lib = library();
        let user = Arc::new(user());
        let index = index(&lib, &user);
        let none = Arc::new(HashSet::new());
        let mut cache = FilteredGames::default();
        let mut f = ViewFilter::default();
        f.toggle(&CategoryRef::Smart("s-inst".into()));
        let shown = cache.get_in(&f, &[], &lib, &user, &index, &none, &none);
        // In the snapshot's order, which sorting by title keeps.
        assert_eq!(titles(&shown), ["Witcher 3", "Hades"]);
        f.toggle(&CategoryRef::Manual("Story".into()));
        let shown = cache.get_in(&f, &[], &lib, &user, &index, &none, &none);
        assert_eq!(titles(&shown), ["Witcher 3", "Alan Wake", "Hades"]);
        assert_eq!(f.active_count(), 1);
        crate::i18n::register();
        assert_eq!(f.category_heading(&user), "Story, Installed");
        // The same inputs never filter again; a new index does.
        cache.get_in(&f, &[], &lib, &user, &index, &none, &none);
        assert_eq!(cache.recomputes(), 2);
        let again = Arc::new((*index).clone());
        cache.get_in(&f, &[], &lib, &user, &again, &none, &none);
        assert_eq!(cache.recomputes(), 3);
    }

    #[test]
    fn a_deleted_smart_category_picks_nothing_and_is_forgotten() {
        let lib = library();
        let mut user = user();
        let mut f = ViewFilter::default();
        f.toggle(&CategoryRef::Smart("s-gone".into()));
        user.smart_categories.remove("s-gone");
        let index = index(&lib, &user);
        let shown = crate::filter::apply_in(
            &f,
            &[],
            &lib,
            &user,
            &index,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(shown.len(), 4);
        f.forget_missing_smart(&user);
        assert!(f.smart_categories.is_empty());
    }

    #[test]
    fn clearing_smart_categories_leaves_no_key_behind() {
        let mut f = ViewFilter::default();
        f.toggle(&CategoryRef::Smart("s-1".into()));
        let saved = serde_json::to_value(&f).unwrap();
        assert_eq!(saved["smart_categories"][0], "s-1");
        // A file with the id and a newer Irori's key, read back and cleared.
        let mut read: ViewFilter =
            serde_json::from_str(r#"{"smart_categories": ["s-1"], "watchParty": true}"#).unwrap();
        assert_eq!(read.smart_categories, ["s-1".to_string()].into());
        read.clear_categories();
        let saved = serde_json::to_value(&read).unwrap();
        assert!(saved.get("smart_categories").is_none());
        assert_eq!(saved["watchParty"], true);
        let back: ViewFilter = serde_json::from_value(saved).unwrap();
        assert!(back.smart_categories.is_empty());
    }

    #[test]
    fn grouping_by_category_puts_pinned_categories_first_and_the_rest_last() {
        let lib = library();
        let mut user = user();
        let mut smart = user.smart_categories["s-inst"].clone();
        smart.look = CategoryLook {
            pinned: true,
            ..Default::default()
        };
        user.smart_categories.insert("s-inst".into(), smart);
        user.categories
            .get_mut("Story")
            .unwrap()
            .insert("legendary:b".into());
        let index = index(&lib, &user);
        let mut games: Vec<Arc<GameInfo>> = lib.games.clone();
        let groups = group_by_category(&mut games, &user, &index);
        let names: Vec<Option<String>> = groups
            .iter()
            .map(|g| match &g.kind {
                GroupKind::Category(c) => c.as_ref().map(|(_, n)| n.to_string()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            names,
            [Some("Installed".into()), Some("Story".into()), None]
        );
        // Hades is in both and sits once, in the pinned one.
        assert_eq!(
            titles(&games),
            ["Witcher 3", "Hades", "Alan Wake", "Celeste"]
        );
        assert_eq!(groups[0].range, 0..2);
        let shown = FilteredGames::default().get_in(
            &ViewFilter {
                group: Grouping::Category,
                ..Default::default()
            },
            &[],
            &lib,
            &Arc::new(user.clone()),
            &index,
            &Arc::new(HashSet::new()),
            &Arc::new(HashSet::new()),
        );
        assert_eq!(shown.len(), 4);
    }

    #[test]
    fn a_saved_view_becomes_rules_and_says_what_it_cant_carry() {
        let view = ViewFilter {
            store: StoreFilter::Store(Runner::Gog),
            install: InstallFilter::Installed,
            favourites_only: true,
            categories: [CategoryFilter::Named("RPG".into())].into(),
            statuses: [EffectiveStatus::Playing].into(),
            modes: [GameMode::Coop].into(),
            max_age: Some(12),
            released: ReleaseWindow::ThisYear,
            hidden: HiddenMode::Show,
            ..Default::default()
        };
        let (smart, rest) = smart_from_view(" Co-op now ", &view);
        assert_eq!(smart.name, "Co-op now");
        assert_eq!(smart.matching, Match::All);
        let kinds: Vec<&str> = smart
            .rules
            .iter()
            .map(|c| c.rule.as_ref().unwrap().kind())
            .collect();
        assert_eq!(
            kinds,
            [
                "store",
                "installed",
                "favourite",
                "category",
                "status",
                "ageRating",
                "mode"
            ]
        );
        assert_eq!(rest.released, ReleaseWindow::ThisYear);
        assert_eq!(rest.hidden, HiddenMode::Show);
        assert!(rest.categories.is_empty());
        crate::i18n::register();
        assert_eq!(rest.describe(&UserData::default()).len(), 2);

        // "Uncategorized" stays behind with the categories picked with it.
        let view = ViewFilter {
            categories: [
                CategoryFilter::Uncategorized,
                CategoryFilter::Named("RPG".into()),
            ]
            .into(),
            ..Default::default()
        };
        let (smart, rest) = smart_from_view("Loose", &view);
        assert!(smart.rules.is_empty());
        assert_eq!(rest.categories.len(), 2);
    }

    #[test]
    fn hidden_games_stay_out_of_shelves_and_counts() {
        let lib = library();
        let mut user = user();
        user.hidden.insert("gog:1".into());
        let index = index(&lib, &user);
        let smart = CategoryRef::Smart("s-inst".into());
        assert_eq!(entries_in(&smart, &lib, &user, &index), ["Hades"]);
        assert_eq!(index.count(&smart), 1);
    }
}
