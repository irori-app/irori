//! Xbox app, Microsoft Store and PC Game Pass: games installed as Windows
//! packages (MSIX/AppX), found without any sign-in.
//!
//! Two sources list them, both written by Windows itself:
//!
//! * Every drive the Xbox app may install to has a `.GamingRoot` file at its
//!   root naming its game folders (usually `XboxGames`). Each game sits in
//!   `<folder>\<game>\Content`, with its `MicrosoftGame.config` (the GDK's
//!   description of a PC game) beside the package's `AppxManifest.xml`.
//! * Gaming Services records every game package it registered, older UWP
//!   games in `WindowsApps` included, in
//!   `HKLM\SOFTWARE\Microsoft\GamingServices\PackageRepository\Package`;
//!   the package's folder is in the AppModel package repository.
//!
//! A package counts as a game when it has a `MicrosoftGame.config`, sits in
//! a gaming root, or Gaming Services knows it: ordinary Store apps have none
//! of these. Packages without an application to start (DLC, frameworks,
//! resource packs) are left out.
//!
//! Games start through the shell (`shell:AppsFolder\<family>!<app id>`), the
//! same activation the Start menu uses. Irori never installs, updates or
//! removes them; the Xbox app does.
//!
//! Parsers here are pure and run on every OS; only [`scan_windows`] reads the
//! Windows registry and is a no-op elsewhere.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ea::decode_text;

/// The id Irori's app names for these games start with: `xbox:<family name>`.
pub const ID: &str = "xbox";
/// The Xbox app's own name, never translated.
pub const DISPLAY_NAME: &str = "Xbox app";
/// The Xbox app's package family.
pub const XBOX_APP_FAMILY: &str = "Microsoft.GamingApp_8wekyb3d8bbwe";
/// The Xbox app's application user model id, to start it.
pub const XBOX_APP_AUMID: &str = "Microsoft.GamingApp_8wekyb3d8bbwe!Microsoft.Xbox.App";
/// The file at a drive's root naming its game folders.
pub const GAMING_ROOT: &str = ".GamingRoot";
/// The GDK's description of a PC game, in its package folder.
pub const GAME_CONFIG: &str = "MicrosoftGame.config";
/// The package's manifest, in its package folder.
pub const APPX_MANIFEST: &str = "AppxManifest.xml";
/// A game's package folder inside its folder in a gaming root.
pub const CONTENT: &str = "Content";

/// Packages of the Xbox platform itself, which Gaming Services may list
/// beside the games.
const PLATFORM_PACKAGES: &[&str] = &[
    "Microsoft.GamingApp",
    "Microsoft.GamingServices",
    "Microsoft.XboxGamingOverlay",
    "Microsoft.XboxIdentityProvider",
    "Microsoft.XboxGameCallableUI",
    "Microsoft.Xbox.TCUI",
    "Microsoft.XboxSpeechToTextOverlay",
];

/// `.GamingRoot` files start with these four bytes.
const GAMING_ROOT_MAGIC: &[u8; 4] = b"RGBX";

/// The game folders a `.GamingRoot` names, relative to its drive: a magic
/// `RGBX`, a little-endian count, then that many NUL-terminated UTF-16LE
/// paths.
pub fn parse_gaming_root(bytes: &[u8]) -> anyhow::Result<Vec<String>> {
    let rest = bytes
        .strip_prefix(GAMING_ROOT_MAGIC)
        .context("The .GamingRoot file isn't one the Xbox app wrote")?;
    let (count, rest) = rest
        .split_first_chunk::<4>()
        .context("The .GamingRoot file is cut short")?;
    let count = u32::from_le_bytes(*count) as usize;
    let units: Vec<u16> = rest
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Ok(units
        .split(|u| *u == 0)
        .map(String::from_utf16_lossy)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(count)
        .collect())
}

/// The folder a `.GamingRoot` entry names on the drive at `drive`. Entries
/// are relative (`XboxGames`); a drive letter is dropped, and entries that
/// climb out of the drive are refused.
pub fn gaming_folder(drive: &Path, entry: &str) -> Option<PathBuf> {
    let relative = match entry.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => &entry[2..],
        _ => entry,
    };
    let mut path = drive.to_path_buf();
    let mut any = false;
    for part in relative.split(['\\', '/']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            _ if part.contains(':') => return None,
            _ => {
                path.push(part);
                any = true;
            }
        }
    }
    any.then_some(path)
}

/// The publisher id Windows derives from a package's publisher (its
/// certificate subject): the first 8 bytes of the SHA-256 of the UTF-16LE
/// subject, in 13 Crockford base32 characters.
pub fn publisher_id(publisher: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let utf16: Vec<u8> = publisher
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let hash = Sha256::digest(&utf16);
    let first = hash
        .first_chunk::<8>()
        .expect("a SHA-256 digest has 32 bytes");
    // 64 bits padded with a zero bit to 65, read 5 bits at a time.
    let bits = u128::from(u64::from_be_bytes(*first)) << 1;
    (0..13)
        .map(|i| ALPHABET[((bits >> (60 - 5 * i)) & 0x1f) as usize] as char)
        .collect()
}

/// A package family name: `<name>_<publisher id>`.
pub fn family_name(name: &str, publisher: &str) -> String {
    format!("{name}_{}", publisher_id(publisher))
}

/// The parts of a package full name,
/// `<name>_<version>_<architecture>_<resource id>_<publisher id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFullName {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub resource_id: String,
    pub publisher_id: String,
}

impl PackageFullName {
    pub fn parse(full_name: &str) -> Option<Self> {
        let parts: Vec<&str> = full_name.split('_').collect();
        let [name, version, architecture, resource_id, publisher_id] = parts[..] else {
            return None;
        };
        (!name.is_empty() && publisher_id.len() == 13).then(|| PackageFullName {
            name: name.to_string(),
            version: version.to_string(),
            architecture: architecture.to_string(),
            resource_id: resource_id.to_string(),
            publisher_id: publisher_id.to_string(),
        })
    }

    pub fn family_name(&self) -> String {
        format!("{}_{}", self.name, self.publisher_id)
    }
}

/// A package's identity, from either file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub publisher: String,
    pub version: String,
}

impl Identity {
    pub fn family_name(&self) -> String {
        family_name(&self.name, &self.publisher)
    }
}

/// An application a package declares in its manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub executable: Option<String>,
    pub display_name: Option<String>,
    pub square150: Option<String>,
    pub square44: Option<String>,
    pub square310: Option<String>,
    pub wide310: Option<String>,
    pub splash: Option<String>,
}

