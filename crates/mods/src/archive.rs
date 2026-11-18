//! Unpacking mod archives: zip and 7-Zip in-process, rar through a program
//! that can read it (`unrar`, `7z` or `bsdtar`; on Windows also the
//! `tar.exe` Windows ships, which is bsdtar). Blocking; run on the blocking
//! pool.
//!
//! An archive is untrusted. Every entry goes through
//! [`archive_guard`]: nothing is written outside the destination, and links
//! are never created, since a mod is plain files. A rar archive is listed
//! first and refused if any name is unsafe, before the program unpacks it.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard;
use irori_core::process::CancellationToken;

/// The archive formats a mod may come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZip,
    Rar,
}

/// The format of `path`, by its content and else its name.
pub fn kind(path: &Path) -> Option<Kind> {
    let mut magic = [0u8; 8];
    let read = File::open(path)
        .and_then(|mut f| io::Read::read(&mut f, &mut magic))
        .unwrap_or(0);
    let magic = &magic[..read];
    if magic.starts_with(b"PK\x03\x04") || magic.starts_with(b"PK\x05\x06") {
        return Some(Kind::Zip);
    }
    if magic.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some(Kind::SevenZip);
    }
    if magic.starts_with(b"Rar!\x1a\x07") {
        return Some(Kind::Rar);
    }
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    match name.rsplit_once('.')?.1 {
        "zip" => Some(Kind::Zip),
        "7z" => Some(Kind::SevenZip),
        "rar" => Some(Kind::Rar),
        _ => None,
    }
}

/// File names the picker offers.
pub const EXTENSIONS: &[&str] = &["zip", "7z", "rar"];

/// Unpack `archive` into the empty folder `dest`.
pub async fn extract(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let Some(kind) = kind(archive) else {
        bail!(
            "{} is not a zip, 7z or rar archive",
            archive.file_name().unwrap_or_default().to_string_lossy()
        );
    };
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    match kind {
        Kind::Zip => {
            let (a, d, c) = (archive.to_path_buf(), dest.to_path_buf(), cancel.clone());
            blocking(move || unzip(&a, &d, &c)).await
        }
        Kind::SevenZip => {
            let (a, d, c) = (archive.to_path_buf(), dest.to_path_buf(), cancel.clone());
            blocking(move || un7z(&a, &d, &c)).await
        }
        Kind::Rar => unrar(archive, dest, cancel).await,
    }
}

async fn blocking(f: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
    tokio::task::spawn_blocking(f)
        .await
        .context("unpacking stopped")?
}

fn cancelled() -> anyhow::Error {
    anyhow::anyhow!("cancelled")
}

pub(crate) fn unzip(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).context("the zip file is damaged")?;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let mut entry = zip.by_index(i).context("the zip file is damaged")?;
        // Mod archives made on Windows use `\`; `enclosed_name` refuses
        // absolute and climbing names.
        let name = entry.name().unwrap_or_default().replace('\\', "/");
        let Some(rel) = archive_guard::relative_entry_path(Path::new(&name)) else {
            if name.trim_matches('/').is_empty() {
                continue;
            }
            bail!("the archive holds an unsafe path: {name}");
        };
        if entry.is_symlink() {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() || name.ends_with('/') {
            archive_guard::ensure_no_symlinks(dest, &rel)?;
            std::fs::create_dir_all(&out)?;
            continue;
        }
        write_entry(dest, &rel, &mut entry)?;
    }
    Ok(())
}

fn write_entry(dest: &Path, rel: &Path, reader: &mut dyn io::Read) -> Result<()> {
    let parent = rel.parent().unwrap_or(Path::new(""));
    archive_guard::ensure_no_symlinks(dest, rel)?;
    std::fs::create_dir_all(dest.join(parent))?;
    let out = dest.join(rel);
    let mut f = File::create(&out).with_context(|| format!("writing {}", out.display()))?;
    io::copy(reader, &mut f).with_context(|| format!("unpacking {}", out.display()))?;
    Ok(())
}

