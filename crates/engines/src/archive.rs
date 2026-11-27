//! Unpacking a downloaded engine build into its install folder: zip, 7-Zip,
//! tar archives (gzip, xz, zstd, bzip2), macOS disk images and single
//! programs (AppImages). Entries that would land outside the folder are
//! refused. Blocking except [`unpack_dmg`]; run on the blocking pool.

use std::fs::File;
use std::io::{self, BufReader, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard;
use irori_core::install::ignore_progress;
use irori_core::process::CancellationToken;

/// Longest target of a symbolic link: more is not a path.
const MAX_LINK_TARGET: u64 = 4096;
/// Chunk size used for cancellable archive writes.
const COPY_BUF_BYTES: usize = 64 * 1024;
/// Minimum unpacked byte allowance for small highly-compressible archives.
const MIN_UN7Z_LIMIT_BYTES: u64 = 256 << 20;
/// Maximum expansion multiplier relative to the compressed archive size.
const MAX_UN7Z_EXPANSION_RATIO: u64 = 100;
/// Absolute unpacked size ceiling for a single 7-Zip archive (512 GiB).
const MAX_UN7Z_TOTAL_BYTES: u64 = 512 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZip,
    Tar,
    Dmg,
    /// A program on its own (an AppImage, a portable `.exe`).
    Plain,
}

pub fn classify(name: &str) -> Kind {
    let name = name.to_ascii_lowercase();
    let ends = |exts: &[&str]| exts.iter().any(|e| name.ends_with(e));
    if ends(&[".zip"]) {
        Kind::Zip
    } else if ends(&[".7z"]) {
        Kind::SevenZip
    } else if ends(&[
        ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tar.bz2", ".tbz2",
    ]) {
        Kind::Tar
    } else if ends(&[".dmg"]) {
        Kind::Dmg
    } else {
        Kind::Plain
    }
}

/// Unpack `archive`, named `name`, into `dest` (which must not exist yet).
/// Disk images need [`unpack_dmg`] instead.
pub fn unpack(archive: &Path, name: &str, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    match classify(name) {
        Kind::Zip => unzip(archive, dest, cancel),
        Kind::SevenZip => un7z(archive, dest, cancel),
        Kind::Tar => {
            irori_runtime::download::extract_tar(archive, dest, 0, ignore_progress(), cancel)
        }
        Kind::Dmg => bail!("disk images are unpacked with unpack_dmg"),
        Kind::Plain => {
            let file = Path::new(name)
                .file_name()
                .filter(|f| !f.is_empty())
                .context("the download has no file name")?;
            let target = dest.join(file);
            std::fs::copy(archive, &target)
                .with_context(|| format!("copying to {}", target.display()))?;
            make_executable(&target)
        }
    }
}

/// Set the executable bit (no-op on Windows).
pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn unzip(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).context("the zip file is damaged")?;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(irori_runtime::download::Cancelled.into());
        }
        let mut entry = zip.by_index(i).context("the zip file is damaged")?;
        let Some(rel) = entry.enclosed_name() else {
            bail!(
                "the zip file holds an unsafe path: {}",
                entry.name().unwrap_or_default()
            );
        };
        let out = dest.join(&rel);
        if entry.is_dir() {
            archive_guard::ensure_no_symlinks(dest, &rel)?;
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let parent = rel.parent().unwrap_or(Path::new(""));
        archive_guard::ensure_no_symlinks(dest, parent)?;
        std::fs::create_dir_all(dest.join(parent))?;
        if entry.is_symlink() {
            // Bundles (`.app`) inside zips link their frameworks.
            let mut target = String::new();
            io::Read::read_to_string(&mut (&mut entry).take(MAX_LINK_TARGET), &mut target)?;
            if archive_guard::check_symlink(dest, &rel, Path::new(&target))? {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &out)
                    .with_context(|| format!("linking {}", out.display()))?;
            }
            continue;
        }
        // `File::create` would write through a link left at `out`.
        if std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink()) {
            std::fs::remove_file(&out)?;
        }
        let mut f = File::create(&out).with_context(|| format!("writing {}", out.display()))?;
        io::copy(&mut entry, &mut f).with_context(|| format!("unpacking {}", out.display()))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                &out,
                std::fs::Permissions::from_mode((mode & 0o777) | 0o600),
            )?;
        }
    }
    Ok(())
}

fn un7z_expansion_limit(archive_bytes: u64) -> u64 {
    archive_bytes
        .saturating_mul(MAX_UN7Z_EXPANSION_RATIO)
        .clamp(MIN_UN7Z_LIMIT_BYTES, MAX_UN7Z_TOTAL_BYTES)
}

