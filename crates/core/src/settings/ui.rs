use serde::{Deserialize, Serialize};

#[cfg(doc)]
use super::AppSettings;
use super::ShownStores;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryLayout {
    #[default]
    Grid,
    List,
    /// A column of covers beside the selected game's page.
    Focus,
}

/// How much of the desktop shows through Irori's window chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Translucency {
    /// On where the system blurs what is behind the window (macOS,
    /// Windows 11, Linux compositors that offer blur), off elsewhere.
    #[default]
    Auto,
    Off,
    /// Translucent title bar and sidebar; pages stay solid.
    Subtle,
    /// Clearer chrome, and pages let a little of the desktop through too.
    Strong,
}

/// The controller button held to open Big Picture (`bigPictureHoldButton`).
/// Values a newer Irori writes read as [`HoldButton::Start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldButton {
    /// Holding a button does nothing.
    Off,
    /// Start (Menu, Options, +).
    #[default]
    Start,
    /// Select (View, Share, Create, −).
    Select,
}

impl<'de> Deserialize<'de> for HoldButton {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "off" => HoldButton::Off,
            "select" => HoldButton::Select,
            _ => HoldButton::Start,
        })
    }
}

/// How tightly the library packs its covers and rows. Separate from the
/// interface size, which scales everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

impl Density {
    /// Factor on the cover size the user picked.
    pub fn cover_scale(self) -> f32 {
        match self {
            Density::Compact => 0.85,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.15,
        }
    }

    /// Factor on the space between covers and the height of list rows' padding.
    pub fn spacing_scale(self) -> f32 {
        match self {
            Density::Compact => 0.55,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.5,
        }
    }

    /// Factor on the height of a row in the list layout.
    pub fn row_scale(self) -> f32 {
        match self {
            Density::Compact => 0.8,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.2,
        }
    }
}

/// The corners of the library's covers (`coverCorners`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverCorners {
    Square,
    /// The theme's large radius.
    #[default]
    Rounded,
    /// Twice the theme's large radius.
    Round,
}

/// What the library writes under each cover (`coverCaption`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverCaption {
    /// The title and a line about the game (install state, size, playtime).
    #[default]
    Full,
    /// Only the title.
    Title,
    /// Nothing: the covers alone.
    None,
}

/// How the library draws box art (`coverFit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverFit {
    /// Every cover filled to 2:3, cut where its shape differs.
    #[default]
    Crop,
    /// Each picture whole in its own shape (tall, square, wide), standing
    /// on its row; tall boxes of one row share a height.
    Whole,
}

/// The shape of the library grid's covers (`coverShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverShape {
    /// Square art: the store's square capsule, else the box art cropped.
    Square,
    /// A wide banner of the game's hero art, 16:9.
    Banner,
    /// Box art, 2:3. Last, so a shape a newer Irori writes reads as this.
    #[default]
    #[serde(other)]
    Portrait,
}

impl CoverShape {
    /// Height over width.
    pub fn ratio(self) -> f32 {
        match self {
            CoverShape::Portrait => 1.5,
            CoverShape::Square => 1.0,
            CoverShape::Banner => 9.0 / 16.0,
        }
    }

    /// How much wider than a box a cover of this shape is drawn at one
    /// cover size, so the size slider keeps covers of about one weight.
    pub fn width_scale(self) -> f32 {
        match self {
            CoverShape::Portrait => 1.0,
            CoverShape::Square => 1.2,
            CoverShape::Banner => 1.75,
        }
    }
}

/// How large the covers in the Focus view's column are
/// (`libraryFocusCoverSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusCoverSize {
    Small,
    Large,
    /// Last, so a size a newer Irori writes reads as this one.
    #[default]
    #[serde(other)]
    Medium,
}

/// How a cover answers the pointer and the keyboard focus (`coverHover`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverHover {
    /// It rises a little, with a shadow and a ring.
    #[default]
    Lift,
    /// Only the ring.
    Outline,
}

