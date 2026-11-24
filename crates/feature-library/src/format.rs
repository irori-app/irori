//! Display strings for library metadata.

use irori_core::util::{format_bytes, parse_rfc3339};
use irori_core::{GameInfo, Runner};
use irori_ui::i18n::{t, t_args};

/// "Just now", "3 hours ago", "Yesterday", "5 days ago", "2 weeks ago",
/// "4 months ago", "2 years ago".
pub fn relative_time(then: i64, now: i64) -> String {
    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    let diff = now - then;
    let plural = |n: i64, one: &str, other: &str| {
        if n == 1 {
            t(one).to_string()
        } else {
            t_args(other, &[("count", &n.to_string())]).to_string()
        }
    };
    match diff {
        d if d < HOUR => t("library.time.just_now").to_string(),
        d if d < DAY => plural(
            d / HOUR,
            "library.time.hours_one",
            "library.time.hours_other",
        ),
        d if d < 2 * DAY => t("library.time.yesterday").to_string(),
        // Two days or more: never singular.
        d if d < 7 * DAY => t_args(
            "library.time.days_other",
            &[("count", &(d / DAY).to_string())],
        )
        .to_string(),
        d if d < 30 * DAY => plural(
            d / (7 * DAY),
            "library.time.weeks_one",
            "library.time.weeks_other",
        ),
        d if d < 365 * DAY => plural(
            d / (30 * DAY),
            "library.time.months_one",
            "library.time.months_other",
        ),
        d => plural(
            d / (365 * DAY),
            "library.time.years_one",
            "library.time.years_other",
        ),
    }
}

/// Relative "last played" text from an RFC 3339 timestamp.
pub fn last_played(rfc3339: Option<&str>, now: i64) -> Option<String> {
    rfc3339
        .and_then(parse_rfc3339)
        .map(|t| relative_time(t, now))
}

/// Playtime, or `None` for games never played.
pub fn playtime(minutes: u64) -> Option<String> {
    (minutes > 0).then(|| crate::i18n::playtime(minutes))
}

/// Size on disk of an installed game, when known. Sideloaded apps have none.
pub fn install_size(game: &GameInfo) -> Option<String> {
    if game.runner() == Runner::Sideload {
        return None;
    }
    let install = game.install.as_ref()?;
    if install.install_size_bytes > 0 {
        Some(format_bytes(install.install_size_bytes))
    } else {
        Some(install.install_size.trim().to_string()).filter(|s| !s.is_empty())
    }
}

/// Current unix time in seconds.
pub fn now() -> i64 {
    irori_core::util::unix_now() as i64
}

