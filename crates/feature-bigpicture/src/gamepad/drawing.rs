//! A controller drawn as the family in hand: an Xbox-style pad (also used
//! for Steam Deck and other controllers), a PlayStation pad, or a Nintendo
//! Pro Controller, whose sticks, D-pad and touchpad sit where theirs do.
//!
//! The drawing is an SVG made from the theme's colours, with the parts a
//! screen uses softly lit and the hovered or active part(s) highlighted and
//! ringed. GPUI rasterizes it at its intrinsic size. It holds no text, so no
//! fonts are loaded for it: labels go over it at [`spot`] (or [`label_spot`]
//! for compact pills), as fractions of the drawing's size.

use std::fmt::Write as _;

use super::input::{ControllerKind, PadButton};

/// Width of the drawing, in its own units.
pub const WIDTH: f32 = 1000.0;
/// Height of the drawing, in its own units.
pub const HEIGHT: f32 = 640.0;
/// Size GPUI rasterizes the drawing at, in pixels: sharp on HiDPI displays.
const RASTER: (u32, u32) = (960, 614);

/// Where the parts of a family's controllers sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Left stick above the D-pad (Xbox, Steam Deck, most others).
    Offset,
    /// D-pad and face buttons above both sticks, a touchpad between.
    PlayStation,
    /// Like [`Shape::Offset`], with Nintendo's rounder face buttons.
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
const FACE: f32 = 62.0;
const FACE_R: f32 = 34.0;
const STICK_R: f32 = 52.0;

fn centres(shape: Shape) -> Centres {
    match shape {
        Shape::PlayStation => Centres {
            dpad: (278.0, 252.0),
            face: (722.0, 252.0),
            left_stick: (392.0, 404.0),
            right_stick: (608.0, 404.0),
            select: (356.0, 178.0),
            start: (644.0, 178.0),
            guide: (500.0, 342.0),
        },
        Shape::Offset | Shape::Nintendo => Centres {
            dpad: (386.0, 386.0),
            face: (722.0, 248.0),
            left_stick: (278.0, 248.0),
            right_stick: (614.0, 386.0),
            select: (418.0, 252.0),
            start: (582.0, 252.0),
            guide: (500.0, 178.0),
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
            PadButton::DPadUp => (dx, dy - 42.0),
            PadButton::DPadDown => (dx, dy + 42.0),
            PadButton::DPadLeft => (dx - 42.0, dy),
            PadButton::DPadRight => (dx + 42.0, dy),
        },
    }
}

/// Where `part` sits, as fractions of the drawing's width and height.
pub fn spot(shape: Shape, part: Part) -> (f32, f32) {
    let (x, y) = centre(shape, part);
    (x / WIDTH, y / HEIGHT)
}

/// Where a text label for `part` goes, as fractions of the drawing's width and
/// height. Compact system pills (`Select`, `Start`, `Guide`) place their text
/// neatly underneath so names like "Options" or "View" never overflow the pill.
pub fn label_spot(shape: Shape, part: Part) -> (f32, f32) {
    let (x, y) = centre(shape, part);
    let y = match part {
        Part::Button(PadButton::Select | PadButton::Start) => y + 36.0,
        Part::Button(PadButton::Guide) => y + 50.0,
        _ => y,
    };
    (x / WIDTH, y / HEIGHT)
}

/// Colours of the drawing, `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colours {
    pub body: String,
    pub outline: String,
    pub part: String,
    /// Soft tint for mapped parts at rest.
    pub lit: String,
    /// Accent border for mapped parts at rest.
    pub lit_stroke: String,
    /// Vivid fill for hovered, selected, or held chord parts.
    pub active: String,
    /// Focus halo ring drawn around active parts.
    pub ring: String,
}

fn is_dpad(part: Part) -> bool {
    matches!(
        part,
        Part::Button(
            PadButton::DPadUp | PadButton::DPadDown | PadButton::DPadLeft | PadButton::DPadRight
        )
    )
}