/// The art cache's default cap in MiB.
pub const DEFAULT_ART_CACHE_LIMIT: u32 = 2048;

/// [`AppSettings::steam_tag_limit`] by default: every tag Steam names.
pub const DEFAULT_STEAM_TAG_LIMIT: u32 = 20;

/// What the library shows behind its covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryBackground {
    #[default]
    None,
    /// The focused game's art, else the last played game's, else the first
    /// game shown, blurred.
    GameArt,
    /// A picture the user picked (`libraryBackgroundPicture`), blurred.
    Picture,
}

/// Irori-only presentation preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiPrefs {
    pub theme: ThemePreference,
    /// Theme name from the bundled/user theme registry ("" = default: the
    /// wallpaper's colours with `wallpaperColors` on, else the desktop's
    /// theme on COSMIC, KDE Plasma and Omarchy, Irori's own elsewhere, in the
    /// system's accent colour where it has one).
    pub theme_name: String,
    /// Tint the default theme with the system's accent colour (`systemAccent`).
    pub system_accent: bool,
    /// Make the default theme of the colours pywal, wallust or matugen made
    /// from the wallpaper (`wallpaperColors`), and follow them as they change.
    pub wallpaper_colors: bool,
    /// The colours file to follow (`wallpaperColorsFile`): pywal's
    /// `colors.json`, or JSON matugen rendered. "" = pywal's, in
    /// `$PYWAL_CACHE_DIR` or `~/.cache/wal`, which wallust writes too.
    pub wallpaper_colors_file: String,
    pub translucency: Translucency,
    /// Base font size in px; drives the rem scale (application zoom).
    pub font_size: f32,
    pub library_layout: LibraryLayout,
    /// Cover tile width in rem for the grid.
    pub tile_size: f32,
    /// Width in rem of the game drawer beside the grid (`libraryDrawerWidth`).
    pub library_drawer_width: f32,
    pub reduce_motion: bool,
    /// Hold the middle mouse button and move the mouse to scroll what is
    /// under it (`middleClickScroll`, off unless turned on).
    pub middle_click_scroll: bool,
    pub show_store_badges: bool,
    /// The game's categories on its cover in the grid (`coverCategories`,
    /// off unless turned on).
    pub cover_categories: bool,
    /// Copies of one game from several stores are one library entry
    /// (`groupCopies`, on unless turned off).
    pub group_copies: bool,
    /// A game without cover art shows the icon of its Windows program on its
    /// stand-in cover (`programIconArt`, on unless turned off).
    pub program_icon_art: bool,
    /// A game page's first trailer starts on its own, muted
    /// (`trailerAutoplay`, off unless turned on).
    pub trailer_autoplay: bool,
    /// Quick sort, install, favourites and grouping controls in the
    /// library's header, shown as far as its width allows
    /// (`libraryQuickControls`, on unless turned off).
    pub library_quick_controls: bool,
    pub sidebar_collapsed: bool,
    pub window_width: f32,
    pub window_height: f32,
    pub window_maximized: bool,
    /// Open the main window maximized, whatever state it was left in
    /// (`startMaximized`).
    pub start_maximized: bool,
    /// The main window's place on its display, in global logical pixels,
    /// where the system lets Irori place windows (not Wayland).
    pub window_x: Option<f32>,
    pub window_y: Option<f32>,
    /// The UUID of the display the main window was on ("" = unknown).
    pub window_display: String,
    /// Stores the library, accounts and store pages show.
    pub stores: ShownStores,
    /// Open Big Picture at startup in Steam's Game Mode or Big Picture.
    pub big_picture_in_game_mode: bool,
    /// Size of Big Picture's interface in percent, for TVs.
    pub big_picture_scale: u32,
    /// How Big Picture's Home arranges games (`shelves`, `spotlight`,
    /// `cinema`, `wall`, `carousel`, `compact`); unknown values read as
    /// `shelves`.
    pub big_picture_layout: String,
    /// Big Picture's theme: `builtin:<id>`, `user:<id>` (a file in the
    /// config folder's `bigpicture/themes`), or "" for Irori's own.
    pub big_picture_theme: String,
    /// Open Big Picture when a controller connects while Irori runs, or
    /// is connected when it starts.
    pub big_picture_on_controller: bool,
    /// Ask the desktop for a shortcut that opens the quick launcher (the
    /// command palette in a window of its own) from anywhere
    /// (`quickLaunchShortcut`); off, `irori --quick-launch` still opens it.
    pub quick_launch_shortcut: bool,
    /// The controller button held to open Big Picture from anywhere in
    /// Irori (`bigPictureHoldButton`).
    pub big_picture_hold_button: HoldButton,
    /// How long that button is held, in milliseconds (`bigPictureHoldMs`).
    pub big_picture_hold_ms: u32,
    /// The accent colour the user picked, `#rrggbb` ("" = the theme's own,
    /// or the system's where Irori follows it).
    pub accent_color: String,
    pub density: Density,
    /// Interface typeface ("" = Manrope, Irori's own).
    pub font_family: String,
    pub library_background: LibraryBackground,
    /// The picture behind the library when `libraryBackground` is `picture`.
    pub library_background_picture: String,
    /// How strongly the background shows, in percent of the most text keeps
    /// legible over it.
    pub library_background_strength: u32,
    /// How much of the theme's background is laid over it, in percent.
    pub library_background_dim: u32,
    /// Big Picture fills the screen (`bigPictureFullscreen`, on unless
    /// turned off); off, it shows in the window as it is.
    pub big_picture_fullscreen: bool,
    pub cover_corners: CoverCorners,
    pub cover_caption: CoverCaption,
    pub cover_hover: CoverHover,
    pub cover_fit: CoverFit,
    /// The shape of the grid's covers (`coverShape`).
    pub cover_shape: CoverShape,
    /// The star on favourites' covers (`coverFavourite`, on unless turned
    /// off).
    pub cover_favourite: bool,
    /// Covers to a row in the Focus view's column (`libraryFocusColumns`,
    /// 1 to 3); 0 fits as many as the window allows.
    pub library_focus_columns: u32,
    /// How large the Focus view's covers are (`libraryFocusCoverSize`).
    pub library_focus_cover_size: FocusCoverSize,
    /// The library switches to Focus when the window grows wide enough for
    /// its page beside the covers, and to the grid when it shrinks below
    /// that (`libraryAutoLayout`, off unless turned on).
    pub library_auto_layout: bool,
    /// A click on a cover in the grid opens the drawer about the game
    /// (`libraryOpenDetailsOnSelect`, on unless turned off); off, it only
    /// selects it, and `I`, Enter or a double click go further.
    pub library_open_details_on_select: bool,
    /// Store badges stay on every cover (`storeBadgesAlways`); off, they
    /// show on the cover pointed at or focused.
    pub store_badges_always: bool,
    /// Store badges show while the library lists one store too
    /// (`storeBadgesOneStore`, off unless turned on), where they all say the
    /// same.
    pub store_badges_one_store: bool,
    /// Game pages show the age-rating card (`showAgeRatings`, on unless
    /// turned off). The library's age filter works either way.
    pub show_age_ratings: bool,
    /// The board whose rating shows first (`ageRatingBoard`): its name
    /// ("PEGI", as [`crate::RatingSystem::from_key`] reads it), or empty for
    /// the board of the user's region.
    pub age_rating_board: String,
    /// ProtonDB tier and Steam Deck badge on covers of not-installed games
    /// (`coverCompatBadges`, on by default on Linux, opt-in elsewhere).
    pub cover_compat_badges: bool,
    /// Kinds of rows the game page's Details leave out (`hiddenDetailRows`,
    /// ids such as `publisher`); ids a later Irori adds stay as written.
    pub hidden_detail_rows: Vec<String>,
    /// Data columns shown in the library's List view (`libraryListColumns`).
    pub library_list_columns: ListColumns,
    /// Row height in the library's List view (`libraryListRowSize`).
    pub library_list_row_size: ListRowSize,
}

