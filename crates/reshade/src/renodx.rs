//! RenoDX: HDR mods for games made without HDR, as ReShade add-ons
//! (`renodx-<game>.addon64`, `.addon32` for 32-bit games), loaded by
//! ReShade's build with full add-on support from the folder ReShade is in.
//!
//! Only the official repository's builds are offered: the `snapshot`
//! release of clshortfuse/renodx on GitHub, rebuilt from its main branch.
//! That release carries `games-index.json`, RenoDX's own catalog (games,
//! their titles and aliases, the mods for each and their files), and
//! GitHub's listing of the release names every file with its SHA-256. A
//! mod is offered when the catalog names a file the release holds with a
//! digest; its version is the date the file was built. Mods the RenoDX
//! wiki links on other people's sites and forks are not offered: the user
//! can download one and choose the file instead.
//!
//! Downloads are checked against the release's digest and must be Windows
//! DLLs for the game's processor; they are cached by digest, never bundled.
//! The add-on is written through [`crate::journal`] as its own mod
//! ([`GraphicsMod::Renodx`]): a file of the same name, or another RenoDX
//! add-on, already in the folder refuses the install, and turning it off
//! renames it so ReShade no longer loads it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard::relative_entry_path;
use irori_core::install::ProgressSink;
use irori_core::json_store;
use irori_core::process::CancellationToken;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::ReshadeError;
use crate::fetch;
use crate::install::Step;
use crate::journal::{self, DISABLED_SUFFIX, Existing, Manifest, ModSource, Planned};
use crate::layer::GraphicsMod;
use crate::pe::{self, Machine};

/// RenoDX's catalog, in its rolling release.
pub const INDEX_URL: &str =
    "https://github.com/clshortfuse/renodx/releases/download/snapshot/games-index.json";
/// GitHub's listing of the rolling release, with every file's digest.
pub const RELEASE_URL: &str =
    "https://api.github.com/repos/clshortfuse/renodx/releases/tags/snapshot";
/// Where the release's files are downloaded from.
pub const DOWNLOADS_URL: &str = "https://github.com/clshortfuse/renodx/releases/download/snapshot/";
/// RenoDX's list of mods, for people to read.
pub const MODS_PAGE: &str = "https://github.com/clshortfuse/renodx/wiki/Mods";

/// Largest add-on Irori writes; RenoDX's are a few megabytes.
const MAX_ADDON_BYTES: u64 = 64 << 20;

/// An official add-on's file name.
static ADDON_NAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^renodx-[A-Za-z0-9][A-Za-z0-9._-]*\.addon(32|64)$").expect("valid regex")
});

static SHA256: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("valid regex"));

/// Where the catalog and the builds come from; [`Endpoints::official`]
/// outside tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub index: String,
    pub release: String,
    /// The release's download folder, ending in `/`.
    pub downloads: String,
}

impl Endpoints {
    pub fn official() -> Self {
        Self {
            index: INDEX_URL.into(),
            release: RELEASE_URL.into(),
            downloads: DOWNLOADS_URL.into(),
        }
    }
}

/// One file of a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// `renodx-<game>.addon64`.
    pub name: String,
    pub machine: Machine,
    pub sha256: String,
    pub size: u64,
    /// The day the file was built (`2026-11-10`): its version.
    pub built: String,
    pub url: String,
}

/// A mod of RenoDX's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenodxMod {
    pub id: String,
    pub title: String,
    /// Another take on the same game (`Vulkan`, `SDR`).
    pub variant: Option<String>,
    /// Titles and aliases of the games it is for.
    pub games: Vec<String>,
    /// Program names the catalog gives for those games.
    pub exes: Vec<String>,
    /// `stable`, `beta` or `experimental`, when the catalog says.
    pub status: Option<String>,
    /// A mod for an engine or any game, rather than for one game.
    pub generic: bool,
    pub builds: Vec<Build>,
}

impl RenodxMod {
    /// The name shown in lists: the title, and the variant when it has one.
    pub fn label(&self) -> String {
        match &self.variant {
            Some(v) => format!("{} ({v})", self.title),
            None => self.title.clone(),
        }
    }

    /// The file for a game built for `machine`.
    pub fn build_for(&self, machine: Machine) -> Option<&Build> {
        self.builds.iter().find(|b| b.machine == machine)
    }

    /// What a manifest records about where an install came from.
    pub fn source(&self) -> ModSource {
        ModSource {
            id: self.id.clone(),
            title: self.label(),
        }
    }
}

/// The mods RenoDX's release offers, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub mods: Vec<RenodxMod>,
}

