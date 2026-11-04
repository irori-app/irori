//! The editor's draft, apart from the screen: the rows it offers, the
//! values each row steps through, and how a step changes the draft.

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use irori_ui::i18n::t;

use crate::layout::HomeLayout;
use crate::model::cycle;
use crate::theme::{
    Background, Corners, CoverShape, CoverSize, FontChoice, Picture, Rgb, ShelfKind, Theme,
};

/// Background colours the editor offers.
const BACKGROUNDS: [Rgb; 9] = [
    Rgb([0x0b, 0x0d, 0x12]),
    Rgb([0x16, 0x10, 0x0d]),
    Rgb([0x0a, 0x0b, 0x1f]),
    Rgb([0x0e, 0x17, 0x12]),
    Rgb([0x0b, 0x1a, 0x23]),
    Rgb([0x1b, 0x1b, 0x1f]),
    Rgb([0x05, 0x05, 0x05]),
    Rgb([0xf3, 0xed, 0xe2]),
    Rgb([0xee, 0xf1, 0xf5]),
];

/// Text colours the editor offers (each is held legible anyway).
const TEXTS: [Rgb; 5] = [
    Rgb([0xf5, 0xf5, 0xf5]),
    Rgb([0xf6, 0xe9, 0xdf]),
    Rgb([0xe8, 0xf6, 0xfb]),
    Rgb([0x2b, 0x24, 0x1e]),
    Rgb([0x11, 0x13, 0x18]),
];

/// Accent colours the editor offers: twelve hues round the wheel.
fn accents() -> Vec<Rgb> {
    (0..12)
        .map(|i| Rgb::from_hsl(i as f32 * 30.0 + 10.0, 0.85, 0.6))
        .collect()
}

/// The colour after (or before) `current` among "the theme's own" (`None`)
/// and `options`. A colour that isn't offered (from a file) counts as the
/// theme's own, so the next step lands on the first offer.
pub(super) fn step_color(current: Option<Rgb>, options: &[Rgb], forward: bool) -> Option<Rgb> {
    let mut all: Vec<Option<Rgb>> = vec![None];
    all.extend(options.iter().copied().map(Some));
    let ix = all.iter().position(|o| *o == current).unwrap_or(0);
    let len = all.len();
    all[if forward {
        (ix + 1) % len
    } else {
        (ix + len - 1) % len
    }]
}

/// `current` moved one place in `options`, wrapping.
pub(super) fn step<T: PartialEq + Copy>(options: &[T], current: T, forward: bool) -> T {
    if forward {
        return cycle(options, current);
    }
    let len = options.len().max(1);
    let ix = options.iter().position(|o| *o == current).unwrap_or(0);
    options
        .get((ix + len - 1) % len)
        .copied()
        .unwrap_or(current)
}

/// A gradient from the theme's colours: its background into a deep shade
/// of its accent.
pub(super) fn gradient_for(theme: &Theme) -> Background {
    let from = theme.palette.background.unwrap_or(BACKGROUNDS[0]);
    let hue = theme
        .palette
        .accent
        .map(|a| a.hsla().h * 360.0)
        .unwrap_or(270.0);
    let light = from.hsla().l > 0.5;
    let to = Rgb::from_hsl(hue, 0.5, if light { 0.82 } else { 0.22 });
    Background::Gradient {
        from,
        to,
        angle: 160,
    }
}

/// A row of the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::view) enum EditorRow {
    Theme,
    Layout,
    Accent,
    BackgroundColor,
    TextColor,
    Backdrop,
    ChoosePicture,
    Font,
    Shape,
    Size,
    Corners,
    Shelf(ShelfKind),
    Name,
    Import,
    Export,
    Delete,
}

impl EditorRow {
    pub(super) fn id(self) -> SharedString {
        match self {
            EditorRow::Shelf(kind) => format!("bp-editor-shelf-{}", kind.id()).into(),
            other => format!("bp-editor-{other:?}").to_lowercase().into(),
        }
    }