fn safe_join_7z_entry(root: &Path, raw_name: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    let normalized = raw_name.replace('\\', "/");
    let Some(rel) = archive_guard::relative_entry_path(Path::new(&normalized)) else {
        if normalized.trim_matches('/').is_empty() {
            return Ok(None);
        }
        bail!("the 7-Zip archive holds an unsafe path: {raw_name}");
    };
    let out = root.join(&rel);
    if !out.starts_with(root) {
        bail!("the 7-Zip archive holds an unsafe path: {raw_name}");
    }
    Ok(Some((rel, out)))
}

fn copy_bounded(
    reader: &mut dyn io::Read,
    writer: &mut dyn io::Write,
    cancel: &CancellationToken,
    total_written: &mut u64,
    max_bytes: u64,
) -> Result<()> {
    let mut buf = [0u8; COPY_BUF_BYTES];
    loop {
        if cancel.is_cancelled() {
            return Err(irori_runtime::download::Cancelled.into());
        }
        let read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        *total_written = total_written.saturating_add(read as u64);
        if *total_written > max_bytes {
            bail!(
                "the 7-Zip archive exceeds the maximum unpacked size ({})",
                irori_core::util::format_bytes(max_bytes)
            );
        }
        if let Some(chunk) = buf.get(..read) {
            writer.write_all(chunk)?;
        }
    }
    Ok(())
}

fn un7z(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let archive_bytes = std::fs::metadata(archive).map_or(0, |m| m.len());
    un7z_with_limit(archive, dest, cancel, un7z_expansion_limit(archive_bytes))
}

fn un7z_with_limit(
    archive: &Path,
    dest: &Path,
    cancel: &CancellationToken,
    max_bytes: u64,
) -> Result<()> {
    let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
        .map_err(|e| anyhow::anyhow!("the 7-Zip archive could not be opened: {e}"))?;
    let mut total_written: u64 = 0;
    let mut failure: Option<anyhow::Error> = None;
    let result = reader.for_each_entries(|entry, data| {
        if cancel.is_cancelled() {
            failure = Some(irori_runtime::download::Cancelled.into());
            return Ok(false);
        }
        let joined = match safe_join_7z_entry(dest, entry.name()) {
            Ok(Some(pair)) => pair,
            Ok(None) => return Ok(true),
            Err(e) => {
                failure = Some(e);
                return Ok(false);
            }
        };
        let (rel, out) = joined;
        let outcome = if entry.is_directory() {
            archive_guard::ensure_no_symlinks(dest, &rel).and_then(|()| {
                std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))
            })
        } else {
            (|| -> Result<()> {
                let parent = rel.parent().unwrap_or(Path::new(""));
                archive_guard::ensure_no_symlinks(dest, &rel)?;
                std::fs::create_dir_all(dest.join(parent))?;
                // `File::create` would write through a link left at `out`.
                if std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink()) {
                    std::fs::remove_file(&out)?;
                }
                let mut f =
                    File::create(&out).with_context(|| format!("writing {}", out.display()))?;
                copy_bounded(data, &mut f, cancel, &mut total_written, max_bytes)
                    .with_context(|| format!("unpacking {}", out.display()))
            })()
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
    result.map_err(|e| {
        if cancel.is_cancelled() {
            anyhow::Error::new(irori_runtime::download::Cancelled)
        } else {
            anyhow::anyhow!("the 7-Zip archive could not be unpacked: {e}")
        }
    })?;
    // 7-Zip keeps no Unix modes Irori can rely on; programs need theirs.
    mark_programs_executable(dest);
    Ok(())
}

/// Whether `lower_name` is a shared library (`.so`, `.so.*`, `.dylib`) or Java
/// bytecode (`.class`, which shares Mach-O's `0xCAFEBABE` magic) that must not
/// receive `+x`.
fn is_non_executable_artifact(lower_name: &str) -> bool {
    lower_name.ends_with(".so")
        || lower_name.contains(".so.")
        || lower_name.ends_with(".dylib")
        || lower_name.ends_with(".class")
}

/// Whether `path` is a genuine Unix/macOS runnable program (`.AppImage`, `.sh`,
/// ELF binary, Mach-O binary, or `#!` script) and not a shared library.
fn should_mark_executable(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if is_non_executable_artifact(&lower) {
        return false;
    }
    if lower.ends_with(".appimage") || lower.ends_with(".sh") {
        return true;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    let mut filled = 0;
    while filled < magic.len() {
        let Some(slot) = magic.get_mut(filled..) else {
            break;
        };
        match file.read(slot) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    let Some(head) = magic.get(..filled) else {
        return false;
    };
    if head.starts_with(b"\x7fELF") || head.starts_with(b"#!") {
        return true;
    }
    if let Ok(four) = <[u8; 4]>::try_from(head) {
        return matches!(
            u32::from_be_bytes(four),
            0xFEED_FACE
                | 0xCEFA_EDFE
                | 0xFEED_FACF
                | 0xCFFA_EDFE
                | 0xCAFE_BABE
                | 0xBEBA_FECA
                | 0xCAFE_BABF
                | 0xBFBA_FECA
        );
    }
    false
}

/// Make genuine executable binaries and scripts up to four folders deep
/// runnable, skipping shared libraries (`.so`, `.so.*`) and symlinks.
fn mark_programs_executable(dir: &Path) {
    fn walk(dir: &Path, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1);
                }
            } else if kind.is_file() && should_mark_executable(&path) {
                let _ = make_executable(&path);
            }
        }
    }
    walk(dir, 4);
}

