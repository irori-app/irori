//! The library's groupings by a game's facts: store, platform, developer,
//! publisher, release year, playtime and size on disk. Each reorders the
//! filtered list into sections (stable, so the chosen order holds within
//! each) and puts every game in exactly one section, so the list keeps each
//! game once. Pure functions, tested here; the heading names come from
//! [`PlaytimeBand::label`] and [`SizeBand::label`].

use std::collections::HashMap;
use std::sync::Arc;

use irori_core::{GameInfo, InstallPlatform, Runner};
use irori_services::UserData;
use irori_ui::i18n::t;

use crate::filter::{Company, GroupKind, StatusGroup, company_key, sections};

/// How long a game has been played, in the sections of
/// [`crate::filter::Grouping::Playtime`]: most played first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlaytimeBand {
    Over100Hours,
    Over50Hours,
    Over10Hours,
    OverAnHour,
    UnderAnHour,
    Never,
}

impl PlaytimeBand {
    /// The band of a game played `minutes` in all.
    pub fn of(minutes: u64) -> Self {
        match minutes {
            0 => Self::Never,
            m if m < 60 => Self::UnderAnHour,
            m if m < 10 * 60 => Self::OverAnHour,
            m if m < 50 * 60 => Self::Over10Hours,
            m if m < 100 * 60 => Self::Over50Hours,
            _ => Self::Over100Hours,
        }
    }

    pub fn label(self) -> String {
        t(match self {
            Self::Over100Hours => "library.group.playtime.over_100",
            Self::Over50Hours => "library.group.playtime.over_50",
            Self::Over10Hours => "library.group.playtime.over_10",
            Self::OverAnHour => "library.group.playtime.over_1",
            Self::UnderAnHour => "library.group.playtime.under_1",
            Self::Never => "library.group.playtime.never",
        })
        .to_string()
    }
}

/// Size on disk, in the sections of [`crate::filter::Grouping::Size`]:
/// largest first, installed games of no known size, then games not
/// installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeBand {
    Over100Gib,
    Over50Gib,
    Over10Gib,
    Over1Gib,
    Under1Gib,
    Unknown,
    NotInstalled,
}

const GIB: u64 = 1024 * 1024 * 1024;

impl SizeBand {
    /// The band of `game`: by the size its install records, in GiB as
    /// Irori writes sizes.
    pub fn of(game: &GameInfo) -> Self {
        let Some(install) = game.install.as_ref().filter(|_| game.is_installed) else {
            return Self::NotInstalled;
        };
        match install.install_size_bytes {
            0 => Self::Unknown,
            b if b < GIB => Self::Under1Gib,
            b if b < 10 * GIB => Self::Over1Gib,
            b if b < 50 * GIB => Self::Over10Gib,
            b if b < 100 * GIB => Self::Over50Gib,
            _ => Self::Over100Gib,
        }
    }

    pub fn label(self) -> String {
        t(match self {
            Self::Over100Gib => "library.group.size.over_100",
            Self::Over50Gib => "library.group.size.over_50",
            Self::Over10Gib => "library.group.size.over_10",
            Self::Over1Gib => "library.group.size.over_1",
            Self::Under1Gib => "library.group.size.under_1",
            Self::Unknown => "library.group.size.unknown",
            Self::NotInstalled => "library.group.size.not_installed",
        })
        .to_string()
    }
}

/// Order of the platform sections.
pub const PLATFORM_ORDER: [InstallPlatform; 4] = [
    InstallPlatform::Windows,
    InstallPlatform::Linux,
    InstallPlatform::Mac,
    InstallPlatform::Browser,
];

/// The platform a game is grouped under: the build installed, else the
/// build that would be installed on `host` (its own when offered, else the
/// first offered in [`PLATFORM_ORDER`]). A store that names no platform
/// sells Windows builds; a web game is a browser game.
pub fn platform_section(game: &GameInfo, host: InstallPlatform) -> InstallPlatform {
    if let Some(platform) = game.installed_platform().filter(|_| game.is_installed) {
        return platform;
    }
    if game.browser_url.is_some() {
        return InstallPlatform::Browser;
    }
    if game.platforms.contains(&host) {
        return host;
    }
    PLATFORM_ORDER
        .into_iter()
        .find(|p| game.platforms.contains(p))
        .unwrap_or(InstallPlatform::Windows)
}

/// The year a game came out, from its `YYYY-…` release date.
pub fn release_year(game: &GameInfo) -> Option<i32> {
    game.release_date
        .as_deref()
        .and_then(|d| d.get(..4)?.parse::<i32>().ok())
}

/// Sections by store, in the store switcher's order.
pub fn group_by_store(games: &mut [Arc<GameInfo>]) -> Vec<StatusGroup> {
    let rank = |g: &Arc<GameInfo>| {
        Runner::ALL
            .iter()
            .position(|r| *r == g.runner())
            .unwrap_or(Runner::ALL.len())
    };
    games.sort_by_cached_key(rank);
    sections(games, |g| GroupKind::Store(g.runner()))
}

