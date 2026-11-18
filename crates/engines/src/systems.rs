//! The systems catalog (`assets/engines/systems.json`): every console or
//! platform Irori plays, the files that hold its games, how to recognise them
//! and the BIOS files it needs.
//!
//! Detecting a game file's system uses, in order: the folder it sits in
//! (`…/SNES/…`), its extension when only one system claims it, and the
//! file's header when several do (`.iso`, `.bin`). Disc images of systems
//! whose discs carry a marker file (`PS3_DISC.SFB`) are recognised by it
//! first, wherever they sit.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::iso9660;
use crate::sfo;

/// Bytes a header check may look at: enough for every rule in the catalog.
const HEADER_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderRule {
    pub offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ascii: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
}

impl HeaderRule {
    fn bytes(&self) -> Vec<u8> {
        match (&self.ascii, &self.hex) {
            (Some(a), _) => a.as_bytes().to_vec(),
            (None, Some(h)) => hex::decode(h).unwrap_or_default(),
            (None, None) => Vec::new(),
        }
    }

    pub fn matches(&self, header: &[u8]) -> bool {
        let want = self.bytes();
        !want.is_empty()
            && header
                .get(self.offset..self.offset + want.len())
                .is_some_and(|got| got == want.as_slice())
    }
}

/// How many of a system's BIOS files it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BiosRule {
    /// Games play without them; some features need them.
    #[default]
    Optional,
    /// One of the files (usually one per region) is enough.
    AnyOf,
    /// Every file is needed.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BiosFile {
    /// The file name engines expect.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Known-good MD5 digests, lower case. Empty when any dump works.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub md5: Vec<String>,
    /// For systems whose BIOS files have many names (PlayStation 2): a regex
    /// any of the user's file names may match. `name` then only describes
    /// the file, and a match is linked under its own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BiosSpec {
    #[serde(default)]
    pub rule: BiosRule,
    #[serde(default)]
    pub files: Vec<BiosFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct System {
    pub id: String,
    pub name: String,
    pub short: String,
    pub maker: String,
    pub year: u16,
    /// The system's name in libretro's databases and thumbnail sets.
    #[serde(default)]
    pub libretro: String,
    /// Background and accent colours of the generated cover.
    pub colors: [String; 2],
    /// Game file extensions, lower case, without the dot.
    pub extensions: Vec<String>,
    /// Folder names (lower case) that hold this system's games.
    pub folders: Vec<String>,
    #[serde(default)]
    pub headers: Vec<HeaderRule>,
    #[serde(default)]
    pub bios: BiosSpec,
    /// Engines that play it, best first (`engine` or `retroarch:<core>`).
    pub engines: Vec<String>,
    /// Games come on discs: `.cue` sheets own their `.bin` tracks.
    #[serde(default)]
    pub disc_images: bool,
    /// Games may span several discs, grouped into one entry.
    #[serde(default)]
    pub multi_disc: bool,
    /// Each game is a folder (DOS, ScummVM) rather than one file.
    #[serde(default)]
    pub folder_games: bool,
    /// For folder games: the file inside the folder engines open, first
    /// match wins (`PS3_GAME/USRDIR/EBOOT.BIN`). Empty opens the folder.
    #[serde(default)]
    pub folder_main: Vec<String>,
    /// For folder games: files inside the folder naming the game, the first
    /// that names one wins (`sce_sys/param.json` of PlayStation 5 games,
    /// `PARAM.SFO` of PlayStation 3 and 4 games), read instead of the
    /// folder's name, which is often only a title id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder_title: Vec<String>,
    /// For folder games: folder names that belong to the game's own layout
    /// (`PS3_GAME`, `USRDIR`, `sce_sys`) and so never name the game.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout_folders: Vec<String>,
    /// Files count as this system's games only inside one of its folders
    /// (arcade `.zip` sets, Xbox `.iso` images), never by extension alone.
    #[serde(default)]
    pub folder_only: bool,
    /// Files at the root of an ISO 9660 image that mark a disc of this
    /// system (`PS3_DISC.SFB`, `UMD_DATA.BIN`). Such an image is the
    /// system's game wherever it sits, `folder_only` systems included, and
    /// one inside a folder-games system's folder is a game of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disc_files: Vec<String>,
}