    pub(super) fn label(self) -> SharedString {
        match self {
            EditorRow::Shelf(kind) => kind.label(),
            EditorRow::Theme => t("bigpicture.editor.theme"),
            EditorRow::Layout => t("bigpicture.editor.layout"),
            EditorRow::Accent => t("bigpicture.editor.accent"),
            EditorRow::BackgroundColor => t("bigpicture.editor.background_color"),
            EditorRow::TextColor => t("bigpicture.editor.text_color"),
            EditorRow::Backdrop => t("bigpicture.editor.backdrop"),
            EditorRow::ChoosePicture => t("bigpicture.editor.choose_picture"),
            EditorRow::Font => t("bigpicture.editor.font"),
            EditorRow::Shape => t("bigpicture.editor.cover_shape"),
            EditorRow::Size => t("bigpicture.editor.cover_size"),
            EditorRow::Corners => t("bigpicture.editor.corners"),
            EditorRow::Name => t("bigpicture.editor.name"),
            EditorRow::Import => t("bigpicture.editor.import"),
            EditorRow::Export => t("bigpicture.editor.export"),
            EditorRow::Delete => t("bigpicture.editor.delete"),
        }
    }

    /// Whether Left and Right change the row's value.
    pub fn has_value(self) -> bool {
        !matches!(
            self,
            EditorRow::ChoosePicture
                | EditorRow::Name
                | EditorRow::Import
                | EditorRow::Export
                | EditorRow::Delete
        )
    }

    pub(super) fn icon(self) -> Option<IconName> {
        Some(match self {
            EditorRow::ChoosePicture => IconName::GalleryVerticalEnd,
            EditorRow::Name => IconName::Replace,
            EditorRow::Import => IconName::FolderOpen,
            EditorRow::Export => IconName::ExternalLink,
            EditorRow::Delete => IconName::Trash,
            _ => return None,
        })
    }
}