/// Copy the app bundles of a macOS disk image into `dest`.
pub async fn unpack_dmg(image: &Path, dest: &Path, scratch: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use irori_core::process::{self, CommandSpec, RunOptions};
        let mount = scratch.join("mount");
        tokio::fs::create_dir_all(&mount).await?;
        tokio::fs::create_dir_all(dest).await?;
        let attach = CommandSpec::new("/usr/bin/hdiutil").args([
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            image.as_os_str(),
        ]);
        // Images with a licence agreement wait for an answer on stdin.
        let result = process::run(
            &attach,
            RunOptions {
                stdin: Some(b"Y\n".to_vec()),
                ..Default::default()
            },
        )
        .await?;
        if result.code != Some(0) {
            bail!(
                "the disk image could not be opened: {}",
                result.stderr.trim()
            );
        }
        let copied = copy_bundles(&mount, dest).await;
        let detach = CommandSpec::new("/usr/bin/hdiutil").args([
            "detach".as_ref(),
            "-force".as_ref(),
            mount.as_os_str(),
        ]);
        if let Err(e) = process::run(&detach, RunOptions::default()).await {
            tracing::warn!("detaching {}: {e:#}", mount.display());
        }
        copied
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, dest, scratch);
        bail!("disk images only open on macOS")
    }
}

