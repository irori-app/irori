//! A controller drawn as the family in hand: an Xbox-style pad (also used
//! for Steam Deck and other controllers), a PlayStation pad, or a Nintendo
//! Pro Controller, whose sticks, D-pad and touchpad sit where theirs do.
//!
//! The drawing is an SVG made from the theme's colours, with the buttons a
//! screen uses lit and one button ringed. GPUI rasterizes it at its
//! intrinsic size. It holds no text, so no fonts are loaded for it: labels
//! go over it at [`spot`], as fractions of the drawing's size.

use std::fmt::Write as _;

use super::input::{ControllerKind, PadButton};

/// Width of the drawing, in its own units.
pub const WIDTH: f32 = 1000.0;
/// Height of the drawing, in its own units.
pub const HEIGHT: f32 = 640.0;
/// Size GPUI rasterizes the drawing at, in pixels: sharp at twice the size
/// the settings page shows it.
const RASTER: (u32, u32) = (900, 576);

/// Where the parts of a family's controllers sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Left stick above the D-pad (Xbox, Steam Deck, most others).
    Offset,
    /// D-pad and face buttons above both sticks, a touchpad between.
    PlayStation,
    /// Like [`Shape::Offset`], with Nintendo's rounder buttons.
    Nintendo,
}

impl Shape {
    pub fn of(kind: ControllerKind) -> Shape {
        match kind {
            ControllerKind::PlayStation4 | ControllerKind::PlayStation5 => Shape::PlayStation,
            ControllerKind::Nintendo => Shape::Nintendo,
            _ => Shape::Offset,
        }
    }
}

/// A part of the drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Button(PadButton),
    LeftStick,
    RightStick,
}

/// Distance of each face button from the middle of the four.
const FACE: f32 = 58.0;
const FACE_R: f32 = 30.0;
const STICK_R: f32 = 52.0;

fn centres(shape: Shape) -> Centres {
    match shape {
        Shape::PlayStation => Centres {
            dpad: (290.0, 250.0),
            face: (710.0, 250.0),
            left_stick: (400.0, 400.0),
            right_stick: (600.0, 400.0),
            select: (365.0, 165.0),
            start: (635.0, 165.0),
            guide: (500.0, 335.0),
        },
        Shape::Offset | Shape::Nintendo => Centres {
            dpad: (395.0, 380.0),
            face: (710.0, 245.0),
            left_stick: (290.0, 245.0),
            right_stick: (605.0, 380.0),
            select: (430.0, 245.0),
            start: (570.0, 245.0),
            guide: (500.0, 175.0),
        },
    }
}

struct Centres {
    dpad: (f32, f32),
    face: (f32, f32),
    left_stick: (f32, f32),
    right_stick: (f32, f32),
    select: (f32, f32),
    start: (f32, f32),
    guide: (f32, f32),
}

/// The middle of `part`, in the drawing's units.
pub fn centre(shape: Shape, part: Part) -> (f32, f32) {
    let c = centres(shape);
    let (fx, fy) = c.face;
    let (dx, dy) = c.dpad;
    match part {
        Part::LeftStick => c.left_stick,
        Part::RightStick => c.right_stick,
        Part::Button(button) => match button {
            PadButton::South => (fx, fy + FACE),
            PadButton::East => (fx + FACE, fy),
            PadButton::West => (fx - FACE, fy),
            PadButton::North => (fx, fy - FACE),
            PadButton::LeftShoulder => (265.0, 92.0),
            PadButton::RightShoulder => (735.0, 92.0),
            PadButton::Select => c.select,
            PadButton::Start => c.start,
            PadButton::Guide => c.guide,
            PadButton::DPadUp => (dx, dy - 40.0),
            PadButton::DPadDown => (dx, dy + 40.0),
            PadButton::DPadLeft => (dx - 40.0, dy),
            PadButton::DPadRight => (dx + 40.0, dy),
        },
    }
}

/// Where a label for `part` goes, as fractions of the drawing's width and
/// height.
pub fn spot(shape: Shape, part: Part) -> (f32, f32) {
    let (x, y) = centre(shape, part);
    (x / WIDTH, y / HEIGHT)
}

/// Colours of the drawing, `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colours {
    pub body: String,
    pub outline: String,
    pub part: String,
    pub lit: String,
    pub ring: String,
}

