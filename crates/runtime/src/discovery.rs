//! Discovery of the Wine and Proton builds on this system.
//!
//! Sources, in priority order: Irori's own downloads, Heroic's downloads,
//! Steam (`compatibilitytools.d` in every library, plus Valve Proton in
//! `steamapps/common` when enabled), Lutris runners, Bottles runners, the
//! system Wine and the user's custom paths. Unlike Heroic, the result is
//! de-duplicated by canonical binary path and only lists builds that are
//! actually usable (a `wine` build needs its `wineserver`).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use irori_core::process::{CommandSpec, RunOptions};
use irori_core::{AppSettings, Paths, WineInstallation, WineKind};
use serde::{Deserialize, Serialize};

use crate::locations;

/// Where a build was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WineSource {
    /// Downloaded by Irori's Wine manager.
    Irori,
    /// Downloaded by Heroic.
    Heroic,
    /// Steam's `compatibilitytools.d` or Valve Proton.
    Steam,
    Lutris,
    Bottles,
    /// The distribution's `wine`.
    System,
    /// A path the user added in settings.
    Custom,
}

impl WineSource {
    pub fn label(self) -> &'static str {
        match self {
            WineSource::Irori => "Irori",
            WineSource::Heroic => "Heroic",
            WineSource::Steam => "Steam",
            WineSource::Lutris => "Lutris",
            WineSource::Bottles => "Bottles",
            WineSource::System => "System",
            WineSource::Custom => "Custom",
        }
    }
}

/// A discovered build together with its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWine {
    pub installation: WineInstallation,
    pub source: WineSource,
}

/// Each build with its source when another build has the same name (Steam's
/// GE-Proton and the one Irori downloaded), so a picker can tell them apart;
/// `None` when the name alone is unique.
pub fn sources_where_shared(
    builds: Vec<DiscoveredWine>,
) -> Vec<(WineInstallation, Option<WineSource>)> {
    let mut counts = std::collections::HashMap::new();
    for build in &builds {
        *counts.entry(build.installation.name.clone()).or_insert(0) += 1;
    }
    builds
        .into_iter()
        .map(|b| {
            let shared = counts.get(&b.installation.name).copied().unwrap_or(0) > 1;
            (b.installation, shared.then_some(b.source))
        })
        .collect()
}

/// Directories scanned for builds. [`ScanRoots::for_settings`] fills them for
/// this system; tests build their own.
#[derive(Debug, Clone, Default)]
pub struct ScanRoots {
    pub irori_wine: Vec<PathBuf>,
    pub irori_proton: Vec<PathBuf>,
    pub heroic_wine: Vec<PathBuf>,
    pub heroic_proton: Vec<PathBuf>,
    /// Steam library roots (each may contain `compatibilitytools.d`,
    /// `steamapps/common`, …).
    pub steam_libraries: Vec<PathBuf>,
    /// Additional `compatibilitytools.d` directories.
    pub steam_compat_dirs: Vec<PathBuf>,
    pub show_valve_proton: bool,
    /// Lutris data dirs (`~/.local/share/lutris`).
    pub lutris: Vec<PathBuf>,
    /// Bottles runner dirs (`…/bottles/runners`).
    pub bottles: Vec<PathBuf>,
    pub custom: Vec<String>,
}

/// `dirs` without repeats (the chosen folder may be the default one).
fn dedup(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.dedup();
    dirs
}

/// The Steam installs whose builds are offered: the one Irori uses, then on
/// Linux every other native, Flatpak or Snap Steam under `home` that lists
/// its libraries. A Valve Proton installed by a Steam that isn't the chosen
/// one (the Flatpak, while the setting still names the native folder) is
/// still on disk and still runs games.
fn steam_installs(steam: &Path, home: &Path) -> Vec<PathBuf> {
    let mut installs = vec![steam.to_path_buf()];
    if cfg!(target_os = "linux") {
        installs.extend(
            irori_core::platform::linux_steam_dirs(home)
                .into_iter()
                .filter(|d| d.join("steamapps/libraryfolders.vdf").is_file()),
        );
    }
    let mut seen = std::collections::HashSet::new();
    installs.retain(|d| seen.insert(locations::canonical(d)));
    installs
}