impl System {
    pub fn has_extension(&self, ext: &str) -> bool {
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
    }

    /// Whether a folder called `name` belongs to a game's own layout.
    pub fn is_layout_folder(&self, name: &str) -> bool {
        self.layout_folders
            .iter()
            .any(|f| f.eq_ignore_ascii_case(name))
    }

    /// Whether `path` may be a disc image this system recognises by its
    /// [`System::disc_files`] (judged by the extension only).
    pub fn takes_disc_image(&self, path: &Path) -> bool {
        !self.disc_files.is_empty()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| self.has_extension(e) && needs_sniff(&e.to_ascii_lowercase()))
    }

    pub fn needs_bios(&self) -> bool {
        self.bios.rule != BiosRule::Optional && !self.bios.files.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct Catalog {
    systems: Vec<System>,
}

static SYSTEMS: Lazy<Vec<System>> = Lazy::new(|| {
    serde_json::from_str::<Catalog>(include_str!("../../../assets/engines/systems.json"))
        .expect("the bundled systems catalog parses")
        .systems
});

/// Every system, in catalog order.
pub fn all() -> &'static [System] {
    &SYSTEMS
}

pub fn get(id: &str) -> Option<&'static System> {
    SYSTEMS.iter().find(|s| s.id == id)
}

/// The system whose folder name `name` is, if any.
pub fn by_folder(name: &str) -> Option<&'static System> {
    let name = normalize_folder(name);
    SYSTEMS
        .iter()
        .find(|s| s.id == name || s.folders.contains(&name))
}