/// What a game's caption says about the order the library is in: the value
/// it is sorted by, with that order's icon. `None` for orders without one
/// to show (title, installed first, an order this version doesn't caption)
/// and for games without the value (never played, size unknown).
/// `last_played` and `added` are RFC 3339 times (`added` may be a bare
/// `YYYY-MM-DD`).
pub fn sort_value(
    sort: irori_services::LibrarySort,
    game: &GameInfo,
    minutes: u64,
    last_played: Option<&str>,
    added: Option<&str>,
    now: i64,
) -> Option<(gpui_kit::assets::IconName, String)> {
    use gpui_kit::assets::IconName;
    use irori_services::LibrarySort;
    match sort {
        LibrarySort::LastPlayed => {
            self::last_played(last_played, now).map(|when| (IconName::CalendarClock, when))
        }
        LibrarySort::Playtime => playtime(minutes).map(|p| (IconName::Clock, p)),
        LibrarySort::InstallSize => install_size(game).map(|s| (IconName::HardDrive, s)),
        LibrarySort::ReleaseDate => game
            .release_date
            .as_deref()
            .and_then(|d| d.get(..4))
            .filter(|y| y.bytes().all(|b| b.is_ascii_digit()))
            .map(|y| (IconName::CalendarDays, y.to_string())),
        LibrarySort::DateAdded => {
            let added = added?;
            let when = parse_rfc3339(added)
                .or_else(|| parse_rfc3339(&format!("{}T00:00:00Z", added.get(..10)?)))?;
            Some((IconName::CalendarDays, relative_time(when, now)))
        }
        LibrarySort::Score => game.rating.map(|r| (IconName::Star, r.to_string())),
        // Orders added later show nothing until they say what to show.
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::InstalledInfo;

    #[test]
    fn captions_name_the_value_the_library_is_sorted_by() {
        use irori_services::LibrarySort;
        crate::i18n::register();
        let now = 1_000_000_000;
        let game = GameInfo {
            runner: Some(Runner::Gog),
            release_date: Some("2015-05-19".into()),
            rating: Some(92),
            install: Some(InstalledInfo {
                install_size_bytes: 1536,
                ..Default::default()
            }),
            ..Default::default()
        };
        let text = |sort, minutes, last: Option<&str>, added: Option<&str>| {
            sort_value(sort, &game, minutes, last, added, now).map(|(_, text)| text)
        };
        assert_eq!(text(LibrarySort::Title, 60, None, None), None);
        assert_eq!(text(LibrarySort::InstalledFirst, 60, None, None), None);
        assert_eq!(
            text(LibrarySort::Playtime, 125, None, None).as_deref(),
            Some("2 h 5 min")
        );
        assert_eq!(text(LibrarySort::Playtime, 0, None, None), None);
        assert_eq!(
            text(LibrarySort::InstallSize, 0, None, None).as_deref(),
            Some("1.5 KiB")
        );
        assert_eq!(
            text(LibrarySort::ReleaseDate, 0, None, None).as_deref(),
            Some("2015")
        );
        assert_eq!(
            text(LibrarySort::Score, 0, None, None).as_deref(),
            Some("92")
        );
        assert_eq!(text(LibrarySort::LastPlayed, 0, None, None), None);
        assert!(
            text(
                LibrarySort::LastPlayed,
                0,
                Some("2001-09-08T00:00:00Z"),
                None
            )
            .is_some()
        );
        // A bare date added reads too; nonsense doesn't.
        assert!(text(LibrarySort::DateAdded, 0, None, Some("2001-09-01")).is_some());
        assert_eq!(text(LibrarySort::DateAdded, 0, None, Some("soon")), None);
    }

    #[test]
    fn relative_times() {
        crate::i18n::register();
        let now = 1_000_000_000;
        let h = 3600;
        assert_eq!(relative_time(now - 60, now), "Just now");
        assert_eq!(relative_time(now + 60, now), "Just now");
        assert_eq!(relative_time(now - h, now), "1 hour ago");
        assert_eq!(relative_time(now - 5 * h, now), "5 hours ago");
        assert_eq!(relative_time(now - 30 * h, now), "Yesterday");
        assert_eq!(relative_time(now - 3 * 24 * h, now), "3 days ago");
        assert_eq!(relative_time(now - 15 * 24 * h, now), "2 weeks ago");
        assert_eq!(relative_time(now - 65 * 24 * h, now), "2 months ago");
        assert_eq!(relative_time(now - 800 * 24 * h, now), "2 years ago");
        assert_eq!(last_played(Some("not a date"), now), None);
        assert_eq!(last_played(None, now), None);
        assert!(last_played(Some("2001-09-08T00:00:00Z"), now).is_some());
    }

    #[test]
    fn sizes_and_playtime() {
        crate::i18n::register();
        assert_eq!(playtime(0), None);
        assert_eq!(playtime(125).as_deref(), Some("2 h 5 min"));
        let mut g = GameInfo {
            runner: Some(Runner::Gog),
            install: Some(InstalledInfo {
                install_size_bytes: 1536,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(install_size(&g).as_deref(), Some("1.5 KiB"));
        g.install = Some(InstalledInfo {
            install_size: " 12.3 GiB ".into(),
            ..Default::default()
        });
        assert_eq!(install_size(&g).as_deref(), Some("12.3 GiB"));
        g.runner = Some(Runner::Sideload);
        assert_eq!(install_size(&g), None);
    }
}