/// What Irori reads from an `AppxManifest.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub identity: Identity,
    pub display_name: Option<String>,
    pub publisher_display_name: Option<String>,
    pub logo: Option<String>,
    /// A framework or resource package, never started on its own.
    pub is_framework: bool,
    pub applications: Vec<Application>,
}

/// An executable a `MicrosoftGame.config` lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Executable {
    pub name: String,
    pub id: Option<String>,
    pub target_device_family: Option<String>,
    pub dev_only: bool,
    pub override_display_name: Option<String>,
}

impl Executable {
    /// Whether this executable is the PC game a player starts: not a
    /// development build, and not one built for a console.
    fn is_pc_game(&self) -> bool {
        !self.dev_only
            && self
                .target_device_family
                .as_deref()
                .is_none_or(|f| f.eq_ignore_ascii_case("PC"))
    }
}

/// What Irori reads from a `MicrosoftGame.config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameConfig {
    pub identity: Identity,
    pub store_id: Option<String>,
    pub title_id: Option<String>,
    pub executables: Vec<Executable>,
    pub display_name: Option<String>,
    pub publisher_display_name: Option<String>,
    pub description: Option<String>,
    pub store_logo: Option<String>,
    pub square150: Option<String>,
    pub square44: Option<String>,
    pub square480: Option<String>,
    pub splash: Option<String>,
}

