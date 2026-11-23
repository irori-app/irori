use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::runner::{GameId, InstallPlatform, Runner};

/// Everything the launcher knows about one library entry. Built by a store and
/// shared read-only (`Arc<GameInfo>`) with the UI; mutations go through the
/// store, which publishes a fresh value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GameInfo {
    pub runner: Option<Runner>,
    pub app_name: String,
    pub title: String,
    /// Sort key derived from the title (lowercase, articles stripped).
    #[serde(skip)]
    pub sort_title: String,
    /// Other names the game is known by, for search: the store's title
    /// when the user renamed it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alt_titles: Vec<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub namespace: Option<String>,
    pub store_url: Option<String>,
    /// Vertical cover (library tile).
    pub art_cover: Option<String>,
    /// Square/landscape art used when no cover exists.
    pub art_square: Option<String>,
    pub art_logo: Option<String>,
    pub art_background: Option<String>,
    pub art_icon: Option<String>,
    pub is_installed: bool,
    pub install: Option<InstalledInfo>,
    /// False for entries that are owned but cannot be installed (e.g. browser-only).
    pub installable: bool,
    pub can_run_offline: bool,
    pub cloud_save_enabled: bool,
    /// Raw save folder template (store variables not expanded).
    pub save_folder: Option<String>,
    /// Save folder with variables expanded, once known.
    pub save_path: Option<String>,
    pub gog_save_locations: Vec<CloudSaveLocation>,
    pub folder_name: Option<String>,
    /// Name of a third-party launcher the game requires (Ubisoft Connect, EA app…).
    pub third_party_managed_app: Option<String>,
    pub is_ea_managed: bool,
    pub is_ubisoft_managed: bool,
    pub is_linux_native: bool,
    pub is_mac_native: bool,
    /// Platforms the store offers builds for.
    pub platforms: Vec<InstallPlatform>,
    /// URL for browser games.
    pub browser_url: Option<String>,
    pub custom_user_agent: Option<String>,
    pub launch_full_screen: bool,
    /// Latest store version, compared with the installed version for updates.
    pub version: Option<String>,
    pub dlcs: Vec<DlcInfo>,
    /// True when this entry is itself a DLC (hidden from the library grid).
    pub is_dlc: bool,
    pub genres: Vec<String>,
    pub release_date: Option<String>,
    /// Aggregated review score, 0–100: GOG's GamesDB, else a critics'
    /// score learned from the store page (Steam's Metacritic, IGDB).
    pub rating: Option<u8>,
    /// Age ratings the store's library data carries (Amazon, Epic), plus
    /// those services learned from the game's store page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub age_ratings: Vec<crate::AgeRating>,
    /// How the game is played, when known: from the store's data, what
    /// services learned from its store page or PCGamingWiki, or the user's
    /// own choice. Normalized ([`crate::game_mode::normalize`]).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::game_mode::lenient"
    )]
    pub game_modes: Vec<crate::GameMode>,
    /// How a controller plays it, when a source says ([`crate::controller_support`]).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::controller_support::lenient"
    )]
    pub controller_support: Option<crate::ControllerSupport>,
    /// When the account got the game, as RFC 3339, when the store says.
    pub date_added: Option<String>,
    /// The user hid the game on the store's own website.
    pub store_hidden: bool,
    pub extra: Option<ExtraInfo>,
    /// The console or platform of an emulated game (a systems catalog id
    /// such as `snes`).
    pub system: Option<String>,
    /// Short labels shown as badges: a ROM's region and revision ("USA",
    /// "Rev 1").
    pub tags: Vec<String>,
    /// The store's user tags ("Puzzle", "Co-op"), strongest first, in the
    /// interface language: Steam's, for Steam games.
    #[serde(default)]
    pub store_tags: Vec<String>,
}

impl GameInfo {
    /// Platform to install when the user picks no options: the native host
    /// build when the store offers one, else Windows.
    pub fn default_platform(&self) -> InstallPlatform {
        let native = crate::HostPlatform::current().native();
        if self.platforms.contains(&native)
            || (self.platforms.is_empty()
                && match native {
                    InstallPlatform::Linux => self.is_linux_native,
                    InstallPlatform::Mac => self.is_mac_native,
                    _ => false,
                })
        {
            native
        } else {
            InstallPlatform::Windows
        }
    }