/// Sections by [`platform_section`], in [`PLATFORM_ORDER`].
pub fn group_by_platform(games: &mut [Arc<GameInfo>], host: InstallPlatform) -> Vec<StatusGroup> {
    let rank = |g: &Arc<GameInfo>| {
        let p = platform_section(g, host);
        PLATFORM_ORDER.iter().position(|o| *o == p).unwrap_or(0)
    };
    games.sort_by_cached_key(rank);
    sections(games, |g| GroupKind::Platform(platform_section(g, host)))
}

/// Sections by developer or publisher, by name, named as the library
/// spells each first; games whose store names none last.
pub fn group_by_company(games: &mut [Arc<GameInfo>], company: Company) -> Vec<StatusGroup> {
    let mut names: HashMap<String, Arc<str>> = HashMap::new();
    for name in games.iter().filter_map(|g| company.of(g)) {
        names
            .entry(company_key(name))
            .or_insert_with(|| Arc::from(name));
    }
    games.sort_by_cached_key(|g| match company.of(g).map(company_key) {
        Some(key) => {
            let shown = names
                .get(&key)
                .map(|n| n.to_lowercase())
                .unwrap_or_default();
            (0, shown, key)
        }
        None => (1, String::new(), String::new()),
    });
    sections(games, |g| {
        let name = company
            .of(g)
            .and_then(|n| names.get(&company_key(n)).cloned());
        GroupKind::Company(company, name)
    })
}

/// Sections by release year, newest first; games of no known date last.
pub fn group_by_year(games: &mut [Arc<GameInfo>]) -> Vec<StatusGroup> {
    games.sort_by_cached_key(|g| match release_year(g) {
        Some(year) => (0, std::cmp::Reverse(year)),
        None => (1, std::cmp::Reverse(0)),
    });
    sections(games, |g| GroupKind::Year(release_year(g)))
}

/// Sections by [`PlaytimeBand`], most played first.
pub fn group_by_playtime(games: &mut [Arc<GameInfo>], user: &UserData) -> Vec<StatusGroup> {
    let band = |g: &GameInfo| {
        let minutes = user
            .playtime
            .get(&g.id().key())
            .map_or(0, |r| r.total_minutes);
        PlaytimeBand::of(minutes)
    };
    games.sort_by_cached_key(|g| band(g));
    sections(games, |g| GroupKind::Playtime(band(g)))
}