/// An attribute's value, `None` when missing or empty.
fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Walks an XML document, calling `element` for every opening tag (with
/// the names of its ancestors) and `text` with every element's text once
/// it closes. Names are local: `uap:VisualElements` is `VisualElements`.
fn walk_xml(
    bytes: &[u8],
    what: &str,
    mut element: impl FnMut(&[String], &str, &BytesStart<'_>),
    mut text: impl FnMut(&[String], &str, String),
) -> anyhow::Result<()> {
    let source = decode_text(bytes);
    let mut reader = Reader::from_str(&source);
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();
    loop {
        match reader
            .read_event()
            .with_context(|| format!("The {what} isn't valid XML"))?
        {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_string();
                element(&stack, &name, &e);
                stack.push(name);
                buf.clear();
            }
            Event::Empty(e) => {
                let name = e.local_name().as_ref().to_string();
                element(&stack, &name, &e);
            }
            Event::Text(t) => buf.push_str(&t.xml10_content()),
            Event::CData(t) => buf.push_str(t.as_ref()),
            Event::GeneralRef(r) => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    buf.push(c);
                } else {
                    let entity = format!("&{};", r.xml10_content());
                    if let Ok(resolved) = quick_xml::escape::unescape(&entity) {
                        buf.push_str(&resolved);
                    }
                }
            }
            Event::End(_) => {
                let name = stack.pop().unwrap_or_default();
                text(&stack, &name, std::mem::take(&mut buf).trim().to_string());
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Reads an `AppxManifest.xml`.
pub fn parse_appx_manifest(bytes: &[u8]) -> anyhow::Result<Manifest> {
    let mut m = Manifest::default();
    let mut root_seen = false;
    walk_xml(
        bytes,
        "package's AppxManifest.xml",
        |stack, name, e| {
            let parent = stack.last().map(String::as_str);
            match (parent, name) {
                (None, "Package") => root_seen = true,
                (Some("Package"), "Identity") => {
                    m.identity = Identity {
                        name: attr(e, "Name").unwrap_or_default(),
                        publisher: attr(e, "Publisher").unwrap_or_default(),
                        version: attr(e, "Version").unwrap_or_default(),
                    };
                }
                (Some("Applications"), "Application") => m.applications.push(Application {
                    id: attr(e, "Id").unwrap_or_default(),
                    executable: attr(e, "Executable"),
                    ..Default::default()
                }),
                (Some("Application"), "VisualElements") => {
                    if let Some(app) = m.applications.last_mut() {
                        app.display_name = attr(e, "DisplayName");
                        app.square150 = attr(e, "Square150x150Logo");
                        app.square44 = attr(e, "Square44x44Logo");
                    }
                }
                (Some("VisualElements"), "DefaultTile") => {
                    if let Some(app) = m.applications.last_mut() {
                        app.wide310 = attr(e, "Wide310x150Logo");
                        app.square310 = attr(e, "Square310x310Logo");
                    }
                }
                (Some("VisualElements"), "SplashScreen") => {
                    if let Some(app) = m.applications.last_mut() {
                        app.splash = attr(e, "Image");
                    }
                }
                _ => {}
            }
        },
        |stack, name, value| {
            if stack.last().map(String::as_str) != Some("Properties") || value.is_empty() {
                return;
            }
            match name {
                "DisplayName" => m.display_name = Some(value),
                "PublisherDisplayName" => m.publisher_display_name = Some(value),
                "Logo" => m.logo = Some(value),
                "Framework" | "ResourcePackage" if value.eq_ignore_ascii_case("true") => {
                    m.is_framework = true
                }
                _ => {}
            }
        },
    )?;
    if !root_seen || m.identity.name.is_empty() || m.identity.publisher.is_empty() {
        bail!("The package's AppxManifest.xml has no identity");
    }
    m.applications.retain(|a| !a.id.is_empty());
    Ok(m)
}

/// Reads a `MicrosoftGame.config`.
pub fn parse_game_config(bytes: &[u8]) -> anyhow::Result<GameConfig> {
    let mut c = GameConfig::default();
    let mut root_seen = false;
    walk_xml(
        bytes,
        "game's MicrosoftGame.config",
        |stack, name, e| {
            let parent = stack.last().map(String::as_str);
            match (parent, name) {
                (None, "Game") => root_seen = true,
                (Some("Game"), "Identity") => {
                    c.identity = Identity {
                        name: attr(e, "Name").unwrap_or_default(),
                        publisher: attr(e, "Publisher").unwrap_or_default(),
                        version: attr(e, "Version").unwrap_or_default(),
                    };
                }
                (Some("ExecutableList"), "Executable") => {
                    if let Some(exe) = attr(e, "Name") {
                        c.executables.push(Executable {
                            name: exe,
                            id: attr(e, "Id"),
                            target_device_family: attr(e, "TargetDeviceFamily"),
                            dev_only: attr(e, "IsDevOnly")
                                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
                            override_display_name: attr(e, "OverrideDisplayName"),
                        });
                    }
                }
                (Some("Game"), "ShellVisuals") => {
                    c.display_name = attr(e, "DefaultDisplayName");
                    c.publisher_display_name = attr(e, "PublisherDisplayName");
                    c.description = attr(e, "Description");
                    c.store_logo = attr(e, "StoreLogo");
                    c.square150 = attr(e, "Square150x150Logo");
                    c.square44 = attr(e, "Square44x44Logo");
                    c.square480 = attr(e, "Square480x480Logo");
                    c.splash = attr(e, "SplashScreenImage");
                }
                _ => {}
            }
        },
        |stack, name, value| {
            if stack.last().map(String::as_str) != Some("Game") || value.is_empty() {
                return;
            }
            match name {
                "StoreId" => c.store_id = Some(value),
                "TitleId" => c.title_id = Some(value),
                _ => {}
            }
        },
    )?;
    if !root_seen {
        bail!("The game's MicrosoftGame.config has no Game element");
    }
    Ok(c)
}

/// Whether a manifest string is a reference into the package's resources
/// (`ms-resource:AppName`) rather than text.
pub fn is_resource_ref(s: &str) -> bool {
    s.trim_start()
        .get(..12)
        .is_some_and(|p| p.eq_ignore_ascii_case("ms-resource:"))
}

/// Text that is shown as is: set, and not a resource reference.
fn plain(s: Option<&String>) -> Option<&str> {
    s.map(|s| s.trim())
        .filter(|s| !s.is_empty() && !is_resource_ref(s))
}

/// The full `ms-resource://` address of a manifest's resource reference in
/// the package named `identity_name`, as Windows' resource cache keys it.
/// A bare key (`ms-resource:AppName`) lives in the package's `Resources`
/// map.
pub fn resource_uri(identity_name: &str, reference: &str) -> Option<String> {
    let rest = reference
        .trim()
        .get(12..)
        .filter(|_| is_resource_ref(reference))?;
    if let Some(full) = rest.strip_prefix("//") {
        return Some(format!("ms-resource://{full}"));
    }
    if let Some(path) = rest.strip_prefix('/') {
        return Some(format!("ms-resource://{identity_name}/{path}"));
    }
    if rest.contains('/') {
        return Some(format!("ms-resource://{identity_name}/{rest}"));
    }
    Some(format!("ms-resource://{identity_name}/Resources/{rest}"))
}

/// The package full name and resource address of an indirect string, the
/// value names Windows' resource cache (`MrtCache`) uses:
/// `@{<package full name>?<ms-resource address>}`.
pub fn parse_indirect_string(name: &str) -> Option<(String, String)> {
    let inner = name.trim().strip_prefix("@{")?.strip_suffix('}')?;
    let (package, uri) = inner.split_once('?')?;
    Some((package.trim().to_string(), uri.trim().to_string()))
}

/// The file Windows shows for a manifest's image `logical` (`Assets\Logo.png`),
/// among the file names in its folder: the largest `scale-N` variant, else the
/// largest `targetsize-N` one, else the file itself. High-contrast variants
/// are skipped. Returns the chosen path relative to the package, with `/`.
pub fn best_asset(logical: &str, files: &[String]) -> Option<String> {
    let logical = logical.replace('\\', "/");
    let (dir, file) = match logical.rsplit_once('/') {
        Some((d, f)) => (Some(d), f),
        None => (None, logical.as_str()),
    };
    let (stem, ext) = file.rsplit_once('.')?;
    let stem_dot = format!("{}.", stem.to_ascii_lowercase());
    let ext_dot = format!(".{}", ext.to_ascii_lowercase());
    let mut best: Option<(u8, u32, &String)> = None;
    for name in files {
        let lower = name.to_ascii_lowercase();
        let rank = if lower == file.to_ascii_lowercase() {
            Some((0, 0))
        } else {
            lower
                .strip_prefix(&stem_dot)
                .and_then(|q| q.strip_suffix(&ext_dot))
                .filter(|q| !q.contains("contrast-"))
                .and_then(qualifier_rank)
        };
        if let Some((kind, size)) = rank
            && best.is_none_or(|(k, s, _)| (kind, size) > (k, s))
        {
            best = Some((kind, size, name));
        }
    }
    let (_, _, name) = best?;
    Some(match dir {
        Some(d) => format!("{d}/{name}"),
        None => name.clone(),
    })
}

/// How good a qualified asset is: `scale-N` beats `targetsize-N`, larger
/// beats smaller. Qualifiers are joined with `_`.
fn qualifier_rank(qualifiers: &str) -> Option<(u8, u32)> {
    let mut rank = None;
    for q in qualifiers.split('_') {
        if let Some(n) = q.strip_prefix("scale-").and_then(|n| n.parse().ok()) {
            rank = Some((2, n));
        } else if let Some(n) = q.strip_prefix("targetsize-").and_then(|n| n.parse().ok()) {
            rank = Some((1, n));
        }
    }
    rank
}

/// The package's images, as paths on this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageArt {
    /// The largest square tile.
    pub square: Option<PathBuf>,
    /// The splash screen (wide), else the wide tile.
    pub wide: Option<PathBuf>,
    /// The small icon.
    pub icon: Option<PathBuf>,
}

/// A game the Xbox app (or the Microsoft Store) installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XboxGame {
    /// The package family name, which stays the same across updates.
    pub family_name: String,
    pub title: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    /// The application to start, in the package's manifest.
    pub app_id: String,
    /// The game's own program, relative to `install_dir`.
    pub executable: Option<String>,
    /// The package's folder, where the game's files and processes are.
    pub install_dir: PathBuf,
    /// The Microsoft Store product id, when the game names it.
    pub store_id: Option<String>,
    pub art: PackageArt,
}

impl XboxGame {
    /// Irori's app name for the game: `xbox:<family name>`.
    pub fn app_name(&self) -> String {
        format!("{ID}:{}", self.family_name)
    }

    /// The application user model id Windows starts the game by.
    pub fn aumid(&self) -> String {
        format!("{}!{}", self.family_name, self.app_id)
    }

    /// The shell address that starts the game, as the Start menu does.
    pub fn launch_uri(&self) -> String {
        format!("shell:AppsFolder\\{}", self.aumid())
    }

    /// The game's page in the Microsoft Store app.
    pub fn store_link(&self) -> String {
        match &self.store_id {
            Some(id) => format!("ms-windows-store://pdp/?ProductId={id}"),
            None => format!("ms-windows-store://pdp/?PFN={}", self.family_name),
        }
    }

    /// The game's page on the Microsoft Store website, when its product id
    /// is known.
    pub fn store_web_url(&self) -> Option<String> {
        self.store_id
            .as_ref()
            .map(|id| format!("https://apps.microsoft.com/detail/{id}"))
    }
}

/// Why a package was found, which decides whether it is a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// In a folder a `.GamingRoot` names.
    GamingRoot,
    /// Listed by Gaming Services.
    GamingServices,
    /// Anywhere else: a game only with a `MicrosoftGame.config`.
    Other,
}

