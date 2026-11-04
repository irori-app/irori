//! Big Picture themes: colours laid over Irori's theme, a typeface, what
//! fills the background, the shape and size of covers, the shelves Home
//! shows and, optionally, the layout they suit. Themes come from the
//! built-in gallery ([`gallery`]) or from `.irori-theme.json` files the user
//! made or imported ([`store`]).
//!
//! A theme file is versioned JSON. Everything in it is checked before it is
//! used: colours must parse, names are short plain text, enumerations take
//! only known values, and a picture must be embedded in the file as a PNG,
//! JPEG or WebP of bounded size. A file never names a path or an address, so
//! importing one reads nothing else and writes only Irori's own copy.

pub mod gallery;
pub mod store;

use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gpui_kit::{Hsla, Image, ImageFormat, Rgba, SharedString};
use irori_core::GameInfo;
use irori_ui::i18n::{t, t_args};
use serde::{Deserialize, Serialize};

use crate::layout::HomeLayout;

/// The `format` every theme file carries.
pub const FORMAT: &str = "irori-bigpicture-theme";
/// The newest file version this Irori reads and the one it writes.
pub const VERSION: u32 = 1;
/// Ending of theme file names.
pub const EXTENSION: &str = ".irori-theme.json";
/// Largest theme file read, in bytes (a picture at its limit, in base64).
pub const MAX_FILE_BYTES: u64 = 12 * 1024 * 1024;
/// Largest embedded picture, in bytes.
pub const MAX_PICTURE_BYTES: usize = 8 * 1024 * 1024;
/// Largest side of an embedded picture, in pixels.
pub const MAX_PICTURE_SIDE: u32 = 8192;
/// Longest theme name, in characters.
pub const MAX_NAME: usize = 48;
/// Longest author line, in characters.
const MAX_AUTHOR: usize = 64;

/// An sRGB colour without alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    /// `#rrggbb` or `#rgb`, in either case.
    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
        let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => Some(Rgb([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17])),
            6 => Some(Rgb([pair(0)?, pair(2)?, pair(4)?])),
            _ => None,
        }
    }

    /// `#rrggbb`.
    pub fn hex(self) -> String {
        let [r, g, b] = self.0;
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    pub fn hsla(self) -> Hsla {
        let [r, g, b] = self.0.map(|c| c as f32 / 255.0);
        Rgba { r, g, b, a: 1.0 }.into()
    }

    /// A colour of hue `degrees` at the given saturation and lightness
    /// (0…1), for the editor's colour wheel.
    pub fn from_hsl(degrees: f32, saturation: f32, lightness: f32) -> Rgb {
        let rgba = Hsla {
            h: (degrees.rem_euclid(360.0)) / 360.0,
            s: saturation,
            l: lightness,
            a: 1.0,
        }
        .to_rgb();
        Rgb([rgba.r, rgba.g, rgba.b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
    }
}

/// Colours a theme lays over Irori's own; each left out keeps the
/// interface theme's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Palette {
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    /// Focus rings and highlighted buttons.
    pub accent: Option<Rgb>,
    /// Menus and panels.
    pub surface: Option<Rgb>,
}

/// The typeface Big Picture writes in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FontChoice {
    /// Manrope, Irori's own.
    #[default]
    Irori,
    /// The system's interface font.
    System,
    Serif,
    Mono,
}

impl FontChoice {
    pub const ALL: [FontChoice; 4] = [
        FontChoice::Irori,
        FontChoice::System,
        FontChoice::Serif,
        FontChoice::Mono,
    ];

    pub fn id(self) -> &'static str {
        match self {
            FontChoice::Irori => "irori",
            FontChoice::System => "system",
            FontChoice::Serif => "serif",
            FontChoice::Mono => "mono",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            FontChoice::Irori => "bigpicture.theme.font.irori",
            FontChoice::System => "bigpicture.theme.font.system",
            FontChoice::Serif => "bigpicture.theme.font.serif",
            FontChoice::Mono => "bigpicture.theme.font.mono",
        })
    }

    /// The family to ask GPUI for; a family the system lacks falls back to
    /// GPUI's default.
    pub fn family(self, mono: &SharedString) -> SharedString {
        match self {
            FontChoice::Irori => irori_ui::theme::FONT_FAMILY.into(),
            FontChoice::System => ".SystemUIFont".into(),
            FontChoice::Serif if cfg!(target_os = "linux") => "Noto Serif".into(),
            FontChoice::Serif => "Georgia".into(),
            FontChoice::Mono => mono.clone(),
        }
    }
}