/// Lower case, with `_` and `.` read as spaces and runs of spaces collapsed.
fn normalize_folder(name: &str) -> String {
    name.to_lowercase()
        .replace(['_', '.'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Systems claiming the extension `ext` on its own (not the systems that
/// need their folder).
pub fn by_extension(ext: &str) -> Vec<&'static System> {
    SYSTEMS
        .iter()
        .filter(|s| !s.folder_only && s.has_extension(ext))
        .collect()
}

/// The system of the game file at `path`. `folder_hint` is a system already
/// known from a parent folder; it wins when it accepts the extension.
pub fn detect(path: &Path, folder_hint: Option<&'static System>) -> Option<&'static System> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    // A disc's own marker file says more than the folder it was put in.
    if let Some(system) = disc_system(path) {
        return Some(system);
    }
    if let Some(hint) = folder_hint
        && (hint.has_extension(&ext) || matches!(ext.as_str(), "zip" | "7z"))
    {
        return Some(hint);
    }
    let candidates = by_extension(&ext);
    match candidates.as_slice() {
        [] => None,
        [one] if one.headers.is_empty() || !needs_sniff(&ext) => Some(one),
        _ => {
            let header = read_header(path).ok()?;
            candidates
                .iter()
                .copied()
                .find(|s| s.headers.iter().any(|r| r.matches(&header)))
                .or_else(|| {
                    // A single claimant without a matching header is still
                    // the best guess.
                    (candidates.len() == 1).then(|| candidates[0])
                })
        }
    }
}

/// Generic extensions that many systems share; their header decides.
fn needs_sniff(ext: &str) -> bool {
    matches!(ext, "iso" | "bin" | "img" | "rvz" | "gcz")
}

/// The system whose marker file sits at the root of the disc image at
/// `path` ([`System::disc_files`]). Opens the file only when its extension
/// is one such a system takes.
pub fn disc_system(path: &Path) -> Option<&'static System> {
    let claimants: Vec<&'static System> = SYSTEMS
        .iter()
        .filter(|s| s.takes_disc_image(path))
        .collect();
    if claimants.is_empty() {
        return None;
    }
    let root = iso9660::Image::open(path)?.root()?;
    claimants.into_iter().find(|s| {
        s.disc_files
            .iter()
            .any(|marker| root.iter().any(|e| e.name.eq_ignore_ascii_case(marker)))
    })
}

/// The title the disc image at `path` gives itself through the system's
/// [`System::folder_title`] files, read inside the image. Blocking.
pub fn image_title(system: &System, path: &Path) -> Option<String> {
    let mut image = iso9660::Image::open(path)?;
    system.folder_title.iter().find_map(|rel| {
        let entry = image.find(rel)?;
        let bytes = image.read(&entry, TITLE_FILE_MAX)?;
        title_of(rel, &bytes)
    })
}

/// The title in the title file `rel` (by its name) holding `bytes`.
fn title_of(rel: &str, bytes: &[u8]) -> Option<String> {
    let name = Path::new(rel).file_name()?.to_str()?.to_ascii_lowercase();
    match name.as_str() {
        "param.json" => title_of_param_json(std::str::from_utf8(bytes).ok()?),
        "param.sfo" => sfo::title(bytes),
        _ => None,
    }
}

/// Whether the game folder `folder` holds data for another game rather than
/// a game (a PlayStation 3 patch or add-on in `dev_hdd0/game`, whose
/// `PARAM.SFO` category is `GD`). Blocking.
pub fn folder_is_game_data(system: &System, folder: &Path) -> bool {
    system.folder_title.iter().any(|rel| {
        let path = folder.join(rel);
        rel.to_ascii_lowercase().ends_with("param.sfo")
            && std::fs::metadata(&path).is_ok_and(|m| m.len() <= TITLE_FILE_MAX)
            && std::fs::read(&path)
                .ok()
                .and_then(|b| sfo::string(&b, "CATEGORY"))
                .is_some_and(|c| c == "GD")
    })
}

fn read_header(path: &Path) -> Result<Vec<u8>> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = vec![0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < buf.len() {
        let n = file.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Title files bigger than this are not what they claim to be.
const TITLE_FILE_MAX: u64 = 1 << 20;

/// The title the game folder `folder` gives itself through the system's
/// [`System::folder_title`] files, the first that names one. Files bigger
/// than [`TITLE_FILE_MAX`] are skipped unread. Blocking.
pub fn folder_title(system: &System, folder: &Path) -> Option<String> {
    system.folder_title.iter().find_map(|rel| {
        let path = folder.join(rel);
        if std::fs::metadata(&path).ok()?.len() > TITLE_FILE_MAX {
            return None;
        }
        title_of(rel, &std::fs::read(&path).ok()?)
    })
}

/// The title in a PlayStation 5 `param.json`: the entry of
/// `localizedParameters` named by `defaultLanguage` (inside it or at the top
/// level), else `en-US`, else the first language listed.
pub fn title_of_param_json(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let langs = json.get("localizedParameters")?.as_object()?;
    let default = langs
        .get("defaultLanguage")
        .or_else(|| json.get("defaultLanguage"))
        .and_then(|v| v.as_str());
    let title_in = |lang: &serde_json::Value| {
        lang.get("titleName")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    default
        .and_then(|d| langs.get(d))
        .and_then(title_in)
        .or_else(|| langs.get("en-US").and_then(title_in))
        .or_else(|| langs.values().filter(|v| v.is_object()).find_map(title_in))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ps5_titles_from_param_json() {
        let json = r#"{"titleId": "PPSA00000", "localizedParameters": {
            "defaultLanguage": "fr-FR",
            "en-US": {"titleName": "Sample Game"},
            "fr-FR": {"titleName": "Jeu d'exemple"}}}"#;
        assert_eq!(title_of_param_json(json).as_deref(), Some("Jeu d'exemple"));
        let no_default = r#"{"localizedParameters": {
            "de-DE": {"titleName": "Beispiel"}, "en-US": {"titleName": " Sample "}}}"#;
        assert_eq!(title_of_param_json(no_default).as_deref(), Some("Sample"));
        let first = r#"{"localizedParameters": {"ja-JP": {"titleName": "サンプル"}}}"#;
        assert_eq!(title_of_param_json(first).as_deref(), Some("サンプル"));
        assert!(title_of_param_json(r#"{"localizedParameters": {}}"#).is_none());
        assert!(title_of_param_json("not json").is_none());

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sce_sys")).unwrap();
        std::fs::write(dir.path().join("sce_sys/param.json"), json).unwrap();
        let ps5 = get("ps5").unwrap();
        assert_eq!(
            folder_title(ps5, dir.path()).as_deref(),
            Some("Jeu d'exemple")
        );
        assert!(folder_title(get("ps4").unwrap(), dir.path()).is_none());
    }

    #[test]
    fn reads_ps3_and_ps4_titles_from_param_sfo() {
        let dir = tempfile::tempdir().unwrap();
        let ps3 = get("ps3").unwrap();
        let ps4 = get("ps4").unwrap();

        // A disc dump keeps the file in `PS3_GAME`.
        let disc = dir.path().join("BLUS30443");
        std::fs::create_dir_all(disc.join("PS3_GAME")).unwrap();
        std::fs::write(
            disc.join("PS3_GAME/PARAM.SFO"),
            sfo::build(&[("TITLE", "Demon's Souls")]),
        )
        .unwrap();
        assert_eq!(folder_title(ps3, &disc).as_deref(), Some("Demon's Souls"));

        // A `PS3_GAME` folder or a PSN game has it at the top.
        let psn = dir.path().join("NPUB30001");
        std::fs::create_dir_all(&psn).unwrap();
        std::fs::write(psn.join("PARAM.SFO"), sfo::build(&[("TITLE", "Flower")])).unwrap();
        assert_eq!(folder_title(ps3, &psn).as_deref(), Some("Flower"));

        let ps4_game = dir.path().join("CUSA00572");
        std::fs::create_dir_all(ps4_game.join("sce_sys")).unwrap();
        std::fs::write(
            ps4_game.join("sce_sys/param.sfo"),
            sfo::build(&[("TITLE", "Sample Quest"), ("TITLE_ID", "CUSA00572")]),
        )
        .unwrap();
        assert_eq!(
            folder_title(ps4, &ps4_game).as_deref(),
            Some("Sample Quest")
        );
        assert!(folder_title(ps3, &ps4_game).is_none());

        // Damaged and oversized files name nothing.
        let broken = dir.path().join("CUSA00001");
        std::fs::create_dir_all(broken.join("sce_sys")).unwrap();
        std::fs::write(broken.join("sce_sys/param.sfo"), b"\0PSF garbage").unwrap();
        assert!(folder_title(ps4, &broken).is_none());
        let mut huge = sfo::build(&[("TITLE", "Too Big")]);
        huge.resize(TITLE_FILE_MAX as usize + 1, 0);
        std::fs::write(broken.join("sce_sys/param.sfo"), huge).unwrap();
        assert!(folder_title(ps4, &broken).is_none());

        assert!(ps3.is_layout_folder("ps3_game") && ps3.is_layout_folder("USRDIR"));
        assert!(ps4.is_layout_folder("sce_sys") && !ps4.is_layout_folder("CUSA00572"));
    }

    #[test]
    fn catalog_is_consistent() {
        let ids: Vec<&str> = all().iter().map(|s| s.id.as_str()).collect();
        for (i, id) in ids.iter().enumerate() {
            assert!(!ids[..i].contains(id), "duplicate system {id}");
        }
        for s in all() {
            assert!(!s.engines.is_empty(), "{} has no engine", s.id);
            assert!(!s.extensions.is_empty(), "{} has no extensions", s.id);
            for ext in &s.extensions {
                assert_eq!(ext, &ext.to_ascii_lowercase());
            }
            for file in &s.bios.files {
                for md5 in &file.md5 {
                    assert_eq!(md5.len(), 32, "{}: {}", s.id, file.name);
                    assert_eq!(md5, &md5.to_ascii_lowercase());
                }
            }
            for rule in &s.headers {
                assert!(
                    !rule.bytes().is_empty(),
                    "{} has an empty header rule",
                    s.id
                );
            }
        }
    }

    #[test]
    fn folder_only_systems_need_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("pacman.zip");
        std::fs::write(&zip, b"PK").unwrap();
        assert!(detect(&zip, None).is_none());
        assert_eq!(detect(&zip, by_folder("MAME")).unwrap().id, "arcade");
        assert!(by_extension("zip").is_empty());
        let ps3 = get("ps3").unwrap();
        assert!(ps3.folder_games && !ps3.folder_main.is_empty());
    }

    #[test]
    fn recognises_disc_images_by_their_marker_files() {
        let dir = tempfile::tempdir().unwrap();
        let ps3 = dir.path().join("BLUS30443.iso");
        let sfo = sfo::build(&[("TITLE", "Demon's Souls™")]);
        std::fs::write(
            &ps3,
            iso9660::build(&[("PS3_DISC.SFB", b".SFB"), ("PS3_GAME/PARAM.SFO", &sfo)]),
        )
        .unwrap();
        // Wherever it sits, also in another system's folder.
        assert_eq!(detect(&ps3, None).unwrap().id, "ps3");
        assert_eq!(detect(&ps3, by_folder("PS2")).unwrap().id, "ps3");
        let system = get("ps3").unwrap();
        assert_eq!(image_title(system, &ps3).as_deref(), Some("Demon's Souls"));

        let psp = dir.path().join("game.iso");
        std::fs::write(&psp, iso9660::build(&[("UMD_DATA.BIN", b"x")])).unwrap();
        assert_eq!(detect(&psp, None).unwrap().id, "psp");

        // Without a marker the folder still decides; alone, nothing does.
        let ps2 = dir.path().join("ps2.iso");
        std::fs::write(&ps2, iso9660::build(&[("SYSTEM.CNF", b"BOOT2")])).unwrap();
        assert!(detect(&ps2, None).is_none());
        assert_eq!(detect(&ps2, by_folder("PS2")).unwrap().id, "ps2");
        assert!(image_title(system, &ps2).is_none());
    }

    #[test]
    fn folders_name_systems() {
        assert_eq!(by_folder("SNES").unwrap().id, "snes");
        assert_eq!(by_folder("Mega_Drive").unwrap().id, "genesis");
        assert_eq!(by_folder("PlayStation 1").unwrap().id, "psx");
        assert!(by_folder("Music").is_none());
    }

    #[test]
    fn detects_by_extension_folder_and_header() {
        let dir = tempfile::tempdir().unwrap();
        let sfc = dir.path().join("Chrono.sfc");
        std::fs::write(&sfc, b"x").unwrap();
        assert_eq!(detect(&sfc, None).unwrap().id, "snes");

        // `.iso` is claimed by several systems; the GameCube magic decides.
        let mut gc = vec![0u8; 64];
        gc[28..32].copy_from_slice(&[0xc2, 0x33, 0x9f, 0x3d]);
        let iso = dir.path().join("game.iso");
        std::fs::write(&iso, &gc).unwrap();
        assert_eq!(detect(&iso, None).unwrap().id, "gamecube");

        // A folder hint wins for zips and its own extensions.
        let zip = dir.path().join("game.zip");
        std::fs::write(&zip, b"PK").unwrap();
        assert!(detect(&zip, None).is_none());
        assert_eq!(detect(&zip, by_folder("gba")).unwrap().id, "gba");
        let unknown_iso = dir.path().join("other.iso");
        std::fs::write(&unknown_iso, [0u8; 64]).unwrap();
        assert_eq!(detect(&unknown_iso, by_folder("psp")).unwrap().id, "psp");
        assert!(detect(&unknown_iso, None).is_none());
    }
}
