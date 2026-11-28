//! Finding a filter value by name in the Filters popover: every system,
//! play status, genre and store tag the library has, each
//! with how many shown games carry it. Typing in the popover's search field
//! lists the values whose names hold the text; picking one ticks it as its
//! own section would.
//!
//! The values and counts are worked out once per library revision and user
//! data ([`ValueCache`]), never per keystroke; a search
//! only folds and compares names.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use irori_services::search::tag_key;
use irori_services::{EffectiveStatus, LibrarySnapshot, UserData};

use crate::filter::{ViewFilter, genre_key};

/// Most values a search lists; more ask for a longer text.
pub const MAX_SHOWN: usize = 40;

/// Which section of the popover a value belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    System,
    Status,
    Genre,
    Tag,
}

/// One value a filter can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A systems catalog id.
    System(String),
    Status(EffectiveStatus),
    /// A genre by its [`genre_key`], with the name the library shows.
    Genre {
        key: String,
        name: String,
    },
    /// A store tag by its [`tag_key`], with the name the library shows.
    Tag {
        key: String,
        name: String,
    },
}

impl Value {
    pub fn section(&self) -> Section {
        match self {
            Value::System(_) => Section::System,
            Value::Status(_) => Section::Status,
            Value::Genre { .. } => Section::Genre,
            Value::Tag { .. } => Section::Tag,
        }
    }

    /// Whether `filter` has this value picked.
    pub fn is_picked(&self, filter: &ViewFilter) -> bool {
        match self {
            Value::System(s) => filter.systems.contains(s),
            Value::Status(s) => filter.statuses.contains(s),
            Value::Genre { key, .. } => filter.genres.iter().any(|g| genre_key(g) == *key),
            Value::Tag { key, .. } => filter.tags.iter().any(|t| tag_key(t) == *key),
        }
    }

    /// Pick the value in `filter`, or take it out when it is picked.
    pub fn toggle(&self, filter: &mut ViewFilter) {
        match self {
            Value::System(s) => {
                if !filter.systems.remove(s) {
                    filter.systems.insert(s.clone());
                }
            }
            Value::Status(s) => {
                if !filter.statuses.remove(s) {
                    filter.statuses.insert(*s);
                }
            }
            Value::Genre { key, name } => {
                let before = filter.genres.len();
                filter.genres.retain(|g| genre_key(g) != *key);
                if filter.genres.len() == before {
                    filter.genres.insert(name.clone());
                }
            }
            Value::Tag { key, name } => {
                let before = filter.tags.len();
                filter.tags.retain(|t| tag_key(t) != *key);
                if filter.tags.len() == before {
                    filter.tags.insert(name.clone());
                }
            }
        }
    }
}

/// A value with its name and how many shown games carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: Value,
    pub label: String,
    pub count: usize,
    /// The label folded for comparing ([`fold`]).
    folded: String,
}

impl Candidate {
    pub fn new(value: Value, label: impl Into<String>, count: usize) -> Self {
        let label = label.into();
        Self {
            folded: fold(&label),
            value,
            label,
            count,
        }
    }
}

/// How names and searches compare: letters and digits only, lower case.
pub fn fold(text: &str) -> String {
    tag_key(text)
}

/// How each value of the library is named in the interface.
pub struct Labels<'a> {
    pub system: &'a dyn Fn(&str) -> String,
    pub status: &'a dyn Fn(EffectiveStatus) -> String,
}