    pub fn id(&self) -> GameId {
        GameId::new(
            self.runner.unwrap_or(Runner::Sideload),
            self.app_name.as_str(),
        )
    }

    pub fn runner(&self) -> Runner {
        self.runner.unwrap_or(Runner::Sideload)
    }

    /// Best art for a vertical tile, falling back through the alternatives.
    pub fn cover_art(&self) -> Option<&str> {
        self.art_cover
            .as_deref()
            .or(self.art_square.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// Best art for a wide hero banner.
    pub fn hero_art(&self) -> Option<&str> {
        self.art_background
            .as_deref()
            .or(self.art_square.as_deref())
            .or(self.art_cover.as_deref())
            .filter(|s| !s.is_empty())
    }

    pub fn install_path(&self) -> Option<PathBuf> {
        self.install
            .as_ref()
            .and_then(|i| (!i.install_path.is_empty()).then(|| PathBuf::from(&i.install_path)))
    }

    pub fn installed_platform(&self) -> Option<InstallPlatform> {
        self.install.as_ref().map(|i| i.platform)
    }

    /// Fill derived fields. Stores call this once after building an entry.
    pub fn finalize(mut self) -> Self {
        self.sort_title = sort_key(&self.title);
        self
    }
}

/// Lowercase title with leading English articles removed, for natural
/// sorting: numbers compare by value ("Game 2" before "Game 10") and Roman
/// numerals after the first word count as their numbers ("Final Fantasy
/// IX" before "Final Fantasy X"). Plain text order of the keys is that
/// order.
pub fn sort_key(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    let bare = ["the ", "a ", "an "]
        .iter()
        .find_map(|article| lower.strip_prefix(article))
        .unwrap_or(&lower);
    natural(bare)
}

/// Width numbers are padded to in sort keys; longer ones stay as they are.
const NUMBER_WIDTH: usize = 10;

/// `text` with each run of digits, and each Roman numeral word, written as
/// a number zero-padded to [`NUMBER_WIDTH`].
fn natural(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut word_index = 0;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c.is_ascii_digit() {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            push_number(&mut out, rest[..end].trim_start_matches('0'));
            rest = &rest[end..];
            word_index += 1;
        } else if c.is_alphanumeric() {
            let end = rest
                .find(|c: char| !c.is_alphanumeric())
                .unwrap_or(rest.len());
            let word = &rest[..end];
            match roman(word, word_index) {
                Some(n) => push_number(&mut out, &n.to_string()),
                None => out.push_str(word),
            }
            rest = &rest[end..];
            word_index += 1;
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn push_number(out: &mut String, digits: &str) {
    for _ in digits.len()..NUMBER_WIDTH {
        out.push('0');
    }
    out.push_str(digits);
}

/// The value of a Roman numeral word from II to XX, the numbering of game
/// series. A lone letter that starts the title is a word ("X-Men", "V
/// Rising"), and "I" alone is too often the pronoun.
fn roman(word: &str, word_index: usize) -> Option<u32> {
    const ROMAN: [&str; 21] = [
        "", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii",
        "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx",
    ];
    let n = ROMAN.iter().position(|r| *r == word)?;
    if n < 2 || (word.len() == 1 && word_index == 0) {
        return None;
    }
    Some(n as u32)
}

/// State of an installed game.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstalledInfo {
    pub install_path: String,
    pub executable: String,
    /// Human readable size ("12.4 GiB") as reported by the store.
    pub install_size: String,
    pub install_size_bytes: u64,
    pub version: String,
    pub platform: InstallPlatform,
    pub is_dlc: bool,
    pub installed_dlcs: Vec<String>,
    pub installed_with_dlcs: bool,
    pub language: Option<String>,
    pub version_etag: Option<String>,
    pub build_id: Option<String>,
    pub branch: Option<String>,
    /// Skip update checks (GOG version rollback).
    pub pinned_version: bool,
    pub is_dosbox: bool,
    pub sdl: Vec<String>,
    /// The Wine prefix another launcher installed the game in (a game
    /// imported from the Epic Games Launcher's prefix), where its saves and
    /// configuration are. The game's default prefix; a prefix the user chose
    /// wins.
    pub store_prefix: Option<String>,
}

/// A DLC entry of a base game.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DlcInfo {
    pub app_name: String,
    pub title: String,
    pub is_installed: bool,
    /// Owned but stored separately (installable through the base game).
    pub owned: bool,
}

/// GOG cloud-save location.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSaveLocation {
    pub name: String,
    pub location: String,
}

/// What a save folder Irori worked out itself was worked out for: the
/// store's default is a path inside the game's Wine prefix, so it is only
/// right for the prefix (and its layout: Proton keeps `drive_c` in `pfx/`)
/// it was found in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavesPathFound {
    /// The prefix the folder was found in.
    pub prefix: String,
    /// Whether that prefix was Proton's.
    pub proton: bool,
    /// The `savesPath` it found, so a folder the user typed over it since
    /// is recognised as theirs.
    pub path: String,
    /// The GOG locations (`gogSaves`) it found, likewise.
    pub gog_saves: Vec<CloudSaveLocation>,
}

