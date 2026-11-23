//! Installing Irori's own update, only where nothing else does: the
//! AppImage, the Linux tarball, the macOS app, and the Windows zip, portable
//! zip and setup ([`InstallKind::Manual`]). Package managers (Flatpak,
//! distribution packages, Homebrew, Scoop, winget) keep updating their own
//! installs; Irori never touches those.
//!
//! An update is only installed when it can be checked: the release's
//! `SHA256SUMS` must list the package, the download must match it, and when
//! GitHub states the asset's own digest that must agree too. On macOS the new
//! app's signature must verify, from the same team as the running one when
//! that is signed by a team.
//!
//! The running program is never replaced in place. The package is unpacked
//! beside the install (`.irori-update`, so moving it in is a rename on one
//! disk) and the swap waits for a restart: Irori starts a copy of itself
//! with [`HELPER_FLAG`], quits, and that helper, once Irori is gone, renames
//! each file aside (`<name>.irori-old`, kept until the next update) and the
//! new one in, then starts the new Irori. When that one fails to start, the
//! helper puts the old files back and starts the old Irori, which says so.
//! The Windows setup installs itself instead: the helper runs it silently.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;
use irori_runtime::download::{self, DigestKind};
use serde::{Deserialize, Serialize};

/// The argument that makes Irori swap in a staged update.
pub const HELPER_FLAG: &str = "--apply-update";
/// GitHub's release of one tag, with its assets.
pub const RELEASE_BY_TAG: &str = "https://api.github.com/repos/irori-app/irori/releases/tags/v";
/// Where release downloads come from.
pub const DOWNLOADS: &str = "https://github.com/irori-app/irori/releases/download/";
/// The checksums every release publishes (`.github/workflows/release.yml`).
const SUMS: &str = "SHA256SUMS";
/// The folder beside the install that holds an unpacked update.
const STAGE_DIR: &str = ".irori-update";
/// What a replaced file is renamed to, kept until the next update.
const OLD_SUFFIX: &str = ".irori-old";
/// How long the new Irori has to show it starts before the helper puts the
/// old one back.
const START_WITHIN: Duration = Duration::from_secs(15);

/// How this install is laid out, which decides what an update replaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Layout {
    /// One AppImage file.
    AppImage { file: PathBuf },
    /// The Linux tarball's programs, wherever the user put them.
    Tarball { dir: PathBuf },
    /// `Irori.app`.
    MacApp { bundle: PathBuf },
    /// The Windows zip, unpacked or installed with its `install.ps1`.
    WindowsZip { dir: PathBuf },
    /// The portable Windows zip.
    WindowsPortable { dir: PathBuf },
    /// Installed by the Windows setup, which installs its updates itself.
    WindowsSetup { dir: PathBuf },
}

/// The systems Irori is packaged for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Self::Mac)
        } else if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else {
            None
        }
    }
}

/// What [`layout`] looks at.
#[derive(Debug, Clone)]
pub struct Probe<'a> {
    pub exe: &'a Path,
    pub os: Os,
    /// `$APPIMAGE`, which the AppImage runtime sets to the file it runs.
    pub appimage: Option<&'a Path>,
    /// `install.ini` beside the program says the Windows setup installed it.
    pub setup: bool,
    /// The `portable` marker beside the program.
    pub portable: bool,
}

/// The layout of an install, `None` where Irori can't update it.
pub fn layout(probe: &Probe) -> Option<Layout> {
    let dir = probe.exe.parent()?.to_path_buf();
    Some(match probe.os {
        Os::Linux => match probe.appimage {
            Some(file) if file.is_absolute() => Layout::AppImage {
                file: file.to_path_buf(),
            },
            _ => Layout::Tarball { dir },
        },
        Os::Mac => {
            // `Irori.app/Contents/MacOS/irori`.
            let contents = dir.parent()?;
            let bundle = contents.parent()?;
            let is_app = dir.file_name()? == "MacOS"
                && contents.file_name()? == "Contents"
                && bundle.extension()? == "app";
            if !is_app {
                return None;
            }
            Layout::MacApp {
                bundle: bundle.to_path_buf(),
            }
        }
        Os::Windows if probe.setup => Layout::WindowsSetup { dir },
        Os::Windows if probe.portable => Layout::WindowsPortable { dir },
        Os::Windows => Layout::WindowsZip { dir },
    })
}

/// This install's layout.
pub fn detect() -> Option<Layout> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe.parent()?;
    let setup = std::fs::read_to_string(dir.join("install.ini"))
        .is_ok_and(|ini| ini.to_ascii_lowercase().contains("channel=setup"));
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    layout(&Probe {
        exe: &exe,
        os: Os::current()?,
        appimage: appimage.as_deref(),
        setup,
        portable: irori_core::paths::is_portable(),
    })
}