/// A title folded for matching: lower case, letters and digits only.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn strings_at(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// A file of the release listing: its digest, size and build time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asset {
    sha256: String,
    size: u64,
    built: String,
}

/// The release listing's add-ons whose address is the release's own and
/// whose digest is a SHA-256.
fn parse_release(bytes: &[u8], downloads: &str) -> Result<HashMap<String, Asset>> {
    let value: Value = serde_json::from_slice(bytes).context("RenoDX's release list is damaged")?;
    let Some(Value::Array(assets)) = value.get("assets") else {
        bail!("RenoDX's release list names no files");
    };
    let mut out = HashMap::new();
    for asset in assets {
        let Some(name) = str_at(asset, "name").filter(|n| ADDON_NAME.is_match(n)) else {
            continue;
        };
        if str_at(asset, "browser_download_url") != Some(format!("{downloads}{name}").as_str()) {
            continue;
        }
        let Some(sha256) = str_at(asset, "digest")
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase)
            .filter(|d| SHA256.is_match(d))
        else {
            continue;
        };
        let size = asset.get("size").and_then(Value::as_u64).unwrap_or(0);
        let built = str_at(asset, "updated_at")
            .and_then(|t| t.get(..10))
            .unwrap_or_default()
            .to_string();
        out.insert(
            name.to_string(),
            Asset {
                sha256,
                size,
                built,
            },
        );
    }
    if out.is_empty() {
        bail!("RenoDX's release list has no mods");
    }
    Ok(out)
}

fn machine_of(arch: Option<&str>, name: &str) -> Option<Machine> {
    let machine = match arch {
        Some("x64") => Machine::X64,
        Some("x86") => Machine::X86,
        _ => return None,
    };
    let ext_ok = match machine {
        Machine::X64 => name.ends_with(".addon64"),
        _ => name.ends_with(".addon32"),
    };
    ext_ok.then_some(machine)
}

impl Catalog {
    /// Join RenoDX's catalog (`games-index.json`) with the release listing:
    /// only files the release holds, with a digest, are offered. A mod the
    /// catalog lists under several games is offered once.
    pub fn parse(index: &[u8], release: &[u8], downloads: &str) -> Result<Self> {
        let index: Value = serde_json::from_slice(index).context("RenoDX's catalog is damaged")?;
        let assets = parse_release(release, downloads)?;
        let Some(Value::Array(games)) = index.get("games") else {
            bail!("RenoDX's catalog lists no games");
        };
        let mut mods: Vec<RenodxMod> = Vec::new();
        for game in games {
            let mut names: Vec<String> = str_at(game, "title")
                .map(String::from)
                .into_iter()
                .collect();
            names.extend(strings_at(game, "aliases"));
            let game_exe = game.get("deploy").and_then(|d| str_at(d, "game_exe"));
            let Some(Value::Array(game_mods)) = game.get("mods") else {
                continue;
            };
            for m in game_mods {
                let Some(id) = str_at(m, "id") else {
                    continue;
                };
                let mut builds = Vec::new();
                if let Some(Value::Array(artifacts)) = m.get("artifacts") {
                    for a in artifacts {
                        let Some(name) = str_at(a, "name") else {
                            continue;
                        };
                        let (Some(asset), Some(machine)) =
                            (assets.get(name), machine_of(str_at(a, "arch"), name))
                        else {
                            continue;
                        };
                        if builds.iter().any(|b: &Build| b.machine == machine) {
                            continue;
                        }
                        builds.push(Build {
                            name: name.to_string(),
                            machine,
                            sha256: asset.sha256.clone(),
                            size: asset.size,
                            built: asset.built.clone(),
                            url: format!("{downloads}{name}"),
                        });
                    }
                }
                if builds.is_empty() {
                    continue;
                }
                let mut exes: Vec<String> = game_exe.map(String::from).into_iter().collect();
                exes.extend(
                    m.get("deploy")
                        .and_then(|d| str_at(d, "game_exe"))
                        .map(String::from),
                );
                let entry = match mods.iter_mut().find(|x| x.id == id) {
                    Some(entry) => entry,
                    None => {
                        let support = str_at(m, "support");
                        mods.push(RenodxMod {
                            id: id.to_string(),
                            title: str_at(m, "title").unwrap_or(id).to_string(),
                            variant: str_at(m, "variant").map(String::from),
                            games: Vec::new(),
                            exes: Vec::new(),
                            status: str_at(m, "status").map(String::from),
                            generic: matches!(support, Some("generic" | "engine"))
                                || str_at(m, "category") == Some("engine"),
                            builds,
                        });
                        mods.last_mut().expect("just pushed")
                    }
                };
                for name in &names {
                    if !entry.games.contains(name) {
                        entry.games.push(name.clone());
                    }
                }
                for exe in exes {
                    if !entry.exes.iter().any(|e| e.eq_ignore_ascii_case(&exe)) {
                        entry.exes.push(exe);
                    }
                }
            }
        }
        if mods.is_empty() {
            bail!("RenoDX's catalog offers no mods");
        }
        mods.sort_by_cached_key(|m| (fold(&m.label()), m.id.clone()));
        Ok(Self { mods })
    }