/// Every value the library's shown games (hidden ones aside) carry, with
/// their counts.
pub fn candidates(
    library: &LibrarySnapshot,
    user: &UserData,
    labels: &Labels<'_>,
) -> Vec<Candidate> {
    let mut systems: BTreeMap<String, usize> = BTreeMap::new();
    let mut statuses: BTreeMap<EffectiveStatus, usize> = BTreeMap::new();
    // key → (first name seen, count)
    let mut genres: HashMap<String, (String, usize)> = HashMap::new();
    let mut tags: HashMap<String, (String, usize)> = HashMap::new();
    for g in library.games.iter() {
        let key = g.id().key();
        if user.hidden.contains(&key) {
            continue;
        }
        if let Some(s) = &g.system {
            *systems.entry(s.clone()).or_default() += 1;
        }
        *statuses.entry(user.effective_status(&key)).or_default() += 1;
        count_names(&mut genres, &g.genres, genre_key);
        count_names(&mut tags, &g.store_tags, tag_key);
    }
    let mut out: Vec<Candidate> =
        Vec::with_capacity(systems.len() + statuses.len() + genres.len() + tags.len());
    out.extend(
        systems
            .into_iter()
            .map(|(s, n)| Candidate::new(Value::System(s.clone()), (labels.system)(&s), n)),
    );
    out.extend(
        statuses
            .into_iter()
            .map(|(s, n)| Candidate::new(Value::Status(s), (labels.status)(s), n)),
    );
    out.extend(genres.into_iter().map(|(key, (name, n))| {
        Candidate::new(
            Value::Genre {
                key,
                name: name.clone(),
            },
            name,
            n,
        )
    }));
    out.extend(tags.into_iter().map(|(key, (name, n))| {
        Candidate::new(
            Value::Tag {
                key,
                name: name.clone(),
            },
            name,
            n,
        )
    }));
    out
}

/// Count each of a game's names once by `key`.
fn count_names(
    counts: &mut HashMap<String, (String, usize)>,
    names: &[String],
    key: impl Fn(&str) -> String,
) {
    let mut seen: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let k = key(name);
        if k.is_empty() || seen.contains(&k) {
            continue;
        }
        counts
            .entry(k.clone())
            .or_insert_with(|| (name.trim().to_owned(), 0))
            .1 += 1;
        seen.push(k);
    }
}

/// The candidates whose names hold `query`, best first: names that start
/// with it, then the rest; within each, the commonest first, then by name.
/// An empty query finds nothing.
pub fn find<'a>(candidates: &'a [Candidate], query: &str) -> Vec<&'a Candidate> {
    let q = fold(query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(bool, &Candidate)> = candidates
        .iter()
        .filter_map(|c| {
            let at = c.folded.find(&q)?;
            Some((at != 0, c))
        })
        .collect();
    hits.sort_by(|(a_inner, a), (b_inner, b)| {
        a_inner
            .cmp(b_inner)
            .then(b.count.cmp(&a.count))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then(a.value.section().cmp(&b.value.section()))
    });
    hits.into_iter().map(|(_, c)| c).collect()
}

/// What a [`ValueCache`] was worked out from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    user: usize,
    language: String,
}

/// The candidates of one set of inputs, worked out again only when one of
/// them changed.
#[derive(Default)]
pub struct ValueCache {
    key: Option<CacheKey>,
    candidates: Arc<[Candidate]>,
}

impl ValueCache {
    /// The candidates for these inputs. `language` tells the interface
    /// language apart (names are translated).
    pub fn get(
        &mut self,
        library: &LibrarySnapshot,
        user: &Arc<UserData>,
        language: String,
        make: impl FnOnce() -> Vec<Candidate>,
    ) -> Arc<[Candidate]> {
        let key = CacheKey {
            revision: library.revision,
            user: Arc::as_ptr(user) as usize,
            language,
        };
        if self.key.as_ref() != Some(&key) {
            self.candidates = make().into();
            self.key = Some(key);
        }
        self.candidates.clone()
    }