#[cfg(target_os = "macos")]
async fn copy_bundles(mount: &Path, dest: &Path) -> Result<()> {
    use irori_core::process::{self, CommandSpec, RunOptions};
    let mut found = false;
    let mut entries = tokio::fs::read_dir(mount).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "app") {
            found = true;
            let target = dest.join(entry.file_name());
            // `ditto` keeps signatures, links and extended attributes.
            let copy =
                CommandSpec::new("/usr/bin/ditto").args([path.as_os_str(), target.as_os_str()]);
            let r = process::run(&copy, RunOptions::default()).await?;
            if r.code != Some(0) {
                bail!("copying {} failed: {}", path.display(), r.stderr.trim());
            }
        }
    }
    if !found {
        bail!("the disk image holds no app");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn classifies_by_name() {
        assert_eq!(classify("RetroArch.7z"), Kind::SevenZip);
        assert_eq!(classify("x.tar.xz"), Kind::Tar);
        assert_eq!(classify("DuckStation-x64.AppImage"), Kind::Plain);
        assert_eq!(classify("PPSSPP_macOS.dmg"), Kind::Dmg);
        assert_eq!(classify("a.ZIP"), Kind::Zip);
    }

    #[test]
    fn unzips_and_copies_programs() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            zip.start_file("emu/emu.exe", opts).unwrap();
            zip.write_all(b"MZ").unwrap();
            zip.finish().unwrap();
        }
        let cancel = CancellationToken::new();
        let dest = dir.path().join("out");
        unpack(&zip_path, "a.zip", &dest, &cancel).unwrap();
        assert_eq!(std::fs::read(dest.join("emu/emu.exe")).unwrap(), b"MZ");

        let app = dir.path().join("download");
        std::fs::write(&app, b"\x7fELF").unwrap();
        let dest = dir.path().join("plain");
        unpack(&app, "Emu-x64.AppImage", &dest, &cancel).unwrap();
        assert!(irori_core::util::is_executable(
            &dest.join("Emu-x64.AppImage")
        ));
    }

    #[cfg(unix)]
    fn zip_of_links(path: &Path, links: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, target) in links {
            zip.add_symlink(*name, *target, opts).unwrap();
        }
        zip.start_file("x/evil", opts).unwrap();
        io::Write::write_all(&mut zip, b"evil").unwrap();
        zip.finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bundle_links_stay_and_links_cannot_lead_out() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let archive = dir.path().join("a.zip");

        zip_of_links(&archive, &[("A.app/Frameworks/y", "../z")]);
        let dest = dir.path().join("fine");
        unpack(&archive, "a.zip", &dest, &cancel).unwrap();
        assert!(dest.join("A.app/Frameworks/y").is_symlink());

        for links in [
            &[("x", "../../outside")][..],
            &[("x", "/etc")][..],
            // Each link looks inside on its own; together they climb out.
            &[("a/b/l", "../.."), ("x", "a/b/l/..")][..],
        ] {
            zip_of_links(&archive, links);
            let dest = dir.path().join("deep/er/out");
            assert!(
                unpack(&archive, "a.zip", &dest, &cancel).is_err(),
                "{links:?}"
            );
            assert!(!dir.path().join("deep/er/evil").exists());
            std::fs::remove_dir_all(dir.path().join("deep")).unwrap();
        }
    }

    #[test]
    fn should_mark_executable_detects_real_executables_and_skips_shared_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let p = dir.path().join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, bytes).unwrap();
            p
        };

        let elf_deep = write("a/b/c/game_bin", b"\x7fELF\x02\x01\x01\x00");
        let macho = write("mac_game", &[0xCF, 0xFA, 0xED, 0xFE, 0, 0, 0, 0]);
        let shebang = write("launch", b"#!/bin/sh\necho hi\n");
        let sh_script = write("start.sh", b"echo hi\n");
        let appimage = write("Game.AppImage", b"not-elf-yet");
        let so_plain = write("lib/libsteam_api.so", b"\x7fELF\x02\x01\x01\x00");
        let so_ver = write("lib/libSDL2-2.0.so.0.3000.0", b"\x7fELF\x02\x01\x01\x00");
        let dylib = write("lib/libfoo.dylib", &[0xCF, 0xFA, 0xED, 0xFE]);
        let java_class = write("Main.class", &[0xCA, 0xFE, 0xBA, 0xBE]);
        let readme = write("README", b"plain text");

        assert!(should_mark_executable(&elf_deep));
        assert!(should_mark_executable(&macho));
        assert!(should_mark_executable(&shebang));
        assert!(should_mark_executable(&sh_script));
        assert!(should_mark_executable(&appimage));
        assert!(!should_mark_executable(&so_plain));
        assert!(!should_mark_executable(&so_ver));
        assert!(!should_mark_executable(&dylib));
        assert!(!should_mark_executable(&java_class));
        assert!(!should_mark_executable(&readme));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for p in [
                &elf_deep, &macho, &shebang, &sh_script, &appimage, &so_plain, &so_ver, &readme,
            ] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            let too_deep = write("a/b/c/d/e/deep_elf", b"\x7fELF");
            std::fs::set_permissions(&too_deep, std::fs::Permissions::from_mode(0o644)).unwrap();

            mark_programs_executable(dir.path());

            assert!(irori_core::util::is_executable(&elf_deep));
            assert!(irori_core::util::is_executable(&macho));
            assert!(irori_core::util::is_executable(&shebang));
            assert!(irori_core::util::is_executable(&sh_script));
            assert!(irori_core::util::is_executable(&appimage));
            assert!(!irori_core::util::is_executable(&so_plain));
            assert!(!irori_core::util::is_executable(&so_ver));
            assert!(!irori_core::util::is_executable(&readme));
            assert!(!irori_core::util::is_executable(&too_deep));
        }
    }

    #[test]
    fn un7z_copy_enforces_expansion_limit_cancellation_and_safe_paths() {
        let root = Path::new("/tmp/extract-root");
        assert!(safe_join_7z_entry(root, "game/bin/run").unwrap().is_some());
        assert!(safe_join_7z_entry(root, "../escape.txt").is_err());
        assert!(safe_join_7z_entry(root, r"a\..\..\escape.txt").is_err());
        assert!(safe_join_7z_entry(root, "/etc/passwd").is_err());

        let cancel = CancellationToken::new();
        let mut total = 0u64;
        let payload = vec![b'A'; 4096];
        let mut out = Vec::new();
        copy_bounded(
            &mut io::Cursor::new(&payload),
            &mut out,
            &cancel,
            &mut total,
            8192,
        )
        .unwrap();
        assert_eq!(total, 4096);
        assert_eq!(out.len(), 4096);

        // Exceeding max_bytes fails with an expansion-limit error.
        let mut out2 = Vec::new();
        let err = copy_bounded(
            &mut io::Cursor::new(&payload),
            &mut out2,
            &cancel,
            &mut total,
            6000,
        )
        .unwrap_err();
        assert!(err.to_string().contains("maximum unpacked size"), "{err:#}");

        // Cancellation aborts immediately with `Cancelled`.
        cancel.cancel();
        let mut total_c = 0u64;
        let mut out3 = Vec::new();
        let err = copy_bounded(
            &mut io::Cursor::new(&payload),
            &mut out3,
            &cancel,
            &mut total_c,
            100_000,
        )
        .unwrap_err();
        assert!(irori_runtime::download::is_cancelled(&err), "{err:#}");
    }
}
