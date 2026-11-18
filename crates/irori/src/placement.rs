//! Where the main window opens: the size, place, display and maximized
//! state it had when it last moved, kept on screen.
//!
//! The window's bounds are saved a second after they stop changing, off the
//! UI thread. When Irori starts, the saved display is found again by its
//! UUID; a window whose display is gone, or that would no longer fit on it
//! (a smaller TV, a higher scale), opens centred and no larger than the
//! display's visible area. Wayland doesn't let programs place their windows,
//! so there only the size, display and maximized state come back.
//!
//! With "Start maximized" (`iroriUi.startMaximized`) the window always opens
//! maximized, on the display it was last on. macOS reports a zoomed window
//! as an ordinary one of the screen's size, so the maximized state is read
//! from the window itself and the size it had before is kept to come back
//! to.

use gpui_kit::{App, Bounds, DisplayId, Pixels, Size, Window, WindowBounds, point, px, size};
use irori_core::settings::UiPrefs;

/// The smallest window Irori opens (its content's minimum is smaller).
const MIN: (f32, f32) = (960.0, 600.0);

/// Saved placement, as `UiPrefs` keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub size: (f32, f32),
    pub origin: Option<(f32, f32)>,
    pub display: String,
    pub maximized: bool,
}

impl Saved {
    pub fn from_prefs(prefs: &UiPrefs) -> Self {
        Self {
            size: (prefs.window_width, prefs.window_height),
            origin: prefs.window_x.zip(prefs.window_y),
            display: prefs.window_display.clone(),
            maximized: prefs.window_maximized,
        }
    }
}

/// A display as placement needs it: its UUID and visible area.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub uuid: String,
    pub visible: Rect,
}

/// A rectangle in global logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains(&self, inner: &Rect) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.x + inner.w <= self.x + self.w
            && inner.y + inner.h <= self.y + self.h
    }
}

/// Where to open: the display (index into `screens`, `None` for the
/// primary), the window's rectangle and whether it is maximized.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub screen: Option<usize>,
    pub rect: Rect,
    pub maximized: bool,
}

/// Decide where the window opens. `primary` is the index of the primary
/// display in `screens`; `positions` is false where the system places
/// windows itself (Wayland).
pub fn place(
    saved: &Saved,
    screens: &[Screen],
    primary: Option<usize>,
    positions: bool,
) -> Placement {
    let screen = screens
        .iter()
        .position(|s| !saved.display.is_empty() && s.uuid == saved.display)
        .or(primary);
    let area = screen.and_then(|i| screens.get(i)).map(|s| s.visible);
    let (mut w, mut h) = (saved.size.0.max(MIN.0), saved.size.1.max(MIN.1));
    if let Some(area) = area {
        // A display smaller than the window: fit it, below the minimum too.
        w = w.min(area.w);
        h = h.min(area.h);
    }
    let rect = match (area, saved.origin.filter(|_| positions)) {
        (Some(area), Some((x, y))) if area.contains(&Rect { x, y, w, h }) => Rect { x, y, w, h },
        (Some(area), _) => {
            let (cx, cy) = area.center();
            Rect {
                x: cx - w / 2.0,
                y: cy - h / 2.0,
                w,
                h,
            }
        }
        (None, _) => Rect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        },
    };
    Placement {
        screen,
        rect,
        maximized: saved.maximized,
    }
}

fn rect(b: Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        w: f32::from(b.size.width),
        h: f32::from(b.size.height),
    }
}

/// Whether this system lets Irori place its windows.
fn positions_honoured(cx: &App) -> bool {
    cx.compositor_name() != "Wayland"
}

/// The bounds and display to open the main window with.
pub fn open_bounds(prefs: &UiPrefs, cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let displays = cx.displays();
    let primary_id = cx.primary_display().map(|d| d.id());
    let screens: Vec<Screen> = displays
        .iter()
        .map(|d| Screen {
            uuid: d.uuid().map(|u| u.to_string()).unwrap_or_default(),
            visible: rect(d.visible_bounds()),
        })
        .collect();
    let primary = displays.iter().position(|d| Some(d.id()) == primary_id);
    let mut saved = Saved::from_prefs(prefs);
    saved.maximized |= prefs.start_maximized;
    let placement = place(&saved, &screens, primary, positions_honoured(cx));
    let display = placement
        .screen
        .and_then(|i| displays.get(i))
        .map(|d| d.id());
    let r = placement.rect;
    let bounds = Bounds {
        origin: point(px(r.x), px(r.y)),
        size: size(px(r.w), px(r.h)),
    };
    let bounds = if placement.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    (bounds, display)
}

/// What to save for the window as it is now, given what was saved before;
/// `None` in fullscreen (Big Picture), which isn't a place to come back to.
pub fn current(window: &Window, previous: &Saved, cx: &App) -> Option<Saved> {
    // Linux and Windows report a maximized window with the bounds it goes
    // back to; macOS reports it as windowed, at the size of the screen.
    let (bounds, maximized, restore_known) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, window.is_maximized(), false),
        WindowBounds::Maximized(b) => (b, true, true),
        WindowBounds::Fullscreen(_) => return None,
    };
    let r = rect(bounds);
    let Size { width, height } = bounds.size;
    let now = Saved {
        size: (f32::from(width), f32::from(height)),
        origin: positions_honoured(cx).then_some((r.x, r.y)),
        display: window
            .display(cx)
            .and_then(|d| d.uuid().ok())
            .map(|u| u.to_string())
            .unwrap_or_default(),
        maximized,
    };
    Some(remember(previous, now, restore_known))
}