/// The drawing of a `shape` controller with the `lit` buttons lit and
/// `ringed` circled.
pub fn svg(
    shape: Shape,
    colours: &Colours,
    lit: &[PadButton],
    ringed: Option<PadButton>,
) -> String {
    let fill = |button: PadButton| {
        if lit.contains(&button) {
            colours.lit.as_str()
        } else {
            colours.part.as_str()
        }
    };
    let (w, h) = RASTER;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {WIDTH} {HEIGHT}">"#
    );
    // Shoulders, behind the body.
    for button in [PadButton::LeftShoulder, PadButton::RightShoulder] {
        let (x, y) = centre(shape, Part::Button(button));
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="190" height="64" rx="30" fill="{}" stroke="{}" stroke-width="4"/>"#,
            x - 95.0,
            y - 30.0,
            fill(button),
            colours.outline
        );
    }
    let _ = write!(
        out,
        r#"<path d="M230 130C300 100 700 100 770 130C860 160 930 330 950 470C970 590 880 630 820 560C780 510 740 490 680 490L320 490C260 490 220 510 180 560C120 630 30 590 50 470C70 330 140 160 230 130Z" fill="{}" stroke="{}" stroke-width="4"/>"#,
        colours.body, colours.outline
    );
    if shape == Shape::PlayStation {
        let _ = write!(
            out,
            r#"<rect x="400" y="150" width="200" height="120" rx="18" fill="{}" stroke="{}" stroke-width="3"/>"#,
            colours.part, colours.outline
        );
    }
    for stick in [Part::LeftStick, Part::RightStick] {
        let (x, y) = centre(shape, stick);
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="{STICK_R}" fill="{}" stroke="{}" stroke-width="4"/><circle cx="{x}" cy="{y}" r="34" fill="{}"/>"#,
            colours.part, colours.outline, colours.body
        );
    }
    // The D-pad: one plus, lit as a whole.
    let (dx, dy) = centres(shape).dpad;
    let dpad_lit = [
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
    ]
    .iter()
    .any(|b| lit.contains(b));
    let _ = write!(
        out,
        r#"<path d="M{} {}h44v40h40v44h-40v40h-44v-40h-40v-44h40Z" fill="{}" stroke="{}" stroke-width="4"/>"#,
        dx - 22.0,
        dy - 62.0,
        if dpad_lit {
            colours.lit.as_str()
        } else {
            colours.part.as_str()
        },
        colours.outline
    );
    let radius = if shape == Shape::Nintendo {
        FACE_R + 2.0
    } else {
        FACE_R
    };
    for button in [
        PadButton::South,
        PadButton::East,
        PadButton::West,
        PadButton::North,
        PadButton::Guide,
    ] {
        let (x, y) = centre(shape, Part::Button(button));
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="{radius}" fill="{}" stroke="{}" stroke-width="4"/>"#,
            fill(button),
            colours.outline
        );
    }
    for button in [PadButton::Select, PadButton::Start] {
        let (x, y) = centre(shape, Part::Button(button));
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="52" height="30" rx="15" fill="{}" stroke="{}" stroke-width="4"/>"#,
            x - 26.0,
            y - 15.0,
            fill(button),
            colours.outline
        );
    }
    if let Some(button) = ringed {
        let (x, y) = centre(shape, Part::Button(button));
        let r = match button {
            PadButton::LeftShoulder | PadButton::RightShoulder => 70.0,
            PadButton::DPadUp
            | PadButton::DPadDown
            | PadButton::DPadLeft
            | PadButton::DPadRight => 76.0,
            _ => 44.0,
        };
        let (x, y) = match button {
            PadButton::DPadUp
            | PadButton::DPadDown
            | PadButton::DPadLeft
            | PadButton::DPadRight => (dx, dy),
            _ => (x, y),
        };
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="{r}" fill="none" stroke="{}" stroke-width="8"/>"#,
            colours.ring
        );
    }
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARTS: [Part; 15] = [
        Part::Button(PadButton::South),
        Part::Button(PadButton::East),
        Part::Button(PadButton::West),
        Part::Button(PadButton::North),
        Part::Button(PadButton::LeftShoulder),
        Part::Button(PadButton::RightShoulder),
        Part::Button(PadButton::Start),
        Part::Button(PadButton::Select),
        Part::Button(PadButton::Guide),
        Part::Button(PadButton::DPadUp),
        Part::Button(PadButton::DPadDown),
        Part::Button(PadButton::DPadLeft),
        Part::Button(PadButton::DPadRight),
        Part::LeftStick,
        Part::RightStick,
    ];

    #[test]
    fn labels_sit_inside_the_drawing_and_apart() {
        for shape in [Shape::Offset, Shape::PlayStation, Shape::Nintendo] {
            for (i, a) in PARTS.iter().enumerate() {
                let (x, y) = spot(shape, *a);
                assert!(
                    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                    "{a:?}"
                );
                for b in &PARTS[i + 1..] {
                    let (ax, ay) = centre(shape, *a);
                    let (bx, by) = centre(shape, *b);
                    let apart = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
                    assert!(apart >= 40.0, "{shape:?}: {a:?} and {b:?} overlap");
                }
            }
        }
    }

    #[test]
    fn the_drawing_lights_and_rings_what_it_is_asked_to() {
        let colours = Colours {
            body: "#202020".into(),
            outline: "#404040".into(),
            part: "#303030".into(),
            lit: "#3366ff".into(),
            ring: "#ffcc00".into(),
        };
        let plain = svg(Shape::Offset, &colours, &[], None);
        assert!(plain.starts_with("<svg") && plain.ends_with("</svg>"));
        assert!(!plain.contains("#3366ff") && !plain.contains("#ffcc00"));
        let lit = svg(
            Shape::PlayStation,
            &colours,
            &[PadButton::South, PadButton::DPadUp],
            Some(PadButton::East),
        );
        assert_eq!(
            lit.matches("#3366ff").count(),
            2,
            "the button and the D-pad"
        );
        assert_eq!(lit.matches("#ffcc00").count(), 1);
        assert!(lit.contains(r#"width="200""#), "the touchpad");
    }

    #[test]
    fn families_get_their_shape() {
        assert_eq!(Shape::of(ControllerKind::PlayStation5), Shape::PlayStation);
        assert_eq!(Shape::of(ControllerKind::Nintendo), Shape::Nintendo);
        assert_eq!(Shape::of(ControllerKind::SteamDeck), Shape::Offset);
    }
}
