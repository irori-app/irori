//! Which input each controller button gives: the family's layout, the
//! button the user chose to confirm with (`iroriController.confirmButton`),
//! the family the buttons are shown as (`iroriController.glyphs`) and the
//! buttons they gave another job (`iroriController.buttons`).
//!
//! One map serves the whole application. It sits in a global that the
//! reader thread reads on each press and the button hints read when they
//! draw, so a change in Settings applies to the next press and the next
//! frame everywhere: Big Picture, the in-game overlay and ordinary screens.
//!
//! The D-pad, the sticks and Guide keep their jobs: they are how a
//! controller reaches every screen and leaves a game. A map that leaves no
//! button to confirm or to go back would strand the user, so it is never
//! used (the defaults stand in) and Settings refuses to save one.

use irori_core::controller_prefs::{ConfirmButton, ControllerPrefs, GlyphStyle};
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use super::input::{ControllerKind, GamepadInput, PadButton};

/// The buttons the user can give another job, in the order the hints look
/// for a button that gives an input (face buttons before Start and Select).
pub const REBINDABLE: [PadButton; 8] = [
    PadButton::South,
    PadButton::East,
    PadButton::West,
    PadButton::North,
    PadButton::LeftShoulder,
    PadButton::RightShoulder,
    PadButton::Start,
    PadButton::Select,
];

/// The jobs a rebindable button can take.
pub const BINDABLE: [GamepadInput; 6] = [
    GamepadInput::Confirm,
    GamepadInput::Back,
    GamepadInput::Alt,
    GamepadInput::Menu,
    GamepadInput::PrevPage,
    GamepadInput::NextPage,
];

impl PadButton {
    /// Stable id in `iroriController.buttons`.
    pub fn id(self) -> &'static str {
        match self {
            PadButton::South => "south",
            PadButton::East => "east",
            PadButton::West => "west",
            PadButton::North => "north",
            PadButton::LeftShoulder => "left_shoulder",
            PadButton::RightShoulder => "right_shoulder",
            PadButton::Start => "start",
            PadButton::Select => "select",
            PadButton::Guide => "guide",
            PadButton::DPadUp => "dpad_up",
            PadButton::DPadDown => "dpad_down",
            PadButton::DPadLeft => "dpad_left",
            PadButton::DPadRight => "dpad_right",
        }
    }

    /// The rebindable button of an id.
    pub fn from_id(id: &str) -> Option<PadButton> {
        REBINDABLE.into_iter().find(|b| b.id() == id)
    }
}

impl GamepadInput {
    /// Stable id of a job in `iroriController.buttons`.
    pub fn id(self) -> &'static str {
        match self {
            GamepadInput::Up => "up",
            GamepadInput::Down => "down",
            GamepadInput::Left => "left",
            GamepadInput::Right => "right",
            GamepadInput::Confirm => "confirm",
            GamepadInput::Back => "back",
            GamepadInput::Alt => "alt",
            GamepadInput::Menu => "menu",
            GamepadInput::PrevPage => "prev_page",
            GamepadInput::NextPage => "next_page",
            GamepadInput::Guide => "guide",
        }
    }

    /// The bindable job of an id.
    pub fn from_id(id: &str) -> Option<GamepadInput> {
        BINDABLE.into_iter().find(|i| i.id() == id)
    }
}

/// What `iroriController.buttons` gives a button that does nothing.
pub const NOTHING: &str = "none";

impl ControllerKind {
    /// What is printed on `button` on this family's controllers.
    pub fn button_label(self, button: PadButton) -> &'static str {
        let playstation = matches!(
            self,
            ControllerKind::PlayStation4 | ControllerKind::PlayStation5
        );
        let nintendo = self == ControllerKind::Nintendo;
        match button {
            PadButton::South if playstation => "✕",
            PadButton::East if playstation => "○",
            PadButton::West if playstation => "□",
            PadButton::North if playstation => "△",
            PadButton::LeftShoulder if playstation => "L1",
            PadButton::RightShoulder if playstation => "R1",
            PadButton::Start if playstation => "Options",
            PadButton::Select if self == ControllerKind::PlayStation5 => "Create",
            PadButton::Select if playstation => "Share",
            PadButton::South if nintendo => "B",
            PadButton::East if nintendo => "A",
            PadButton::West if nintendo => "Y",
            PadButton::North if nintendo => "X",
            PadButton::LeftShoulder if nintendo => "L",
            PadButton::RightShoulder if nintendo => "R",
            PadButton::Start if nintendo => "+",
            PadButton::Select if nintendo => "−",
            PadButton::South => "A",
            PadButton::East => "B",
            PadButton::West => "X",
            PadButton::North => "Y",
            PadButton::LeftShoulder => "LB",
            PadButton::RightShoulder => "RB",
            PadButton::Start => "Menu",
            PadButton::Select => "View",
            PadButton::Guide => "Guide",
            PadButton::DPadUp
            | PadButton::DPadDown
            | PadButton::DPadLeft
            | PadButton::DPadRight => "D-pad",
        }
    }
}

