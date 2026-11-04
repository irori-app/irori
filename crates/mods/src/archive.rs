//! Unpacking mod archives: zip and 7-Zip in-process, rar through a program
//! that can read it (`unrar`, `7z` or `bsdtar`). Blocking; run on the
//! blocking pool.
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
    which::which("bsdtar").ok().map(RarTool::Bsdtar)
}

/// Whether rar archives can be unpacked here.
pub fn can_unpack_rar() -> bool {
    rar_tool().is_some()
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
pub(crate) fn rar_names_for_test(seven_zip: bool, stdout: &str) -> Vec<String> {
    let tool = if seven_zip {
        RarTool::SevenZip(PathBuf::new())
    } else {
        RarTool::Unrar(PathBuf::new())
    };
    rar_names(&tool, stdout)
}