fn un7z(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
        .map_err(|e| anyhow::anyhow!("the 7-Zip archive could not be opened: {e}"))?;
    let mut failure: Option<anyhow::Error> = None;
    let result = reader.for_each_entries(|entry, data| {
        if cancel.is_cancelled() {
            failure = Some(cancelled());
            return Ok(false);
        }
        let name = entry.name().replace('\\', "/");
        let Some(rel) = archive_guard::relative_entry_path(Path::new(&name)) else {
            if name.trim_matches('/').is_empty() {
                return Ok(true);
            }
            failure = Some(anyhow::anyhow!("the archive holds an unsafe path: {name}"));
            return Ok(false);
        };
        let outcome = if entry.is_directory() {
            archive_guard::ensure_no_symlinks(dest, &rel)
                .and_then(|()| Ok(std::fs::create_dir_all(dest.join(&rel))?))
        } else {
            write_entry(dest, &rel, data)
        };
        match outcome {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    result.map_err(|e| anyhow::anyhow!("the 7-Zip archive could not be unpacked: {e}"))
}

/// A program that unpacks rar archives, with how to list and unpack.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RarTool {
    Unrar(PathBuf),
    SevenZip(PathBuf),
    Bsdtar(PathBuf),
}

fn rar_tool() -> Option<RarTool> {
    if let Ok(p) = which::which("unrar") {
        return Some(RarTool::Unrar(p));
    }
    for name in ["7z", "7zz", "7za"] {
        if let Ok(p) = which::which(name) {
            return Some(RarTool::SevenZip(p));
        }
    }
    #[cfg(windows)]
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(base) {
            let p = PathBuf::from(dir).join("7-Zip").join("7z.exe");
            if p.is_file() {
                return Some(RarTool::SevenZip(p));
            }
        }
    }
    if let Ok(p) = which::which("bsdtar") {
        return Some(RarTool::Bsdtar(p));
    }
    windows_tar(std::env::var_os("SystemRoot"))
        .filter(|p| cfg!(windows) && p.is_file())
        .map(RarTool::Bsdtar)
}

/// The `tar.exe` Windows 10 (1803) and later ship in `System32`: libarchive's
/// bsdtar, which reads rar 4 and 5, so stock Windows unpacks rar mods with no
/// other program. Named by its full path: a `tar` found on `PATH` may be GNU
/// tar (Git for Windows, MSYS2), which can't read rar at all.
fn windows_tar(system_root: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let root = system_root.filter(|r| !r.is_empty())?;
    Some(PathBuf::from(root).join("System32").join("tar.exe"))
}

/// Whether rar archives can be unpacked here.
pub fn can_unpack_rar() -> bool {
    rar_tool().is_some()
}

/// The command that installs 7-Zip here, for the hint shown while rar
/// archives can't be unpacked: the system's package manager on the common
/// Linux distributions, Homebrew on macOS, winget on Windows. `None` where
/// Irori can't tell (other distributions, its Flatpak, whose sandbox a
/// program installed on the host can't reach).
pub fn seven_zip_command() -> Option<&'static str> {
    if cfg!(windows) {
        return Some("winget install 7zip.7zip");
    }
    if cfg!(target_os = "macos") {
        return Some("brew install sevenzip");
    }
    if irori_core::host::is_flatpak() {
        return None;
    }
    let os_release = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())?;
    seven_zip_command_for(&os_release)
}

/// [`seven_zip_command`] for a Linux system described by `os_release`, by
/// its `ID` and then each `ID_LIKE`.
pub fn seven_zip_command_for(os_release: &str) -> Option<&'static str> {
    let field = |key: &str| {
        os_release.lines().find_map(|l| {
            let value = l.strip_prefix(key)?.strip_prefix('=')?;
            Some(value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
        })
    };
    let likes = field("ID_LIKE").unwrap_or_default();
    let ids = field("ID")
        .into_iter()
        .chain(likes.split_whitespace().map(str::to_string));
    ids.into_iter().find_map(|id| {
        Some(match id.as_str() {
            "arch" => "sudo pacman -S 7zip",
            "debian" | "ubuntu" => "sudo apt install 7zip",
            "fedora" | "rhel" => "sudo dnf install 7zip",
            "opensuse" | "suse" | "opensuse-tumbleweed" | "opensuse-leap" => {
                "sudo zypper install 7zip"
            }
            "gentoo" => "sudo emerge app-arch/7zip",
            _ => return None,
        })
    })
}