/// The job a button has on a family before anything is rebound: the bottom
/// face button confirms, the right one goes back, unless `swap` (Nintendo's
/// layout, or the user's choice) turns the face buttons round.
pub fn base_input(button: PadButton, swap: bool) -> GamepadInput {
    match button {
        PadButton::South if swap => GamepadInput::Back,
        PadButton::East if swap => GamepadInput::Confirm,
        PadButton::North if swap => GamepadInput::Alt,
        PadButton::West if swap => GamepadInput::Menu,
        PadButton::South => GamepadInput::Confirm,
        PadButton::East => GamepadInput::Back,
        PadButton::West => GamepadInput::Alt,
        PadButton::North => GamepadInput::Menu,
        PadButton::Start | PadButton::Select => GamepadInput::Menu,
        PadButton::LeftShoulder => GamepadInput::PrevPage,
        PadButton::RightShoulder => GamepadInput::NextPage,
        PadButton::Guide => GamepadInput::Guide,
        PadButton::DPadUp => GamepadInput::Up,
        PadButton::DPadDown => GamepadInput::Down,
        PadButton::DPadLeft => GamepadInput::Left,
        PadButton::DPadRight => GamepadInput::Right,
    }
}

/// The buttons' jobs (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ButtonMap {
    confirm: ConfirmButton,
    glyphs: GlyphStyle,
    /// Rebound buttons: the job each has, `None` for none.
    rebound: Vec<(PadButton, Option<GamepadInput>)>,
}

impl ButtonMap {
    /// The map the preferences describe. Entries this version doesn't know
    /// are left out (and stay in the file); a map that would leave nothing
    /// to confirm or go back with keeps only the confirm button and glyphs.
    pub fn from_prefs(prefs: &ControllerPrefs) -> ButtonMap {
        let rebound = prefs
            .buttons
            .iter()
            .filter_map(|(button, job)| {
                let button = PadButton::from_id(button)?;
                let job = match job.as_str() {
                    NOTHING => None,
                    id => Some(GamepadInput::from_id(id)?),
                };
                Some((button, job))
            })
            .collect();
        let map = ButtonMap {
            confirm: prefs.confirm_button,
            glyphs: prefs.glyphs,
            rebound,
        };
        if map.usable() {
            map
        } else {
            tracing::warn!("controller buttons leave nothing to confirm or go back with; ignored");
            ButtonMap {
                rebound: Vec::new(),
                ..map
            }
        }
    }

    /// The family a controller of `kind` is shown as.
    pub fn display_kind(&self, kind: ControllerKind) -> ControllerKind {
        match self.glyphs {
            GlyphStyle::Auto => kind,
            GlyphStyle::Xbox => ControllerKind::Xbox,
            GlyphStyle::PlayStation => ControllerKind::PlayStation5,
            GlyphStyle::Nintendo => ControllerKind::Nintendo,
        }
    }

    /// Whether the face buttons are turned round on a controller of `kind`:
    /// as the family shown has it, or as the user chose.
    pub fn swaps(&self, kind: ControllerKind) -> bool {
        match self.confirm {
            ConfirmButton::Auto => self.display_kind(kind).swaps_face_buttons(),
            ConfirmButton::South => false,
            ConfirmButton::East => true,
        }
    }

    /// The job `button` has on a controller of `kind`, `None` when the user
    /// gave it none.
    pub fn input(&self, button: PadButton, kind: ControllerKind) -> Option<GamepadInput> {
        match self.rebound.iter().find(|(b, _)| *b == button) {
            Some((_, job)) => *job,
            None => Some(base_input(button, self.swaps(kind))),
        }
    }

    /// Whether `button` was given another job than its own.
    pub fn is_rebound(&self, button: PadButton) -> bool {
        self.rebound.iter().any(|(b, _)| *b == button)
    }

    /// The button that gives `input` on a controller of `kind`, the face
    /// buttons first; `None` when no button does.
    pub fn button_for(&self, input: GamepadInput, kind: ControllerKind) -> Option<PadButton> {
        match input {
            GamepadInput::Up => Some(PadButton::DPadUp),
            GamepadInput::Down => Some(PadButton::DPadDown),
            GamepadInput::Left => Some(PadButton::DPadLeft),
            GamepadInput::Right => Some(PadButton::DPadRight),
            GamepadInput::Guide => Some(PadButton::Guide),
            _ => REBINDABLE
                .into_iter()
                .find(|b| self.input(*b, kind) == Some(input)),
        }
    }

