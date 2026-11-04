//! What the album tab shows for one period: the screenshots the user took
//! and the clips the replay buffer saved in it of games in the library,
//! newest first, and those games by how many each has.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{Local, NaiveDate, TimeZone as _};
use irori_core::GameId;
use irori_services::ScreenshotIndex;

use super::model::game_id;
use super::period::{Period, Span};

/// One screenshot in the album.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub id: GameId,
    pub path: Arc<Path>,
    /// Its address for the art store.
    pub url: Arc<str>,
    /// Unix time it was taken.
    pub taken: i64,
    pub day: NaiveDate,
    /// A video clip, drawn over its game's art and played by the system.
    pub video: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub period: Period,
    /// Newest first.
    pub shots: Vec<Shot>,
    /// The games shown, with their screenshot count: most first, then the
    /// one with the latest screenshot.
    pub games: Vec<(GameId, usize)>,
}

impl Album {
    /// The screenshots of `game`, or all of them.
    pub fn shown(&self, game: Option<&GameId>) -> Vec<Shot> {
        self.shots
            .iter()
            .filter(|s| game.is_none_or(|g| s.id == *g))
            .cloned()
            .collect()
    }

    /// The game with the most screenshots.
    pub fn top(&self) -> Option<&(GameId, usize)> {
        self.games.first()
    }

    /// How many of the shots are clips.
    pub fn clips(&self) -> usize {
        self.shots.iter().filter(|s| s.video).count()
    }
}

/// A unix time as a local calendar day.
pub fn local_day(unix: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(unix, 0)
        .earliest()
        .map(|t| t.date_naive())
}

/// The album of `period`. `is_game` says which games are in the library: a
/// screenshot of a removed game has no title or art to show with it. All
/// time holds every screenshot, also those from before the session history
/// began (Steam keeps its own).
pub fn compute(
    index: &ScreenshotIndex,
    period: Period,
    is_game: impl Fn(&GameId) -> bool,
    day_of: impl Fn(i64) -> Option<NaiveDate>,
    today: NaiveDate,
) -> Album {
    let mut ids: HashMap<&str, Option<GameId>> = HashMap::new();
    let mut shots = Vec::new();
    for s in &index.shots {
        let inside = |d: &NaiveDate| match period.span {
            Span::All => *d <= today,
            _ => period.contains(*d, today),
        };
        let Some(day) = day_of(s.taken).filter(inside) else {
            continue;
        };
        let id = ids
            .entry(s.game.as_str())
            .or_insert_with(|| game_id(&s.game).filter(&is_game));
        let Some(id) = id.clone() else {
            continue;
        };
        shots.push(Shot {
            id,
            url: format!("file://{}", s.path.display()).into(),
            path: s.path.clone().into(),
            taken: s.taken,
            day,
            video: s.video,
        });
    }
    shots.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| a.path.cmp(&b.path)));
    // Shots are newest first, so the first time a game is seen is its latest.
    let mut games: Vec<(GameId, usize)> = Vec::new();
    for s in &shots {
        match games.iter_mut().find(|(id, _)| *id == s.id) {
            Some((_, n)) => *n += 1,
            None => games.push((s.id.clone(), 1)),
        }
    }
    games.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    Album {
        period,
        shots,
        games,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::model::tests::day;
    use irori_services::Screenshot;

    fn shot(game: &str, taken: i64) -> Screenshot {
        Screenshot {
            path: format!("/shots/{game}-{taken}.png").into(),
            taken,
            game: game.into(),
            video: false,
        }
    }

    /// One day per 100 seconds from 2026-10-01, so tests need no time zone.
    fn day_of(unix: i64) -> Option<NaiveDate> {
        day("2026-10-01").checked_add_days(chrono::Days::new((unix / 100) as u64))
    }

    #[test]
    fn keeps_the_periods_screenshots_of_library_games() {
        let index = ScreenshotIndex {
            shots: vec![
                shot("gog:hades", 3050),
                shot("steam:1245620", 3000),
                shot("steam:1245620", 1000),
                shot("gog:removed", 900),
                shot("steam:1245620", 500),
                shot("gog:hades", 400),
                // September and November.
                shot("gog:hades", -100),
                shot("gog:hades", 3200),
                shot("not a key", 600),
            ],
            folders: Vec::new(),
        };
        let october = Period::containing(Span::Month, day("2026-10-15"), None);
        let album = compute(
            &index,
            october,
            |id| id.app_name.as_ref() != "removed",
            day_of,
            day("2026-11-03"),
        );
        let taken: Vec<i64> = album.shots.iter().map(|s| s.taken).collect();
        assert_eq!(taken, vec![3050, 3000, 1000, 500, 400]);
        let elden = game_id("steam:1245620").unwrap();
        let hades = game_id("gog:hades").unwrap();
        assert_eq!(album.games, vec![(elden.clone(), 3), (hades.clone(), 2)]);
        assert_eq!(album.top(), Some(&(elden.clone(), 3)));
        assert_eq!(album.shown(Some(&hades)).len(), 2);
        assert_eq!(album.shown(None).len(), 5);
        assert_eq!(album.shots[0].day, day("2026-10-31"));
        assert_eq!(&*album.shots[0].url, "file:///shots/gog:hades-3050.png");
        assert_eq!(album.clips(), 0);
    }

    #[test]
    fn clips_count_with_the_screenshots_of_their_game() {
        let mut clip = shot("gog:hades", 300);
        clip.path = "/clips/Hades/Hades 2026-10-04 19-15-05.mp4".into();
        clip.video = true;
        let index = ScreenshotIndex {
            shots: vec![clip, shot("gog:hades", 200)],
            folders: Vec::new(),
        };
        let all = Period::containing(Span::All, day("2026-11-03"), None);
        let album = compute(&index, all, |_| true, day_of, day("2026-11-03"));
        assert_eq!(album.clips(), 1);
        assert!(album.shots[0].video && !album.shots[1].video);
        assert_eq!(album.games.len(), 1);
        assert_eq!(album.games[0].1, 2);
    }

    #[test]
    fn ties_go_to_the_game_with_the_latest_screenshot() {
        let index = ScreenshotIndex {
            shots: vec![
                shot("gog:a", 300),
                shot("gog:b", 200),
                shot("gog:b", 150),
                shot("gog:a", 100),
            ],
            folders: Vec::new(),
        };
        // All time reaches before the history's first day.
        let all = Period::containing(Span::All, day("2026-11-03"), Some(day("2026-10-03")));
        let album = compute(&index, all, |_| true, day_of, day("2026-11-03"));
        assert_eq!(album.shots.len(), 4);
        assert_eq!(album.top().map(|(id, _)| id.key()), Some("gog:a".into()));
    }
}
