//! What the in-game overlay can do on each system, in one table so the
//! panel, the settings and the docs say the same thing.
//!
//! The panel and its tabs are the same everywhere; what differs is what the
//! system lets a program do above another one:
//!
//! - **Holding the controller** from the game needs a grab only Linux's
//!   evdev offers. Windows has no exclusive mode for XInput or
//!   Windows.Gaming.Input pads, and seizing a HID device on macOS takes it
//!   from Irori's own reader too, so there the game also sees the presses
//!   made in the overlay.
//! - **A game's own volume** needs per-program volume: PulseAudio/PipeWire
//!   streams on Linux, Core Audio sessions on Windows. macOS has no
//!   per-program volume, so only the system's changes there.
//! - **A keyboard shortcut** opens the overlay where a program may register
//!   one for the whole desktop without extra permissions: `RegisterHotKey`
//!   on Windows, `RegisterEventHotKey` on macOS. Linux has the Guide button
//!   only (a portal shortcut would ask the user to bind it first).

/// A system Irori runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Os {
    /// The system this build runs on.
    pub const fn current() -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }
}

/// The keyboard shortcut that opens and closes the overlay while a game
/// Irori started runs: Ctrl+Shift+O (Control+Shift+O on a Mac). Only
/// registered while such a game runs, so other programs keep it otherwise.
pub const SHORTCUT_KEY: char = 'O';

/// What the overlay does on one system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    /// The overlay is offered at all.
    pub available: bool,
    /// The controllers are held from the game while it is open.
    pub holds_controllers: bool,
    /// The game's own volume can be changed, not only the system's.
    pub game_volume: bool,
    /// Ctrl+Shift+O opens it too.
    pub shortcut: bool,
}

impl Support {
    /// The table.
    pub const fn of(os: Os) -> Support {
        match os {
            Os::Linux => Support {
                available: true,
                holds_controllers: true,
                game_volume: true,
                shortcut: false,
            },
            Os::Windows => Support {
                available: true,
                holds_controllers: false,
                game_volume: true,
                shortcut: true,
            },
            Os::MacOs => Support {
                available: true,
                holds_controllers: false,
                game_volume: false,
                shortcut: true,
            },
            Os::Other => Support {
                available: false,
                holds_controllers: false,
                game_volume: false,
                shortcut: false,
            },
        }
    }

    /// This system's row.
    pub const fn current() -> Support {
        Support::of(Os::current())
    }
}

/// The shortcut as this system writes it.
pub fn shortcut_label(os: Os) -> String {
    match os {
        Os::MacOs => format!("⌃⇧{SHORTCUT_KEY}"),
        _ => format!("Ctrl+Shift+{SHORTCUT_KEY}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_each_system_offers() {
        let linux = Support::of(Os::Linux);
        let windows = Support::of(Os::Windows);
        let mac = Support::of(Os::MacOs);
        assert!(linux.available && windows.available && mac.available);
        assert!(!Support::of(Os::Other).available);
        // Only evdev can hold a controller from the game.
        assert!(linux.holds_controllers);
        assert!(!windows.holds_controllers && !mac.holds_controllers);
        // macOS has no per-program volume, and the services agree.
        assert!(linux.game_volume && windows.game_volume && !mac.game_volume);
        assert_eq!(
            Support::current().game_volume,
            irori_services::audio::PER_GAME
        );
        // A desktop-wide shortcut where one needs no extra permission.
        assert!(!linux.shortcut && windows.shortcut && mac.shortcut);
        assert_eq!(shortcut_label(Os::Windows), "Ctrl+Shift+O");
        assert_eq!(shortcut_label(Os::MacOs), "⌃⇧O");
    }
}
