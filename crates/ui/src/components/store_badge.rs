//! The small labels naming where a game comes from: its store, or for a ROM
//! the emulator that plays it, and a "+N" past the first few.

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
};
use irori_core::Runner;

/// Badges a cover shows before the rest become "+N".
pub const MAX_BADGES: usize = 3;

/// What one badge names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadgeMark {
    Store(Runner),
    /// A ROM's emulator, or its system when no emulator is known.
    Emulator(SharedString),
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

/// Small neutral label naming a game's store, a ROM's emulator, or how
/// many more stores there are.
#[derive(IntoElement)]
pub struct StoreBadge {
    label: SharedString,
    icon: Option<IconName>,
}

impl StoreBadge {
    pub fn new(runner: Runner) -> Self {
        Self {
            label: runner.short_name().into(),
            icon: None,
        }
    }

    /// The badge of `mark`.
    pub fn mark(mark: BadgeMark) -> Self {
        match mark {
            BadgeMark::Store(runner) => Self::new(runner),
            BadgeMark::Emulator(name) => Self {
                label: name,
                icon: Some(IconName::Joystick),
            },
        }
    }

    /// "+2": the stores the row leaves out.
    pub fn more(count: usize) -> Self {
        Self {
            label: format!("+{count}").into(),
            icon: None,
        }
    }
}

impl RenderOnce for StoreBadge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = self.label;
        Tag::secondary()
            .xsmall()
            .outline()
            .font_medium()
            .child(match self.icon {
                Some(icon) => h_flex()
                    .gap_0p5()
                    .child(Icon::new(icon).xsmall())
                    .child(label)
                    .into_any_element(),
                None => label.into_any_element(),
            })
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
}