    pub fn get(&self, id: &str) -> Option<&RenodxMod> {
        self.mods.iter().find(|m| m.id == id)
    }

    /// The mods for a game, by its title (or one of the catalog's aliases)
    /// or its program's file name; stable mods first.
    pub fn suggest(&self, title: &str, exe_name: Option<&str>) -> Vec<&RenodxMod> {
        let title = fold(title);
        let mut found: Vec<&RenodxMod> = self
            .mods
            .iter()
            .filter(|m| {
                (!title.is_empty() && m.games.iter().any(|g| fold(g) == title))
                    || exe_name
                        .is_some_and(|exe| m.exes.iter().any(|e| e.eq_ignore_ascii_case(exe)))
            })
            .collect();
        found.sort_by_key(|m| m.status.as_deref() != Some("stable"));
        found
    }
}

async fn get(url: &str) -> Result<Vec<u8>> {
    Ok(irori_runtime::download::get_text(url).await?.into_bytes())
}

/// RenoDX's catalog, asked once a day; `fresh` asks again now (an install
/// checks its download against the digest of the current build).
pub async fn catalog(endpoints: &Endpoints, cache: &Path, fresh: bool) -> Result<Catalog> {
    let index_path = cache.join("games-index.json");
    let release_path = cache.join("snapshot.json");
    let validate_index = |b: &[u8]| -> Result<Vec<u8>> {
        serde_json::from_slice::<Value>(b).context("RenoDX's catalog is damaged")?;
        Ok(b.to_vec())
    };
    let downloads = endpoints.downloads.clone();
    let validate_release = move |b: &[u8]| -> Result<Vec<u8>> {
        parse_release(b, &downloads)?;
        Ok(b.to_vec())
    };
    let (index_url, release_url) = (endpoints.index.clone(), endpoints.release.clone());
    let fetch_index = async move || {
        get(&index_url)
            .await
            .context("Could not read RenoDX's list of mods")
    };
    let fetch_release = async move || {
        get(&release_url)
            .await
            .context("Could not read RenoDX's release")
    };
    let (index, release) = if fresh {
        (
            fetch::refreshed("RenoDX", index_path, fetch_index, validate_index).await?,
            fetch::refreshed("RenoDX", release_path, fetch_release, validate_release).await?,
        )
    } else {
        (
            fetch::cached("RenoDX", index_path, fetch_index, validate_index).await?,
            fetch::cached("RenoDX", release_path, fetch_release, validate_release).await?,
        )
    };
    Catalog::parse(&index, &release, &endpoints.downloads)
}

/// Check that `bytes` are a ReShade add-on for a game built for `machine`:
/// a Windows DLL of that architecture.
pub fn check_addon(bytes: &[u8], machine: Machine) -> Result<()> {
    let info = pe::read_bytes(bytes).map_err(|_| ReshadeError::NotAddon)?;
    if info.machine != machine {
        bail!(ReshadeError::AddonArch);
    }
    Ok(())
}

/// A build's file, from the cache when it holds that digest, else
/// downloaded and checked against the release's digest.
pub async fn download(
    build: &Build,
    cache: &Path,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    if !ADDON_NAME.is_match(&build.name) || !SHA256.is_match(&build.sha256) {
        bail!("RenoDX's release names an invalid file");
    }
    let path = cache
        .join("addons")
        .join(format!("{}-{}", &build.sha256[..16], build.name));
    let matches = |b: &[u8]| hex::encode(Sha256::digest(b)) == build.sha256;
    if let Ok(bytes) = tokio::fs::read(&path).await
        && matches(&bytes)
    {
        return Ok(bytes);
    }
    let bytes = fetch::fetch_bytes(&build.url, &cache.join("downloads"), progress, cancel)
        .await
        .with_context(|| format!("Could not download {}", build.name))?;
    if !matches(&bytes) {
        bail!(
            "The downloaded {} is not the file RenoDX's release lists",
            build.name
        );
    }
    check_addon(&bytes, build.machine)?;
    let save = bytes.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || json_store::write_bytes(&path, &save)).await?
    {
        tracing::warn!("caching {}: {e:#}", build.name);
    }
    Ok(bytes)
}

