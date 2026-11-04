//! The Year in review's summary card as a PNG to share: the year, its
//! numbers and the most played games with their covers, over the top game's
//! blurred art. It is drawn as an SVG and rendered with resvg, the
//! rasterizer GPUI already ships, with Irori's own typeface (and the
//! system's fonts for scripts it lacks). No window or web engine is
//! involved, so it renders off the UI thread at a fixed size.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use resvg::{tiny_skia, usvg};

/// Size of the card, in pixels: a portrait that fits the usual feeds.
pub const WIDTH: u32 = 1080;
pub const HEIGHT: u32 = 1350;
/// Longest a game's title runs before it is cut short.
const TITLE_CHARS: usize = 30;
/// Cover size in the game list, in pixels.
const COVER_W: f32 = 72.;
const COVER_H: f32 = 96.;

/// An sRGB colour.
pub type Rgb = [u8; 3];

/// The theme's colours the card is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardColors {
    pub background: Rgb,
    pub foreground: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    /// Fill of the number tiles.
    pub surface: Rgb,
}

/// One of the most played games.
#[derive(Debug, Clone, PartialEq)]
pub struct CardGame {
    pub title: String,
    /// "120 h · 29%".
    pub detail: String,
    /// Share of the year, from 0 to 1.
    pub share: f32,
    /// A cover image on disk (JPEG, PNG or WebP).
    pub cover: Option<PathBuf>,
}

/// Everything the card shows, already in the interface language.
#[derive(Debug, Clone, PartialEq)]
pub struct CardInput {
    pub colors: CardColors,
    /// "YEAR IN REVIEW".
    pub eyebrow: String,
    /// "2026".
    pub year: String,
    /// "412 h played".
    pub headline: String,
    /// Up to four figures with their caption.
    pub facts: Vec<(String, String)>,
    /// "Most played".
    pub games_title: String,
    pub games: Vec<CardGame>,
    /// Art behind the top of the card (blurred here).
    pub backdrop: Option<PathBuf>,
    /// "Irori".
    pub brand: String,
}

/// Text made safe to place in SVG.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `text` cut to `max` characters, with an ellipsis when it was longer.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

fn hex([r, g, b]: Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// An image file as a data URL, when it is a format resvg reads.
fn data_url(bytes: &[u8]) -> Option<String> {
    let mime = if bytes.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{data}"))
}

/// The images the card embeds, read from disk: the backdrop, then one per
/// game. Images that cannot be read are left out.
fn read_images(input: &CardInput) -> (Option<String>, Vec<Option<String>>) {
    let read = |p: &Option<PathBuf>| {
        p.as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| data_url(&b))
    };
    (
        read(&input.backdrop),
        input.games.iter().map(|g| read(&g.cover)).collect(),
    )
}