/// Change the value of `row` of a draft one step; `picture` is the one the
/// backdrop may switch to. Returns whether anything changed.
pub(super) fn change_draft(
    theme: &mut Theme,
    layout: &mut HomeLayout,
    picture: Option<&Picture>,
    row: EditorRow,
    forward: bool,
) -> bool {
    let before = (theme.clone(), *layout);
    match row {
        EditorRow::Layout => *layout = step(&HomeLayout::ALL, *layout, forward),
        EditorRow::Accent => {
            theme.palette.accent = step_color(theme.palette.accent, &accents(), forward)
        }
        EditorRow::BackgroundColor => {
            theme.palette.background = step_color(theme.palette.background, &BACKGROUNDS, forward);
            // Panels follow the new background.
            theme.palette.surface = None;
        }
        EditorRow::TextColor => {
            theme.palette.foreground = step_color(theme.palette.foreground, &TEXTS, forward)
        }
        EditorRow::Backdrop => {
            let mut kinds = vec!["art", "gradient", "solid"];
            if picture.is_some() {
                kinds.push("picture");
            }
            theme.background = match step(&kinds, theme.background.kind(), forward) {
                "gradient" => gradient_for(theme),
                "solid" => Background::Solid,
                "picture" => picture
                    .cloned()
                    .map_or(Background::Art, Background::Picture),
                _ => Background::Art,
            };
        }
        EditorRow::Font => theme.font = step(&FontChoice::ALL, theme.font, forward),
        EditorRow::Shape => {
            theme.covers.shape = step(&CoverShape::ALL, theme.covers.shape, forward)
        }
        EditorRow::Size => theme.covers.size = step(&CoverSize::ALL, theme.covers.size, forward),
        EditorRow::Corners => {
            theme.covers.corners = step(&Corners::ALL, theme.covers.corners, forward)
        }
        EditorRow::Shelf(kind) => match theme.shelves.iter().position(|s| *s == kind) {
            Some(ix) => {
                theme.shelves.remove(ix);
            }
            None => {
                // A shelf turned on goes where the editor lists it.
                let order = |k: &ShelfKind| ShelfKind::ALL.iter().position(|a| a == k);
                let at = theme
                    .shelves
                    .iter()
                    .position(|s| order(s) > order(&kind))
                    .unwrap_or(theme.shelves.len());
                theme.shelves.insert(at, kind);
            }
        },
        _ => {}
    }
    // A gradient made from the colours follows them.
    if matches!(row, EditorRow::Accent | EditorRow::BackgroundColor)
        && matches!(theme.background, Background::Gradient { .. })
    {
        theme.background = gradient_for(theme);
    }
    (theme.clone(), *layout) != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_step_through_the_offers_and_back_to_the_theme() {
        let offers = [Rgb([1, 1, 1]), Rgb([2, 2, 2])];
        assert_eq!(step_color(None, &offers, true), Some(offers[0]));
        assert_eq!(step_color(Some(offers[1]), &offers, true), None);
        assert_eq!(step_color(None, &offers, false), Some(offers[1]));
        // A colour from a file steps onto the first offer.
        assert_eq!(
            step_color(Some(Rgb([9, 9, 9])), &offers, true),
            Some(offers[0])
        );
        assert_eq!(accents().len(), 12);
    }

    #[test]
    fn the_draft_changes_one_step_at_a_time() {
        let mut theme = Theme::default();
        let mut layout = HomeLayout::Shelves;
        assert!(change_draft(
            &mut theme,
            &mut layout,
            None,
            EditorRow::Layout,
            true
        ));
        assert_eq!(layout, HomeLayout::Spotlight);
        change_draft(&mut theme, &mut layout, None, EditorRow::Layout, false);
        change_draft(&mut theme, &mut layout, None, EditorRow::Layout, false);
        assert_eq!(layout, HomeLayout::Compact);

        // Shelves toggle and come back in the editor's order.
        let shelf = |k| EditorRow::Shelf(k);
        change_draft(
            &mut theme,
            &mut layout,
            None,
            shelf(ShelfKind::Favourites),
            true,
        );
        assert!(!theme.shelves.contains(&ShelfKind::Favourites));
        change_draft(
            &mut theme,
            &mut layout,
            None,
            shelf(ShelfKind::PlanToPlay),
            true,
        );
        change_draft(
            &mut theme,
            &mut layout,
            None,
            shelf(ShelfKind::Favourites),
            true,
        );
        assert_eq!(
            theme.shelves,
            vec![
                ShelfKind::ContinuePlaying,
                ShelfKind::Favourites,
                ShelfKind::Installed,
                ShelfKind::PlanToPlay,
                ShelfKind::Systems
            ]
        );

        // The backdrop offers the picture only when there is one.
        let kinds = |picture: Option<&Picture>| {
            let mut theme = Theme::default();
            let mut layout = HomeLayout::Shelves;
            (0..5)
                .map(|_| {
                    change_draft(&mut theme, &mut layout, picture, EditorRow::Backdrop, true);
                    theme.background.kind()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(None),
            ["gradient", "solid", "art", "gradient", "solid"]
        );
        let png = {
            let image = image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]));
            let mut out = std::io::Cursor::new(Vec::new());
            image.write_to(&mut out, image::ImageFormat::Png).unwrap();
            Picture::from_bytes(out.into_inner()).unwrap()
        };
        assert_eq!(
            kinds(Some(&png)),
            ["gradient", "solid", "picture", "art", "gradient"]
        );

        // A gradient follows the background colour.
        theme.background = gradient_for(&theme);
        change_draft(
            &mut theme,
            &mut layout,
            None,
            EditorRow::BackgroundColor,
            true,
        );
        assert!(matches!(
            theme.background,
            Background::Gradient { from, .. } if Some(from) == theme.palette.background
        ));
        assert!(!change_draft(
            &mut theme,
            &mut layout,
            None,
            EditorRow::Export,
            true
        ));
    }

    #[test]
    fn gradients_follow_the_theme_colours() {
        let mut theme = Theme::default();
        theme.palette.background = Some(Rgb([0xf3, 0xed, 0xe2]));
        let Background::Gradient { from, to, .. } = gradient_for(&theme) else {
            panic!("a gradient");
        };
        assert_eq!(from, Rgb([0xf3, 0xed, 0xe2]));
        assert!(to.hsla().l > 0.5, "light themes get a light gradient");
    }
}