/// Folders of compatibility tools outside Steam's libraries: each known
/// Steam's `compatibilitytools.d`, the folders `STEAM_EXTRA_COMPAT_TOOLS_PATHS`
/// names (colon-separated, as Steam reads it; NixOS and other distributions
/// put packaged Protons there), and the system's.
pub fn steam_compat_dirs(steam: &Path, home: &Path, extra: Option<&str>) -> Vec<PathBuf> {
    let flatpak = home.join(".var/app/com.valvesoftware.Steam");
    let mut dirs = vec![
        steam.join("compatibilitytools.d"),
        home.join(".steam/root/compatibilitytools.d"),
        home.join(".local/share/Steam/compatibilitytools.d"),
        flatpak.join("data/Steam/compatibilitytools.d"),
        flatpak.join(".local/share/Steam/compatibilitytools.d"),
        home.join("snap/steam/common/.local/share/Steam/compatibilitytools.d"),
    ];
    dirs.extend(
        extra
            .unwrap_or_default()
            .split(':')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(irori_core::paths::expand_home),
    );
    dirs.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
    dirs.push(PathBuf::from("/usr/local/share/steam/compatibilitytools.d"));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

impl ScanRoots {
    /// The real locations on this system for `app`'s settings.
    pub fn for_settings(app: &AppSettings) -> Self {
        let home = locations::home();
        let steam = locations::steam_path(app);
        let heroic = locations::heroic_tools_dir();
        Self {
            // Builds stay where they were installed when the folder for
            // downloads changes, so both places are read.
            irori_wine: dedup(vec![
                Paths::get().wine_dir(),
                locations::wine_downloads_dir(app),
            ]),
            irori_proton: dedup(vec![
                Paths::get().proton_dir(),
                locations::proton_downloads_dir(app),
            ]),
            heroic_wine: heroic.iter().map(|d| d.join("wine")).collect(),
            heroic_proton: heroic.iter().map(|d| d.join("proton")).collect(),
            steam_libraries: steam_installs(&steam, &home)
                .iter()
                .flat_map(|install| crate::vdf::steam_libraries(install))
                .collect(),
            steam_compat_dirs: steam_compat_dirs(
                &steam,
                &home,
                std::env::var("STEAM_EXTRA_COMPAT_TOOLS_PATHS")
                    .ok()
                    .as_deref(),
            ),
            show_valve_proton: app.show_valve_proton,
            lutris: vec![
                dirs::data_dir()
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("lutris"),
                home.join(".var/app/net.lutris.Lutris/data/lutris"),
            ],
            bottles: vec![
                dirs::data_dir()
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("bottles/runners"),
                home.join(".var/app/com.usebottles.bottles/data/bottles/runners"),
            ],
            custom: app.custom_wine_paths.clone(),
        }
    }
}

/// Every usable build: scanned sources plus the system Wine, de-duplicated,
/// validated and sorted (Proton first, then Wine; within each, by source and
/// newest version first).
pub async fn discover(app: &AppSettings) -> Vec<DiscoveredWine> {
    if cfg!(windows) {
        return Vec::new();
    }
    if cfg!(target_os = "macos") {
        let app = app.clone();
        return tokio::task::spawn_blocking(move || mac_installations(&app))
            .await
            .unwrap_or_default();
    }
    let roots = ScanRoots::for_settings(app);
    let mut found = tokio::task::spawn_blocking(move || scan(&roots))
        .await
        .unwrap_or_default();
    if let Some(system) = system_wine().await {
        found.push(system);
    }
    finalize(found)
}

/// De-duplicate by canonical binary, drop unusable builds and sort.
pub fn finalize(found: Vec<DiscoveredWine>) -> Vec<DiscoveredWine> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<DiscoveredWine> = found
        .into_iter()
        .filter(|w| {
            let ok = is_valid(&w.installation);
            if !ok {
                tracing::debug!("skipping unusable Wine build {}", w.installation.bin);
            }
            ok
        })
        .filter(|w| seen.insert(locations::canonical(Path::new(&w.installation.bin))))
        .collect();
    out.sort_by(compare);
    out
}

fn kind_rank(kind: WineKind) -> u8 {
    match kind {
        WineKind::Proton => 0,
        _ => 1,
    }
}

fn compare(a: &DiscoveredWine, b: &DiscoveredWine) -> Ordering {
    kind_rank(a.installation.kind)
        .cmp(&kind_rank(b.installation.kind))
        .then(a.source.cmp(&b.source))
        .then_with(|| version_cmp(&b.installation.name, &a.installation.name))
}