async fn unrar(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    use irori_core::process::{CommandSpec, RunOptions, run};
    let Some(tool) = rar_tool() else {
        bail!("unpacking rar archives needs unrar, 7-Zip or bsdtar; install one and try again");
    };
    let a = archive.as_os_str();
    let (list, unpack) = match &tool {
        RarTool::Unrar(p) => (
            CommandSpec::new(p).args(["lb".as_ref(), a]),
            CommandSpec::new(p).args([
                "x".as_ref(),
                "-o+".as_ref(),
                "-y".as_ref(),
                a,
                dest.as_os_str(),
            ]),
        ),
        RarTool::SevenZip(p) => (
            CommandSpec::new(p).args(["l".as_ref(), "-slt".as_ref(), "-ba".as_ref(), a]),
            CommandSpec::new(p)
                .arg("x")
                .arg("-y")
                .arg(format!("-o{}", dest.display()))
                .arg(a),
        ),
        RarTool::Bsdtar(p) => (
            CommandSpec::new(p).args(["-tf".as_ref(), a]),
            CommandSpec::new(p).args(["-xf".as_ref(), a, "-C".as_ref(), dest.as_os_str()]),
        ),
    };
    let listing = run(&list, RunOptions::default().cancel(cancel.clone()))
        .await?
        .check()
        .context("the rar archive could not be read")?;
    let names = rar_names(&tool, &listing.stdout);
    if names.is_empty() {
        bail!("the rar archive is empty");
    }
    if let Some(bad) = names
        .iter()
        .find(|n| archive_guard::relative_entry_path(Path::new(&n.replace('\\', "/"))).is_none())
    {
        bail!("the archive holds an unsafe path: {bad}");
    }
    run(&unpack, RunOptions::default().cancel(cancel.clone()))
        .await?
        .check()
        .context("the rar archive could not be unpacked")?;
    let dest = dest.to_path_buf();
    blocking(move || remove_links(&dest)).await
}

/// Entry names in a tool's listing.
fn rar_names(tool: &RarTool, stdout: &str) -> Vec<String> {
    match tool {
        RarTool::SevenZip(_) => stdout
            .lines()
            .filter_map(|l| l.strip_prefix("Path = "))
            .map(str::to_string)
            .collect(),
        _ => stdout
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// A mod is plain files: drop any link an unpacking program created.
fn remove_links(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::fs::remove_file(entry.path())?;
        } else if kind.is_dir() {
            remove_links(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tar_is_named_by_its_full_path() {
        assert_eq!(
            windows_tar(Some(r"C:\Windows".into())),
            Some(
                PathBuf::from(r"C:\Windows")
                    .join("System32")
                    .join("tar.exe")
            )
        );
        assert_eq!(windows_tar(Some("".into())), None);
        assert_eq!(windows_tar(None), None);
    }

    #[test]
    fn seven_zip_command_follows_the_distribution() {
        let cmd = seven_zip_command_for;
        assert_eq!(cmd("ID=arch\n"), Some("sudo pacman -S 7zip"));
        assert_eq!(
            cmd("NAME=\"Linux Mint\"\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some("sudo apt install 7zip")
        );
        assert_eq!(
            cmd("ID=cachyos\nID_LIKE=arch\n"),
            Some("sudo pacman -S 7zip")
        );
        assert_eq!(cmd("ID=\"fedora\"\n"), Some("sudo dnf install 7zip"));
        assert_eq!(
            cmd("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Some("sudo zypper install 7zip")
        );
        assert_eq!(cmd("ID=nixos\n"), None);
        // `ID_LIKE` is not taken for `ID`.
        assert_eq!(cmd("ID_LIKE=debian\nID=x\n"), Some("sudo apt install 7zip"));
        assert_eq!(cmd(""), None);
    }
}

#[cfg(test)]
pub(crate) fn rar_names_for_test(seven_zip: bool, stdout: &str) -> Vec<String> {
    let tool = if seven_zip {
        RarTool::SevenZip(PathBuf::new())
    } else {
        RarTool::Unrar(PathBuf::new())
    };
    rar_names(&tool, stdout)
}