/// `now`, keeping the size and place of `previous` while the window is
/// maximized and the system doesn't say what it goes back to, so leaving
/// the maximized state later restores the window's own size.
pub fn remember(previous: &Saved, now: Saved, restore_known: bool) -> Saved {
    if now.maximized && !restore_known {
        Saved {
            size: previous.size,
            origin: previous.origin,
            ..now
        }
    } else {
        now
    }
}

/// Write `saved` into the settings' window fields.
pub fn store(saved: &Saved, prefs: &mut UiPrefs) {
    prefs.window_width = saved.size.0;
    prefs.window_height = saved.size.1;
    prefs.window_x = saved.origin.map(|o| o.0);
    prefs.window_y = saved.origin.map(|o| o.1);
    prefs.window_display = saved.display.clone();
    prefs.window_maximized = saved.maximized;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(uuid: &str, x: f32, w: f32, h: f32) -> Screen {
        Screen {
            uuid: uuid.into(),
            visible: Rect { x, y: 0.0, w, h },
        }
    }

    fn saved(display: &str, origin: Option<(f32, f32)>) -> Saved {
        Saved {
            size: (1200.0, 800.0),
            origin,
            display: display.into(),
            maximized: false,
        }
    }

    #[test]
    fn a_zoomed_window_keeps_the_size_it_goes_back_to() {
        let before = saved("a", Some((100.0, 80.0)));
        let zoomed = Saved {
            size: (1920.0, 1055.0),
            origin: Some((0.0, 25.0)),
            display: "a".into(),
            maximized: true,
        };
        let kept = remember(&before, zoomed.clone(), false);
        assert!(kept.maximized);
        assert_eq!(kept.size, before.size);
        assert_eq!(kept.origin, before.origin);
        // Where the system gives the restore bounds, they are saved as given.
        assert_eq!(remember(&before, zoomed.clone(), true), zoomed);
        // A window that isn't maximized is saved as it is.
        let moved = saved("a", Some((300.0, 200.0)));
        assert_eq!(remember(&before, moved.clone(), false), moved);
    }

    #[test]
    fn the_saved_maximized_state_comes_back() {
        let screens = [screen("a", 0.0, 1920.0, 1080.0)];
        let mut s = saved("a", Some((100.0, 80.0)));
        s.maximized = true;
        assert!(place(&s, &screens, Some(0), true).maximized);
    }

    #[test]
    fn the_window_comes_back_where_it_was_on_its_display() {
        let screens = [
            screen("a", 0.0, 1920.0, 1080.0),
            screen("b", 1920.0, 2560.0, 1440.0),
        ];
        let p = place(&saved("b", Some((2000.0, 100.0))), &screens, Some(0), true);
        assert_eq!(p.screen, Some(1));
        assert_eq!(
            p.rect,
            Rect {
                x: 2000.0,
                y: 100.0,
                w: 1200.0,
                h: 800.0
            }
        );
    }

    #[test]
    fn a_missing_display_opens_centred_on_the_primary() {
        let screens = [screen("a", 0.0, 1920.0, 1080.0)];
        let p = place(
            &saved("gone", Some((3000.0, 100.0))),
            &screens,
            Some(0),
            true,
        );
        assert_eq!(p.screen, Some(0));
        assert_eq!((p.rect.x, p.rect.y), (360.0, 140.0));
    }

    #[test]
    fn a_window_off_its_display_is_centred_and_a_large_one_fits() {
        let screens = [screen("tv", 0.0, 1280.0, 720.0)];
        // Saved on a bigger screen, partly off this one.
        let mut s = saved("tv", Some((500.0, 300.0)));
        s.size = (1600.0, 1000.0);
        let p = place(&s, &screens, Some(0), true);
        assert_eq!(
            p.rect,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 720.0
            }
        );
    }

    #[test]
    fn wayland_keeps_size_and_display_but_not_the_place() {
        let screens = [
            screen("a", 0.0, 1920.0, 1080.0),
            screen("b", 1920.0, 2560.0, 1440.0),
        ];
        let p = place(&saved("b", Some((2000.0, 100.0))), &screens, Some(0), false);
        assert_eq!(p.screen, Some(1));
        assert_eq!((p.rect.x, p.rect.y), (1920.0 + 680.0, 320.0));
    }

    #[test]
    fn small_saved_sizes_grow_to_the_minimum_and_maximized_stays() {
        let mut s = saved("", None);
        s.size = (300.0, 200.0);
        s.maximized = true;
        let p = place(&s, &[], None, true);
        assert_eq!((p.rect.w, p.rect.h), MIN);
        assert!(p.maximized);
    }

    #[test]
    fn settings_round_trip() {
        let s = saved("b", Some((10.0, 20.0)));
        let mut prefs = UiPrefs::default();
        store(&s, &mut prefs);
        assert_eq!(Saved::from_prefs(&prefs), s);
    }
}