/// The card as SVG. `backdrop` and `covers` are data URLs (see
/// [`read_images`]).
pub fn svg(input: &CardInput, backdrop: Option<&str>, covers: &[Option<String>]) -> String {
    let c = input.colors;
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    let (bg, fg, muted, accent, surface) = (
        hex(c.background),
        hex(c.foreground),
        hex(c.muted),
        hex(c.accent),
        hex(c.surface),
    );
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="Manrope, sans-serif">"#
    );
    let _ = write!(
        s,
        r#"<defs><filter id="blur" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur stdDeviation="28"/></filter><linearGradient id="fade" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{bg}" stop-opacity="0.35"/><stop offset="0.75" stop-color="{bg}" stop-opacity="1"/></linearGradient><clipPath id="top"><rect width="{w}" height="820"/></clipPath></defs>"#
    );
    let _ = write!(s, r#"<rect width="{w}" height="{h}" fill="{bg}"/>"#);
    if let Some(url) = backdrop {
        let _ = write!(
            s,
            r#"<g clip-path="url(#top)"><image href="{url}" x="-60" y="-60" width="{}" height="940" preserveAspectRatio="xMidYMid slice" filter="url(#blur)" opacity="0.8"/></g>"#,
            w + 120.
        );
    }
    let _ = write!(s, r#"<rect width="{w}" height="820" fill="url(#fade)"/>"#);

    // Eyebrow with a rule before it, the year, the headline.
    let _ = write!(
        s,
        r#"<rect x="80" y="113" width="44" height="3" rx="1.5" fill="{muted}"/><text x="140" y="124" font-size="30" font-weight="700" letter-spacing="4" fill="{muted}">{}</text>"#,
        escape(&input.eyebrow.to_uppercase())
    );
    let _ = write!(
        s,
        r#"<text x="72" y="340" font-size="230" font-weight="800" letter-spacing="-6" fill="{fg}">{}</text>"#,
        escape(&input.year)
    );
    let _ = write!(
        s,
        r#"<text x="80" y="430" font-size="64" font-weight="700" fill="{accent}">{}</text>"#,
        escape(&input.headline)
    );

    // Up to four figures in tiles.
    let facts = &input.facts[..input.facts.len().min(4)];
    if !facts.is_empty() {
        let gap = 20.;
        let tile_w = (w - 160. - gap * (facts.len() as f32 - 1.)) / facts.len() as f32;
        for (ix, (value, label)) in facts.iter().enumerate() {
            let x = 80. + ix as f32 * (tile_w + gap);
            let _ = write!(
                s,
                r#"<rect x="{x}" y="490" width="{tile_w}" height="160" rx="28" fill="{surface}" fill-opacity="0.85"/><text x="{}" y="568" font-size="56" font-weight="800" fill="{fg}">{}</text><text x="{}" y="616" font-size="26" font-weight="500" fill="{muted}">{}</text>"#,
                x + 28.,
                escape(&clip(value, 8)),
                x + 28.,
                escape(&clip(label, 14))
            );
        }
    }

    // The most played games: cover, title, detail and their share.
    let _ = write!(
        s,
        r#"<text x="80" y="736" font-size="30" font-weight="700" letter-spacing="3" fill="{muted}">{}</text>"#,
        escape(&input.games_title.to_uppercase())
    );
    let top = input.games.iter().map(|g| g.share).fold(0f32, f32::max);
    for (ix, game) in input.games.iter().take(5).enumerate() {
        let y = 776. + ix as f32 * 112.;
        let clip_id = format!("cover{ix}");
        let _ = write!(
            s,
            r#"<clipPath id="{clip_id}"><rect x="80" y="{y}" width="{COVER_W}" height="{COVER_H}" rx="10"/></clipPath><rect x="80" y="{y}" width="{COVER_W}" height="{COVER_H}" rx="10" fill="{surface}"/>"#
        );
        if let Some(Some(url)) = covers.get(ix) {
            let _ = write!(
                s,
                r#"<image href="{url}" x="80" y="{y}" width="{COVER_W}" height="{COVER_H}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip_id})"/>"#
            );
        }
        let text_x = 80. + COVER_W + 28.;
        let bar_w = w - 80. - text_x;
        let filled = if top > 0. {
            (game.share / top).clamp(0.02, 1.) * bar_w
        } else {
            0.
        };
        let _ = write!(
            s,
            r#"<text x="{text_x}" y="{}" font-size="34" font-weight="700" fill="{fg}">{}</text><text x="{text_x}" y="{}" font-size="26" font-weight="500" fill="{muted}">{}</text><rect x="{text_x}" y="{}" width="{bar_w}" height="8" rx="4" fill="{surface}"/><rect x="{text_x}" y="{}" width="{filled}" height="8" rx="4" fill="{accent}"/>"#,
            y + 34.,
            escape(&clip(&game.title, TITLE_CHARS)),
            y + 68.,
            escape(&game.detail),
            y + 84.,
            y + 84.
        );
    }

    let _ = write!(
        s,
        r#"<text x="{}" y="{}" font-size="28" font-weight="800" letter-spacing="2" text-anchor="end" fill="{muted}">{}</text>"#,
        w - 80.,
        h - 56.,
        escape(&input.brand)
    );
    s.push_str("</svg>");
    s
}

/// Render the card to PNG bytes. `fonts` are Irori's own font files; the
/// system's fonts fill in scripts they lack. Blocking: call it off the UI
/// thread.
pub fn render_png(input: &CardInput, fonts: &[&[u8]]) -> Result<Vec<u8>> {
    let (backdrop, covers) = read_images(input);
    let text = svg(input, backdrop.as_deref(), &covers);
    let mut options = usvg::Options {
        font_family: "Manrope".into(),
        ..Default::default()
    };
    {
        let db = options.fontdb_mut();
        db.load_system_fonts();
        for font in fonts {
            db.load_font_data(font.to_vec());
        }
        db.set_sans_serif_family("Manrope");
    }
    let tree = usvg::Tree::from_str(&text, &options).context("Cannot draw the card")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(WIDTH, HEIGHT).context("Cannot make room for the card")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().context("Cannot encode the card")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> CardInput {
        CardInput {
            colors: CardColors {
                background: [16, 14, 12],
                foreground: [240, 236, 230],
                muted: [160, 150, 140],
                accent: [240, 140, 60],
                surface: [40, 36, 32],
            },
            eyebrow: "Year in review".into(),
            year: "2026".into(),
            headline: "412 h played".into(),
            facts: vec![
                ("214".into(), "days played".into()),
                ("38".into(), "games".into()),
            ],
            games_title: "Most played".into(),
            games: vec![CardGame {
                title: "Knights & <Dragons> \"Remastered\" — The Very Long Edition".into(),
                detail: "120 h · 29%".into(),
                share: 0.29,
                cover: None,
            }],
            backdrop: None,
            brand: "Irori".into(),
        }
    }

    #[test]
    fn text_is_escaped_and_long_titles_cut_short() {
        let svg = svg(&input(), None, &[None]);
        assert!(svg.contains("Knights &amp; &lt;Dragons&gt; &quot;Remaster"));
        assert!(!svg.contains("Very Long Edition"));
        assert!(svg.contains('…'));
        assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
        assert_eq!(clip("Hades", 30), "Hades");
        assert_eq!(escape("a\u{7}b"), "ab");
    }

    #[test]
    fn only_images_resvg_reads_are_embedded() {
        assert!(
            data_url(b"\xff\xd8\xff\xe0rest")
                .unwrap()
                .starts_with("data:image/jpeg;base64,")
        );
        assert!(
            data_url(b"\x89PNG\r\n")
                .unwrap()
                .starts_with("data:image/png;")
        );
        assert!(
            data_url(b"RIFF\0\0\0\0WEBPVP8 ")
                .unwrap()
                .starts_with("data:image/webp;")
        );
        assert!(data_url(b"<svg/>").is_none());
    }

    #[test]
    fn renders_a_png_of_the_card_size() {
        let fonts: [&[u8]; 1] = [include_bytes!("../../../../assets/fonts/Manrope-Bold.ttf")];
        let png = render_png(&input(), &fonts).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (WIDTH, HEIGHT));
        // The headline is drawn in the accent colour somewhere on the card.
        let accent = pixmap
            .pixels()
            .iter()
            .any(|p| p.red() > 200 && p.green() > 110 && p.green() < 170 && p.blue() < 90);
        assert!(accent);
    }
}
