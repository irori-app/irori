//! Raw Linux input events of a controller Irori holds from the game (see
//! `grab.rs`), turned into the same inputs gilrs would give. While a device
//! is grabbed gilrs sees nothing of it, so Irori reads its events itself.
//!
//! The kernel's gamepad codes name buttons by position (`BTN_SOUTH` is the
//! bottom face button on every family), as gilrs does, so [`map_button`]
//! applies unchanged. Only what the overlay needs is read: the bottom and
//! right face buttons, the shoulders, Guide, the D-pad (as buttons or as a
//! hat) and the left stick. Pure logic, tested without hardware.

use std::collections::BTreeSet;

use super::input::{AxisChange, ControllerKind, GamepadInput, PadButton, StickAxis, map_button};

pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_MODE: u16 = 0x13c;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;

/// The button a key code is, of those the overlay reads.
fn button(code: u16) -> Option<PadButton> {
    Some(match code {
        BTN_SOUTH => PadButton::South,
        BTN_EAST => PadButton::East,
        BTN_TL => PadButton::LeftShoulder,
        BTN_TR => PadButton::RightShoulder,
        BTN_SELECT => PadButton::Select,
        BTN_START => PadButton::Start,
        BTN_MODE => PadButton::Guide,
        BTN_DPAD_UP => PadButton::DPadUp,
        BTN_DPAD_DOWN => PadButton::DPadDown,
        BTN_DPAD_LEFT => PadButton::DPadLeft,
        BTN_DPAD_RIGHT => PadButton::DPadRight,
        _ => return None,
    })
}

/// A change of one input: pressed (`true`) or released.
pub type Transition = (GamepadInput, bool);

/// One grabbed controller's state.
#[derive(Debug, Clone)]
pub struct RawPad {
    kind: ControllerKind,
    /// The left stick's range on each axis, from the device.
    x_range: Option<(i32, i32)>,
    y_range: Option<(i32, i32)>,
    stick_x: StickAxis,
    stick_y: StickAxis,
    stick_held: [bool; 2],
    hat: (i32, i32),
    keys: BTreeSet<u16>,
}

impl RawPad {
    pub fn new(kind: ControllerKind) -> Self {
        Self {
            kind,
            x_range: None,
            y_range: None,
            stick_x: StickAxis::default(),
            stick_y: StickAxis::default(),
            stick_held: [false; 2],
            hat: (0, 0),
            keys: BTreeSet::new(),
        }
    }

    /// Give the left stick's range on `axis` (`ABS_X` or `ABS_Y`).
    pub fn set_range(&mut self, axis: u16, min: i32, max: i32) {
        if max <= min {
            return;
        }
        match axis {
            ABS_X => self.x_range = Some((min, max)),
            ABS_Y => self.y_range = Some((min, max)),
            _ => {}
        }
    }

    /// Whether anything is still pressed or deflected, so releasing the grab
    /// now would hand the game a release it never saw pressed.
    pub fn any_held(&self) -> bool {
        !self.keys.is_empty() || self.hat != (0, 0) || self.stick_held.iter().any(|h| *h)
    }

    /// Feed one event; returns the inputs it presses and releases.
    pub fn event(&mut self, kind: u16, code: u16, value: i32) -> Vec<Transition> {
        match kind {
            EV_KEY => self.key(code, value),
            EV_ABS => self.abs(code, value),
            _ => Vec::new(),
        }
    }