    /// The candidates last worked out, if any.
    #[cfg(test)]
    pub fn cached(&self) -> Option<Arc<[Candidate]>> {
        self.key.as_ref().map(|_| self.candidates.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use irori_core::{GameInfo, GameMode, Runner};

    use super::*;

    fn game(app: &str, genres: &[&str], tags: &[&str], modes: &[GameMode]) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                runner: Some(Runner::Steam),
                app_name: app.into(),
                title: app.into(),
                genres: genres.iter().map(|s| s.to_string()).collect(),
                store_tags: tags.iter().map(|s| s.to_string()).collect(),
                game_modes: modes.to_vec(),
                developer: Some("Valve".into()),
                publisher: Some("Devolver".into()),
                ..Default::default()
            }
            .finalize(),
        )
    }

    fn sample() -> (LibrarySnapshot, UserData) {
        let games = vec![
            game(
                "1",
                &["Role-playing", "Action"],
                &["RPG", "Open World", "Action-RPG"],
                &[GameMode::Single],
            ),
            game("2", &["role-playing "], &["Roguelike"], &[GameMode::Coop]),
            game("3", &["Puzzle"], &["Rogue-like"], &[GameMode::Coop]),
            game("4", &["Puzzle"], &[], &[]),
        ];
        let lib = LibrarySnapshot::new(
            1,
            irori_services::copies::Grouped {
                entries: games,
                ..Default::default()
            },
            HashMap::new(),
        );
        let mut user = UserData::default();
        user.hidden.insert("steam:4".into());
        (lib, user)
    }

    fn all(lib: &LibrarySnapshot, user: &UserData) -> Vec<Candidate> {
        let labels = Labels {
            system: &|s| s.to_uppercase(),
            status: &|s| format!("{s:?}"),
        };
        candidates(lib, user, &labels)
    }

    #[test]
    fn finds_values_of_every_section_with_counts() {
        let (lib, user) = sample();
        let values = all(&lib, &user);
        let found = find(&values, "rogue");
        let names: Vec<(&str, usize)> = found.iter().map(|c| (c.label.as_str(), c.count)).collect();
        // One tag for both spellings, counted per game.
        assert_eq!(names, [("Roguelike", 2)]);
        // Genres are compared like the genre filter; hidden games don't count.
        let rpg = find(&values, "role play");
        assert_eq!(rpg.len(), 1);
        assert_eq!((rpg[0].label.as_str(), rpg[0].count), ("Role-playing", 2));
        assert!(find(&values, "puzzle").iter().all(|c| c.count == 1));
        // Names that start with the text come before those that hold it.
        let rpg_tags = find(&values, "rpg");
        assert_eq!(rpg_tags[0].label, "RPG");
        assert_eq!(rpg_tags[1].label, "Action-RPG");
        // Removed popover sections (game modes, developers, publishers) stay out.
        assert!(find(&values, "coop").is_empty());
        assert!(find(&values, "valve").is_empty());
        assert!(find(&values, "devolver").is_empty());
        assert!(find(&values, "  ").is_empty());
        assert!(find(&values, "zzz").is_empty());
    }

    #[test]
    fn picking_a_value_ticks_it_as_its_section_does() {
        let (lib, user) = sample();
        let values = all(&lib, &user);
        let mut filter = ViewFilter::default();
        for c in find(&values, "rogue") {
            assert!(!c.value.is_picked(&filter));
            c.value.toggle(&mut filter);
            assert!(c.value.is_picked(&filter));
        }
        assert_eq!(filter.tags.len(), 1);
        let genre = find(&values, "role")[0].value.clone();
        genre.toggle(&mut filter);
        assert!(genre.is_picked(&filter));
        genre.toggle(&mut filter);
        assert!(filter.genres.is_empty());
        let status = Value::Status(EffectiveStatus::NotPlayed);
        status.toggle(&mut filter);
        assert!(status.is_picked(&filter));
        status.toggle(&mut filter);
        assert!(filter.statuses.is_empty());
        let sys = Value::System("snes".into());
        sys.toggle(&mut filter);
        assert!(sys.is_picked(&filter));
        sys.toggle(&mut filter);
        assert!(filter.systems.is_empty());
    }

    #[test]
    fn values_are_worked_out_once_per_input() {
        let (lib, user) = sample();
        let user = Arc::new(user);
        let mut cache = ValueCache::default();
        let mut made = 0;
        for _ in 0..3 {
            cache.get(&lib, &user, "en".into(), || {
                made += 1;
                Vec::new()
            });
        }
        cache.get(&lib, &user, "de".into(), || {
            made += 1;
            Vec::new()
        });
        assert_eq!(made, 2);
    }
}