/// A picture embedded in a theme, ready to draw.
#[derive(Debug, Clone)]
pub struct Picture {
    pub image: Arc<Image>,
}

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        self.image.id == other.image.id && self.image.bytes == other.image.bytes
    }
}

impl Picture {
    /// Check `bytes` (a PNG, JPEG or WebP within the size limits) and keep
    /// them.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Picture, ThemeError> {
        if bytes.len() > MAX_PICTURE_BYTES {
            return Err(ThemeError::PictureTooLarge);
        }
        let format = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Png) => ImageFormat::Png,
            Ok(image::ImageFormat::Jpeg) => ImageFormat::Jpeg,
            Ok(image::ImageFormat::WebP) => ImageFormat::Webp,
            _ => return Err(ThemeError::PictureFormat),
        };
        let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .ok_or(ThemeError::PictureFormat)?;
        if width == 0 || height == 0 {
            return Err(ThemeError::PictureFormat);
        }
        if width > MAX_PICTURE_SIDE || height > MAX_PICTURE_SIDE {
            return Err(ThemeError::PictureTooLarge);
        }
        Ok(Picture {
            image: Arc::new(Image::from_bytes(format, bytes)),
        })
    }

    fn mime(&self) -> &'static str {
        match self.image.format {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
            _ => "image/png",
        }
    }

    /// The picture as a `data:` address.
    fn data_url(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mime(),
            BASE64.encode(&self.image.bytes)
        )
    }

    fn from_data_url(url: &str) -> Result<Picture, ThemeError> {
        let rest = url
            .trim()
            .strip_prefix("data:")
            .ok_or(ThemeError::PictureNotEmbedded)?;
        let (kind, data) = rest
            .split_once(";base64,")
            .ok_or(ThemeError::PictureNotEmbedded)?;
        if !matches!(kind, "image/png" | "image/jpeg" | "image/webp") {
            return Err(ThemeError::PictureFormat);
        }
        // Base64 is a third larger than what it holds.
        if data.len() / 4 * 3 > MAX_PICTURE_BYTES + 3 {
            return Err(ThemeError::PictureTooLarge);
        }
        let bytes = BASE64
            .decode(data.trim())
            .map_err(|_| ThemeError::PictureFormat)?;
        Picture::from_bytes(bytes)
    }
}

/// What fills the screen behind Big Picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    /// The focused game's art, blurred.
    Art,
    /// Two colours blending at `angle` degrees (0 runs upward, 90 to the
    /// right).
    Gradient { from: Rgb, to: Rgb, angle: u16 },
    /// A picture of the user's.
    Picture(Picture),
    /// The background colour alone.
    Solid,
}

impl Background {
    pub fn kind(&self) -> &'static str {
        match self {
            Background::Art => "art",
            Background::Gradient { .. } => "gradient",
            Background::Picture(_) => "picture",
            Background::Solid => "solid",
        }
    }

    pub fn label(&self) -> SharedString {
        kind_label(self.kind())
    }
}

/// Translated name of a background kind.
pub fn kind_label(kind: &str) -> SharedString {
    t(match kind {
        "gradient" => "bigpicture.theme.background.gradient",
        "picture" => "bigpicture.theme.background.picture",
        "solid" => "bigpicture.theme.background.solid",
        _ => "bigpicture.theme.background.art",
    })
}

/// The shape of covers, and the art that fills it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CoverShape {
    /// Box art, 2:3.
    #[default]
    Portrait,
    Square,
    /// Wide art, 16:9.
    Landscape,
}

impl CoverShape {
    pub const ALL: [CoverShape; 3] = [
        CoverShape::Portrait,
        CoverShape::Square,
        CoverShape::Landscape,
    ];