impl Layout {
    /// The release asset this layout installs from, as `package.py` names
    /// it; `None` for an architecture no package is built for.
    pub fn asset(&self, version: &str, arch: &str) -> Option<String> {
        let arm = match arch {
            "x86_64" => false,
            "aarch64" => true,
            _ => return None,
        };
        let suffix = if arm { "-arm64" } else { "" };
        Some(match self {
            Layout::AppImage { .. } => {
                format!(
                    "Irori-{version}-{}.AppImage",
                    if arm { "aarch64" } else { "x86_64" }
                )
            }
            Layout::Tarball { .. } => format!("Irori-{version}-linux{suffix}.tar.gz"),
            Layout::MacApp { .. } => {
                format!(
                    "Irori-{version}-macos-{}.zip",
                    if arm { "arm64" } else { "x86_64" }
                )
            }
            Layout::WindowsZip { .. } => format!("Irori-{version}-windows{suffix}.zip"),
            Layout::WindowsPortable { .. } => {
                format!("Irori-{version}-windows{suffix}-portable.zip")
            }
            Layout::WindowsSetup { .. } => format!("Irori-{version}-windows{suffix}-setup.exe"),
        })
    }

    /// Where the update is unpacked: beside what it replaces, on one disk.
    fn stage_dir(&self) -> Option<PathBuf> {
        let parent = match self {
            Layout::AppImage { file } => file.parent()?.to_path_buf(),
            Layout::MacApp { bundle } => bundle.parent()?.to_path_buf(),
            Layout::Tarball { dir }
            | Layout::WindowsZip { dir }
            | Layout::WindowsPortable { dir }
            | Layout::WindowsSetup { dir } => dir.clone(),
        };
        Some(parent.join(STAGE_DIR))
    }

    /// The program that starts Irori once the update is in.
    pub fn program(&self) -> PathBuf {
        match self {
            Layout::AppImage { file } => file.clone(),
            Layout::Tarball { dir } => dir.join("irori"),
            Layout::MacApp { bundle } => bundle.join("Contents/MacOS/irori"),
            Layout::WindowsZip { dir }
            | Layout::WindowsPortable { dir }
            | Layout::WindowsSetup { dir } => dir.join("irori.exe"),
        }
    }
}

/// The SHA-256 `SHA256SUMS` lists for `name`, by name only.
pub fn expected_digest(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
    })
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, which GitHub computes when the asset is uploaded.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// What to download for one release.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Picked {
    package: String,
    package_url: String,
    sums_url: String,
    /// GitHub's own digest of the package, when it states one.
    github_digest: Option<String>,
}

/// The package and checksums of `release`, refusing anything that can't be
/// checked or doesn't come from `downloads`.
fn pick(release: &GhRelease, version: &str, asset: &str, downloads: &str) -> Result<Picked> {
    if release.draft || release.prerelease || release.tag_name != format!("v{version}") {
        bail!("GitHub didn't answer with the release of Irori {version}");
    }
    let find = |name: &str| release.assets.iter().find(|a| a.name == name);
    let Some(sums) = find(SUMS) else {
        bail!(
            "Irori {version} publishes no checksums, so it can't be checked and isn't installed. \
             Download it from the release page instead."
        );
    };
    let Some(package) = find(asset) else {
        bail!("Irori {version} has no {asset} for this computer");
    };
    for url in [&sums.browser_download_url, &package.browser_download_url] {
        if !url.starts_with(downloads) {
            bail!("The release's download isn't on GitHub");
        }
    }
    Ok(Picked {
        package: package.name.clone(),
        package_url: package.browser_download_url.clone(),
        sums_url: sums.browser_download_url.clone(),
        github_digest: package
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase),
    })
}

/// One file (or the app bundle) the update replaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Swap {
    /// The new one, in the stage folder.
    pub new: PathBuf,
    /// Where it goes.
    pub target: PathBuf,
}

/// An update ready to go in at the next restart: `state/app-update-staged.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    pub version: String,
    /// The version it replaces, which comes back on a rollback.
    #[serde(default)]
    pub from: String,
    pub layout: Layout,
    #[serde(default)]
    pub swaps: Vec<Swap>,
    /// The Windows setup to run silently instead of swapping files.
    #[serde(default)]
    pub installer: Option<PathBuf>,
    pub stage: PathBuf,
    /// Written by the helper when the new version didn't start and the old
    /// one came back.
    #[serde(default)]
    pub rolled_back: bool,
}

/// Where downloads and the release's details come from; tests point both at
/// a local server.
#[derive(Debug, Clone)]
pub struct Source {
    pub release_by_tag: String,
    pub downloads: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            release_by_tag: RELEASE_BY_TAG.into(),
            downloads: DOWNLOADS.into(),
        }
    }
}