/// Version-aware name comparison: `-latest` builds rank highest, digit runs
/// compare numerically (`GE-Proton10-2` > `GE-Proton9-27`).
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let latest = |s: &str| s.to_ascii_lowercase().contains("latest");
    match (latest(a), latest(b)) {
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    natural_cmp(a, b)
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(d);
                    y.next();
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let ord = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                let ord = c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Pick the build to use when none is configured: a GE or CachyOS Proton
/// (best umu support), else any Proton, else a Wine build. Within a group the
/// list order (newest first) decides.
pub fn recommended(list: &[WineInstallation]) -> Option<&WineInstallation> {
    let score = |w: &WineInstallation| {
        let name = w.name.to_ascii_lowercase();
        match w.kind {
            WineKind::Proton if name.contains("ge-proton") || name.contains("cachyos") => 0,
            WineKind::Proton => 1,
            WineKind::Wine => 2,
            _ => 3,
        }
    };
    list.iter()
        .enumerate()
        .min_by_key(|(i, w)| (score(w), *i))
        .map(|(_, w)| w)
}

/// Heroic's `validWine`: the binary exists, and for `wine` builds the
/// `wineserver` too.
pub fn is_valid(w: &WineInstallation) -> bool {
    if w.bin.is_empty() {
        return false;
    }
    let bin = unquote(&w.bin);
    if !Path::new(&bin).is_file() {
        return false;
    }
    match w.kind {
        WineKind::Wine => w
            .wineserver
            .as_deref()
            .is_some_and(|s| !s.is_empty() && Path::new(s).is_file()),
        _ => true,
    }
}

/// A present build whose runtime family supports the current host.
pub fn is_usable(w: &WineInstallation) -> bool {
    if !irori_core::HostPlatform::current().supports_wine(w.kind) || !is_valid(w) {
        return false;
    }
    if cfg!(target_os = "macos") {
        // Imported Linux Wine binaries can exist on a shared disk. Reject ELF
        // rather than trying to initialize a prefix with an incompatible build.
        use std::io::Read as _;
        let mut magic = [0; 4];
        if std::fs::File::open(unquote(&w.bin))
            .and_then(|mut f| f.read_exact(&mut magic))
            .is_ok()
            && magic == *b"\x7fELF"
        {
            return false;
        }
        if w.kind == WineKind::Toolkit && !toolkit_supported() {
            return false;
        }
    }
    true
}

/// Apple's toolkit requires Apple silicon and macOS Sonoma or later.
pub fn toolkit_supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
        && sysinfo::System::os_version()
            .and_then(|v| v.split('.').next()?.parse::<u32>().ok())
            .is_some_and(|v| v >= 14)
}

/// Old configs stored binaries wrapped in single quotes; strip them.
pub fn unquote(bin: &str) -> String {
    if bin.len() >= 2 && bin.starts_with('\'') && bin.ends_with('\'') {
        bin.replace('\'', "")
    } else {
        bin.to_string()
    }
}

/// `{wineserver}` next to a Wine binary.
pub fn wineserver_for(bin: &Path) -> Option<PathBuf> {
    let dir = bin.parent()?;
    let candidate = dir.join("wineserver");
    if candidate.is_file() {
        return Some(candidate);
    }
    let real = std::fs::canonicalize(bin).ok()?;
    let candidate = real.parent()?.join("wineserver");
    candidate.is_file().then_some(candidate)
}

/// A Wine build rooted at `dir` (`dir/bin/wine`), with its libraries.
pub fn wine_build(dir: &Path, name: &str) -> WineInstallation {
    let root = [
        dir.to_path_buf(),
        dir.join("Contents/Resources"),
        dir.join("Wine.app/Contents/Resources"),
        dir.join("Contents/Resources/wine"),
        dir.join("Wine.app/Contents/Resources/wine"),
    ]
    .into_iter()
    .find(|p| p.join("bin/wine").is_file() || p.join("bin/wine64").is_file())
    .unwrap_or_else(|| dir.to_path_buf());
    let bin = if root.join("bin/wine").is_file() {
        root.join("bin/wine")
    } else if root.join("bin/wine64").is_file() {
        root.join("bin/wine64")
    } else {
        root.join("bin/wine")
    };
    let lib = root.join("lib64");
    let lib32 = root.join("lib");
    WineInstallation {
        bin: bin.to_string_lossy().into_owned(),
        name: latest_name(dir, name),
        kind: WineKind::Wine,
        lib: lib.is_dir().then(|| lib.to_string_lossy().into_owned()),
        lib32: lib32.is_dir().then(|| lib32.to_string_lossy().into_owned()),
        wineserver: wineserver_for(&bin).map(|p| p.to_string_lossy().into_owned()),
    }
}

