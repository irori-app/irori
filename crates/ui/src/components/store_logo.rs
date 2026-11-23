//! The marks of the stores, launchers and emulators Irori names, from
//! Simple Icons (`assets/brand/stores`, see its `NOTICE.md`). Each is drawn
//! whole, in one colour: the text colour where it stands, so it reads on
//! every theme the way an icon does. A brand without a mark there is named
//! in text instead.

use gpui_kit::component::Icon;
use irori_core::Runner;

/// A brand Irori has a mark for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreLogo {
    Steam,
    EpicGames,
    Gog,
    Itch,
    BattleNet,
    Ea,
    Ubisoft,
    Rockstar,
    RetroArch,
    Dolphin,
    Lutris,
}

impl StoreLogo {
    /// Every mark, for tests.
    pub const ALL: [StoreLogo; 11] = [
        StoreLogo::Steam,
        StoreLogo::EpicGames,
        StoreLogo::Gog,
        StoreLogo::Itch,
        StoreLogo::BattleNet,
        StoreLogo::Ea,
        StoreLogo::Ubisoft,
        StoreLogo::Rockstar,
        StoreLogo::RetroArch,
        StoreLogo::Dolphin,
        StoreLogo::Lutris,
    ];

    /// A store's mark. Amazon, ZOOM and Xbox have none in Simple Icons,
    /// and added games, ROMs and launchers aren't one brand.
    pub fn of_runner(runner: Runner) -> Option<Self> {
        Some(match runner {
            Runner::Steam => StoreLogo::Steam,
            Runner::Legendary => StoreLogo::EpicGames,
            Runner::Gog => StoreLogo::Gog,
            Runner::Itch => StoreLogo::Itch,
            Runner::Nile
            | Runner::Zoom
            | Runner::Xbox
            | Runner::Sideload
            | Runner::Emulation
            | Runner::Launchers => return None,
        })
    }

    /// A third-party launcher's mark by its id (`irori_launchers_data::Launcher::id`).
    pub fn of_launcher(id: &str) -> Option<Self> {
        Some(match id {
            "battlenet" => StoreLogo::BattleNet,
            "ea" => StoreLogo::Ea,
            "ubisoft" => StoreLogo::Ubisoft,
            "rockstar" => StoreLogo::Rockstar,
            _ => return None,
        })
    }

    /// An emulator's mark by its engine id (`assets/engines/<id>.json`).
    pub fn of_engine(id: &str) -> Option<Self> {
        Some(match id {
            "retroarch" => StoreLogo::RetroArch,
            "dolphin" => StoreLogo::Dolphin,
            _ => return None,
        })
    }

    /// The SVG as Simple Icons publishes it.
    pub fn svg(self) -> &'static [u8] {
        macro_rules! mark {
            ($file:literal) => {
                include_bytes!(concat!("../../../../assets/brand/stores/", $file))
            };
        }
        match self {
            StoreLogo::Steam => mark!("steam.svg"),
            StoreLogo::EpicGames => mark!("epicgames.svg"),
            StoreLogo::Gog => mark!("gogdotcom.svg"),
            StoreLogo::Itch => mark!("itchdotio.svg"),
            StoreLogo::BattleNet => mark!("battledotnet.svg"),
            StoreLogo::Ea => mark!("ea.svg"),
            StoreLogo::Ubisoft => mark!("ubisoft.svg"),
            StoreLogo::Rockstar => mark!("rockstargames.svg"),
            StoreLogo::RetroArch => mark!("retroarch.svg"),
            StoreLogo::Dolphin => mark!("dolphin.svg"),
            StoreLogo::Lutris => mark!("lutris.svg"),
        }
    }

    /// The mark as an icon: sized like one and drawn in the text colour
    /// (or whatever `text_color` the caller sets).
    pub fn icon(self) -> Icon {
        Icon::empty().data(self.svg())
    }
}

/// `runner`'s mark as an icon, else `fallback`: what store cards and
/// buttons show beside a store's name.
pub fn runner_icon(runner: Runner, fallback: impl Into<Icon>) -> Icon {
    match StoreLogo::of_runner(runner) {
        Some(logo) => logo.icon(),
        None => fallback.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mark_is_a_plain_one_colour_svg() {
        for logo in StoreLogo::ALL {
            let svg = std::str::from_utf8(logo.svg()).expect("UTF-8");
            assert!(svg.starts_with("<svg "), "{logo:?}");
            assert!(svg.contains(r#"viewBox="0 0 24 24""#), "{logo:?}");
            // Drawn in the text colour: a mark that brought its own colours
            // or pictures couldn't be held legible on every theme.
            for colour in ["fill=", "stroke=", "<image", "style="] {
                assert!(!svg.contains(colour), "{logo:?} has {colour}");
            }
        }
    }

    #[test]
    fn stores_without_a_mark_keep_their_text() {
        assert_eq!(
            StoreLogo::of_runner(Runner::Legendary),
            Some(StoreLogo::EpicGames)
        );
        for runner in [Runner::Nile, Runner::Zoom, Runner::Xbox, Runner::Sideload] {
            assert_eq!(StoreLogo::of_runner(runner), None, "{runner:?}");
        }
        assert_eq!(
            StoreLogo::of_launcher("rockstar"),
            Some(StoreLogo::Rockstar)
        );
        assert_eq!(StoreLogo::of_launcher("xbox"), None);
        assert_eq!(
            StoreLogo::of_engine("retroarch"),
            Some(StoreLogo::RetroArch)
        );
        assert_eq!(StoreLogo::of_engine("duckstation"), None);
    }
}
