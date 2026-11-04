//! Games related to one game, from what the library already knows: the same
//! series (titles that share their name before a number or subtitle), the
//! same developer or publisher, and shared genres. Nothing is fetched.

use std::collections::BTreeSet;
use std::sync::Arc;

use irori_core::GameInfo;

use crate::library::LibrarySnapshot;
use crate::search::fold;
use crate::userdata::UserData;

/// Why a game is related, strongest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelatedReason {
    /// The same series.
    Series,
    /// Made by the same developer (its name as the game shows it).
    Developer(String),
    /// Published by the same publisher.
    Publisher(String),
    /// Shares genres; the shared ones, in the game's order.
    Genres(Vec<String>),
}

/// One related game.
#[derive(Debug, Clone)]
pub struct Related {
    pub game: Arc<GameInfo>,
    pub reason: RelatedReason,
}

/// Up to `limit` games related to `game`, closest first. `genres` are the
/// game's genres when the page knows more than the library entry (its store
/// page); hidden games are left out.
pub fn related_games(
    game: &GameInfo,
    genres: &[String],
    library: &LibrarySnapshot,
    user: &UserData,
    limit: usize,
) -> Vec<Related> {
    let id = game.id();
    let series = series_stem(&game.title);
    let developer = game.developer.as_deref().and_then(company);
    let publisher = game.publisher.as_deref().and_then(company);
    let own_genres: Vec<String> = if genres.is_empty() {
        game.genres.clone()
    } else {
        genres.to_vec()
    };
    let wanted: BTreeSet<String> = own_genres.iter().map(|g| fold(g)).collect();
    // The same game from another store is not a related game.
    let own_title = fold(&game.title);
    let mut scored: Vec<(u32, Related)> = library
        .games
        .iter()
        .filter(|g| g.id() != id && !user.hidden.contains(&g.id().key()))
        .filter(|g| fold(&g.title) != own_title)
        .filter_map(|other| {
            let same_series = series
                .as_deref()
                .zip(series_stem(&other.title))
                .is_some_and(|(a, b)| same_series(a, &b));
            let same_dev =
                developer.is_some() && other.developer.as_deref().and_then(company) == developer;
            let same_pub =
                publisher.is_some() && other.publisher.as_deref().and_then(company) == publisher;
            let shared: Vec<String> = if wanted.is_empty() {
                Vec::new()
            } else {
                let theirs: BTreeSet<String> = other.genres.iter().map(|g| fold(g)).collect();
                own_genres
                    .iter()
                    .filter(|g| theirs.contains(&fold(g)))
                    .cloned()
                    .collect()
            };
            // Genres alone need more than one in common, or the only one
            // either game has: "Action" is not a likeness.
            let genre_match = !shared.is_empty()
                && (shared.len() >= 2 || wanted.len() == 1 || other.genres.len() == 1);
            let mut score = 0;
            if same_series {
                score += 100;
            }
            if same_dev {
                score += 40;
            }
            if same_pub {
                score += 20;
            }
            if genre_match {
                score += 10 * shared.len().min(3) as u32;
            }
            if score == 0 {
                return None;
            }
            let reason = if same_series {
                RelatedReason::Series
            } else if same_dev {
                RelatedReason::Developer(game.developer.clone().unwrap_or_default())
            } else if same_pub {
                RelatedReason::Publisher(game.publisher.clone().unwrap_or_default())
            } else {
                RelatedReason::Genres(shared)
            };
            Some((
                score,
                Related {
                    game: other.clone(),
                    reason,
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.game.sort_title.cmp(&b.1.game.sort_title))
    });
    scored.into_iter().take(limit).map(|(_, r)| r).collect()
}

/// The series part of a title: folded, without a leading article, cut
/// before a subtitle (`:`, ` - `) and without trailing sequel numbers or
/// edition words. `None` when too little is left to compare.
pub fn series_stem(title: &str) -> Option<String> {
    let cut = title
        .split([':', '–', '—'])
        .next()
        .unwrap_or(title)
        .split(" - ")
        .next()
        .unwrap_or(title);
    let folded = fold(cut);
    let mut words: Vec<&str> = folded.split(' ').collect();
    if matches!(words.first(), Some(&("the" | "a" | "an"))) && words.len() > 1 {
        words.remove(0);
    }
    const EDITION: &[&str] = &[
        "remastered",
        "remake",
        "definitive",
        "edition",
        "goty",
        "complete",
        "enhanced",
        "deluxe",
        "anniversary",
        "hd",
        "redux",
        "director",
        "directors",
        "cut",
        "collection",
        "trilogy",
    ];
    while let Some(last) = words.last() {
        let number = last.chars().all(|c| c.is_ascii_digit())
            || (words.len() > 1
                && last.len() <= 4
                && last.chars().all(|c| matches!(c, 'i' | 'v' | 'x')));
        if words.len() > 1 && (number || EDITION.contains(last)) {
            words.pop();
        } else {
            break;
        }
    }
    let stem = words.join(" ");
    (stem.chars().filter(|c| c.is_alphanumeric()).count() >= 4).then_some(stem)
}

/// Whether two series stems are one series: equal, or one is the other's
/// first words ("doom" and "doom eternal").
fn same_series(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short == long || (long.starts_with(short) && long.as_bytes().get(short.len()) == Some(&b' '))
}

/// A company name for comparing: folded, without legal suffixes.
fn company(name: &str) -> Option<String> {
    // "S.A." and "SA" are one suffix.
    let folded = fold(&name.replace('.', ""));
    let mut words: Vec<&str> = folded.split(' ').collect();
    const SUFFIX: &[&str] = &[
        "inc",
        "ltd",
        "llc",
        "gmbh",
        "co",
        "corp",
        "corporation",
        "sa",
        "srl",
        "ab",
        "bv",
        "oy",
        "kk",
        "limited",
        "entertainment",
        "games",
        "studios",
        "studio",
        "interactive",
    ];
    while words.len() > 1 && words.last().is_some_and(|w| SUFFIX.contains(w)) {
        words.pop();
    }
    let name = words.join(" ");
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(app: &str, title: &str, f: impl FnOnce(&mut GameInfo)) -> Arc<GameInfo> {
        let mut g = GameInfo {
            app_name: app.into(),
            title: title.into(),
            ..Default::default()
        };
        f(&mut g);
        Arc::new(g.finalize())
    }

    fn library(games: Vec<Arc<GameInfo>>) -> LibrarySnapshot {
        let mut s = LibrarySnapshot::default();
        s.games = games;
        s
    }

    #[test]
    fn series_stems() {
        assert_eq!(
            series_stem("The Witcher 3: Wild Hunt").as_deref(),
            Some("witcher")
        );
        assert_eq!(series_stem("The Witcher 2").as_deref(), Some("witcher"));
        assert_eq!(series_stem("Dark Souls III").as_deref(), Some("dark souls"));
        assert_eq!(
            series_stem("Dark Souls: Remastered").as_deref(),
            Some("dark souls")
        );
        assert_eq!(series_stem("Half-Life 2").as_deref(), Some("half life"));
        assert_eq!(
            series_stem("Mass Effect Legendary Edition").as_deref(),
            Some("mass effect legendary")
        );
        assert_eq!(series_stem("Ys"), None);
        assert!(same_series("doom", "doom eternal"));
        assert!(!same_series("dark", "darksiders"));
    }

    #[test]
    fn ranks_series_then_studio_then_genres() {
        let me = game("w3", "The Witcher 3: Wild Hunt", |g| {
            g.developer = Some("CD PROJEKT RED".into());
            g.publisher = Some("CD PROJEKT S.A.".into());
            g.genres = vec!["RPG".into(), "Open world".into(), "Action".into()];
        });
        let games = vec![
            me.clone(),
            game("w2", "The Witcher 2: Assassins of Kings", |_| {}),
            game("cp", "Cyberpunk 2077", |g| {
                g.developer = Some("CD Projekt Red".into());
                g.publisher = Some("CD Projekt SA".into());
            }),
            game("gw", "Gwent", |g| {
                g.publisher = Some("CD PROJEKT S.A.".into())
            }),
            game("sk", "Skyrim", |g| {
                g.genres = vec!["RPG".into(), "Open World".into()];
            }),
            // One shared broad genre is not enough.
            game("dm", "Doom", |g| {
                g.genres = vec!["Action".into(), "Shooter".into()]
            }),
            game("hidden", "The Witcher Enhanced Edition", |_| {}),
        ];
        let lib = library(games);
        let mut user = UserData::default();
        user.hidden.insert(
            GameInfo {
                app_name: "hidden".into(),
                ..Default::default()
            }
            .id()
            .key(),
        );
        let related = related_games(&me, &[], &lib, &user, 10);
        let titles: Vec<&str> = related.iter().map(|r| r.game.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "The Witcher 2: Assassins of Kings",
                "Cyberpunk 2077",
                "Gwent",
                "Skyrim"
            ]
        );
        assert_eq!(related[0].reason, RelatedReason::Series);
        assert_eq!(
            related[1].reason,
            RelatedReason::Developer("CD PROJEKT RED".into())
        );
        assert_eq!(
            related[2].reason,
            RelatedReason::Publisher("CD PROJEKT S.A.".into())
        );
        assert_eq!(
            related[3].reason,
            RelatedReason::Genres(vec!["RPG".into(), "Open world".into()])
        );
        // The page's richer genres take over the entry's.
        let more = related_games(&me, &["Shooter".into(), "Action".into()], &lib, &user, 10);
        assert!(more.iter().any(|r| r.game.title == "Doom"));
    }
}