/// A macOS Wine bundle's own version (`CFBundleShortVersionString`, else
/// `CFBundleVersion`) from its `Info.plist`, at `dir` or in a `Wine.app`
/// inside it.
pub fn bundle_version(dir: &Path) -> Option<String> {
    ["Contents/Info.plist", "Wine.app/Contents/Info.plist"]
        .iter()
        .find_map(|p| plist::Value::from_file(dir.join(p)).ok())
        .and_then(|info| {
            let info = info.into_dictionary()?;
            ["CFBundleShortVersionString", "CFBundleVersion"]
                .iter()
                .find_map(|k| info.get(k)?.as_string().map(str::trim).map(str::to_string))
                .filter(|v| !v.is_empty())
        })
}

/// The name a build at `dir` is listed under: its folder's, and for a
/// `…-latest` build also the version it holds now, so it reads apart from
/// the numbered build it was copied from.
fn latest_name(dir: &Path, name: &str) -> String {
    if !name.to_ascii_lowercase().ends_with("-latest") {
        return name.to_string();
    }
    match bundle_version(dir) {
        Some(version) => format!("{name} ({version})"),
        None => name.to_string(),
    }
}

/// The folders the wrapper makers put their wrappers in, with the maker's
/// name: Sikarugir (Wineskin's current successor), Kegworks (its previous
/// name) and Wineskin.
const WRAPPER_MAKERS: [&str; 3] = ["Sikarugir", "Kegworks", "Wineskin"];

/// Wine inside a Sikarugir, Kegworks or Wineskin wrapper app: the engine in
/// `Contents/SharedSupport/wine` (current wrappers; some engines ship only
/// `wine`, no `wine64`), else in `Contents/Frameworks/wswine.bundle` (older
/// Wineskin).
pub fn wrapper_wine(wrapper: &Path) -> Option<PathBuf> {
    [
        "Contents/SharedSupport/wine/bin",
        "Contents/Frameworks/wswine.bundle/bin",
    ]
    .iter()
    .flat_map(|dir| ["wine64", "wine"].map(|bin| wrapper.join(dir).join(bin)))
    .find(|bin| bin.is_file())
}

/// The wrappers in `~/Applications/<maker>` and `/Applications/<maker>` under
/// each of `roots`, as Wine builds named `<wrapper> (<maker>)`. Their own
/// prefixes stay theirs: only the Wine engine is offered.
pub fn wrapper_builds(roots: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for root in roots {
        for maker in WRAPPER_MAKERS {
            for (name, dir) in sorted_entries(&root.join(maker)) {
                let Some(stem) = name.strip_suffix(".app") else {
                    continue;
                };
                if let Some(bin) = wrapper_wine(&dir) {
                    out.push((bin, format!("{stem} ({maker})")));
                }
            }
        }
    }
    out
}

/// Discover macOS Wine bundles and externally managed CrossOver/Whisky installs.
/// Nothing here initializes or alters an external application's bottles.
pub fn mac_installations(app: &AppSettings) -> Vec<DiscoveredWine> {
    let mut out = Vec::new();
    for (base, source) in [(Paths::get().tools_dir(), WineSource::Irori)]
        .into_iter()
        .chain(locations::heroic_tools_dir().map(|p| (p, WineSource::Heroic)))
    {
        scan_wine_dir(&base.join("wine"), source, &mut out);
        for (name, dir) in sorted_entries(&base.join("game-porting-toolkit")) {
            let mut installation = wine_build(&dir, &name);
            installation.kind = WineKind::Toolkit;
            out.push(DiscoveredWine {
                installation,
                source,
            });
        }
    }
    let home = locations::home();
    let mut candidates: Vec<(PathBuf, String, WineKind)> = Vec::new();
    for base in [PathBuf::from("/Applications"), home.join("Applications")] {
        // Named by the app, so CrossOver and its Preview, side by side, can
        // be told apart in the pickers.
        for name in ["CrossOver", "CrossOver Preview"] {
            candidates.push((
                base.join(format!("{name}.app"))
                    .join("Contents/SharedSupport/CrossOver/bin/wine"),
                name.into(),
                WineKind::Crossover,
            ));
        }
        for name in ["Wine Stable.app", "Wine Staging.app", "Wine Devel.app"] {
            candidates.push((
                base.join(name).join("Contents/Resources/wine/bin/wine64"),
                name.into(),
                WineKind::Wine,
            ));
        }
    }
    candidates.push((
        home.join("Library/Application Support/com.isaacmarovitz.Whisky/Libraries/Wine/bin/wine64"),
        "Whisky".into(),
        WineKind::Toolkit,
    ));
    for (bin, name) in wrapper_builds(&[home.join("Applications"), PathBuf::from("/Applications")])
    {
        candidates.push((bin, name, WineKind::Wine));
    }
    // Homebrew for Apple silicon and Homebrew for Intel (run under Rosetta)
    // can both be installed: their builds say which they are.
    for (prefix, label) in [("/opt/homebrew", ""), ("/usr/local", " (/usr/local)")] {
        for bin in ["bin/wine64", "bin/wine"] {
            candidates.push((
                Path::new(prefix).join(bin),
                format!("System Wine{label}"),
                WineKind::Wine,
            ));
        }
        candidates.push((
            Path::new(prefix).join("opt/game-porting-toolkit/bin/wine64"),
            format!("Game Porting Toolkit{label}"),
            WineKind::Toolkit,
        ));
    }
    for custom in &app.custom_wine_paths {
        let path = irori_core::paths::expand_home(&unquote(custom));
        let text = path.to_string_lossy().to_lowercase();
        let kind = if text.contains(".app/contents/sharedsupport/crossover/") {
            WineKind::Crossover
        } else if text.contains("whisky") || text.contains("game-porting-toolkit") {
            WineKind::Toolkit
        } else {
            WineKind::Wine
        };
        candidates.push((path, "Custom Wine".into(), kind));
    }
    for (bin, name, kind) in candidates {
        out.push(DiscoveredWine {
            installation: WineInstallation {
                bin: bin.to_string_lossy().into_owned(),
                name,
                kind,
                wineserver: wineserver_for(&bin).map(|p| p.to_string_lossy().into_owned()),
                ..Default::default()
            },
            source: WineSource::System,
        });
    }
    finalize(out)
        .into_iter()
        .filter(|w| is_usable(&w.installation))
        .collect()
}

