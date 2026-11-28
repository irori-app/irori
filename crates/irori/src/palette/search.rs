//! Which games the palette lists for a query: ranked by the library's own
//! search (exact, prefix, word, then fuzzy; renamed games by their store
//! title too), so the palette finds what the library's search field finds.
//! Only a bounded number of game rows is built: each draws its cover, and
//! the palette measures every row it holds.

use std::sync::Arc;

use irori_core::GameInfo;
use irori_services::search::{Query, SearchIndex};

/// The most game rows a query lists when games are the only section; typing
/// more narrows them.
pub(super) const MAX_GAMES: usize = 25;

/// The games of `games` (in the order `index` was built for) that `query`
/// finds and `keep` lets through, best match first with their relevance score,
/// at most `cap`. Equal matches keep the library's order. Nothing for an empty
/// query.
pub(super) fn rank_games_scored(
    games: &[Arc<GameInfo>],
    index: &SearchIndex,
    query: &str,
    keep: impl Fn(&GameInfo) -> bool,
    cap: usize,
) -> Vec<(i64, Arc<GameInfo>)> {
    let Some(query) = Query::new(query) else {
        return Vec::new();
    };
    let mut found: Vec<(i64, usize)> = games
        .iter()
        .enumerate()
        .filter(|(_, g)| keep(g))
        .filter_map(|(ix, g)| index.score(ix, &query, g).map(|s| (s, ix)))
        .collect();
    // Stable, so the library's order decides between equal scores.
    found.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    found
        .into_iter()
        .take(cap)
        .map(|(score, ix)| (score, games[ix].clone()))
        .collect()
}

/// The games of `games` that `query` finds and `keep` lets through, without
/// scores.
#[cfg(test)]
pub(super) fn rank_games(
    games: &[Arc<GameInfo>],
    index: &SearchIndex,
    query: &str,
    keep: impl Fn(&GameInfo) -> bool,
    cap: usize,
) -> Vec<Arc<GameInfo>> {
    rank_games_scored(games, index, query, keep, cap)
        .into_iter()
        .map(|(_, g)| g)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn game(app: &str, title: &str) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(Runner::Gog),
            app_name: app.into(),
            title: title.into(),
            ..Default::default()
        })
    }

    fn titles(found: &[Arc<GameInfo>]) -> Vec<&str> {
        found.iter().map(|g| g.title.as_str()).collect()
    }

    #[test]
    fn ranks_like_the_library_search_and_stops_at_the_cap() {
        let games = vec![
            game("1", "Shadow of the Tomb Raider"),
            game("2", "Hades II"),
            game("3", "Hades"),
            game("4", "The Witcher 3"),
        ];
        let index = SearchIndex::build(&games);
        // The exact title first, then the prefix match.
        assert_eq!(
            titles(&rank_games(&games, &index, "hades", |_| true, 10)),
            ["Hades", "Hades II"]
        );
        // Roman numerals read as digits, as in the library.
        assert_eq!(
            titles(&rank_games(&games, &index, "hades 2", |_| true, 10)),
            ["Hades II"]
        );
        assert_eq!(
            rank_games(&games, &index, "hades", |_| true, 1).len(),
            1,
            "never more rows than the cap"
        );
        assert!(rank_games(&games, &index, "  ", |_| true, 10).is_empty());
    }

    #[test]
    fn leaves_out_what_keep_refuses() {
        let games = vec![game("1", "Hades"), game("2", "Hades II")];
        let index = SearchIndex::build(&games);
        let found = rank_games(&games, &index, "hades", |g| g.app_name != "1", 10);
        assert_eq!(titles(&found), ["Hades II"]);
    }
}