    /// Whether some button confirms and some goes back, whatever the family.
    pub fn usable(&self) -> bool {
        [ControllerKind::Xbox, ControllerKind::Nintendo]
            .into_iter()
            .all(|kind| {
                self.button_for(GamepadInput::Confirm, kind).is_some()
                    && self.button_for(GamepadInput::Back, kind).is_some()
            })
    }

    /// The preferences with `job` given to `button`, which hands its own
    /// job to the button that had `job`, so nothing is lost. Buttons back at
    /// their own job are left out of `iroriController.buttons`. `None` when
    /// the result would leave nothing to confirm or go back with.
    pub fn assigning(
        prefs: &ControllerPrefs,
        job: GamepadInput,
        button: PadButton,
        kind: ControllerKind,
    ) -> Option<ControllerPrefs> {
        let map = ButtonMap::from_prefs(prefs);
        let previous = map.button_for(job, kind);
        let displaced = map.input(button, kind);
        let mut jobs: Vec<(PadButton, Option<GamepadInput>)> = REBINDABLE
            .into_iter()
            .map(|b| (b, map.input(b, kind)))
            .collect();
        for (b, j) in &mut jobs {
            if *b == button {
                *j = Some(job);
            } else if Some(*b) == previous {
                *j = displaced;
            }
        }
        let swap = map.swaps(kind);
        let mut next = prefs.clone();
        for (b, j) in jobs {
            if j == Some(base_input(b, swap)) {
                next.buttons.remove(b.id());
            } else {
                next.buttons.insert(
                    b.id().to_owned(),
                    j.map_or(NOTHING, GamepadInput::id).to_owned(),
                );
            }
        }
        ButtonMap::from_prefs(&next)
            .usable_as_written(&next)
            .then_some(next)
    }

    /// Whether every entry of `prefs` was kept (none refused as stranding).
    fn usable_as_written(&self, prefs: &ControllerPrefs) -> bool {
        prefs
            .buttons
            .keys()
            .all(|id| PadButton::from_id(id).is_none_or(|b| self.is_rebound(b)))
    }

    /// The preferences without rebound buttons.
    pub fn reset(prefs: &ControllerPrefs) -> ControllerPrefs {
        ControllerPrefs {
            buttons: Default::default(),
            ..prefs.clone()
        }
    }
}

static CURRENT: Lazy<RwLock<ButtonMap>> = Lazy::new(|| RwLock::new(ButtonMap::default()));

/// Use `map` from now on; returns whether it changed.
pub(crate) fn set(map: ButtonMap) -> bool {
    let mut current = CURRENT.write();
    if *current == map {
        return false;
    }
    *current = map;
    true
}

/// The map in use.
pub fn current() -> ButtonMap {
    CURRENT.read().clone()
}

/// The button giving `input` on a controller of `kind` with the map in use.
pub(crate) fn button_for(input: GamepadInput, kind: ControllerKind) -> Option<PadButton> {
    CURRENT.read().button_for(input, kind)
}