/// A Proton build rooted at `dir` (`dir/proton`).
pub fn proton_build(dir: &Path, name: &str) -> WineInstallation {
    WineInstallation {
        bin: dir.join("proton").to_string_lossy().into_owned(),
        name: name.to_string(),
        kind: WineKind::Proton,
        ..Default::default()
    }
}

fn sorted_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    entries.sort();
    entries
}

fn scan_proton_dir(dir: &Path, source: WineSource, out: &mut Vec<DiscoveredWine>) {
    for (name, path) in sorted_entries(dir) {
        if name.starts_with("UMU-Latest") {
            continue;
        }
        if path.join("proton").is_file() {
            out.push(DiscoveredWine {
                installation: proton_build(&path, &name),
                source,
            });
        }
    }
}

fn scan_wine_dir(dir: &Path, source: WineSource, out: &mut Vec<DiscoveredWine>) {
    for (name, path) in sorted_entries(dir) {
        if Path::new(&wine_build(&path, &name).bin).is_file() {
            out.push(DiscoveredWine {
                installation: wine_build(&path, &name),
                source,
            });
        }
    }
}

/// Scan every directory in `roots` (blocking filesystem work).
pub fn scan(roots: &ScanRoots) -> Vec<DiscoveredWine> {
    let mut out = Vec::new();
    for d in &roots.irori_proton {
        scan_proton_dir(d, WineSource::Irori, &mut out);
    }
    for d in &roots.heroic_proton {
        scan_proton_dir(d, WineSource::Heroic, &mut out);
    }
    for lib in &roots.steam_libraries {
        if roots.show_valve_proton {
            scan_proton_dir(
                &lib.join("steam/steamapps/common"),
                WineSource::Steam,
                &mut out,
            );
            scan_proton_dir(&lib.join("steamapps/common"), WineSource::Steam, &mut out);
        }
        scan_proton_dir(
            &lib.join("root/compatibilitytools.d"),
            WineSource::Steam,
            &mut out,
        );
        scan_proton_dir(
            &lib.join("compatibilitytools.d"),
            WineSource::Steam,
            &mut out,
        );
    }
    for d in &roots.steam_compat_dirs {
        // `STEAM_EXTRA_COMPAT_TOOLS_PATHS` may name a build itself rather
        // than a folder of them.
        if d.join("proton").is_file() {
            let name = d
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(DiscoveredWine {
                installation: proton_build(d, &name),
                source: WineSource::Steam,
            });
        } else {
            scan_proton_dir(d, WineSource::Steam, &mut out);
        }
    }
    for d in &roots.irori_wine {
        scan_wine_dir(d, WineSource::Irori, &mut out);
    }
    for d in &roots.heroic_wine {
        scan_wine_dir(d, WineSource::Heroic, &mut out);
    }
    for d in &roots.lutris {
        scan_wine_dir(&d.join("runners/wine"), WineSource::Lutris, &mut out);
        scan_proton_dir(&d.join("runners/proton"), WineSource::Lutris, &mut out);
    }
    for d in &roots.bottles {
        scan_proton_dir(d, WineSource::Bottles, &mut out);
        scan_wine_dir(d, WineSource::Bottles, &mut out);
    }
    for path in &roots.custom {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let installation = if path.ends_with("proton") {
            WineInstallation {
                bin: path.to_string(),
                name: format!("Custom Proton - {path}"),
                kind: WineKind::Proton,
                ..Default::default()
            }
        } else {
            WineInstallation {
                bin: path.to_string(),
                name: format!("Custom Wine - {path}"),
                kind: WineKind::Wine,
                wineserver: wineserver_for(Path::new(path))
                    .map(|p| p.to_string_lossy().into_owned()),
                ..Default::default()
            }
        };
        out.push(DiscoveredWine {
            installation,
            source: WineSource::Custom,
        });
    }
    out
}

