//! The small labels naming where a game comes from: its store (its mark
//! where Irori has one, see [`StoreLogo`]), or for a ROM the emulator that
//! plays it, and a "+N" past the first few.

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, rems,
};
use irori_core::Runner;

use super::StoreLogo;

/// Badges a cover shows before the rest become "+N".
pub const MAX_BADGES: usize = 3;

/// What one badge names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadgeMark {
    Store(Runner),
    /// A third-party launcher's game, by the launcher's id (`battlenet`,
    /// `ea`, `ubisoft`, `rockstar`, `xbox`).
    Launcher(SharedString),
    /// A ROM's emulator (`engine`, its id, when one is known), or its system.
    Emulator {
        name: SharedString,
        engine: Option<SharedString>,
    },
}

/// The badges a cover shows and how many more there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadgeRow {
    pub shown: Vec<BadgeMark>,
    pub more: usize,
}

/// The first `max` of `marks` and the count of the rest.
pub fn badge_row(mut marks: Vec<BadgeMark>, max: usize) -> BadgeRow {
    if marks.len() <= max {
        return BadgeRow {
            shown: marks,
            more: 0,
        };
    }
    let more = marks.len() - max;
    marks.truncate(max);
    BadgeRow { shown: marks, more }
}

/// The name of a third-party launcher's games' badge, never translated.
fn launcher_label(id: &str) -> &'static str {
    match id {
        "battlenet" => "Battle.net",
        "ea" => "EA",
        "ubisoft" => "Ubisoft",
        "rockstar" => "Rockstar",
        "xbox" => "Xbox",
        _ => Runner::Launchers.short_name(),
    }
}

/// What a badge draws before (or instead of) its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BadgeIcon {
    /// A brand's mark, which names the store on its own.
    Logo(StoreLogo),
    Joystick,
}

/// Small neutral label naming a game's store, a ROM's emulator, or how
/// many more stores there are.
#[derive(IntoElement)]
pub struct StoreBadge {
    label: Option<SharedString>,
    icon: Option<BadgeIcon>,
}

impl StoreBadge {
    pub fn new(runner: Runner) -> Self {
        match StoreLogo::of_runner(runner) {
            Some(logo) => Self::logo(logo),
            None => Self::text(runner.short_name()),
        }
    }

    fn text(label: impl Into<SharedString>) -> Self {
        Self {
            label: Some(label.into()),
            icon: None,
        }
    }

    fn logo(logo: StoreLogo) -> Self {
        Self {
            label: None,
            icon: Some(BadgeIcon::Logo(logo)),
        }
    }

    /// The badge of `mark`.
    pub fn mark(mark: BadgeMark) -> Self {
        match mark {
            BadgeMark::Store(runner) => Self::new(runner),
            BadgeMark::Launcher(id) => match StoreLogo::of_launcher(&id) {
                Some(logo) => Self::logo(logo),
                None => Self::text(launcher_label(&id)),
            },
            // The emulator's name stays beside its mark: it also names the
            // core RetroArch plays the game with.
            BadgeMark::Emulator { name, engine } => Self {
                label: Some(name),
                icon: Some(match engine.as_deref().and_then(StoreLogo::of_engine) {
                    Some(logo) => BadgeIcon::Logo(logo),
                    None => BadgeIcon::Joystick,
                }),
            },
        }
    }

    /// "+2": the stores the row leaves out.
    pub fn more(count: usize) -> Self {
        Self::text(format!("+{count}"))
    }
}

impl RenderOnce for StoreBadge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let icon = self.icon.map(|icon| match icon {
            BadgeIcon::Logo(logo) => logo.icon().xsmall(),
            BadgeIcon::Joystick => Icon::new(IconName::Joystick).xsmall(),
        });
        Tag::secondary().xsmall().outline().font_medium().child(
            h_flex()
                .gap_0p5()
                // A mark alone keeps the height a line of text gives.
                .min_h(rems(1.))
                .children(icon)
                .children(self.label),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stores(n: usize) -> Vec<BadgeMark> {
        [
            Runner::Steam,
            Runner::Gog,
            Runner::Legendary,
            Runner::Nile,
            Runner::Itch,
        ][..n]
            .iter()
            .map(|r| BadgeMark::Store(*r))
            .collect()
    }

    #[test]
    fn rows_past_three_badges_end_in_a_count() {
        assert_eq!(badge_row(stores(2), MAX_BADGES).more, 0);
        assert_eq!(badge_row(stores(3), MAX_BADGES).more, 0);
        let four = badge_row(stores(4), MAX_BADGES);
        assert_eq!((four.shown.len(), four.more), (3, 1));
        let five = badge_row(stores(5), MAX_BADGES);
        assert_eq!((five.shown.len(), five.more), (3, 2));
        assert_eq!(five.shown, stores(3));
    }

    #[test]
    fn badges_show_a_mark_where_there_is_one_and_text_elsewhere() {
        let steam = StoreBadge::new(Runner::Steam);
        assert_eq!(steam.icon, Some(BadgeIcon::Logo(StoreLogo::Steam)));
        assert_eq!(steam.label, None);
        let amazon = StoreBadge::new(Runner::Nile);
        assert_eq!((amazon.icon, amazon.label), (None, Some("Amazon".into())));
        let ubisoft = StoreBadge::mark(BadgeMark::Launcher("ubisoft".into()));
        assert_eq!(ubisoft.icon, Some(BadgeIcon::Logo(StoreLogo::Ubisoft)));
        let xbox = StoreBadge::mark(BadgeMark::Launcher("xbox".into()));
        assert_eq!((xbox.icon, xbox.label), (None, Some("Xbox".into())));
        let retroarch = StoreBadge::mark(BadgeMark::Emulator {
            name: "RetroArch · Snes9x".into(),
            engine: Some("retroarch".into()),
        });
        assert_eq!(retroarch.icon, Some(BadgeIcon::Logo(StoreLogo::RetroArch)));
        assert_eq!(retroarch.label, Some("RetroArch · Snes9x".into()));
        let system = StoreBadge::mark(BadgeMark::Emulator {
            name: "SNES".into(),
            engine: None,
        });
        assert_eq!(system.icon, Some(BadgeIcon::Joystick));
    }
}