/// The game a package describes, from its manifest and game config (either
/// may be missing, not both), or `None` when it isn't a game Irori lists.
/// `folder_title` is the name of its folder in a gaming root, which the Xbox
/// app names after the game; `resolve` turns resource references into text.
pub fn game_from_package(
    dir: &Path,
    manifest: Option<&Manifest>,
    config: Option<&GameConfig>,
    source: Source,
    folder_title: Option<&str>,
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) -> Option<XboxGame> {
    if source == Source::Other && config.is_none() {
        return None;
    }
    if manifest.is_some_and(|m| m.is_framework) {
        return None;
    }
    let identity = manifest
        .map(|m| &m.identity)
        .or(config.map(|c| &c.identity))
        .filter(|i| !i.name.is_empty() && !i.publisher.is_empty())?;
    if PLATFORM_PACKAGES
        .iter()
        .any(|p| identity.name.eq_ignore_ascii_case(p))
    {
        return None;
    }

    // The application to start: the config's PC game, matched to the
    // manifest's application by program or id.
    let exe = config.and_then(|c| c.executables.iter().find(|e| e.is_pc_game()));
    let apps = manifest.map(|m| m.applications.as_slice()).unwrap_or(&[]);
    let app = exe
        .and_then(|exe| {
            apps.iter().find(|a| {
                exe.id
                    .as_deref()
                    .is_some_and(|id| a.id.eq_ignore_ascii_case(id))
                    || a.executable
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(&exe.name))
            })
        })
        .or_else(|| apps.first());
    let app_id = match (app, exe) {
        (Some(app), _) => app.id.clone(),
        // The GDK names a game's application `Game` unless it says otherwise.
        (None, Some(exe)) if manifest.is_none() => exe.id.clone().unwrap_or_else(|| "Game".into()),
        _ => return None,
    };

    let text = |s: Option<&String>| -> Option<String> {
        let s = s?.trim();
        if s.is_empty() {
            None
        } else if is_resource_ref(s) {
            resolve(identity, s).filter(|r| !r.trim().is_empty())
        } else {
            Some(s.to_string())
        }
    };
    let title = exe
        .and_then(|e| plain(e.override_display_name.as_ref()).map(str::to_string))
        .or_else(|| text(config.and_then(|c| c.display_name.as_ref())))
        .or_else(|| text(app.and_then(|a| a.display_name.as_ref())))
        .or_else(|| text(manifest.and_then(|m| m.display_name.as_ref())))
        .or_else(|| folder_title.map(str::to_string))
        .unwrap_or_else(|| identity.name.clone());
    let publisher = text(config.and_then(|c| c.publisher_display_name.as_ref()))
        .or_else(|| text(manifest.and_then(|m| m.publisher_display_name.as_ref())));

    let pick = |candidates: &[Option<&String>]| -> Option<PathBuf> {
        candidates
            .iter()
            .flatten()
            .find_map(|logical| asset_on_disk(dir, logical))
    };
    let art = PackageArt {
        square: pick(&[
            config.and_then(|c| c.square480.as_ref()),
            app.and_then(|a| a.square310.as_ref()),
            config.and_then(|c| c.square150.as_ref()),
            app.and_then(|a| a.square150.as_ref()),
            config.and_then(|c| c.store_logo.as_ref()),
            manifest.and_then(|m| m.logo.as_ref()),
        ]),
        wide: pick(&[
            config.and_then(|c| c.splash.as_ref()),
            app.and_then(|a| a.splash.as_ref()),
            app.and_then(|a| a.wide310.as_ref()),
        ]),
        icon: pick(&[
            config.and_then(|c| c.square44.as_ref()),
            app.and_then(|a| a.square44.as_ref()),
            config.and_then(|c| c.store_logo.as_ref()),
            manifest.and_then(|m| m.logo.as_ref()),
        ]),
    };

    Some(XboxGame {
        family_name: identity.family_name(),
        title,
        publisher,
        description: text(config.and_then(|c| c.description.as_ref())),
        version: Some(identity.version.clone()).filter(|v| !v.is_empty()),
        app_id,
        executable: exe
            .map(|e| e.name.clone())
            .or_else(|| app.and_then(|a| a.executable.clone())),
        install_dir: dir.to_path_buf(),
        store_id: config.and_then(|c| c.store_id.clone()),
        art,
    })
}

/// The file on disk for a manifest image, at its best scale. Images that
/// would leave the package folder are ignored.
fn asset_on_disk(dir: &Path, logical: &str) -> Option<PathBuf> {
    let normalized = logical.replace('\\', "/");
    if normalized.split('/').any(|p| p == ".." || p.contains(':')) || normalized.starts_with('/') {
        return None;
    }
    let folder = match normalized.rsplit_once('/') {
        Some((d, _)) => dir.join(d),
        None => dir.to_path_buf(),
    };
    let files: Vec<String> = fs::read_dir(&folder)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let chosen = best_asset(&normalized, &files)?;
    Some(chosen.split('/').fold(dir.to_path_buf(), |p, c| p.join(c)))
}

/// The file in `dir` named `name`, matched case-insensitively.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        e.file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
            .then(|| e.path())
            .filter(|p| p.is_file())
    })
}

/// Reads the package in `dir`. `Ok(None)` when it has neither file or isn't
/// a game; an error names a file that couldn't be read.
pub fn read_package(
    dir: &Path,
    source: Source,
    folder_title: Option<&str>,
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) -> anyhow::Result<Option<XboxGame>> {
    let read = |name: &str| -> anyhow::Result<Option<Vec<u8>>> {
        match find_file(dir, name) {
            Some(path) => fs::read(&path)
                .map(Some)
                .with_context(|| format!("Could not read {}", path.display())),
            None => Ok(None),
        }
    };
    let config = read(GAME_CONFIG)?
        .map(|b| parse_game_config(&b))
        .transpose()
        .with_context(|| format!("In {}", dir.display()))?;
    let manifest = read(APPX_MANIFEST)?
        .map(|b| parse_appx_manifest(&b))
        .transpose()
        .with_context(|| format!("In {}", dir.display()))?;
    if config.is_none() && manifest.is_none() {
        return Ok(None);
    }
    Ok(game_from_package(
        dir,
        manifest.as_ref(),
        config.as_ref(),
        source,
        folder_title,
        resolve,
    ))
}