impl SavesPathFound {
    /// Whether the folders are still the ones Irori found.
    pub fn is_current(&self, saves_path: &str, gog_saves: &[CloudSaveLocation]) -> bool {
        self.path == saves_path && self.gog_saves == gog_saves
    }

    /// Whether the folders are still the ones Irori found, but the game's
    /// prefix (or its layout) is no longer the one they were found in, so
    /// the store's default must be worked out again.
    pub fn is_stale(
        &self,
        saves_path: &str,
        gog_saves: &[CloudSaveLocation],
        prefix: &str,
        proton: bool,
    ) -> bool {
        self.is_current(saves_path, gog_saves) && (self.prefix != prefix || self.proton != proton)
    }
}

#[cfg(test)]
mod saves_path_found_tests {
    use super::*;

    #[test]
    fn only_a_found_folder_of_another_prefix_is_stale() {
        let found = SavesPathFound {
            prefix: "/p/a".into(),
            proton: false,
            path: "/p/a/drive_c/saves".into(),
            gog_saves: Vec::new(),
        };
        let path = "/p/a/drive_c/saves";
        assert!(!found.is_stale(path, &[], "/p/a", false));
        assert!(found.is_stale(path, &[], "/p/b", false));
        // Proton keeps drive_c in pfx/, so the layout counts too.
        assert!(found.is_stale(path, &[], "/p/a", true));
        // A folder typed over the found one is the user's.
        assert!(!found.is_stale("/mine", &[], "/p/b", false));
        let gog = [CloudSaveLocation {
            name: "saves".into(),
            location: "/mine".into(),
        }];
        assert!(!found.is_stale(path, &gog, "/p/b", false));
    }
}

/// Store-side details that are fetched lazily for the game page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtraInfo {
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub requirements: Vec<Requirement>,
    pub release_date: Option<String>,
    pub store_url: Option<String>,
    pub changelog: Option<String>,
    pub genres: Vec<String>,
    /// Full-size screenshot URLs from the store page.
    pub screenshots: Vec<String>,
    /// Trailers from the store page (Steam), the store's pick first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub videos: Vec<Video>,
    /// Age ratings from the store page (Steam, GOG).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub age_ratings: Vec<crate::AgeRating>,
    /// Game modes from the store page (Steam's categories, GOG's features).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::game_mode::lenient"
    )]
    pub game_modes: Vec<crate::GameMode>,
    /// How a controller plays it, when a source says ([`crate::controller_support`]).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::controller_support::lenient"
    )]
    pub controller_support: Option<crate::ControllerSupport>,
    /// The source that filled what the store page lacked (`"IGDB"`), named
    /// on the game page; `None` when everything is the store's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_from: Option<String>,
    /// A critics' score the store page or a source filling it gives
    /// (Steam's Metacritic, IGDB's critics' rating).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<CriticScore>,
}