/// Row height in the library's List view (`libraryListRowSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListRowSize {
    Compact,
    Large,
    /// Last, so a size a newer Irori writes reads as this one.
    #[default]
    #[serde(other)]
    Medium,
}

/// Which data columns the library's List view shows (`libraryListColumns`).
/// Title and actions always stay; narrow windows still drop columns from the
/// chosen set so the title stays readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ListColumns {
    pub store: bool,
    pub size: bool,
    pub playtime: bool,
    pub last_played: bool,
    pub status: bool,
    /// ProtonDB compatibility column ("On Linux").
    pub compat: bool,
}

impl Default for ListColumns {
    fn default() -> Self {
        Self {
            store: true,
            size: false,
            playtime: true,
            last_played: true,
            status: true,
            compat: cfg!(target_os = "linux"),
        }
    }
}

/// Shader cache room games get unless the user changes it, in GiB.
pub const DEFAULT_SHADER_CACHE_GIB: u32 = 10;

/// Sizes offered for Big Picture's interface, in percent.
pub const BIG_PICTURE_SCALES: [u32; 5] = [100, 125, 150, 175, 200];

impl UiPrefs {
    /// Big Picture's scale factor, limited to the offered range.
    pub fn big_picture_zoom(&self) -> f32 {
        self.big_picture_scale.clamp(100, 200) as f32 / 100.0
    }