/// Download `version` for `layout`, check it and unpack it beside the
/// install. Nothing of the install changes; the stage folder goes when any
/// step fails.
pub async fn stage(
    source: &Source,
    layout: &Layout,
    current: &str,
    version: &str,
    arch: &str,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Staged> {
    let asset = layout
        .asset(version, arch)
        .context("No package of Irori is built for this computer")?;
    let body = irori_core::http::client()
        .get(format!("{}{version}", source.release_by_tag))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Could not reach GitHub")?
        .error_for_status()
        .context("GitHub refused the request")?
        .text()
        .await
        .context("Unexpected answer from GitHub")?;
    let release: GhRelease =
        serde_json::from_str(&body).context("Unexpected answer from GitHub")?;
    let picked = pick(&release, version, &asset, &source.downloads)?;
    let sums = download::get_text(&picked.sums_url)
        .await
        .context("Couldn't read the release's checksums")?;
    let Some(expected) = expected_digest(&sums, &picked.package) else {
        bail!("The release's checksums don't list {}", picked.package);
    };
    if picked
        .github_digest
        .as_deref()
        .is_some_and(|d| !download::checksum_matches(&expected, d))
    {
        bail!("The release's checksums disagree with GitHub's, so the update wasn't downloaded");
    }
    let root = layout
        .stage_dir()
        .context("Irori's folder couldn't be found")?;
    let stage = root.join(version);
    clear_stage(&root).await;
    tokio::fs::create_dir_all(&stage).await.with_context(|| {
        format!(
            "Irori can't write beside itself in {}; download the update from the release page",
            root.parent().unwrap_or(&root).display()
        )
    })?;
    let result = async {
        let archive = stage.join(&picked.package);
        let got = download::download_hashed(
            &picked.package_url,
            &archive,
            progress,
            cancel,
            DigestKind::Sha256,
        )
        .await
        .context("Couldn't download the update")?;
        if !download::checksum_matches(&expected, &got.digest) {
            bail!("The download doesn't match the release's checksum, so it wasn't used");
        }
        let (layout, stage) = (layout.clone(), stage.clone());
        let (swaps, installer) =
            tokio::task::spawn_blocking(move || unpack(&layout, &archive, &stage)).await??;
        Ok((swaps, installer))
    }
    .await;
    match result {
        Ok((swaps, installer)) => Ok(Staged {
            version: version.to_string(),
            from: current.to_string(),
            layout: layout.clone(),
            swaps,
            installer,
            stage,
            rolled_back: false,
        }),
        Err(e) => {
            clear_stage(&root).await;
            Err(e)
        }
    }
}

/// Remove the stage folder: only ever Irori's own `.irori-update`.
async fn clear_stage(root: &Path) {
    if root.file_name().is_some_and(|n| n == STAGE_DIR) {
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}

/// Remove a staged update's folder, once it went in or was given up.
/// Blocking.
pub fn discard_stage(stage: &Path) {
    let root = if stage.file_name().is_some_and(|n| n == STAGE_DIR) {
        stage
    } else {
        match stage.parent() {
            Some(p) if p.file_name().is_some_and(|n| n == STAGE_DIR) => p,
            _ => return,
        }
    };
    let _ = std::fs::remove_dir_all(root);
}

/// Take what `layout` replaces out of the downloaded `archive` into `stage`.
fn unpack(layout: &Layout, archive: &Path, stage: &Path) -> Result<(Vec<Swap>, Option<PathBuf>)> {
    match layout {
        Layout::AppImage { file } => {
            make_executable(archive)?;
            Ok((
                vec![Swap {
                    new: archive.to_path_buf(),
                    target: file.clone(),
                }],
                None,
            ))
        }
        Layout::WindowsSetup { .. } => Ok((Vec::new(), Some(archive.to_path_buf()))),
        Layout::Tarball { dir } => {
            let names = ["irori", "irori-sign-in"];
            let out = stage.join("files");
            extract_tar_programs(archive, &out, &names)?;
            Ok((program_swaps(&out, dir, &names, "irori")?, None))
        }
        Layout::WindowsZip { dir } | Layout::WindowsPortable { dir } => {
            let out = stage.join("files");
            let names = extract_zip_programs(archive, &out)?;
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            Ok((program_swaps(&out, dir, &names, "irori.exe")?, None))
        }
        Layout::MacApp { bundle } => {
            let out = stage.join("app");
            let app = unpack_mac_app(archive, &out)?;
            verify_mac_app(&app, bundle)?;
            Ok((
                vec![Swap {
                    new: app,
                    target: bundle.clone(),
                }],
                None,
            ))
        }
    }
}

/// The swaps of the unpacked `names` into `dir`: the main program always,
/// the others only where the install has them already.
fn program_swaps(out: &Path, dir: &Path, names: &[&str], main: &str) -> Result<Vec<Swap>> {
    if !out.join(main).is_file() {
        bail!("The update holds no {main}");
    }
    Ok(names
        .iter()
        .filter(|name| out.join(name).is_file() && (**name == main || dir.join(name).exists()))
        .map(|name| Swap {
            new: out.join(name),
            target: dir.join(name),
        })
        .collect())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .context("Couldn't make the update runnable")
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// `Irori/<name>` of the tarball, for each of `names`, into `out`. Entries
/// are matched by their exact path and written under fixed names, so the
/// archive can't put anything anywhere else.
fn extract_tar_programs(archive: &Path, out: &Path, names: &[&str]) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries().context("The update is damaged")? {
        let mut entry = entry.context("The update is damaged")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().context("The update is damaged")?.into_owned();
        let mut parts = path.components();
        let (Some(root), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let name = name.as_os_str().to_string_lossy();
        if root.as_os_str() != "Irori" || !names.contains(&name.as_ref()) {
            continue;
        }
        let dest = out.join(name.as_ref());
        let mut file = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut file).context("Couldn't unpack the update")?;
        make_executable(&dest)?;
    }
    Ok(())
}

/// The programs and libraries at the top of the Windows zip's folder
/// (`Irori/` or `Irori-portable/`) into `out`; returns their names.
fn extract_zip_programs(archive: &Path, out: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(out)?;
    let file = std::fs::File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(std::io::BufReader::new(file)).context("The update is damaged")?;
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("The update is damaged")?;
        if !entry.is_file() {
            continue;
        }
        let Ok(path) = entry.name() else { continue };
        let path = path.replace('\\', "/");
        let mut parts = path.split('/');
        let (Some(root), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let wanted = (root == "Irori" || root == "Irori-portable")
            && !name.is_empty()
            && !name.starts_with('.')
            && (lower.ends_with(".exe") || lower.ends_with(".dll"));
        if !wanted {
            continue;
        }
        let name = name.to_string();
        let mut file = std::fs::File::create(out.join(&name))?;
        std::io::copy(&mut entry, &mut file).context("Couldn't unpack the update")?;
        names.push(name);
    }
    Ok(names)
}

/// `Irori.app` out of the macOS zip into `out`, through `ditto`, which keeps
/// what the signature covers exactly as packed.
fn unpack_mac_app(archive: &Path, out: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out)?;
    let status = std::process::Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(archive)
        .arg(out)
        .status()
        .context("Couldn't unpack the update")?;
    if !status.success() {
        bail!("Couldn't unpack the update ({status})");
    }
    let app = out.join("Irori.app");
    if !app.join("Contents/MacOS/irori").is_file() {
        bail!("The update holds no Irori.app");
    }
    Ok(app)
}

/// The `TeamIdentifier=` line `codesign -dv` prints; `None` for an ad-hoc
/// signature ("not set").
pub fn team_identifier(codesign_output: &str) -> Option<String> {
    codesign_output.lines().find_map(|line| {
        let team = line.trim().strip_prefix("TeamIdentifier=")?.trim();
        (!team.is_empty() && team != "not set").then(|| team.to_string())
    })
}

/// The new app's signature verifies, and comes from the team that signed
/// the running one when a team did.
fn verify_mac_app(app: &Path, running: &Path) -> Result<()> {
    let codesign = |args: &[&str], path: &Path| {
        std::process::Command::new("/usr/bin/codesign")
            .args(args)
            .arg(path)
            .output()
            .context("Couldn't check the update's signature")
    };
    let verified = codesign(&["--verify", "--deep", "--strict"], app)?;
    if !verified.status.success() {
        bail!("The update's signature doesn't verify, so it wasn't installed");
    }
    let team = |path: &Path| -> Result<Option<String>> {
        let out = codesign(&["-dv", "--verbose=2"], path)?;
        Ok(team_identifier(&String::from_utf8_lossy(&out.stderr)))
    };
    if let Some(ours) = team(running)?
        && team(app)?.as_deref() != Some(ours.as_str())
    {
        bail!("The update isn't signed by the team that signed this Irori, so it wasn't installed");
    }
    Ok(())
}

fn old_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(OLD_SUFFIX);
    PathBuf::from(name)
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Move each new file in, the replaced one aside to `<name>.irori-old`.
/// Every step is a rename on one disk; when one fails, what was done is put
/// back and the install is as it was.
pub fn apply(swaps: &[Swap]) -> Result<()> {
    for (done, swap) in swaps.iter().enumerate() {
        let old = old_path(&swap.target);
        let step = (|| -> Result<()> {
            remove_any(&old).with_context(|| format!("removing {}", old.display()))?;
            if swap.target.exists() {
                std::fs::rename(&swap.target, &old)
                    .with_context(|| format!("moving {} aside", swap.target.display()))?;
            }
            if let Err(e) = std::fs::rename(&swap.new, &swap.target) {
                let _ = std::fs::rename(&old, &swap.target);
                return Err(e).with_context(|| format!("moving in {}", swap.target.display()));
            }
            Ok(())
        })();
        if let Err(e) = step {
            roll_back(&swaps[..done]);
            return Err(e);
        }
    }
    Ok(())
}

/// Put the replaced files back, newest swap first.
pub fn roll_back(swaps: &[Swap]) {
    for swap in swaps.iter().rev() {
        let old = old_path(&swap.target);
        if !old.exists() {
            continue;
        }
        let _ = remove_any(&swap.target);
        if let Err(e) = std::fs::rename(&old, &swap.target) {
            tracing::warn!("putting back {}: {e}", swap.target.display());
        }
    }
}

/// Start the helper that puts `staged` in once this Irori has quit. The
/// caller quits right after.
pub fn start_helper(staged: &Staged, record: &Path) -> Result<()> {
    let program = helper_program(staged)?;
    let mut command = detached(&program);
    command
        .arg(HELPER_FLAG)
        .arg(std::process::id().to_string())
        .arg(record);
    command.spawn().context("Couldn't start the update")?;
    Ok(())
}

/// What runs the helper: this program, except where the update replaces
/// what it runs from in a way a running program can't take.
fn helper_program(staged: &Staged) -> Result<PathBuf> {
    match &staged.layout {
        // The program runs from the AppImage's mount, which goes with this
        // Irori: the AppImage itself starts the helper.
        Layout::AppImage { file } => Ok(file.clone()),
        // The setup overwrites the install folder, so the helper runs from a
        // copy of the program and its libraries in the stage folder.
        Layout::WindowsSetup { dir } => {
            let copy = staged.stage.join("helper");
            std::fs::create_dir_all(&copy).context("Couldn't prepare the update")?;
            for entry in std::fs::read_dir(dir).context("Couldn't prepare the update")? {
                let path = entry?.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if name == "irori.exe" || name.ends_with(".dll") {
                    std::fs::copy(&path, copy.join(path.file_name().unwrap_or_default()))
                        .context("Couldn't prepare the update")?;
                }
            }
            Ok(copy.join("irori.exe"))
        }
        _ => std::env::current_exe().context("Couldn't find Irori's program"),
    }
}

/// `irori --apply-update <pid> <record>`: wait for Irori (`pid`) to quit,
/// put the update in and start it, or put the old one back. Returns the exit
/// code.
pub fn helper_main(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    let [pid, record] = &args[..] else {
        return 2;
    };
    let (Ok(pid), record) = (pid.parse::<u32>(), PathBuf::from(record)) else {
        return 2;
    };
    let Some(mut staged) = std::fs::read(&record)
        .ok()
        .and_then(|b| serde_json::from_slice::<Staged>(&b).ok())
    else {
        return 2;
    };
    if !wait_until_gone(pid, Duration::from_secs(120)) {
        return 3;
    }
    let program = staged.layout.program();
    if let Some(installer) = &staged.installer {
        // The setup closes nothing of ours (Irori is gone) and keeps the
        // channel it was installed from.
        let ok = std::process::Command::new(installer)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .status()
            .is_ok_and(|s| s.success());
        let _ = start(&program);
        discard_stage(&staged.stage);
        if !ok {
            staged.rolled_back = true;
            let _ = write_record(&record, &staged);
        }
        return if ok { 0 } else { 1 };
    }
    if apply(&staged.swaps).is_err() {
        staged.rolled_back = true;
        let _ = write_record(&record, &staged);
        let _ = start(&program);
        discard_stage(&staged.stage);
        return 1;
    }
    let started = start(&program).map(|child| starts_well(child, START_WITHIN));
    if !matches!(started, Ok(true)) {
        roll_back(&staged.swaps);
        staged.rolled_back = true;
        let _ = write_record(&record, &staged);
        let _ = start(&program);
        discard_stage(&staged.stage);
        return 1;
    }
    discard_stage(&staged.stage);
    0
}

fn write_record(path: &Path, staged: &Staged) -> Result<()> {
    irori_core::json_store::write(path, staged)
}

/// Whether process `pid` ended within `limit`.
fn wait_until_gone(pid: u32, limit: Duration) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let deadline = Instant::now() + limit;
    let mut sys = System::new();
    loop {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        if sys.process(pid).is_none() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Start Irori on its own, outliving this helper.
fn start(program: &Path) -> std::io::Result<std::process::Child> {
    detached(program).spawn()
}

/// `program` in a process group of its own, without our standard streams, so
/// it outlives whoever starts it. Irori starting itself is no game or tool,
/// so it goes as GPUI restarts itself, without `CommandSpec`.
fn detached(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
}

/// Whether a freshly started Irori is still running after `within`, or
/// ended well before it (a second instance hands over and exits 0).
fn starts_well(mut child: std::process::Child, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use sha2::{Digest as _, Sha256};

    #[cfg(unix)]
    fn probe(exe: &str, os: Os) -> Probe<'_> {
        Probe {
            exe: Path::new(exe),
            os,
            appimage: None,
            setup: false,
            portable: false,
        }
    }

    // The probes are Unix paths, which Windows reads differently.
    #[cfg(unix)]
    #[test]
    fn layouts_follow_how_irori_was_unpacked() {
        assert_eq!(
            layout(&probe("/home/a/Irori/irori", Os::Linux)),
            Some(Layout::Tarball {
                dir: "/home/a/Irori".into()
            })
        );
        let appimage = Path::new("/home/a/Apps/Irori.AppImage");
        assert_eq!(
            layout(&Probe {
                appimage: Some(appimage),
                ..probe("/tmp/.mount_IroriX/usr/bin/irori", Os::Linux)
            }),
            Some(Layout::AppImage {
                file: appimage.into()
            })
        );
        assert_eq!(
            layout(&probe(
                "/Applications/Irori.app/Contents/MacOS/irori",
                Os::Mac
            )),
            Some(Layout::MacApp {
                bundle: "/Applications/Irori.app".into()
            })
        );
        assert_eq!(layout(&probe("/opt/irori/irori", Os::Mac)), None);
        let windows = |setup, portable| {
            layout(&Probe {
                setup,
                portable,
                ..probe("/c/Irori/irori.exe", Os::Windows)
            })
        };
        assert!(matches!(
            windows(true, false),
            Some(Layout::WindowsSetup { .. })
        ));
        assert!(matches!(
            windows(false, true),
            Some(Layout::WindowsPortable { .. })
        ));
        assert!(matches!(
            windows(false, false),
            Some(Layout::WindowsZip { .. })
        ));
    }

    #[test]
    fn assets_are_named_as_the_release_names_them() {
        let dir = PathBuf::from("/x");
        let asset = |l: Layout, arch| l.asset("1.4.0", arch).unwrap();
        assert_eq!(
            asset(Layout::Tarball { dir: dir.clone() }, "x86_64"),
            "Irori-1.4.0-linux.tar.gz"
        );
        assert_eq!(
            asset(Layout::Tarball { dir: dir.clone() }, "aarch64"),
            "Irori-1.4.0-linux-arm64.tar.gz"
        );
        assert_eq!(
            asset(Layout::AppImage { file: dir.clone() }, "aarch64"),
            "Irori-1.4.0-aarch64.AppImage"
        );
        assert_eq!(
            asset(
                Layout::MacApp {
                    bundle: dir.clone()
                },
                "aarch64"
            ),
            "Irori-1.4.0-macos-arm64.zip"
        );
        assert_eq!(
            asset(Layout::WindowsPortable { dir: dir.clone() }, "x86_64"),
            "Irori-1.4.0-windows-portable.zip"
        );
        assert_eq!(
            asset(Layout::WindowsSetup { dir: dir.clone() }, "aarch64"),
            "Irori-1.4.0-windows-arm64-setup.exe"
        );
        assert_eq!(Layout::WindowsZip { dir }.asset("1.4.0", "riscv64"), None);
    }

    /// Installing depends on the release publishing `SHA256SUMS` over every
    /// package, under the names [`Layout::asset`] asks for.
    #[test]
    fn the_release_publishes_checksums_of_every_package() {
        let workflow = include_str!("../../../.github/workflows/release.yml");
        assert!(workflow.contains("sha256sum -- * > SHA256SUMS"));
        for name in [
            "Irori-$VERSION-linux.tar.gz",
            "Irori-$VERSION-linux-arm64.tar.gz",
            "Irori-$VERSION-x86_64.AppImage",
            "Irori-$VERSION-aarch64.AppImage",
            "Irori-$VERSION-windows.zip",
            "Irori-$VERSION-windows-portable.zip",
            "Irori-$VERSION-windows-setup.exe",
            "Irori-$VERSION-windows-arm64.zip",
            "Irori-$VERSION-windows-arm64-portable.zip",
            "Irori-$VERSION-windows-arm64-setup.exe",
        ] {
            assert!(
                workflow.contains(name),
                "{name} isn't required by the release"
            );
        }
        // The macOS zips are among the release's files, which the checksums
        // cover whole.
        assert!(workflow.contains("macos-*"));
        let package = include_str!("../../../packaging/package.py");
        assert!(package.contains(r#"f"Irori-{version}-macos-{arch}.zip""#));
    }

    #[test]
    fn checksums_are_found_by_exact_name_only() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums =
            format!("{a}  Irori-1.4.0-linux.tar.gz\n{b} *Irori-1.4.0-windows.zip\nshort  x\n");
        assert_eq!(
            expected_digest(&sums, "Irori-1.4.0-linux.tar.gz"),
            Some(a.clone())
        );
        assert_eq!(
            expected_digest(&sums, "Irori-1.4.0-windows.zip"),
            Some(b.to_lowercase())
        );
        assert_eq!(expected_digest(&sums, "Irori-1.4.0-linux"), None);
        assert_eq!(expected_digest(&sums, "x"), None);
        // A lone digest never stands in for a missing name.
        assert_eq!(expected_digest(&format!("{a}\n"), "anything"), None);
    }

    #[test]
    fn team_identifiers_are_read_from_codesign() {
        let signed = "Executable=/A\nIdentifier=app.irori.Irori\nTeamIdentifier=ABCDE12345\n";
        assert_eq!(team_identifier(signed), Some("ABCDE12345".into()));
        assert_eq!(team_identifier("TeamIdentifier=not set\n"), None);
        assert_eq!(team_identifier(""), None);
    }

    fn release(assets: &[(&str, &str, Option<&str>)]) -> GhRelease {
        GhRelease {
            tag_name: "v1.4.0".into(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|(name, url, digest)| GhAsset {
                    name: (*name).into(),
                    browser_download_url: (*url).into(),
                    digest: digest.map(str::to_string),
                })
                .collect(),
        }
    }

    #[test]
    fn releases_without_checksums_or_off_github_are_refused() {
        let base = "https://github.com/irori-app/irori/releases/download/v1.4.0/";
        let asset = "Irori-1.4.0-linux.tar.gz";
        let good = release(&[
            (asset, &format!("{base}{asset}"), Some("sha256:ABC")),
            (SUMS, &format!("{base}{SUMS}"), None),
        ]);
        let picked = pick(&good, "1.4.0", asset, DOWNLOADS).unwrap();
        assert_eq!(picked.github_digest.as_deref(), Some("abc"));
        let no_sums = release(&[(asset, &format!("{base}{asset}"), None)]);
        let e = pick(&no_sums, "1.4.0", asset, DOWNLOADS).unwrap_err();
        assert!(format!("{e:#}").contains("no checksums"));
        let elsewhere = release(&[
            (asset, "https://example.com/x", None),
            (SUMS, &format!("{base}{SUMS}"), None),
        ]);
        assert!(pick(&elsewhere, "1.4.0", asset, DOWNLOADS).is_err());
        assert!(pick(&good, "1.5.0", asset, DOWNLOADS).is_err());
        let mut draft = good.clone();
        draft.draft = true;
        assert!(pick(&draft, "1.4.0", asset, DOWNLOADS).is_err());
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn swaps_keep_the_old_files_and_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Irori");
        let stage = install.join(STAGE_DIR).join("1.4.0/files");
        write(&install.join("irori"), "old irori");
        write(&install.join("irori-sign-in"), "old sign-in");
        write(&stage.join("irori"), "new irori");
        write(&stage.join("irori-sign-in"), "new sign-in");
        let swaps = program_swaps(&stage, &install, &["irori", "irori-sign-in"], "irori").unwrap();
        assert_eq!(swaps.len(), 2);
        apply(&swaps).unwrap();
        let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(install.join("irori")), "new irori");
        assert_eq!(read(install.join("irori.irori-old")), "old irori");
        assert_eq!(read(install.join("irori-sign-in")), "new sign-in");
        roll_back(&swaps);
        assert_eq!(read(install.join("irori")), "old irori");
        assert_eq!(read(install.join("irori-sign-in")), "old sign-in");
        assert!(!install.join("irori.irori-old").exists());
    }

    #[test]
    fn a_failed_swap_leaves_the_install_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Irori");
        let stage = install.join(STAGE_DIR).join("files");
        write(&install.join("irori"), "old irori");
        write(&install.join("irori-sign-in"), "old sign-in");
        write(&stage.join("irori"), "new irori");
        let swaps = vec![
            Swap {
                new: stage.join("irori"),
                target: install.join("irori"),
            },
            // Its new file is missing: the second step fails.
            Swap {
                new: stage.join("irori-sign-in"),
                target: install.join("irori-sign-in"),
            },
        ];
        assert!(apply(&swaps).is_err());
        let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(install.join("irori")), "old irori");
        assert_eq!(read(install.join("irori-sign-in")), "old sign-in");
    }

    #[test]
    fn only_programs_the_install_has_are_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("bin");
        let out = dir.path().join("out");
        write(&install.join("irori"), "old");
        write(&out.join("irori"), "new");
        write(&out.join("irori-sign-in"), "new");
        let swaps = program_swaps(&out, &install, &["irori", "irori-sign-in"], "irori").unwrap();
        assert_eq!(swaps.len(), 1);
        assert!(program_swaps(&out, &install, &["irori.exe"], "irori.exe").is_err());
    }

    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
            let mut tar = tar::Builder::new(gz);
            for (name, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, *data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        out
    }

    #[test]
    fn the_tarball_gives_up_only_its_programs() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(
            &archive,
            tarball(&[
                ("Irori/irori", b"new"),
                ("Irori/LICENSE", b"text"),
                ("Irori/sub/irori-sign-in", b"elsewhere"),
                ("Other/irori-sign-in", b"elsewhere"),
            ]),
        )
        .unwrap();
        let out = dir.path().join("out");
        extract_tar_programs(&archive, &out, &["irori", "irori-sign-in"]).unwrap();
        assert_eq!(std::fs::read(out.join("irori")).unwrap(), b"new");
        assert!(!out.join("irori-sign-in").exists());
        assert!(!out.join("LICENSE").exists());
    }

    #[test]
    fn the_windows_zip_gives_up_its_programs_and_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for (name, data) in [
                ("Irori/irori.exe", "exe"),
                ("Irori/vcruntime140.dll", "dll"),
                ("Irori/install.ps1", "script"),
                ("Irori/../evil.exe", "no"),
                ("Elsewhere/irori.exe", "no"),
            ] {
                zip.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut zip, data.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let out = dir.path().join("out");
        let mut names = extract_zip_programs(&archive, &out).unwrap();
        names.sort();
        assert_eq!(names, vec!["irori.exe", "vcruntime140.dll"]);
        assert!(!dir.path().join("evil.exe").exists());
    }

    fn sha256(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn bytes(body: Vec<u8>) -> Resp {
        Resp {
            status: 200,
            body,
            ..Default::default()
        }
    }

    /// A stand-in for GitHub serving release 1.4.0's tarball and checksums.
    fn release_server(package: Vec<u8>, listed: String, github: Option<String>) -> TestServer {
        let asset = "Irori-1.4.0-linux.tar.gz";
        TestServer::start(move |req| {
            let base = format!(
                "http://{}/download/",
                req.header("host").unwrap_or_default()
            );
            match req.path.as_str() {
                "/tag/v1.4.0" => Resp::json(serde_json::json!({
                    "tag_name": "v1.4.0",
                    "assets": [
                        {"name": asset, "browser_download_url": format!("{base}{asset}"),
                         "digest": github.as_ref().map(|d| format!("sha256:{d}"))},
                        {"name": SUMS, "browser_download_url": format!("{base}{SUMS}")},
                    ]
                })),
                "/download/SHA256SUMS" => bytes(format!("{listed}  {asset}\n").into_bytes()),
                "/download/Irori-1.4.0-linux.tar.gz" => bytes(package.clone()),
                _ => Resp::status(404),
            }
        })
    }

    async fn stage_from(server: &TestServer, layout: &Layout) -> Result<Staged> {
        let source = Source {
            release_by_tag: server.url("/tag/v"),
            downloads: server.url("/download/"),
        };
        stage(
            &source,
            layout,
            "1.3.0",
            "1.4.0",
            "x86_64",
            &irori_core::install::ignore_progress(),
            &CancellationToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn a_checked_download_is_staged_beside_the_install() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Irori");
        write(&install.join("irori"), "old");
        let layout = Layout::Tarball {
            dir: install.clone(),
        };
        let package = tarball(&[("Irori/irori", b"new irori")]);
        let digest = sha256(&package);
        let server = release_server(package, digest.clone(), Some(digest));
        let staged = stage_from(&server, &layout).await.unwrap();
        assert_eq!(staged.from, "1.3.0");
        assert_eq!(staged.swaps.len(), 1);
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(&staged.swaps[0].new), "new irori");
        // The install itself is untouched until the restart.
        assert_eq!(read(&install.join("irori")), "old");
        apply(&staged.swaps).unwrap();
        assert_eq!(read(&install.join("irori")), "new irori");
    }

    #[tokio::test]
    async fn a_download_that_doesnt_match_is_thrown_away() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("Irori");
        write(&install.join("irori"), "old");
        let layout = Layout::Tarball {
            dir: install.clone(),
        };
        let server = release_server(b"tampered".to_vec(), "0".repeat(64), None);
        let e = stage_from(&server, &layout).await.unwrap_err();
        assert!(format!("{e:#}").contains("doesn't match"));
        assert!(!install.join(STAGE_DIR).exists());
        assert_eq!(
            std::fs::read_to_string(install.join("irori")).unwrap(),
            "old"
        );

        // Checksums that disagree with GitHub's own digest stop it before
        // anything is downloaded.
        let package = tarball(&[("Irori/irori", b"new irori")]);
        let server = release_server(package.clone(), sha256(&package), Some("1".repeat(64)));
        let e = stage_from(&server, &layout).await.unwrap_err();
        assert!(format!("{e:#}").contains("disagree"));
        assert!(!install.join(STAGE_DIR).exists());
    }
}
