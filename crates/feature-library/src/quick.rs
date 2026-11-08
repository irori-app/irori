//! Which of the library header's quick controls fit (`iroriUi.libraryQuickControls`).
//!
//! The header measures the room between its title and its buttons, and each
//! control's own width whenever it is shown. Controls join in a fixed order
//! while the next one fits, so a longer language (German, Russian) simply
//! shows fewer of them instead of colliding with the title. There are no
//! breakpoints: everything here is in measured pixels.
//!
//! Only filters the toolbar keeps behind its Filters popover are here: the
//! sort and the grouping already sit in the toolbar, and a second copy of
//! them made the header read as cluttered (issue #142).

/// One quick control, in the order they are added as room allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickControl {
    /// All / Installed / Not installed.
    Install,
    /// Favourites only.
    Favourites,
}

/// How many quick controls there are.
pub const COUNT: usize = 2;

impl QuickControl {
    /// Every control, in the order they are shown and the reverse of the
    /// order they are hidden in.
    pub const ORDER: [QuickControl; COUNT] = [QuickControl::Install, QuickControl::Favourites];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// How many of [`QuickControl::ORDER`] fit in `room` pixels with `gap`
/// pixels between neighbours. `widths` holds each control's last measured
/// width; a control never measured counts as fitting, so it is drawn once
/// and measured (its strip clips it meanwhile). Controls fit as a prefix:
/// a narrow later control never skips ahead of a wide earlier one.
pub fn fitting(room: f32, widths: &[Option<f32>; COUNT], gap: f32) -> usize {
    if room <= 0.0 {
        return 0;
    }
    let mut used = 0.0;
    for (ix, width) in widths.iter().enumerate() {
        let Some(width) = *width else {
            continue;
        };
        let needed = used + if ix > 0 { gap } else { 0.0 } + width;
        if needed > room {
            return ix;
        }
        used = needed;
    }
    widths.len()
}

/// The measurements the header keeps between frames.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuickLayout {
    /// Room for the controls, in pixels; zero until first measured.
    pub room: f32,
    /// Each control's width the last time it was shown.
    pub widths: [Option<f32>; COUNT],
    /// The interface language the widths were measured in: another one
    /// measures again.
    pub language: String,
}

impl QuickLayout {
    /// The controls to draw this frame.
    pub fn shown(&self, gap: f32) -> &'static [QuickControl] {
        &QuickControl::ORDER[..fitting(self.room, &self.widths, gap)]
    }

    /// Record the room measured; true when it changed enough to matter.
    pub fn set_room(&mut self, room: f32) -> bool {
        changed(&mut self.room, room)
    }

    /// Record a control's measured width; true when it changed.
    pub fn set_width(&mut self, control: QuickControl, width: f32) -> bool {
        let slot = &mut self.widths[control.index()];
        match slot {
            Some(old) => changed(old, width),
            None => {
                *slot = Some(width);
                true
            }
        }
    }

    /// Forget the widths when the language changed, since every label did.
    pub fn follow_language(&mut self, language: &str) {
        if self.language != language {
            self.language = language.to_owned();
            self.widths = [None; COUNT];
        }
    }
}

/// Store `new` in `old` unless it is within half a pixel (subpixel layout
/// noise must not cause another frame).
fn changed(old: &mut f32, new: f32) -> bool {
    if (*old - new).abs() > 0.5 {
        *old = new;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: [Option<f32>; COUNT] = [Some(240.0), Some(110.0)];

    #[test]
    fn controls_join_in_order_as_room_allows() {
        assert_eq!(fitting(0.0, &W, 8.0), 0);
        assert_eq!(fitting(239.0, &W, 8.0), 0);
        assert_eq!(fitting(240.0, &W, 8.0), 1);
        // The gap counts between neighbours only.
        assert_eq!(fitting(240.0 + 8.0 + 110.0, &W, 8.0), 2);
        assert_eq!(fitting(240.0 + 8.0 + 109.0, &W, 8.0), 1);
        assert_eq!(fitting(10_000.0, &W, 8.0), 2);
    }

    #[test]
    fn a_narrow_control_never_skips_a_wide_one() {
        // Favourites alone would fit, but Install comes first.
        let widths = [Some(400.0), Some(50.0)];
        assert_eq!(fitting(300.0, &widths, 8.0), 0);
    }

    #[test]
    fn unmeasured_controls_are_drawn_to_be_measured() {
        assert_eq!(fitting(300.0, &[None; COUNT], 8.0), 2);
        let widths = [None, Some(110.0)];
        assert_eq!(fitting(8.0 + 110.0, &widths, 8.0), 2);
        // Nothing is drawn before the room is known.
        assert_eq!(fitting(0.0, &[None; COUNT], 8.0), 0);
    }

    #[test]
    fn longer_labels_show_fewer_controls() {
        let mut layout = QuickLayout {
            room: 360.0,
            widths: W,
            language: "en".into(),
        };
        assert_eq!(
            layout.shown(8.0),
            &[QuickControl::Install, QuickControl::Favourites]
        );
        // German measured: the same room holds one.
        layout.follow_language("de");
        assert_eq!(layout.widths, [None; COUNT]);
        for (control, width) in QuickControl::ORDER.into_iter().zip([330.0, 120.0]) {
            assert!(layout.set_width(control, width));
        }
        assert_eq!(layout.shown(8.0), &[QuickControl::Install]);
        layout.follow_language("de");
        assert!(layout.widths[0].is_some(), "same language keeps widths");
    }

    #[test]
    fn subpixel_noise_is_not_a_change() {
        let mut layout = QuickLayout::default();
        assert!(layout.set_room(400.0));
        assert!(!layout.set_room(400.3));
        assert!(layout.set_room(401.0));
        assert!(layout.set_width(QuickControl::Favourites, 90.0));
        assert!(!layout.set_width(QuickControl::Favourites, 90.2));
        assert!(layout.set_width(QuickControl::Favourites, 92.0));
    }

    #[test]
    fn nothing_repeats_the_toolbar() {
        // The toolbar always shows Sort and Group; the header must not.
        assert_eq!(
            QuickControl::ORDER,
            [QuickControl::Install, QuickControl::Favourites]
        );
    }
}