/// A critics' score, 0–100, and who gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CriticScore {
    /// Shown as it is (`"Metacritic"`, `"IGDB"`); never translated.
    pub source: String,
    pub score: u8,
}

impl CriticScore {
    /// A score of `source`, when `score` is one (1–100).
    pub fn new(source: &str, score: f64) -> Option<Self> {
        let score = score.round();
        (score >= 1.0 && score <= 100.0).then(|| Self {
            source: source.to_string(),
            score: score as u8,
        })
    }
}

/// A trailer on a store page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub name: String,
    /// A still of it, shown before it plays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// What plays: an `https` file or stream address.
    pub url: String,
}

/// One row of system requirements.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Requirement {
    pub title: String,
    pub minimum: String,
    pub recommended: String,
}

/// An alternative way to launch a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LaunchOption {
    /// Extra parameters appended to the launch command.
    #[serde(alias = "basic")]
    Basic { name: String, parameters: String },
    /// Run a different executable.
    AltExe { executable: String },
    /// Launch a DLC that ships as its own game.
    Dlc {
        #[serde(rename = "dlcAppName")]
        dlc_app_name: String,
        #[serde(rename = "dlcTitle")]
        dlc_title: String,
    },
}

impl LaunchOption {
    pub fn label(&self) -> String {
        match self {
            LaunchOption::Basic { name, .. } => name.clone(),
            LaunchOption::AltExe { executable } => executable.clone(),
            LaunchOption::Dlc { dlc_title, .. } => dlc_title.clone(),
        }
    }
}

/// An achievement of a game: GOG, Epic, Steam, or RetroAchievements for
/// the ROM library.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Achievement {
    pub id: String,
    pub key: String,
    /// False for a hidden achievement: its name and description stay
    /// covered until it is unlocked or the user asks to see them.
    pub visible: bool,
    pub name: String,
    pub description: String,
    pub image_unlocked: String,
    pub image_locked: String,
    /// Percentage of players that unlocked it.
    pub rarity: f32,
    pub rarity_label: String,
    /// RFC 3339 timestamp, if unlocked.
    pub unlocked_at: Option<String>,
    /// What it is worth: RetroAchievements points, Epic XP; 0 when the
    /// store gives achievements no value.
    pub points: u32,
    /// RFC 3339 timestamp of a hardcore unlock (RetroAchievements: played
    /// without save states or rewinding).
    pub hardcore_unlocked_at: Option<String>,
    /// How far towards it the player is, `0.0..=1.0`, for achievements the
    /// store tracks progress of; `None` otherwise.
    pub progress: Option<f32>,
    /// The set it belongs to, for games whose store groups achievements
    /// (Steam's Achievement Sets: the base game, each DLC); `None` for the
    /// rest, and in answers cached before sets were read.
    pub group: Option<AchievementGroup>,
}

/// A named group of a game's achievements, such as the base game's or a
/// DLC's Achievement Set on Steam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct AchievementGroup {
    /// The store's id of the group, unique within the game.
    pub id: String,
    /// Its name in the asked language; may be empty.
    pub name: String,
    /// The app name of the DLC the group's achievements need; `None` for
    /// the base game's group.
    pub dlc_app_id: Option<String>,
    /// Where the store shows it among the game's groups, lowest first.
    pub order: u32,
    /// Whether the player can earn it: false for a DLC's set they don't
    /// own. `None` when the store doesn't say (offline, local files, a
    /// signed-out Steam, every other store).
    pub achievable: Option<bool>,
    /// RFC 3339 timestamp of when the player completed it, when the store
    /// says so.
    pub completed_at: Option<String>,
}

impl AchievementGroup {
    /// Whether the store said the player can't earn it (a DLC they don't
    /// own).
    pub fn unearnable(&self) -> bool {
        self.achievable == Some(false)
    }
}

impl Achievement {
    /// Whether the player has it, in any mode.
    pub fn is_unlocked(&self) -> bool {
        self.unlocked_at.is_some() || self.hardcore_unlocked_at.is_some()
    }