/// The games an Xbox scan found, and what it couldn't read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XboxScan {
    pub games: Vec<XboxGame>,
    /// Problems worded for the user, one per unreadable package.
    pub warnings: Vec<String>,
    /// Whether the Xbox app is installed (Windows only).
    pub xbox_app: bool,
}

impl XboxScan {
    /// Adds a game unless one of the same family was already found.
    fn add(&mut self, seen: &mut HashSet<String>, game: XboxGame) {
        if seen.insert(game.family_name.to_ascii_lowercase()) {
            self.games.push(game);
        }
    }
}

/// The games in the gaming roots of `drives` (each drive's root folder):
/// every `<folder>\<game>\Content` its `.GamingRoot` leads to. A folder
/// whose package isn't readable yet (a download in progress) is skipped.
pub fn scan_gaming_roots(
    drives: &[PathBuf],
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) -> XboxScan {
    let mut scan = XboxScan::default();
    let mut seen = HashSet::new();
    scan_gaming_roots_into(&mut scan, &mut seen, drives, resolve);
    scan
}

fn scan_gaming_roots_into(
    scan: &mut XboxScan,
    seen: &mut HashSet<String>,
    drives: &[PathBuf],
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) {
    for drive in drives {
        let Ok(bytes) = fs::read(drive.join(GAMING_ROOT)) else {
            continue;
        };
        let folders = match parse_gaming_root(&bytes) {
            Ok(f) => f,
            Err(e) => {
                scan.warnings
                    .push(format!("{e:#} ({})", drive.join(GAMING_ROOT).display()));
                continue;
            }
        };
        for folder in folders.iter().filter_map(|f| gaming_folder(drive, f)) {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            let mut games: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            games.sort();
            for game_dir in games {
                let content = game_dir.join(CONTENT);
                if !content.is_dir() {
                    continue;
                }
                let title = game_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                match read_package(&content, Source::GamingRoot, title.as_deref(), resolve) {
                    Ok(Some(game)) => scan.add(seen, game),
                    Ok(None) => {}
                    Err(e) => scan.warnings.push(format!("{e:#}")),
                }
            }
        }
    }
}

/// The games in the package folders Gaming Services lists. A folder that
/// is gone is skipped: Gaming Services may still list a game whose drive is
/// unplugged.
pub fn scan_packages(
    dirs: &[PathBuf],
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) -> XboxScan {
    let mut scan = XboxScan::default();
    let mut seen = HashSet::new();
    scan_packages_into(&mut scan, &mut seen, dirs, resolve);
    scan
}

fn scan_packages_into(
    scan: &mut XboxScan,
    seen: &mut HashSet<String>,
    dirs: &[PathBuf],
    resolve: &dyn Fn(&Identity, &str) -> Option<String>,
) {
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        match read_package(dir, Source::GamingServices, None, resolve) {
            Ok(Some(game)) => scan.add(seen, game),
            Ok(None) => {}
            Err(e) => scan.warnings.push(format!("{e:#}")),
        }
    }
}

/// The total size of the files under `dir`, without following links.
/// Blocking; can take a while on a large game.
pub fn folder_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Every Xbox game on this computer: the gaming roots of every drive, then
/// the packages Gaming Services lists. Windows only; empty elsewhere.
pub fn scan_windows() -> XboxScan {
    #[cfg(windows)]
    {
        windows::scan()
    }
    #[cfg(not(windows))]
    {
        XboxScan::default()
    }
}

#[cfg(windows)]
mod windows {
    //! The Windows side of the scan: drives, the Gaming Services list, the
    //! package repository and Windows' cache of resolved resource strings.

    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

    use super::{Identity, PackageFullName, XboxScan, parse_indirect_string, resource_uri};

    const GAMING_SERVICES: &str = r"SOFTWARE\Microsoft\GamingServices\PackageRepository\Package";
    const PACKAGE_REPOSITORY: &str = r"SOFTWARE\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\PackageRepository\Packages";
    const FAMILIES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Families";
    const MRT_CACHE: &str = r"Software\Classes\Local Settings\MrtCache";

    pub(super) fn scan() -> XboxScan {
        let strings = ResourceStrings::load();
        let resolve = |identity: &Identity, reference: &str| strings.get(identity, reference);
        let mut scan = XboxScan::default();
        let mut seen = HashSet::new();
        let drives: Vec<PathBuf> = (b'A'..=b'Z')
            .map(|l| PathBuf::from(format!("{}:\\", l as char)))
            .filter(|d| d.join(super::GAMING_ROOT).is_file())
            .collect();
        super::scan_gaming_roots_into(&mut scan, &mut seen, &drives, &resolve);
        super::scan_packages_into(&mut scan, &mut seen, &gaming_services_packages(), &resolve);
        scan.xbox_app = CURRENT_USER
            .open(format!(r"{FAMILIES}\{}", super::XBOX_APP_FAMILY))
            .is_ok();
        scan
    }