    pub fn id(self) -> &'static str {
        match self {
            CoverShape::Portrait => "portrait",
            CoverShape::Square => "square",
            CoverShape::Landscape => "landscape",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            CoverShape::Portrait => "bigpicture.theme.shape.portrait",
            CoverShape::Square => "bigpicture.theme.shape.square",
            CoverShape::Landscape => "bigpicture.theme.shape.landscape",
        })
    }

    /// Height over width.
    pub fn ratio(self) -> f32 {
        match self {
            CoverShape::Portrait => 1.5,
            CoverShape::Square => 1.0,
            CoverShape::Landscape => 0.5625,
        }
    }

    /// The game's art that suits the shape best.
    pub fn art(self, game: &GameInfo) -> Option<&str> {
        match self {
            CoverShape::Portrait => game.cover_art(),
            CoverShape::Square => game
                .art_square
                .as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| game.cover_art()),
            CoverShape::Landscape => game.hero_art(),
        }
    }
}

/// How large covers are next to what the layout would choose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CoverSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CoverSize {
    pub const ALL: [CoverSize; 3] = [CoverSize::Small, CoverSize::Medium, CoverSize::Large];

    pub fn id(self) -> &'static str {
        match self {
            CoverSize::Small => "small",
            CoverSize::Medium => "medium",
            CoverSize::Large => "large",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            CoverSize::Small => "bigpicture.theme.size.small",
            CoverSize::Medium => "bigpicture.theme.size.medium",
            CoverSize::Large => "bigpicture.theme.size.large",
        })
    }

    pub fn factor(self) -> f32 {
        match self {
            CoverSize::Small => 0.82,
            CoverSize::Medium => 1.0,
            CoverSize::Large => 1.2,
        }
    }
}

/// Corners of covers and panels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Corners {
    Sharp,
    /// The interface theme's radius.
    #[default]
    Rounded,
    Round,
}

impl Corners {
    pub const ALL: [Corners; 3] = [Corners::Sharp, Corners::Rounded, Corners::Round];

    pub fn id(self) -> &'static str {
        match self {
            Corners::Sharp => "sharp",
            Corners::Rounded => "rounded",
            Corners::Round => "round",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            Corners::Sharp => "bigpicture.theme.corners.sharp",
            Corners::Rounded => "bigpicture.theme.corners.rounded",
            Corners::Round => "bigpicture.theme.corners.round",
        })
    }
}

/// Covers' shape, size and corners.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CoverStyle {
    pub shape: CoverShape,
    pub size: CoverSize,
    pub corners: Corners,
}

/// A shelf Home can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShelfKind {
    ContinuePlaying,
    Favourites,
    Installed,
    RecentlyAdded,
    /// Games the user marked "Plan to play".
    PlanToPlay,
    /// One shelf per emulated system.
    Systems,
}

impl ShelfKind {
    pub const ALL: [ShelfKind; 6] = [
        ShelfKind::ContinuePlaying,
        ShelfKind::Favourites,
        ShelfKind::Installed,
        ShelfKind::RecentlyAdded,
        ShelfKind::PlanToPlay,
        ShelfKind::Systems,
    ];

    /// Home's shelves unless a theme says otherwise.
    pub const DEFAULT: [ShelfKind; 4] = [
        ShelfKind::ContinuePlaying,
        ShelfKind::Favourites,
        ShelfKind::Installed,
        ShelfKind::Systems,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ShelfKind::ContinuePlaying => "continue_playing",
            ShelfKind::Favourites => "favourites",
            ShelfKind::Installed => "installed",
            ShelfKind::RecentlyAdded => "recently_added",
            ShelfKind::PlanToPlay => "plan_to_play",
            ShelfKind::Systems => "systems",
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            ShelfKind::ContinuePlaying => "bigpicture.shelf.continue_playing",
            ShelfKind::Favourites => "bigpicture.shelf.favourites",
            ShelfKind::Installed => "bigpicture.shelf.installed",
            ShelfKind::RecentlyAdded => "bigpicture.shelf.recently_added",
            ShelfKind::PlanToPlay => "bigpicture.shelf.plan_to_play",
            ShelfKind::Systems => "bigpicture.shelf.systems",
        })
    }
}

fn by_id<T: Copy>(all: &[T], id: &str, of: impl Fn(T) -> &'static str) -> Option<T> {
    all.iter().copied().find(|v| of(*v) == id)
}

/// A Big Picture theme, checked and ready to use.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub author: String,
    /// The layout the theme was made for; applying the theme selects it.
    pub layout: Option<HomeLayout>,
    pub palette: Palette,
    pub font: FontChoice,
    pub background: Background,
    pub covers: CoverStyle,
    /// Home's shelves, in order.
    pub shelves: Vec<ShelfKind>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "Irori".into(),
            author: String::new(),
            layout: None,
            palette: Palette::default(),
            font: FontChoice::default(),
            background: Background::Art,
            covers: CoverStyle::default(),
            shelves: ShelfKind::DEFAULT.to_vec(),
        }
    }
}