    fn key(&mut self, code: u16, value: i32) -> Vec<Transition> {
        let Some(button) = button(code) else {
            return Vec::new();
        };
        let input = map_button(button, self.kind);
        match value {
            // 2 is the kernel's autorepeat; directions repeat on Irori's timing.
            0 => {
                if self.keys.remove(&code) {
                    vec![(input, false)]
                } else {
                    Vec::new()
                }
            }
            1 => {
                if self.keys.insert(code) {
                    vec![(input, true)]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn abs(&mut self, code: u16, value: i32) -> Vec<Transition> {
        match code {
            ABS_HAT0X => {
                let old = self.hat.0;
                self.hat.0 = value.signum();
                hat_change(old, self.hat.0, GamepadInput::Left, GamepadInput::Right)
            }
            ABS_HAT0Y => {
                let old = self.hat.1;
                self.hat.1 = value.signum();
                // The hat's negative Y is up.
                hat_change(old, self.hat.1, GamepadInput::Up, GamepadInput::Down)
            }
            ABS_X | ABS_Y => {
                let horizontal = code == ABS_X;
                let range = if horizontal {
                    self.x_range
                } else {
                    self.y_range
                };
                let Some(normalized) = range.map(|r| normalize(value, r)) else {
                    return Vec::new();
                };
                // The kernel's Y grows downwards, gilrs's upwards.
                let changes = if horizontal {
                    self.stick_x.update(normalized)
                } else {
                    self.stick_y.update(-normalized)
                };
                let slot = usize::from(!horizontal);
                changes
                    .into_iter()
                    .map(|change| {
                        let (positive, pressed) = match change {
                            AxisChange::Press(p) => (p, true),
                            AxisChange::Release(p) => (p, false),
                        };
                        self.stick_held[slot] = pressed;
                        let input = match (horizontal, positive) {
                            (true, true) => GamepadInput::Right,
                            (true, false) => GamepadInput::Left,
                            (false, true) => GamepadInput::Up,
                            (false, false) => GamepadInput::Down,
                        };
                        (input, pressed)
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

/// `value` in `range` as `-1.0..=1.0`.
fn normalize(value: i32, (min, max): (i32, i32)) -> f32 {
    let span = (i64::from(max) - i64::from(min)) as f32;
    let offset = (i64::from(value) - i64::from(min)) as f32;
    (offset / span * 2.0 - 1.0).clamp(-1.0, 1.0)
}

/// What a hat axis moving from `old` to `new` (-1, 0 or 1) presses and
/// releases.
fn hat_change(
    old: i32,
    new: i32,
    negative: GamepadInput,
    positive: GamepadInput,
) -> Vec<Transition> {
    let of = |v: i32| match v {
        -1 => Some(negative),
        1 => Some(positive),
        _ => None,
    };
    if old == new {
        return Vec::new();
    }
    of(old)
        .map(|i| (i, false))
        .into_iter()
        .chain(of(new).map(|i| (i, true)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use GamepadInput::*;

    #[test]
    fn buttons_map_by_position_like_gilrs() {
        let mut pad = RawPad::new(ControllerKind::Xbox);
        assert_eq!(pad.event(EV_KEY, BTN_SOUTH, 1), [(Confirm, true)]);
        assert!(pad.any_held());
        assert_eq!(pad.event(EV_KEY, BTN_SOUTH, 2), []);
        assert_eq!(pad.event(EV_KEY, BTN_SOUTH, 0), [(Confirm, false)]);
        assert!(!pad.any_held());
        assert_eq!(pad.event(EV_KEY, BTN_SOUTH, 0), [], "no double release");
        assert_eq!(pad.event(EV_KEY, BTN_TR, 1), [(NextPage, true)]);
        assert_eq!(pad.event(EV_KEY, BTN_MODE, 1), [(Guide, true)]);
        assert_eq!(
            pad.event(EV_KEY, 0x2c0, 1),
            [],
            "a button the overlay ignores"
        );

        let mut nintendo = RawPad::new(ControllerKind::Nintendo);
        assert_eq!(nintendo.event(EV_KEY, BTN_EAST, 1), [(Confirm, true)]);
    }

    #[test]
    fn the_hat_presses_and_releases_directions() {
        let mut pad = RawPad::new(ControllerKind::PlayStation5);
        assert_eq!(pad.event(EV_ABS, ABS_HAT0Y, -1), [(Up, true)]);
        assert_eq!(pad.event(EV_ABS, ABS_HAT0Y, 1), [(Up, false), (Down, true)]);
        assert!(pad.any_held());
        assert_eq!(pad.event(EV_ABS, ABS_HAT0Y, 0), [(Down, false)]);
        assert_eq!(pad.event(EV_ABS, ABS_HAT0X, 1), [(Right, true)]);
        assert_eq!(pad.event(EV_ABS, ABS_HAT0X, 1), []);
        assert_eq!(pad.event(EV_ABS, ABS_HAT0X, 0), [(Right, false)]);
        assert!(!pad.any_held());
    }

    #[test]
    fn the_stick_needs_its_range_and_reads_y_upwards() {
        let mut pad = RawPad::new(ControllerKind::Xbox);
        assert_eq!(pad.event(EV_ABS, ABS_Y, -32768), [], "no range yet");
        pad.set_range(ABS_X, -32768, 32767);
        pad.set_range(ABS_Y, -32768, 32767);
        pad.set_range(ABS_HAT0X, 0, 0);
        assert_eq!(pad.event(EV_ABS, ABS_Y, -32768), [(Up, true)]);
        assert!(pad.any_held());
        assert_eq!(pad.event(EV_ABS, ABS_Y, -100), [(Up, false)]);
        assert!(!pad.any_held());
        // A stick reporting 0..255, centred at 128.
        pad.set_range(ABS_X, 0, 255);
        assert_eq!(pad.event(EV_ABS, ABS_X, 128), []);
        assert_eq!(pad.event(EV_ABS, ABS_X, 255), [(Right, true)]);
    }
}