    /// How long [`Self::big_picture_hold_button`] is held to open Big
    /// Picture, limited to the offered range.
    pub fn big_picture_hold(&self) -> std::time::Duration {
        std::time::Duration::from_millis(u64::from(self.big_picture_hold_ms.clamp(300, 3000)))
    }

    /// The accent colour the user picked, if it is a valid `#rrggbb`.
    pub fn accent_rgb(&self) -> Option<u32> {
        let hex = self.accent_color.trim().strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    }
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            theme_name: String::new(),
            system_accent: true,
            wallpaper_colors: false,
            wallpaper_colors_file: String::new(),
            translucency: Translucency::Auto,
            font_size: 16.0,
            library_layout: LibraryLayout::Grid,
            tile_size: 11.0,
            library_drawer_width: 26.0,
            reduce_motion: false,
            middle_click_scroll: false,
            show_store_badges: true,
            cover_categories: false,
            group_copies: true,
            program_icon_art: true,
            trailer_autoplay: false,
            library_quick_controls: true,
            sidebar_collapsed: false,
            window_width: 1440.0,
            window_height: 900.0,
            window_maximized: false,
            start_maximized: false,
            window_x: None,
            window_y: None,
            window_display: String::new(),
            stores: ShownStores::default(),
            big_picture_in_game_mode: true,
            big_picture_scale: 100,
            big_picture_layout: String::new(),
            big_picture_theme: String::new(),
            big_picture_on_controller: false,
            quick_launch_shortcut: false,
            big_picture_hold_button: HoldButton::Start,
            big_picture_hold_ms: 1000,
            accent_color: String::new(),
            density: Density::Comfortable,
            font_family: String::new(),
            library_background: LibraryBackground::None,
            library_background_picture: String::new(),
            library_background_strength: 70,
            library_background_dim: 20,
            big_picture_fullscreen: true,
            cover_corners: CoverCorners::Rounded,
            cover_caption: CoverCaption::Full,
            cover_hover: CoverHover::Lift,
            cover_fit: CoverFit::Crop,
            cover_shape: CoverShape::Portrait,
            cover_favourite: true,
            library_focus_columns: 0,
            library_focus_cover_size: FocusCoverSize::Medium,
            library_auto_layout: false,
            library_open_details_on_select: true,
            store_badges_always: false,
            store_badges_one_store: false,
            show_age_ratings: true,
            age_rating_board: String::new(),
            cover_compat_badges: cfg!(target_os = "linux"),
            hidden_detail_rows: Vec::new(),
            library_list_columns: ListColumns::default(),
            library_list_row_size: ListRowSize::Medium,
        }
    }
}