    /// The folders of the packages Gaming Services registered.
    fn gaming_services_packages() -> Vec<PathBuf> {
        let Ok(list) = LOCAL_MACHINE.open(GAMING_SERVICES) else {
            return Vec::new();
        };
        let Ok(values) = list.values() else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = values
            .filter_map(|(full_name, _)| {
                PackageFullName::parse(&full_name)?;
                let key = LOCAL_MACHINE
                    .open(format!(r"{PACKAGE_REPOSITORY}\{full_name}"))
                    .ok()?;
                key.get_string("Path").ok().map(PathBuf::from)
            })
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// The resource strings Windows already resolved for packages, keyed by
    /// package name and lower-cased `ms-resource://` address.
    struct ResourceStrings(HashMap<(String, String), String>);

    impl ResourceStrings {
        fn load() -> Self {
            let mut map = HashMap::new();
            if let Ok(cache) = CURRENT_USER.open(MRT_CACHE) {
                collect(&cache, 0, &mut map);
            }
            ResourceStrings(map)
        }

        fn get(&self, identity: &Identity, reference: &str) -> Option<String> {
            let uri = resource_uri(&identity.name, reference)?.to_lowercase();
            self.0.get(&(identity.name.to_lowercase(), uri)).cloned()
        }
    }

    /// Reads cached strings from `key` and the keys under it: the cache is
    /// one key per resource file, holding one or two levels of keys whose
    /// values are named `@{<package>?<address>}`.
    fn collect(key: &Key, depth: u8, map: &mut HashMap<(String, String), String>) {
        if let Ok(values) = key.values() {
            for (name, value) in values {
                let Some((package, uri)) = parse_indirect_string(&name) else {
                    continue;
                };
                let Some(full) = PackageFullName::parse(&package) else {
                    continue;
                };
                if let Ok(text) = String::try_from(value) {
                    map.entry((full.name.to_lowercase(), uri.to_lowercase()))
                        .or_insert(text);
                }
            }
        }
        if depth >= 3 {
            return;
        }
        if let Ok(names) = key.keys() {
            for name in names {
                if let Ok(child) = key.open(&name) {
                    collect(&child, depth + 1, map);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MICROSOFT: &str =
        "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";

    fn no_strings(_: &Identity, _: &str) -> Option<String> {
        None
    }

    pub(crate) fn gaming_root_bytes(folders: &[&str]) -> Vec<u8> {
        let mut bytes = GAMING_ROOT_MAGIC.to_vec();
        bytes.extend((folders.len() as u32).to_le_bytes());
        for f in folders {
            bytes.extend(f.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        bytes
    }

    pub(crate) const CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <Identity Name="Studio.SkyRacer" Publisher="CN=A4954634-DF4B-47C7-AB70-D3215D246AF1" Version="1.6.0.0"/>
  <MSAAppId>0000000000000000</MSAAppId>
  <TitleId>1A2B3C4D</TitleId>
  <StoreId>9NZTKMP36L8J</StoreId>
  <ExecutableList>
    <Executable Name="SkyRacer_Console.exe" TargetDeviceFamily="Scarlett" Id="Console"/>
    <Executable Name="SkyRacer_Debug.exe" TargetDeviceFamily="PC" IsDevOnly="true" Id="Debug"/>
    <Executable Name="SkyRacer.exe" TargetDeviceFamily="PC" Id="Game"/>
  </ExecutableList>
  <ShellVisuals DefaultDisplayName="Sky Racer: Tom &amp; Jerry's Cup"
    PublisherDisplayName="Example Studio"
    StoreLogo="StoreLogo.png"
    Square150x150Logo="Logo.png"
    Square44x44Logo="SmallLogo.png"
    Square480x480Logo="Logo480.png"
    Description="Race the clouds"
    SplashScreenImage="SplashScreen.png"/>
</Game>"#;

    pub(crate) const MANIFEST: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Studio.SkyRacer" Publisher="CN=A4954634-DF4B-47C7-AB70-D3215D246AF1"
            Version="1.6.0.0" ProcessorArchitecture="x64"/>
  <Properties>
    <DisplayName>ms-resource:AppDisplayName</DisplayName>
    <PublisherDisplayName>Example Studio</PublisherDisplayName>
    <Logo>StoreLogo.png</Logo>
  </Properties>
  <Applications>
    <Application Id="Game" Executable="gamelaunchhelper.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="ms-resource:AppDisplayName" Square150x150Logo="Logo.png"
          Square44x44Logo="SmallLogo.png" Description="Sky Racer" BackgroundColor="#000000">
        <uap:DefaultTile Wide310x150Logo="WideLogo.png"/>
        <uap:SplashScreen Image="SplashScreen.png"/>
      </uap:VisualElements>
    </Application>
  </Applications>
</Package>"##;

    const UWP_MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Microsoft.MicrosoftSolitaireCollection" Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US" Version="4.12.0.0"/>
  <Properties>
    <DisplayName>ms-resource:Resources/AppStoreName</DisplayName>
    <PublisherDisplayName>Microsoft Studios</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Applications>
    <Application Id="App" Executable="Solitaire.exe">
      <uap:VisualElements DisplayName="ms-resource:AppTitle" Square150x150Logo="Assets\Tile.png" Square44x44Logo="Assets\Icon.png">
        <uap:DefaultTile Square310x310Logo="Assets\LargeTile.png"/>
      </uap:VisualElements>
    </Application>
  </Applications>
</Package>"#;

    #[test]
    fn reads_gaming_roots() {
        let bytes = gaming_root_bytes(&["XboxGames"]);
        // The size Windows reports for the default file.
        assert_eq!(bytes.len(), 28);
        assert_eq!(parse_gaming_root(&bytes).unwrap(), ["XboxGames"]);
        assert_eq!(
            parse_gaming_root(&gaming_root_bytes(&["XboxGames", r"Games\Xbox"])).unwrap(),
            ["XboxGames", r"Games\Xbox"]
        );
        // The count limits the entries; trailing garbage is ignored.
        let mut extra = gaming_root_bytes(&["XboxGames"]);
        extra.extend("Other".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse_gaming_root(&extra).unwrap(), ["XboxGames"]);
        assert!(parse_gaming_root(b"XXXX\x01\0\0\0").is_err());
        assert!(parse_gaming_root(b"RGBX\x01").is_err());

        let drive = Path::new("/d");
        assert_eq!(
            gaming_folder(drive, "XboxGames"),
            Some(PathBuf::from("/d/XboxGames"))
        );
        assert_eq!(
            gaming_folder(drive, r"D:\Games\Xbox\"),
            Some(PathBuf::from("/d/Games/Xbox"))
        );
        assert_eq!(gaming_folder(drive, r"..\Windows"), None);
        assert_eq!(gaming_folder(drive, r"a\C:\b"), None);
        assert_eq!(gaming_folder(drive, r"\"), None);
    }

    #[test]
    fn derives_package_family_names() {
        assert_eq!(publisher_id(MICROSOFT), "8wekyb3d8bbwe");
        assert_eq!(
            family_name("Microsoft.GamingApp", MICROSOFT),
            XBOX_APP_FAMILY
        );
        let full = PackageFullName::parse(
            "Microsoft.MicrosoftSolitaireCollection_4.12.0.0_x64__8wekyb3d8bbwe",
        )
        .unwrap();
        assert_eq!(full.version, "4.12.0.0");
        assert_eq!(full.architecture, "x64");
        assert_eq!(full.resource_id, "");
        assert_eq!(
            full.family_name(),
            "Microsoft.MicrosoftSolitaireCollection_8wekyb3d8bbwe"
        );
        assert_eq!(PackageFullName::parse("Name_1.0_x64_8wekyb3d8bbwe"), None);
        assert_eq!(PackageFullName::parse("a_b_c_d_short"), None);
    }

    #[test]
    fn reads_game_configs() {
        let c = parse_game_config(CONFIG.as_bytes()).unwrap();
        assert_eq!(c.identity.name, "Studio.SkyRacer");
        assert_eq!(c.identity.version, "1.6.0.0");
        assert_eq!(c.store_id.as_deref(), Some("9NZTKMP36L8J"));
        assert_eq!(c.title_id.as_deref(), Some("1A2B3C4D"));
        assert_eq!(
            c.display_name.as_deref(),
            Some("Sky Racer: Tom & Jerry's Cup")
        );
        assert_eq!(c.executables.len(), 3);
        assert!(c.executables[1].dev_only);
        assert!(!c.executables[0].is_pc_game() && !c.executables[1].is_pc_game());
        assert!(c.executables[2].is_pc_game());
        assert_eq!(c.square480.as_deref(), Some("Logo480.png"));
        assert_eq!(c.splash.as_deref(), Some("SplashScreen.png"));
        assert!(parse_game_config(b"<Package/>").is_err());
        assert!(parse_game_config(b"<Game><Identity").is_err());
    }

    #[test]
    fn reads_appx_manifests() {
        let m = parse_appx_manifest(MANIFEST.as_bytes()).unwrap();
        assert_eq!(m.identity.name, "Studio.SkyRacer");
        assert_eq!(
            m.display_name.as_deref(),
            Some("ms-resource:AppDisplayName")
        );
        assert_eq!(m.publisher_display_name.as_deref(), Some("Example Studio"));
        assert!(!m.is_framework);
        let app = &m.applications[0];
        assert_eq!(app.id, "Game");
        assert_eq!(app.executable.as_deref(), Some("gamelaunchhelper.exe"));
        assert_eq!(app.square44.as_deref(), Some("SmallLogo.png"));
        assert_eq!(app.wide310.as_deref(), Some("WideLogo.png"));
        assert_eq!(app.splash.as_deref(), Some("SplashScreen.png"));

        let uwp = parse_appx_manifest(UWP_MANIFEST.as_bytes()).unwrap();
        assert_eq!(
            uwp.identity.family_name(),
            "Microsoft.MicrosoftSolitaireCollection_8wekyb3d8bbwe"
        );
        assert_eq!(
            uwp.applications[0].square310.as_deref(),
            Some(r"Assets\LargeTile.png")
        );

        let framework = parse_appx_manifest(
            br#"<Package><Identity Name="Microsoft.VCLibs.140.00" Publisher="CN=Microsoft"/>
                <Properties><Framework>true</Framework></Properties></Package>"#,
        )
        .unwrap();
        assert!(framework.is_framework && framework.applications.is_empty());
        assert!(parse_appx_manifest(b"<Package><Properties/></Package>").is_err());

        // Manifests written as UTF-16 with a byte-order mark read the same.
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(MANIFEST.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(parse_appx_manifest(&utf16).unwrap(), m);
    }

    #[test]
    fn resolves_resource_addresses() {
        assert!(is_resource_ref("ms-resource:AppName"));
        assert!(is_resource_ref("MS-RESOURCE:AppName"));
        assert!(!is_resource_ref("Halo"));
        assert_eq!(
            resource_uri("Pkg.Name", "ms-resource:AppName").as_deref(),
            Some("ms-resource://Pkg.Name/Resources/AppName")
        );
        assert_eq!(
            resource_uri("Pkg.Name", "ms-resource:Resources/AppStoreName").as_deref(),
            Some("ms-resource://Pkg.Name/Resources/AppStoreName")
        );
        assert_eq!(
            resource_uri("Pkg.Name", "ms-resource:/Strings/Title").as_deref(),
            Some("ms-resource://Pkg.Name/Strings/Title")
        );
        assert_eq!(
            resource_uri("Pkg.Name", "ms-resource://Other/Resources/X").as_deref(),
            Some("ms-resource://Other/Resources/X")
        );
        assert_eq!(resource_uri("Pkg.Name", "Halo"), None);
        assert_eq!(
            parse_indirect_string(
                "@{Pkg.Name_1.0.0.0_x64__8wekyb3d8bbwe? ms-resource://Pkg.Name/Resources/AppName}"
            ),
            Some((
                "Pkg.Name_1.0.0.0_x64__8wekyb3d8bbwe".into(),
                "ms-resource://Pkg.Name/Resources/AppName".into()
            ))
        );
        assert_eq!(parse_indirect_string("Pkg"), None);
    }

    #[test]
    fn picks_the_largest_asset() {
        let files: Vec<String> = [
            "StoreLogo.png",
            "StoreLogo.scale-100.png",
            "StoreLogo.scale-400.png",
            "StoreLogo.scale-200.png",
            "StoreLogo.scale-400_contrast-black.png",
            "Icon.targetsize-48.png",
            "Icon.targetsize-256_altform-unplated.png",
            "Icon.png",
            "Other.scale-400.png",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            best_asset(r"Assets\StoreLogo.png", &files).as_deref(),
            Some("Assets/StoreLogo.scale-400.png")
        );
        assert_eq!(
            best_asset("Icon.png", &files).as_deref(),
            Some("Icon.targetsize-256_altform-unplated.png")
        );
        assert_eq!(
            best_asset("Plain.png", &["plain.PNG".to_string()]).as_deref(),
            Some("plain.PNG")
        );
        assert_eq!(best_asset("Missing.png", &files), None);
        assert_eq!(best_asset("NoExtension", &files), None);
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }

    #[test]
    fn games_come_from_config_and_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for f in [
            "Logo480.png",
            "SplashScreen.png",
            "SmallLogo.scale-200.png",
            "SmallLogo.png",
        ] {
            write(&dir.join(f), b"png");
        }
        let config = parse_game_config(CONFIG.as_bytes()).unwrap();
        let manifest = parse_appx_manifest(MANIFEST.as_bytes()).unwrap();
        let game = game_from_package(
            dir,
            Some(&manifest),
            Some(&config),
            Source::GamingRoot,
            Some("Sky Racer"),
            &no_strings,
        )
        .unwrap();
        assert_eq!(game.title, "Sky Racer: Tom & Jerry's Cup");
        assert_eq!(game.publisher.as_deref(), Some("Example Studio"));
        assert_eq!(game.app_id, "Game");
        assert_eq!(game.executable.as_deref(), Some("SkyRacer.exe"));
        assert_eq!(game.store_id.as_deref(), Some("9NZTKMP36L8J"));
        assert_eq!(game.version.as_deref(), Some("1.6.0.0"));
        let family = family_name("Studio.SkyRacer", "CN=A4954634-DF4B-47C7-AB70-D3215D246AF1");
        assert_eq!(game.family_name, family);
        assert_eq!(game.app_name(), format!("xbox:{family}"));
        assert_eq!(
            game.launch_uri(),
            format!("shell:AppsFolder\\{family}!Game")
        );
        assert_eq!(
            game.store_link(),
            "ms-windows-store://pdp/?ProductId=9NZTKMP36L8J"
        );
        assert_eq!(
            game.store_web_url().as_deref(),
            Some("https://apps.microsoft.com/detail/9NZTKMP36L8J")
        );
        assert_eq!(game.art.square, Some(dir.join("Logo480.png")));
        assert_eq!(game.art.wide, Some(dir.join("SplashScreen.png")));
        assert_eq!(game.art.icon, Some(dir.join("SmallLogo.scale-200.png")));

        // Without a config, the manifest's resource title resolves through
        // Windows' cache, else the folder's name stands in.
        let resolve = |i: &Identity, r: &str| {
            (i.name == "Studio.SkyRacer" && r == "ms-resource:AppDisplayName")
                .then(|| "Sky Racer Deluxe".to_string())
        };
        let from_manifest = game_from_package(
            dir,
            Some(&manifest),
            None,
            Source::GamingRoot,
            Some("Folder"),
            &resolve,
        )
        .unwrap();
        assert_eq!(from_manifest.title, "Sky Racer Deluxe");
        assert_eq!(
            from_manifest.executable.as_deref(),
            Some("gamelaunchhelper.exe")
        );
        assert_eq!(from_manifest.store_id, None);
        assert!(
            from_manifest
                .store_link()
                .ends_with(&format!("?PFN={family}"))
        );
        let untitled = game_from_package(
            dir,
            Some(&manifest),
            None,
            Source::GamingRoot,
            Some("Folder"),
            &no_strings,
        )
        .unwrap();
        assert_eq!(untitled.title, "Folder");

        // A config alone starts its game's application.
        let config_only =
            game_from_package(dir, None, Some(&config), Source::Other, None, &no_strings).unwrap();
        assert_eq!(config_only.app_id, "Game");
    }

    #[test]
    fn ordinary_apps_dlc_and_the_platform_are_not_games() {
        let dir = Path::new("/nonexistent");
        let uwp = parse_appx_manifest(UWP_MANIFEST.as_bytes()).unwrap();
        // A Store app outside the gaming folders, unknown to Gaming Services.
        assert_eq!(
            game_from_package(dir, Some(&uwp), None, Source::Other, None, &no_strings),
            None
        );
        // The same package listed by Gaming Services is a game.
        let listed = game_from_package(
            dir,
            Some(&uwp),
            None,
            Source::GamingServices,
            None,
            &no_strings,
        )
        .unwrap();
        assert_eq!(listed.title, "Microsoft.MicrosoftSolitaireCollection");
        assert_eq!(listed.publisher.as_deref(), Some("Microsoft Studios"));
        assert_eq!(listed.app_id, "App");

        // DLC: a package without applications.
        let mut dlc = uwp.clone();
        dlc.applications.clear();
        assert_eq!(
            game_from_package(
                dir,
                Some(&dlc),
                None,
                Source::GamingServices,
                None,
                &no_strings
            ),
            None
        );
        // The Xbox app itself.
        let mut app = uwp.clone();
        app.identity.name = "Microsoft.GamingApp".into();
        assert_eq!(
            game_from_package(
                dir,
                Some(&app),
                None,
                Source::GamingServices,
                None,
                &no_strings
            ),
            None
        );
        // A config listing only console builds.
        let console = parse_game_config(
            br#"<Game configVersion="1"><Identity Name="A.B" Publisher="CN=X" Version="1.0.0.0"/>
                <ExecutableList><Executable Name="b.exe" TargetDeviceFamily="Scarlett"/></ExecutableList></Game>"#,
        )
        .unwrap();
        assert_eq!(
            game_from_package(
                dir,
                None,
                Some(&console),
                Source::GamingRoot,
                None,
                &no_strings
            ),
            None
        );
    }

    #[test]
    fn scans_gaming_roots_and_package_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let drive = tmp.path().join("D");
        write(&drive.join(GAMING_ROOT), &gaming_root_bytes(&["XboxGames"]));
        let games = drive.join("XboxGames");
        let sky = games.join("Sky Racer/Content");
        write(&sky.join("MicrosoftGame.config"), CONFIG.as_bytes());
        write(&sky.join("appxmanifest.xml"), MANIFEST.as_bytes());
        write(&sky.join("SkyRacer.exe"), b"MZ0123456789");
        // A download in progress: no package files yet.
        fs::create_dir_all(games.join("Downloading/Content")).unwrap();
        // A folder without Content, and a broken config.
        fs::create_dir_all(games.join("GameSave")).unwrap();
        write(
            &games.join("Broken/Content/MicrosoftGame.config"),
            b"<Game><Identity",
        );

        // A drive without a gaming root.
        let other = tmp.path().join("E");
        fs::create_dir_all(&other).unwrap();

        let scan = scan_gaming_roots(&[drive.clone(), other], &no_strings);
        assert_eq!(scan.games.len(), 1, "{:?}", scan.warnings);
        assert_eq!(scan.games[0].install_dir, sky);
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].contains("Broken"), "{:?}", scan.warnings);
        assert_eq!(
            folder_size(&sky),
            (CONFIG.len() + MANIFEST.len() + 12) as u64
        );

        // Gaming Services packages: a UWP game, a gone folder, and the same
        // game as the gaming root (kept once when merged).
        let uwp = tmp.path().join("WindowsApps/Solitaire");
        write(&uwp.join("AppxManifest.xml"), UWP_MANIFEST.as_bytes());
        let listed = scan_packages(&[uwp, tmp.path().join("gone")], &no_strings);
        assert_eq!(listed.games.len(), 1);
        assert!(listed.warnings.is_empty());

        let mut merged = XboxScan::default();
        let mut seen = HashSet::new();
        scan_gaming_roots_into(&mut merged, &mut seen, &[drive], &no_strings);
        scan_packages_into(
            &mut merged,
            &mut seen,
            std::slice::from_ref(&sky),
            &no_strings,
        );
        assert_eq!(merged.games.len(), 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn finds_nothing_outside_windows() {
        assert_eq!(scan_windows(), XboxScan::default());
    }
}