/// Whether a file name is a ReShade add-on's (`.addon`, `.addon32`,
/// `.addon64`), in any letter case.
pub fn is_addon_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".addon", ".addon32", ".addon64"]
        .iter()
        .any(|ext| lower.len() > ext.len() && lower.ends_with(ext))
}

/// An add-on file the user chose: its name and bytes, checked for the
/// game's `machine`. Blocking.
pub fn read_local(path: &Path, machine: Machine) -> Result<(String, Vec<u8>)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_addon_name(n))
        .filter(|n| relative_entry_path(Path::new(n)).as_deref() == Some(Path::new(n)))
        .ok_or(ReshadeError::NotAddon)?
        .to_string();
    let meta =
        std::fs::metadata(path).with_context(|| format!("Could not open {}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_ADDON_BYTES {
        bail!(ReshadeError::NotAddon);
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    check_addon(&bytes, machine)?;
    Ok((name, bytes))
}

/// A RenoDX add-on in `dir` other than `name`, on or turned off: two of
/// them would both hook the game. Blocking.
pub fn other_addon(dir: &Path, name: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().find_map(|entry| {
        let file = entry.file_name().to_string_lossy().into_owned();
        let base = file.strip_suffix(DISABLED_SUFFIX).unwrap_or(&file);
        let ours = base.eq_ignore_ascii_case(name);
        (!ours && base.to_ascii_lowercase().starts_with("renodx-") && is_addon_name(base))
            .then_some(file)
    })
}

/// Which add-on to install.
#[derive(Debug, Clone)]
pub enum AddonChoice {
    /// A build of RenoDX's release.
    Official { build: Build, source: ModSource },
    /// A file the user downloaded.
    File(PathBuf),
}

/// An add-on install beside an installed ReShade.
#[derive(Debug, Clone)]
pub struct AddonRequest {
    /// ReShade's manifest: the add-on goes into its folder, for its
    /// program and architecture.
    pub reshade: Manifest,
    pub choice: AddonChoice,
    /// The add-on's manifest file ([`GraphicsMod::journal_path`]).
    pub journal: PathBuf,
    /// RenoDX's cache folder.
    pub cache: PathBuf,
}

/// Install an add-on beside ReShade. ReShade must be the complete build
/// with add-on support.
pub async fn install(
    req: AddonRequest,
    step: Arc<dyn Fn(Step) + Send + Sync>,
    progress: ProgressSink,
    cancel: CancellationToken,
) -> Result<Manifest> {
    let reshade = &req.reshade;
    if reshade.kind != GraphicsMod::Reshade || !reshade.addon || !reshade.complete {
        bail!("RenoDX needs ReShade with add-on support installed first");
    }
    let machine = reshade.machine;
    let (name, bytes, version, source) = match req.choice {
        AddonChoice::Official { build, source } => {
            if build.machine != machine {
                bail!(ReshadeError::NoAddonBuild);
            }
            step(Step::DownloadingAddon);
            let bytes = download(&build, &req.cache, &progress, &cancel).await?;
            (build.name, bytes, build.built, Some(source))
        }
        AddonChoice::File(path) => {
            step(Step::Checking);
            let (name, bytes) =
                tokio::task::spawn_blocking(move || read_local(&path, machine)).await??;
            (name, bytes, String::new(), None)
        }
    };
    if cancel.is_cancelled() {
        bail!(irori_runtime::Cancelled);
    }
    step(Step::Writing);
    let base = Manifest {
        kind: GraphicsMod::Renodx,
        version,
        exe: reshade.exe.clone(),
        machine,
        api: reshade.api,
        dll: name.clone(),
        dir: reshade.dir.clone(),
        enabled: true,
        complete: false,
        overrides: Vec::new(),
        files: Vec::new(),
        created_dirs: Vec::new(),
        addon: false,
        source,
    };
    let journal = req.journal.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(other) = other_addon(&base.dir, &name) {
            bail!(ReshadeError::AddonTaken { name: other });
        }
        let files = vec![Planned::new(&name, bytes, Existing::Refuse)];
        journal::apply(&journal, base, files, &|| cancel.is_cancelled())
    })
    .await
    .context("The RenoDX install stopped")?
}

#[cfg(test)]
#[path = "renodx_tests.rs"]
mod tests;