/// Why a theme file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// Not JSON, or not a Big Picture theme.
    NotATheme,
    TooLarge,
    /// Made by a newer Irori, in this file version.
    Newer(u32),
    Name,
    Author,
    /// A colour that does not parse: the field and its value.
    Color(&'static str, String),
    /// A value the field does not take: the field and the value.
    Value(&'static str, String),
    /// A picture given as a path or an address instead of inside the file.
    PictureNotEmbedded,
    PictureFormat,
    PictureTooLarge,
}

impl ThemeError {
    /// What went wrong, for the user.
    pub fn message(&self) -> SharedString {
        match self {
            ThemeError::NotATheme => t("bigpicture.theme.error.not_a_theme"),
            ThemeError::TooLarge => t("bigpicture.theme.error.too_large"),
            ThemeError::Newer(version) => t_args(
                "bigpicture.theme.error.newer",
                &[("version", version.to_string().as_str())],
            ),
            ThemeError::Name => t_args(
                "bigpicture.theme.error.name",
                &[("max", MAX_NAME.to_string().as_str())],
            ),
            ThemeError::Author => t("bigpicture.theme.error.author"),
            ThemeError::Color(field, value) => t_args(
                "bigpicture.theme.error.color",
                &[("field", field), ("value", value.as_str())],
            ),
            ThemeError::Value(field, value) => t_args(
                "bigpicture.theme.error.value",
                &[("field", field), ("value", value.as_str())],
            ),
            ThemeError::PictureNotEmbedded => t("bigpicture.theme.error.picture_embedded"),
            ThemeError::PictureFormat => t("bigpicture.theme.error.picture_format"),
            ThemeError::PictureTooLarge => t_args(
                "bigpicture.theme.error.picture_size",
                &[
                    (
                        "megabytes",
                        (MAX_PICTURE_BYTES / 1024 / 1024).to_string().as_str(),
                    ),
                    ("pixels", MAX_PICTURE_SIDE.to_string().as_str()),
                ],
            ),
        }
    }
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ThemeError {}

// ---- file format ---------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThemeFile {
    format: String,
    version: u32,
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<String>,
    colors: ColorsFile,
    font: Option<String>,
    background: Option<BackgroundFile>,
    covers: CoversFile,
    shelves: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ColorsFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    surface: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct BackgroundFile {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    angle: Option<u16>,
    /// A `data:image/…;base64,` address.
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CoversFile {
    shape: Option<String>,
    size: Option<String>,
    corners: Option<String>,
}

/// A plain line of at most `max` characters, without control characters.
fn plain_line(text: &str, max: usize) -> Option<String> {
    let text = text.trim();
    (text.chars().count() <= max && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

fn color(field: &'static str, value: &Option<String>) -> Result<Option<Rgb>, ThemeError> {
    value
        .as_deref()
        .map(|v| Rgb::parse(v).ok_or_else(|| ThemeError::Color(field, v.to_owned())))
        .transpose()
}

fn choice<T: Copy>(
    field: &'static str,
    value: &Option<String>,
    all: &[T],
    of: impl Fn(T) -> &'static str,
    default: T,
) -> Result<T, ThemeError> {
    match value.as_deref() {
        None => Ok(default),
        Some(v) => by_id(all, v, of).ok_or_else(|| ThemeError::Value(field, v.to_owned())),
    }
}

impl Theme {
    /// Read and check a theme file's text.
    pub fn from_json(text: &str) -> Result<Theme, ThemeError> {
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(ThemeError::TooLarge);
        }
        let file: ThemeFile = serde_json::from_str(text).map_err(|_| ThemeError::NotATheme)?;
        if file.format != FORMAT || file.version == 0 {
            return Err(ThemeError::NotATheme);
        }
        if file.version > VERSION {
            return Err(ThemeError::Newer(file.version));
        }
        let name = plain_line(&file.name, MAX_NAME)
            .filter(|n| !n.is_empty())
            .ok_or(ThemeError::Name)?;
        let author = plain_line(&file.author, MAX_AUTHOR).ok_or(ThemeError::Author)?;
        let layout = file
            .layout
            .as_deref()
            .map(|id| HomeLayout::from_id(id).ok_or_else(|| ThemeError::Value("layout", id.into())))
            .transpose()?;
        let palette = Palette {
            background: color("colors.background", &file.colors.background)?,
            foreground: color("colors.foreground", &file.colors.foreground)?,
            accent: color("colors.accent", &file.colors.accent)?,
            surface: color("colors.surface", &file.colors.surface)?,
        };
        let font = choice(
            "font",
            &file.font,
            &FontChoice::ALL,
            FontChoice::id,
            FontChoice::Irori,
        )?;
        let background = match file.background {
            None => Background::Art,
            Some(bg) => match bg.kind.as_str() {
                "art" => Background::Art,
                "solid" => Background::Solid,
                "gradient" => {
                    let from = color("background.from", &bg.from)?
                        .ok_or_else(|| ThemeError::Color("background.from", String::new()))?;
                    let to = color("background.to", &bg.to)?
                        .ok_or_else(|| ThemeError::Color("background.to", String::new()))?;
                    let angle = bg.angle.unwrap_or(180);
                    if angle > 360 {
                        return Err(ThemeError::Value("background.angle", angle.to_string()));
                    }
                    Background::Gradient { from, to, angle }
                }
                "picture" => {
                    let url = bg.image.ok_or(ThemeError::PictureNotEmbedded)?;
                    Background::Picture(Picture::from_data_url(&url)?)
                }
                other => return Err(ThemeError::Value("background.kind", other.into())),
            },
        };
        let covers = CoverStyle {
            shape: choice(
                "covers.shape",
                &file.covers.shape,
                &CoverShape::ALL,
                CoverShape::id,
                CoverShape::Portrait,
            )?,
            size: choice(
                "covers.size",
                &file.covers.size,
                &CoverSize::ALL,
                CoverSize::id,
                CoverSize::Medium,
            )?,
            corners: choice(
                "covers.corners",
                &file.covers.corners,
                &Corners::ALL,
                Corners::id,
                Corners::Rounded,
            )?,
        };
        let shelves = match file.shelves {
            None => ShelfKind::DEFAULT.to_vec(),
            Some(ids) => {
                let mut out: Vec<ShelfKind> = Vec::with_capacity(ids.len());
                for id in &ids {
                    let kind = by_id(&ShelfKind::ALL, id, ShelfKind::id)
                        .ok_or_else(|| ThemeError::Value("shelves", id.clone()))?;
                    if !out.contains(&kind) {
                        out.push(kind);
                    }
                }
                out
            }
        };
        Ok(Theme {
            name,
            author,
            layout,
            palette,
            font,
            background,
            covers,
            shelves,
        })
    }

    /// The theme as a file, in the newest version.
    pub fn to_json(&self) -> String {
        let file = ThemeFile {
            format: FORMAT.into(),
            version: VERSION,
            name: self.name.clone(),
            author: self.author.clone(),
            layout: self.layout.map(|l| l.id().to_owned()),
            colors: ColorsFile {
                background: self.palette.background.map(Rgb::hex),
                foreground: self.palette.foreground.map(Rgb::hex),
                accent: self.palette.accent.map(Rgb::hex),
                surface: self.palette.surface.map(Rgb::hex),
            },
            font: Some(self.font.id().into()),
            background: Some(match &self.background {
                Background::Gradient { from, to, angle } => BackgroundFile {
                    kind: "gradient".into(),
                    from: Some(from.hex()),
                    to: Some(to.hex()),
                    angle: Some(*angle),
                    image: None,
                },
                Background::Picture(picture) => BackgroundFile {
                    kind: "picture".into(),
                    image: Some(picture.data_url()),
                    ..Default::default()
                },
                other => BackgroundFile {
                    kind: other.kind().into(),
                    ..Default::default()
                },
            }),
            covers: CoversFile {
                shape: Some(self.covers.shape.id().into()),
                size: Some(self.covers.size.id().into()),
                corners: Some(self.covers.corners.id().into()),
            },
            shelves: Some(self.shelves.iter().map(|s| s.id().to_owned()).collect()),
        };
        // Serializing plain strings and numbers cannot fail.
        serde_json::to_string_pretty(&file).expect("theme files serialize")
    }
}

#[cfg(test)]
mod tests;