/// The distribution's Wine: `wine` on `PATH`, named by `wine --version`
/// (e.g. `wine-11.17 (Staging)`).
pub async fn system_wine() -> Option<DiscoveredWine> {
    let bin = which::which("wine").ok()?;
    let spec = CommandSpec::new(&bin).arg("--version");
    let res = run_with_timeout(&spec, Duration::from_secs(10)).await?;
    let name = res.stdout.lines().next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let wineserver = wineserver_for(&bin)
        .or_else(|| which::which("wineserver").ok())
        .or_else(|| {
            [
                "/usr/lib/wine/wineserver64",
                "/usr/lib/wine/wineserver",
                "/usr/lib64/wine/wineserver",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        });
    Some(DiscoveredWine {
        installation: WineInstallation {
            bin: bin.to_string_lossy().into_owned(),
            name,
            kind: WineKind::Wine,
            lib: None,
            lib32: None,
            wineserver: wineserver.map(|p| p.to_string_lossy().into_owned()),
        },
        source: WineSource::System,
    })
}

/// Run a short probe command, giving up (and killing it) after `limit`.
pub async fn run_with_timeout(
    spec: &CommandSpec,
    limit: Duration,
) -> Option<irori_core::process::ExecResult> {
    let cancel = irori_core::process::CancellationToken::new();
    let timer = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(limit).await;
            cancel.cancel();
        })
    };
    let res = irori_core::process::run(spec, RunOptions::default().cancel(cancel)).await;
    timer.abort();
    res.ok().filter(|r| !r.aborted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn mac_bundle_runtime_layouts() {
        for layout in [
            "Contents/Resources/wine",
            "Wine.app/Contents/Resources/wine",
            "Contents/Resources",
            "",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join(layout);
            touch(&root.join("bin/wine64"));
            touch(&root.join("bin/wineserver"));
            let installation = wine_build(dir.path(), "macOS Wine");
            assert_eq!(Path::new(&installation.bin), root.join("bin/wine64"));
            assert!(is_valid(&installation));
        }
    }

    #[test]
    fn finds_sikarugir_kegworks_and_wineskin_wrappers() {
        let root = tempfile::tempdir().unwrap();
        let apps = root.path().join("Applications");
        let new = apps.join("Sikarugir/Witcher.app/Contents/SharedSupport/wine/bin");
        touch(&new.join("wine"));
        touch(&new.join("wineserver"));
        let keg = apps.join("Kegworks/Hades.app/Contents/SharedSupport/wine/bin");
        touch(&keg.join("wine64"));
        touch(&keg.join("wine"));
        let old = apps.join("Wineskin/Old.app/Contents/Frameworks/wswine.bundle/bin");
        touch(&old.join("wine"));
        // Not an app, and an app without an engine.
        touch(&apps.join("Wineskin/notes.txt"));
        std::fs::create_dir_all(apps.join("Sikarugir/Empty.app/Contents")).unwrap();

        let found = wrapper_builds(std::slice::from_ref(&apps));
        let names: Vec<_> = found.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names,
            ["Witcher (Sikarugir)", "Hades (Kegworks)", "Old (Wineskin)"]
        );
        assert_eq!(found[0].0, new.join("wine"));
        assert_eq!(found[1].0, keg.join("wine64"));
        assert_eq!(found[2].0, old.join("wine"));
    }

    #[test]
    fn latest_builds_show_the_version_they_hold() {
        let dir = tempfile::tempdir().unwrap();
        let latest = dir.path().join("Wine-Crossover-latest");
        touch(&latest.join("Contents/Resources/wine/bin/wine64"));
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleShortVersionString".into(),
            plist::Value::String("23.7.1".into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(latest.join("Contents/Info.plist"))
            .unwrap();
        let numbered = dir.path().join("Wine-Crossover-23.7.1");
        std::fs::create_dir_all(&numbered).unwrap();
        std::fs::copy(
            latest.join("Contents/Info.plist"),
            dir.path().join("Info.plist"),
        )
        .unwrap();

        assert_eq!(
            wine_build(&latest, "Wine-Crossover-latest").name,
            "Wine-Crossover-latest (23.7.1)"
        );
        // A numbered build already says which it is.
        assert_eq!(
            wine_build(&numbered, "Wine-Crossover-23.7.1").name,
            "Wine-Crossover-23.7.1"
        );
        // Without a bundle version the folder's name stays.
        let bare = dir.path().join("GE-latest");
        touch(&bare.join("bin/wine"));
        assert_eq!(wine_build(&bare, "GE-latest").name, "GE-latest");
    }

    #[test]
    fn builds_sharing_a_name_carry_their_source() {
        let build = |name: &str, bin: &str, source| DiscoveredWine {
            installation: WineInstallation {
                bin: bin.into(),
                name: name.into(),
                kind: WineKind::Proton,
                ..Default::default()
            },
            source,
        };
        let labelled = sources_where_shared(vec![
            build("GE-Proton10-2", "/irori/proton", WineSource::Irori),
            build("GE-Proton10-2", "/steam/proton", WineSource::Steam),
            build("Proton 9.0", "/valve/proton", WineSource::Steam),
        ]);
        let sources: Vec<_> = labelled.iter().map(|(_, s)| *s).collect();
        assert_eq!(
            sources,
            [Some(WineSource::Irori), Some(WineSource::Steam), None]
        );
    }

    #[test]
    fn natural_ordering() {
        let mut names = vec![
            "GE-Proton9-27",
            "GE-Proton10-2",
            "GE-Proton-latest",
            "GE-Proton9-3",
            "Proton-CachyOS-latest",
        ];
        names.sort_by(|a, b| version_cmp(b, a));
        assert_eq!(
            names,
            vec![
                "Proton-CachyOS-latest",
                "GE-Proton-latest",
                "GE-Proton10-2",
                "GE-Proton9-27",
                "GE-Proton9-3"
            ]
        );
    }

    #[test]
    fn scans_dedupes_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("irori/proton/GE-Proton9-27/proton"));
        touch(&root.join("irori/proton/GE-Proton10-2/proton"));
        touch(&root.join("irori/proton/UMU-Latest/proton"));
        std::fs::create_dir_all(root.join("irori/proton/broken")).unwrap();
        touch(&root.join("irori/wine/wine-ge-8-26/bin/wine"));
        touch(&root.join("irori/wine/wine-ge-8-26/bin/wineserver"));
        std::fs::create_dir_all(root.join("irori/wine/wine-ge-8-26/lib64")).unwrap();
        touch(&root.join("irori/wine/no-server/bin/wine"));
        let steam = root.join("steam");
        touch(&steam.join("compatibilitytools.d/GE-Proton9-27/proton"));
        touch(&steam.join("steamapps/common/Proton 9.0/proton"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&steam, root.join("steam-link")).unwrap();
        touch(&root.join("lutris/runners/wine/lutris-7.2/bin/wine"));
        touch(&root.join("lutris/runners/wine/lutris-7.2/bin/wineserver"));
        touch(&root.join("bottles/soda-9/bin/wine"));
        touch(&root.join("bottles/soda-9/bin/wineserver"));
        touch(&root.join("bottles/ge-proton/proton"));
        let custom = root.join("custom/proton");
        touch(&custom);

        let mut roots = ScanRoots {
            irori_wine: vec![root.join("irori/wine")],
            irori_proton: vec![root.join("irori/proton")],
            steam_libraries: vec![steam.clone()],
            steam_compat_dirs: vec![root.join("steam-link/compatibilitytools.d")],
            lutris: vec![root.join("lutris")],
            bottles: vec![root.join("bottles")],
            custom: vec![
                custom.to_string_lossy().into_owned(),
                "/nonexistent/wine".into(),
            ],
            ..Default::default()
        };
        let list = finalize(scan(&roots));
        let names: Vec<(&str, WineSource)> = list
            .iter()
            .map(|w| (w.installation.name.as_str(), w.source))
            .collect();
        let custom_name = format!("Custom Proton - {}", custom.display());
        assert_eq!(
            names,
            vec![
                ("GE-Proton10-2", WineSource::Irori),
                ("GE-Proton9-27", WineSource::Irori),
                ("GE-Proton9-27", WineSource::Steam),
                ("ge-proton", WineSource::Bottles),
                (custom_name.as_str(), WineSource::Custom),
                ("wine-ge-8-26", WineSource::Irori),
                ("lutris-7.2", WineSource::Lutris),
                ("soda-9", WineSource::Bottles),
            ]
        );
        let ge = &list[5].installation;
        assert!(ge.lib.as_deref().unwrap().ends_with("lib64"));
        assert!(ge.lib32.is_none());
        assert!(Path::new(ge.wineserver.as_deref().unwrap()).ends_with("bin/wineserver"));

        roots.show_valve_proton = true;
        let list = finalize(scan(&roots));
        assert!(list.iter().any(|w| w.installation.name == "Proton 9.0"));
        let installs: Vec<WineInstallation> = list.into_iter().map(|w| w.installation).collect();
        assert_eq!(recommended(&installs).unwrap().name, "GE-Proton10-2");
    }

    /// Heroic #5988: builds Steam itself would find through
    /// `STEAM_EXTRA_COMPAT_TOOLS_PATHS` (a folder of tools, or one tool).
    // Steam's variable separates folders with ':', as on Linux.
    #[cfg(unix)]
    #[test]
    fn extra_compat_tool_paths_are_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("home");
        touch(&root.join("nix/tools/GE-Proton10-1/proton"));
        touch(&root.join("nix/proton-cachyos/proton"));
        let extra = format!(
            "{}::{} ",
            root.join("nix/tools").display(),
            root.join("nix/proton-cachyos").display()
        );
        let dirs = steam_compat_dirs(&home.join(".steam/steam"), &home, Some(&extra));
        assert!(dirs.contains(&root.join("nix/tools")));
        assert!(dirs.contains(&root.join("nix/proton-cachyos")));
        assert!(
            dirs.contains(&home.join("snap/steam/common/.local/share/Steam/compatibilitytools.d"))
        );
        assert_eq!(
            dirs.last(),
            Some(&PathBuf::from(
                "/usr/local/share/steam/compatibilitytools.d"
            ))
        );
        let roots = ScanRoots {
            steam_compat_dirs: dirs,
            ..Default::default()
        };
        let names: Vec<String> = finalize(scan(&roots))
            .into_iter()
            .map(|w| w.installation.name)
            .collect();
        assert!(names.contains(&"GE-Proton10-1".to_string()));
        assert!(names.contains(&"proton-cachyos".to_string()));
        assert!(steam_compat_dirs(Path::new("/s"), Path::new("/h"), None).len() >= 8);
    }

    /// Heroic #5953: Valve's Proton from a Flatpak Steam is offered while
    /// the setting still names the native Steam's folder.
    #[cfg(target_os = "linux")]
    #[test]
    fn every_steam_install_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let native = home.join(".steam/steam");
        std::fs::create_dir_all(native.join("steamapps")).unwrap();
        let flatpak = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        touch(&flatpak.join("steamapps/libraryfolders.vdf"));
        let installs = steam_installs(&native, home);
        assert_eq!(installs, vec![native.clone(), flatpak]);
        assert_eq!(steam_installs(&native, home).len(), 2, "listed once");
    }

    #[test]
    fn recommendation_falls_back() {
        let wine = WineInstallation {
            name: "wine-11".into(),
            kind: WineKind::Wine,
            ..Default::default()
        };
        let valve = WineInstallation {
            name: "Proton 9.0".into(),
            kind: WineKind::Proton,
            ..Default::default()
        };
        assert_eq!(recommended(&[wine.clone(), valve.clone()]), Some(&valve));
        assert_eq!(recommended(std::slice::from_ref(&wine)), Some(&wine));
        assert_eq!(recommended(&[]), None);
    }

    #[test]
    fn validation() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin/wine");
        touch(&bin);
        let mut w = WineInstallation {
            bin: format!("'{}'", bin.display()),
            kind: WineKind::Wine,
            ..Default::default()
        };
        assert!(!is_valid(&w), "wine without wineserver is invalid");
        touch(&dir.path().join("bin/wineserver"));
        w.wineserver = Some(
            dir.path()
                .join("bin/wineserver")
                .to_string_lossy()
                .into_owned(),
        );
        assert!(is_valid(&w));
        w.kind = WineKind::Proton;
        w.wineserver = None;
        assert!(is_valid(&w));
        assert!(!is_valid(&WineInstallation::default()));
    }
}