/// Sections by [`SizeBand`], largest first.
pub fn group_by_size(games: &mut [Arc<GameInfo>]) -> Vec<StatusGroup> {
    games.sort_by_cached_key(|g| SizeBand::of(g));
    sections(games, |g| GroupKind::Size(SizeBand::of(g)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::InstalledInfo;

    fn game(app: &str, runner: Runner, f: impl FnOnce(&mut GameInfo)) -> Arc<GameInfo> {
        let mut g = GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: app.into(),
            ..Default::default()
        };
        f(&mut g);
        Arc::new(g.finalize())
    }

    fn installed(platform: InstallPlatform, bytes: u64) -> impl FnOnce(&mut GameInfo) {
        move |g| {
            g.is_installed = true;
            g.install = Some(InstalledInfo {
                platform,
                install_size_bytes: bytes,
                ..Default::default()
            });
        }
    }

    fn order(games: &[Arc<GameInfo>]) -> Vec<&str> {
        games.iter().map(|g| g.app_name.as_str()).collect()
    }

    #[test]
    fn stores_follow_the_switcher_order() {
        let mut games = vec![
            game("a", Runner::Sideload, |_| {}),
            game("b", Runner::Gog, |_| {}),
            game("c", Runner::Legendary, |_| {}),
            game("d", Runner::Gog, |_| {}),
        ];
        let groups = group_by_store(&mut games);
        assert_eq!(order(&games), ["c", "b", "d", "a"]);
        let kinds: Vec<_> = groups
            .iter()
            .map(|g| (g.kind.clone(), g.range.len()))
            .collect();
        assert_eq!(
            kinds,
            [
                (GroupKind::Store(Runner::Legendary), 1),
                (GroupKind::Store(Runner::Gog), 2),
                (GroupKind::Store(Runner::Sideload), 1),
            ]
        );
    }

    #[test]
    fn platforms_follow_the_build_that_runs_here() {
        let host = InstallPlatform::Linux;
        let both = game("both", Runner::Gog, |g| {
            g.platforms = vec![InstallPlatform::Windows, InstallPlatform::Linux];
        });
        assert_eq!(platform_section(&both, host), InstallPlatform::Linux);
        assert_eq!(
            platform_section(&both, InstallPlatform::Mac),
            InstallPlatform::Windows
        );
        // The build installed wins over what the store offers.
        let mut proton = (*both).clone();
        installed(InstallPlatform::Windows, 0)(&mut proton);
        assert_eq!(platform_section(&proton, host), InstallPlatform::Windows);
        let unnamed = game("x", Runner::Legendary, |_| {});
        assert_eq!(platform_section(&unnamed, host), InstallPlatform::Windows);
        let web = game("w", Runner::Sideload, |g| {
            g.browser_url = Some("https://example.org".into())
        });
        assert_eq!(platform_section(&web, host), InstallPlatform::Browser);

        let mut games = vec![web, both, unnamed];
        let groups = group_by_platform(&mut games, host);
        assert_eq!(order(&games), ["x", "both", "w"]);
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn companies_by_name_with_unknown_last() {
        let made = |app: &str, dev: Option<&str>| {
            let dev = dev.map(str::to_string);
            game(app, Runner::Steam, move |g| g.developer = dev)
        };
        let mut games = vec![
            made("1", None),
            made("2", Some("valve")),
            made("3", Some("Supergiant Games")),
            made("4", Some("Valve ")),
        ];
        let groups = group_by_company(&mut games, Company::Developer);
        assert_eq!(order(&games), ["3", "2", "4", "1"]);
        let kinds: Vec<_> = groups.iter().map(|g| g.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                GroupKind::Company(Company::Developer, Some(Arc::from("Supergiant Games"))),
                GroupKind::Company(Company::Developer, Some(Arc::from("valve"))),
                GroupKind::Company(Company::Developer, None),
            ]
        );
    }

    #[test]
    fn years_newest_first_with_unknown_last() {
        let out = |app: &str, date: Option<&str>| {
            let date = date.map(str::to_string);
            game(app, Runner::Gog, move |g| g.release_date = date)
        };
        let mut games = vec![
            out("a", Some("2015-05-19")),
            out("b", None),
            out("c", Some("2020-09-17")),
            out("d", Some("2015")),
            out("e", Some("bad")),
        ];
        let groups = group_by_year(&mut games);
        assert_eq!(order(&games), ["c", "a", "d", "b", "e"]);
        let kinds: Vec<_> = groups
            .iter()
            .map(|g| (g.kind.clone(), g.range.len()))
            .collect();
        assert_eq!(
            kinds,
            [
                (GroupKind::Year(Some(2020)), 1),
                (GroupKind::Year(Some(2015)), 2),
                (GroupKind::Year(None), 2),
            ]
        );
    }

    #[test]
    fn playtime_bands_most_played_first() {
        assert_eq!(PlaytimeBand::of(0), PlaytimeBand::Never);
        assert_eq!(PlaytimeBand::of(59), PlaytimeBand::UnderAnHour);
        assert_eq!(PlaytimeBand::of(60), PlaytimeBand::OverAnHour);
        assert_eq!(PlaytimeBand::of(600), PlaytimeBand::Over10Hours);
        assert_eq!(PlaytimeBand::of(3000), PlaytimeBand::Over50Hours);
        assert_eq!(PlaytimeBand::of(6000), PlaytimeBand::Over100Hours);

        let mut user = UserData::default();
        let mut play = |app: &str, minutes: u64| {
            let key = irori_core::GameId::new(Runner::Gog, app).key();
            user.playtime.entry(key).or_default().total_minutes = minutes;
        };
        play("a", 30);
        play("c", 7000);
        let mut games = vec![
            game("a", Runner::Gog, |_| {}),
            game("b", Runner::Gog, |_| {}),
            game("c", Runner::Gog, |_| {}),
        ];
        let groups = group_by_playtime(&mut games, &user);
        assert_eq!(order(&games), ["c", "a", "b"]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[2].kind, GroupKind::Playtime(PlaytimeBand::Never));
    }

    #[test]
    fn sizes_largest_first_then_unknown_then_not_installed() {
        let mut games = vec![
            game("none", Runner::Gog, |_| {}),
            game(
                "small",
                Runner::Gog,
                installed(InstallPlatform::Linux, GIB / 2),
            ),
            game(
                "unknown",
                Runner::Sideload,
                installed(InstallPlatform::Linux, 0),
            ),
            game(
                "big",
                Runner::Gog,
                installed(InstallPlatform::Windows, 120 * GIB),
            ),
            game(
                "mid",
                Runner::Gog,
                installed(InstallPlatform::Windows, 20 * GIB),
            ),
        ];
        let groups = group_by_size(&mut games);
        assert_eq!(order(&games), ["big", "mid", "small", "unknown", "none"]);
        let kinds: Vec<_> = groups.iter().map(|g| g.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                GroupKind::Size(SizeBand::Over100Gib),
                GroupKind::Size(SizeBand::Over10Gib),
                GroupKind::Size(SizeBand::Under1Gib),
                GroupKind::Size(SizeBand::Unknown),
                GroupKind::Size(SizeBand::NotInstalled),
            ]
        );
    }
}
