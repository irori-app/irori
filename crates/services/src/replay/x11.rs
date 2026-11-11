//! The clip and screenshot keys on an X11 session without the desktop
//! portal's global shortcuts: Irori grabs Alt+F10 and Alt+F12 on the root
//! window (`XGrabKey`), so the X server hands them to Irori whichever window
//! has the focus. Only the X server is asked: keyboards are never read from
//! `/dev/input`. A key another program already grabbed fails the whole grab,
//! and Irori falls back to the commands the user binds instead.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use tokio::sync::mpsc::UnboundedReceiver;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask};

/// What a grabbed key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Clip,
    Screenshot,
}

/// X keysyms of the keys Irori grabs (`X11/keysymdef.h`).
const XK_F10: u32 = 0xffc7;
const XK_F12: u32 = 0xffc9;

/// A key held down repeats; a second clip a moment later is never meant.
const REPEAT_GAP: Duration = Duration::from_millis(800);

/// The keycode that types `keysym` unshifted, from the server's keyboard
/// mapping (`per_keycode` keysyms for each keycode from `min`).
pub(crate) fn keycode_for(keysyms: &[u32], per_keycode: u8, min: u8, keysym: u32) -> Option<u8> {
    let per = usize::from(per_keycode).max(1);
    keysyms
        .chunks(per)
        .position(|syms| syms.first() == Some(&keysym))
        .and_then(|i| u8::try_from(usize::from(min) + i).ok())
}

/// The lock modifiers the user may have on, which the server treats as part
/// of the combination: Caps Lock and (usually) Num Lock.
fn lock_variants() -> [ModMask; 4] {
    [
        ModMask::from(0u16),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::LOCK | ModMask::M2,
    ]
}

/// Lets one press of each action through per [`REPEAT_GAP`].
#[derive(Debug, Default)]
pub(crate) struct Debounce {
    last: [Option<Instant>; 2],
}

impl Debounce {
    pub(crate) fn accept(&mut self, action: Action, now: Instant) -> bool {
        let slot = &mut self.last[action as usize];
        if slot.is_some_and(|at| now.saturating_duration_since(at) < REPEAT_GAP) {
            return false;
        }
        *slot = Some(now);
        true
    }
}

/// Grab the keys on the X server named by `DISPLAY` and listen for them on
/// a thread of their own, which blocks until the server sends an event and
/// ends when the receiver is dropped or the server goes away.
pub(crate) fn grab() -> Result<UnboundedReceiver<Action>> {
    let (conn, screen) = x11rb::connect(None).context("connecting to the X server")?;
    let setup = conn.setup();
    let root = setup.roots.get(screen).context("no X screen")?.root;
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn
        .get_keyboard_mapping(min, max.saturating_sub(min).saturating_add(1))?
        .reply()
        .context("reading the keyboard mapping")?;
    let code = |keysym| {
        keycode_for(&mapping.keysyms, mapping.keysyms_per_keycode, min, keysym)
            .context("the keyboard has no F10 or F12 key")
    };
    let (clip, screenshot) = (code(XK_F10)?, code(XK_F12)?);
    for key in [clip, screenshot] {
        for locks in lock_variants() {
            conn.grab_key(
                false,
                root,
                ModMask::M1 | locks,
                key,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .check()
            .context("another program holds the keys")?;
        }
    }
    conn.flush()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("irori-x11-keys".into())
        .spawn(move || {
            let mut debounce = Debounce::default();
            while let Ok(event) = conn.wait_for_event() {
                let Event::KeyPress(press) = event else {
                    continue;
                };
                let action = match press.detail {
                    k if k == clip => Action::Clip,
                    k if k == screenshot => Action::Screenshot,
                    _ => continue,
                };
                if debounce.accept(action, Instant::now()) && tx.send(action).is_err() {
                    break;
                }
            }
        })
        .context("starting the key listener")?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycodes_come_from_the_mapping() {
        // Keycodes 8.. with two keysyms each: F10 at 76, F12 at 96.
        let mut keysyms = vec![0; 2 * 100];
        keysyms[2 * (76 - 8)] = XK_F10;
        keysyms[2 * (96 - 8)] = XK_F12;
        keysyms[2 * (97 - 8) + 1] = 0xffca; // only shifted: never picked
        assert_eq!(keycode_for(&keysyms, 2, 8, XK_F10), Some(76));
        assert_eq!(keycode_for(&keysyms, 2, 8, XK_F12), Some(96));
        assert_eq!(keycode_for(&keysyms, 2, 8, 0xffca), None);
        assert_eq!(keycode_for(&[], 0, 8, XK_F10), None);
    }

    #[test]
    fn a_held_key_saves_once() {
        let mut debounce = Debounce::default();
        let start = Instant::now();
        assert!(debounce.accept(Action::Clip, start));
        assert!(!debounce.accept(Action::Clip, start + Duration::from_millis(30)));
        // The other action has its own pace.
        assert!(debounce.accept(Action::Screenshot, start + Duration::from_millis(40)));
        assert!(debounce.accept(Action::Clip, start + REPEAT_GAP));
        assert!(!debounce.accept(Action::Screenshot, start + Duration::from_millis(100)));
    }

    #[test]
    fn every_lock_combination_is_grabbed() {
        let masks: Vec<u16> = lock_variants().iter().map(|m| u16::from(*m)).collect();
        assert_eq!(masks, [0, 2, 16, 18]);
    }
}