    /// Whether it is locked in a set the player can't earn
    /// ([`AchievementGroup::unearnable`]).
    pub fn is_unearnable(&self) -> bool {
        !self.is_unlocked()
            && self
                .group
                .as_ref()
                .is_some_and(AchievementGroup::unearnable)
    }

    /// When it was unlocked: the earlier of the two modes.
    pub fn unlock_time(&self) -> Option<&str> {
        match (&self.unlocked_at, &self.hardcore_unlocked_at) {
            (Some(a), Some(b)) => Some(
                if crate::util::parse_rfc3339(b) < crate::util::parse_rfc3339(a) {
                    b
                } else {
                    a
                },
            ),
            (a, b) => a.as_deref().or(b.as_deref()),
        }
    }
}

/// User account of a store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserInfo {
    pub account_id: String,
    pub display_name: String,
}

/// Per-game metadata kept by the launcher itself (not the store).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameOverrides {
    pub title: Option<String>,
    pub art_cover: Option<String>,
    pub art_square: Option<String>,
    pub art_background: Option<String>,
    pub art_logo: Option<String>,
    /// The game's modes as the user set them, replacing what stores and
    /// wikis say (an empty list says the game has none of them).
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::game_mode::lenient_option"
    )]
    pub game_modes: Option<Vec<crate::GameMode>>,
    /// How this copy joins other stores' copies of the game in one library
    /// entry, as the user decided: `Some(key)` joins the game with that
    /// `GameId::key()`, `Some("")` keeps it apart from copies Irori would
    /// match by itself, `None` lets Irori match it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<String>,
    /// The user picked this copy as the one its entry plays and installs
    /// (an installed copy still goes first).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default_copy: bool,
    /// Web pages the user keeps with the game (a wiki, a mod site, a
    /// guide), in their order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<GameLink>,
    /// The developer as the user wrote it, replacing the store's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    /// The publisher as the user wrote it, replacing the store's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// The genres as the user listed them, replacing the store's and the
    /// store page's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
}

/// A change of several games' details at once ("Edit details…"): each part
/// `None` leaves that detail as it is, `Some(None)` gives it back to the
/// store, `Some(Some(_))` sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataEdit {
    pub developer: Option<Option<String>>,
    pub publisher: Option<Option<String>>,
    pub genres: Option<Option<Vec<String>>>,
}

impl MetadataEdit {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Apply it to one game's overrides.
    pub fn apply(&self, o: &mut GameOverrides) {
        let text = |v: &Option<String>| {
            v.as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        if let Some(v) = &self.developer {
            o.developer = text(v);
        }
        if let Some(v) = &self.publisher {
            o.publisher = text(v);
        }
        if let Some(v) = &self.genres {
            o.genres = v.as_ref().map(|list| {
                let mut out: Vec<String> = Vec::new();
                for g in list.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
                    if !out.iter().any(|o| o.eq_ignore_ascii_case(g)) {
                        out.push(g.to_string());
                    }
                }
                out
            });
        }
    }
}

impl GameOverrides {
    /// Lay the user's details over `game`'s: developer, publisher, genres.
    pub fn apply_metadata(&self, game: &mut GameInfo) {
        if let Some(d) = &self.developer {
            game.developer = Some(d.clone());
        }
        if let Some(p) = &self.publisher {
            game.publisher = Some(p.clone());
        }
        if let Some(g) = &self.genres {
            game.genres = g.clone();
        }
    }
}

/// A web page the user keeps with a game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct GameLink {
    /// What the user calls it; empty shows the page's host.
    pub name: String,
    /// An `https://` or `http://` address.
    pub url: String,
}

impl GameLink {
    /// A link to `url` named `name`, when `url` is a web address (a bare
    /// host gets `https://`). Names and addresses are trimmed.
    pub fn new(name: &str, url: &str) -> Option<Self> {
        let url = url.trim();
        let url = if url.contains("://") {
            url.to_owned()
        } else {
            format!("https://{url}")
        };
        let lower = url.to_ascii_lowercase();
        let rest = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let valid = host.contains('.')
            && !host.starts_with('.')
            && !url.chars().any(|c| c.is_whitespace() || c.is_control());
        valid.then(|| Self {
            name: name.trim().to_owned(),
            url,
        })
    }

