//! Back + Start for the in-game overlay on Windows (`iroriOverlay.backStart`).
//!
//! Windows.Gaming.Input, which the reader thread uses, reports nothing to a
//! program in the background, and no documented Windows API reports the
//! Guide button. So while a game Irori started runs (and the overlay and
//! the chord are on), a thread asks XInput, which answers whichever window
//! is in front, for Back + Start pressed together on any controller, and
//! opens or closes the overlay. The thread waits without polling when that
//! isn't wanted, rescans every second while no controller is connected and
//! reads every [`POLL`] while one is: a chord is held far longer than that.

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use parking_lot::{Condvar, Mutex};

use super::input::BackStart;
use super::reader::Message;

/// How often connected controllers are read.
const POLL: Duration = Duration::from_millis(50);
/// How often the slots are looked at while no controller is connected.
const RESCAN: Duration = Duration::from_secs(1);

/// Whether the thread should read the controllers now.
#[derive(Default)]
pub(crate) struct Wanted {
    on: Mutex<bool>,
    changed: Condvar,
}

impl Wanted {
    pub(crate) fn set(&self, on: bool) {
        let mut now = self.on.lock();
        if *now != on {
            *now = on;
            self.changed.notify_all();
        }
    }

    fn wait(&self) {
        let mut on = self.on.lock();
        while !*on {
            self.changed.wait(&mut on);
        }
    }

    fn is_on(&self) -> bool {
        *self.on.lock()
    }
}

/// The buttons held on each of XInput's four slots; `None` for an empty
/// slot.
fn read() -> [Option<u16>; 4] {
    use windows_sys::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};
    let mut out = [None; 4];
    for (slot, buttons) in out.iter_mut().enumerate() {
        // SAFETY: XInputGetState writes one XINPUT_STATE into the local.
        let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
        if unsafe { XInputGetState(slot as u32, &mut state) } == 0 {
            *buttons = Some(state.Gamepad.wButtons);
        }
    }
    out
}

/// Start the thread; `wanted` turns its reading on and off.
pub(crate) fn start(wanted: Arc<Wanted>, tx: UnboundedSender<Message>) {
    let spawned = std::thread::Builder::new()
        .name("irori-xinput".into())
        .spawn(move || {
            let mut chord = BackStart::default();
            loop {
                wanted.wait();
                let slots = read();
                for (slot, buttons) in slots.iter().enumerate() {
                    if chord.update(slot, buttons.unwrap_or(0)) {
                        let _ = tx.unbounded_send(Message::BackStart);
                    }
                }
                let connected = slots.iter().any(Option::is_some);
                std::thread::sleep(if connected { POLL } else { RESCAN });
                if !wanted.is_on() {
                    // Presses held across a pause start again from nothing.
                    chord = BackStart::default();
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("starting the Back + Start reader: {e}");
    }
}