/// The drawing of a `shape` controller with `lit` parts softly highlighted,
/// `active` parts vividly highlighted with a halo ring, and an optional `+`
/// chord indicator on `Guide` when `chord_guide` is true.
pub fn svg(
    shape: Shape,
    colours: &Colours,
    lit: &[Part],
    active: &[Part],
    chord_guide: bool,
) -> String {
    let style_of = |part: Part| -> (&str, &str) {
        let is_active =
            active.contains(&part) || (chord_guide && part == Part::Button(PadButton::Guide));
        if is_active {
            (colours.active.as_str(), colours.active.as_str())
        } else if lit.contains(&part) {
            (colours.lit.as_str(), colours.lit_stroke.as_str())
        } else {
            (colours.part.as_str(), colours.outline.as_str())
        }
    };

    let (w, h) = RASTER;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {WIDTH} {HEIGHT}">"#
    );

    // Subtle rear triggers behind the shoulder bumpers.
    let _ = write!(
        out,
        r#"<path d="M210 74C220 34 310 30 334 52L330 92L204 98Z" fill="{}" stroke="{}" stroke-width="3" opacity="0.72"/>"#,
        colours.part, colours.outline
    );
    let _ = write!(
        out,
        r#"<path d="M790 74C780 34 690 30 666 52L670 92L796 98Z" fill="{}" stroke="{}" stroke-width="3" opacity="0.72"/>"#,
        colours.part, colours.outline
    );

    // Shoulder bumpers, behind the body shell.
    for button in [PadButton::LeftShoulder, PadButton::RightShoulder] {
        let part = Part::Button(button);
        let (x, y) = centre(shape, part);
        let (fill, stroke) = style_of(part);
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="194" height="62" rx="30" fill="{fill}" stroke="{stroke}" stroke-width="4"/>"#,
            x - 97.0,
            y - 28.0
        );
    }

    // Controller body shell.
    let _ = write!(
        out,
        r#"<path d="M230 130C300 100 700 100 770 130C860 160 930 330 950 470C970 590 880 630 820 560C780 510 740 490 680 490L320 490C260 490 220 510 180 560C120 630 30 590 50 470C70 330 140 160 230 130Z" fill="{}" stroke="{}" stroke-width="4"/>"#,
        colours.body, colours.outline
    );

    // PlayStation center touchpad.
    if shape == Shape::PlayStation {
        let _ = write!(
            out,
            r#"<rect x="405" y="150" width="190" height="110" rx="16" fill="{}" stroke="{}" stroke-width="3"/>"#,
            colours.part, colours.outline
        );
    }

    // Left and right analog sticks (left stick lights and rings when Move is active).
    for stick in [Part::LeftStick, Part::RightStick] {
        let (x, y) = centre(shape, stick);
        let (fill, stroke) = style_of(stick);
        let inner_stroke = if active.contains(&stick) || lit.contains(&stick) {
            colours.body.as_str()
        } else {
            colours.outline.as_str()
        };
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="{STICK_R}" fill="{fill}" stroke="{stroke}" stroke-width="4"/><circle cx="{x}" cy="{y}" r="33" fill="none" stroke="{inner_stroke}" stroke-width="3" opacity="0.65"/>"#,
        );
    }

    // D-pad drawn as a single cross, lit and ringed as a whole.
    let (dx, dy) = centres(shape).dpad;
    let dpad_active = active.iter().copied().any(is_dpad);
    let dpad_lit = lit.iter().copied().any(is_dpad);
    let (dpad_fill, dpad_stroke) = if dpad_active {
        (colours.active.as_str(), colours.active.as_str())
    } else if dpad_lit {
        (colours.lit.as_str(), colours.lit_stroke.as_str())
    } else {
        (colours.part.as_str(), colours.outline.as_str())
    };
    let _ = write!(
        out,
        r#"<path d="M{} {}h46v40h40v46h-40v40h-46v-40h-40v-46h40Z" fill="{dpad_fill}" stroke="{dpad_stroke}" stroke-width="4" stroke-linejoin="round"/>"#,
        dx - 23.0,
        dy - 63.0
    );

    // Face buttons.
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
    ] {
        let part = Part::Button(button);
        let (x, y) = centre(shape, part);
        let (fill, stroke) = style_of(part);
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="{radius}" fill="{fill}" stroke="{stroke}" stroke-width="4"/>"#
        );
    }

    // Guide / PS / Home button with centered ring emblem and optional '+' chord badge.
    {
        let part = Part::Button(PadButton::Guide);
        let (x, y) = centre(shape, part);
        let (fill, stroke) = style_of(part);
        let emblem = if active.contains(&part) || chord_guide {
            colours.body.as_str()
        } else if lit.contains(&part) {
            colours.lit_stroke.as_str()
        } else {
            colours.outline.as_str()
        };
        let _ = write!(
            out,
            r#"<circle cx="{x}" cy="{y}" r="30" fill="{fill}" stroke="{stroke}" stroke-width="4"/><circle cx="{x}" cy="{y}" r="12" fill="none" stroke="{emblem}" stroke-width="3.5"/><circle cx="{x}" cy="{y}" r="4" fill="{emblem}"/>"#
        );
        if chord_guide {
            let bx = x + 28.0;
            let by = y - 24.0;
            let _ = write!(
                out,
                r#"<circle cx="{bx}" cy="{by}" r="15" fill="{}" stroke="{}" stroke-width="3"/><path d="M{} {by}h12M{bx} {}v12" stroke="{}" stroke-width="3.5" stroke-linecap="round"/>"#,
                colours.body,
                colours.active,
                bx - 6.0,
                by - 6.0,
                colours.active
            );
        }
    }

    // Select (View / Create / -) and Start (Menu / Options / +) capsules.
    for button in [PadButton::Select, PadButton::Start] {
        let part = Part::Button(button);
        let (x, y) = centre(shape, part);
        let (fill, stroke) = style_of(part);
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="44" height="22" rx="11" fill="{fill}" stroke="{stroke}" stroke-width="3.5"/>"#,
            x - 22.0,
            y - 11.0
        );
    }

    // Focus halo rings around every active part (deduplicating D-pad directions).
    let mut ringed_dpad = false;
    for &part in active {
        if is_dpad(part) {
            if !ringed_dpad {
                ringed_dpad = true;
                let _ = write!(
                    out,
                    r#"<circle cx="{dx}" cy="{dy}" r="78" fill="none" stroke="{}" stroke-width="7"/>"#,
                    colours.ring
                );
            }
            continue;
        }
        let (x, y) = centre(shape, part);
        let ring = colours.ring.as_str();
        let _ = match part {
            Part::LeftStick | Part::RightStick => write!(
                out,
                r#"<circle cx="{x}" cy="{y}" r="{}" fill="none" stroke="{ring}" stroke-width="7"/>"#,
                STICK_R + 12.0
            ),
            Part::Button(PadButton::LeftShoulder | PadButton::RightShoulder) => write!(
                out,
                r#"<rect x="{}" y="{}" width="212" height="80" rx="38" fill="none" stroke="{ring}" stroke-width="7"/>"#,
                x - 106.0,
                y - 37.0
            ),
            Part::Button(PadButton::Select | PadButton::Start) => write!(
                out,
                r#"<rect x="{}" y="{}" width="62" height="40" rx="20" fill="none" stroke="{ring}" stroke-width="6"/>"#,
                x - 31.0,
                y - 20.0
            ),
            Part::Button(_) => write!(
                out,
                r#"<circle cx="{x}" cy="{y}" r="48" fill="none" stroke="{ring}" stroke-width="7"/>"#
            ),
        };
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

    fn sample_colours() -> Colours {
        Colours {
            body: "#202020".into(),
            outline: "#404040".into(),
            part: "#303030".into(),
            lit: "#334466".into(),
            lit_stroke: "#6688cc".into(),
            active: "#3366ff".into(),
            ring: "#ffcc00".into(),
        }
    }

    #[test]
    fn labels_sit_inside_the_drawing_and_apart() {
        for shape in [Shape::Offset, Shape::PlayStation, Shape::Nintendo] {
            for (i, a) in PARTS.iter().enumerate() {
                let (x, y) = spot(shape, *a);
                let (lx, ly) = label_spot(shape, *a);
                assert!(
                    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                    "{a:?}"
                );
                assert!(
                    (0.0..=1.0).contains(&lx) && (0.0..=1.0).contains(&ly),
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
    fn the_drawing_lights_active_parts_left_stick_and_guide_chord() {
        let colours = sample_colours();
        let plain = svg(Shape::Offset, &colours, &[], &[], false);
        assert!(plain.starts_with("<svg") && plain.ends_with("</svg>"));
        assert!(
            !plain.contains("#334466") && !plain.contains("#3366ff") && !plain.contains("#ffcc00")
        );

        let lit_svg = svg(
            Shape::PlayStation,
            &colours,
            &[
                Part::Button(PadButton::South),
                Part::Button(PadButton::DPadUp),
                Part::LeftStick,
            ],
            &[Part::Button(PadButton::DPadUp), Part::LeftStick],
            false,
        );
        // South is softly lit; D-pad and LeftStick are active and both ringed.
        assert_eq!(lit_svg.matches("#334466").count(), 1, "only South at rest");
        assert_eq!(
            lit_svg.matches("#ffcc00").count(),
            2,
            "both D-pad and LeftStick get focus halos"
        );
        assert!(lit_svg.contains(r#"width="190""#), "PlayStation touchpad");

        let chord_svg = svg(
            Shape::Offset,
            &colours,
            &[
                Part::Button(PadButton::Guide),
                Part::Button(PadButton::West),
            ],
            &[],
            true,
        );
        assert!(
            chord_svg.contains("#3366ff"),
            "Guide highlighted in chord mode"
        );
        assert!(
            chord_svg.contains(r#"r="15""#),
            "plus badge shown next to Guide"
        );
    }

    #[test]
    fn families_get_their_shape() {
        assert_eq!(Shape::of(ControllerKind::PlayStation5), Shape::PlayStation);
        assert_eq!(Shape::of(ControllerKind::Nintendo), Shape::Nintendo);
        assert_eq!(Shape::of(ControllerKind::SteamDeck), Shape::Offset);
    }
}