    /// The name to show: the user's, else the page's host without `www.`.
    pub fn label(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        host.strip_prefix("www.").unwrap_or(host).to_owned()
    }
}

impl GameOverrides {
    /// Whether any field replaces the store's value.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Playtime record of one game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaytimeRecord {
    /// Total minutes played.
    pub total_minutes: u64,
    /// Seconds played beyond `total_minutes` (carried to the next session).
    pub extra_seconds: u64,
    /// RFC 3339 timestamp of the last session start.
    pub last_played: Option<String>,
    /// RFC 3339 timestamp of the first session start.
    pub first_played: Option<String>,
}

pub type PlaytimeMap = BTreeMap<String, PlaytimeRecord>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_sort_in_natural_order() {
        let mut titles = vec![
            "Game 10",
            "The Game 2",
            "Final Fantasy X",
            "Final Fantasy IX",
            "Final Fantasy VII",
            "Final Fantasy XIII-2",
            "Half-Life 2",
            "Half-Life",
            "Game 02",
            "V Rising",
            "Mega Man X",
            "Mega Man 2",
        ];
        titles.sort_by_key(|t| sort_key(t));
        assert_eq!(
            titles,
            [
                "Final Fantasy VII",
                "Final Fantasy IX",
                "Final Fantasy X",
                "Final Fantasy XIII-2",
                "The Game 2",
                "Game 02",
                "Game 10",
                "Half-Life",
                "Half-Life 2",
                "Mega Man 2",
                "Mega Man X",
                "V Rising",
            ]
        );
        // Words aren't numbers, a leading lone letter or "I" stays a word.
        assert_eq!(sort_key("Civilization VI"), sort_key("Civilization 6"));
        assert_eq!(sort_key("X-Men"), "x-men");
        assert_eq!(sort_key("Rayman I"), "rayman i");
        assert_eq!(sort_key("Mix Tape"), "mix tape");
        assert_eq!(sort_key("Some Game"), "some game");
    }

    #[test]
    fn metadata_edits_set_keep_or_give_back_each_detail() {
        let mut o = GameOverrides {
            publisher: Some("Old".into()),
            ..Default::default()
        };
        MetadataEdit {
            developer: Some(Some(" Supergiant ".into())),
            genres: Some(Some(vec![
                "Roguelike".into(),
                "roguelike".into(),
                " ".into(),
            ])),
            ..Default::default()
        }
        .apply(&mut o);
        assert_eq!(o.developer.as_deref(), Some("Supergiant"));
        assert_eq!(o.publisher.as_deref(), Some("Old"), "left as it was");
        assert_eq!(o.genres, Some(vec!["Roguelike".to_string()]));
        MetadataEdit {
            developer: Some(None),
            publisher: Some(Some("  ".into())),
            ..Default::default()
        }
        .apply(&mut o);
        assert!(o.developer.is_none() && o.publisher.is_none());

        let mut game = GameInfo {
            developer: Some("Store".into()),
            genres: vec!["Action".into()],
            ..Default::default()
        };
        o.apply_metadata(&mut game);
        assert_eq!(game.developer.as_deref(), Some("Store"));
        assert_eq!(game.genres, ["Roguelike"]);
    }

    #[test]
    fn links_take_web_addresses_only() {
        let link = GameLink::new(" Wiki ", " https://hades.fandom.com/wiki ").unwrap();
        assert_eq!(link.url, "https://hades.fandom.com/wiki");
        assert_eq!(link.label(), "Wiki");
        let bare = GameLink::new("", "www.nexusmods.com/hades").unwrap();
        assert_eq!(bare.url, "https://www.nexusmods.com/hades");
        assert_eq!(bare.label(), "nexusmods.com");
        assert!(GameLink::new("", "http://example.org").is_some());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "steam://run/1",
            "",
            "notahost",
            "https://a b.com",
        ] {
            assert!(GameLink::new("x", bad).is_none(), "{bad}");
        }
    }
}