/// The job `button` has on a controller of `kind` with the map in use.
pub(crate) fn input(button: PadButton, kind: ControllerKind) -> Option<GamepadInput> {
    CURRENT.read().input(button, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(confirm: ConfirmButton, buttons: &[(&str, &str)]) -> ControllerPrefs {
        ControllerPrefs {
            confirm_button: confirm,
            buttons: buttons
                .iter()
                .map(|(b, j)| ((*b).to_owned(), (*j).to_owned()))
                .collect(),
            ..ControllerPrefs::default()
        }
    }

    #[test]
    fn the_confirm_button_follows_the_family_unless_chosen() {
        let auto = ButtonMap::default();
        let xbox = ControllerKind::Xbox;
        let switch = ControllerKind::Nintendo;
        assert_eq!(
            auto.input(PadButton::South, xbox),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(
            auto.input(PadButton::East, switch),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(
            auto.input(PadButton::South, switch),
            Some(GamepadInput::Back)
        );

        let south = ButtonMap::from_prefs(&prefs(ConfirmButton::South, &[]));
        assert_eq!(
            south.input(PadButton::South, switch),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(
            south.input(PadButton::East, switch),
            Some(GamepadInput::Back)
        );
        assert_eq!(
            south.input(PadButton::North, switch),
            Some(GamepadInput::Menu)
        );

        let east = ButtonMap::from_prefs(&prefs(ConfirmButton::East, &[]));
        assert_eq!(
            east.input(PadButton::East, xbox),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(east.input(PadButton::South, xbox), Some(GamepadInput::Back));
    }

    #[test]
    fn showing_nintendo_buttons_turns_the_face_buttons_round() {
        let shown = ButtonMap::from_prefs(&ControllerPrefs {
            glyphs: GlyphStyle::Nintendo,
            ..ControllerPrefs::default()
        });
        let xbox = ControllerKind::Xbox;
        assert_eq!(shown.display_kind(xbox), ControllerKind::Nintendo);
        assert_eq!(
            shown.input(PadButton::East, xbox),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(
            shown.button_for(GamepadInput::Confirm, xbox),
            Some(PadButton::East)
        );
        assert_eq!(ControllerKind::Nintendo.button_label(PadButton::East), "A");
    }

    #[test]
    fn rebound_buttons_take_their_new_job_and_hints_follow() {
        let map = ButtonMap::from_prefs(&prefs(
            ConfirmButton::Auto,
            &[("west", "menu"), ("north", "alt"), ("select", "none")],
        ));
        let xbox = ControllerKind::Xbox;
        assert_eq!(map.input(PadButton::West, xbox), Some(GamepadInput::Menu));
        assert_eq!(map.input(PadButton::North, xbox), Some(GamepadInput::Alt));
        assert_eq!(map.input(PadButton::Select, xbox), None);
        assert_eq!(map.input(PadButton::DPadUp, xbox), Some(GamepadInput::Up));
        assert_eq!(
            map.button_for(GamepadInput::Alt, xbox),
            Some(PadButton::North)
        );
        assert_eq!(
            map.button_for(GamepadInput::Menu, xbox),
            Some(PadButton::West)
        );
        assert!(map.is_rebound(PadButton::West));
        assert!(!map.is_rebound(PadButton::South));
    }

    #[test]
    fn unknown_entries_are_skipped() {
        let map = ButtonMap::from_prefs(&prefs(
            ConfirmButton::Auto,
            &[
                ("paddle_1", "confirm"),
                ("west", "teleport"),
                ("guide", "back"),
            ],
        ));
        assert_eq!(map, ButtonMap::default());
    }

    #[test]
    fn a_map_without_confirm_or_back_is_never_used() {
        let stranded = prefs(ConfirmButton::Auto, &[("south", "none"), ("east", "menu")]);
        let map = ButtonMap::from_prefs(&stranded);
        assert_eq!(
            map.input(PadButton::South, ControllerKind::Xbox),
            Some(GamepadInput::Confirm)
        );
        assert!(map.usable());
        let moved = prefs(
            ConfirmButton::Auto,
            &[("south", "back"), ("east", "confirm")],
        );
        assert!(ButtonMap::from_prefs(&moved).usable());
    }

    #[test]
    fn assigning_a_job_swaps_it_with_the_button_that_had_it() {
        let xbox = ControllerKind::Xbox;
        let start = ControllerPrefs::default();
        let swapped =
            ButtonMap::assigning(&start, GamepadInput::Alt, PadButton::North, xbox).unwrap();
        let map = ButtonMap::from_prefs(&swapped);
        assert_eq!(map.input(PadButton::North, xbox), Some(GamepadInput::Alt));
        assert_eq!(map.input(PadButton::West, xbox), Some(GamepadInput::Menu));
        assert_eq!(swapped.buttons.len(), 2);
        // Swapping back leaves nothing written.
        let back =
            ButtonMap::assigning(&swapped, GamepadInput::Alt, PadButton::West, xbox).unwrap();
        assert!(back.buttons.is_empty());
        // Confirm and Back can trade places; the map stays usable.
        let traded =
            ButtonMap::assigning(&start, GamepadInput::Confirm, PadButton::East, xbox).unwrap();
        let map = ButtonMap::from_prefs(&traded);
        assert_eq!(
            map.input(PadButton::East, xbox),
            Some(GamepadInput::Confirm)
        );
        assert_eq!(map.input(PadButton::South, xbox), Some(GamepadInput::Back));
        assert!(ButtonMap::reset(&traded).buttons.is_empty());
    }

    #[test]
    fn ids_round_trip() {
        for button in REBINDABLE {
            assert_eq!(PadButton::from_id(button.id()), Some(button));
        }
        for job in BINDABLE {
            assert_eq!(GamepadInput::from_id(job.id()), Some(job));
        }
        assert_eq!(PadButton::from_id("guide"), None, "Guide keeps its job");
    }
}
