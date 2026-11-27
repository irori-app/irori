//! On-demand Windows `aria2c.exe` downloader, mirroring `media/ffmpeg_download.rs`.
//!
//! Only offered on Windows (`x86_64` and `aarch64`, where ARM64 executes the
//! official 64-bit release transparently via Prism). Linux and macOS use their
//! native package managers or Flatpak host discovery instead.
//!
//! Downloads `aria2-1.37.0-win-64bit-build1.zip` from the official `aria2/aria2`
//! GitHub release, verifies it against the pinned SHA-256 constant before
//! unpacking, and extracts ONLY `aria2c.exe` into `<data>/tools/aria2/aria2c.exe`.

use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::install::ignore_progress;
use irori_core::process::CancellationToken;
use irori_core::{Paths, host};
use irori_runtime::download::{self, DigestKind};

use super::model::Aria2Status;

/// Official upstream release version.
pub const VERSION: &str = "1.37.0";
/// Release download base URL.
pub const SOURCE_BASE: &str = "https://github.com/aria2/aria2/releases/download/release-1.37.0";
/// Human-facing project release page shown before downloading.
pub const SOURCE_PAGE: &str = "https://github.com/aria2/aria2/releases/tag/release-1.37.0";
/// Official Windows 64-bit archive filename (used on both `x86_64` and `aarch64`).
pub const ARCHIVE_NAME: &str = "aria2-1.37.0-win-64bit-build1.zip";
/// Verified SHA-256 of `aria2-1.37.0-win-64bit-build1.zip` from `aria2/aria2` `release-1.37.0`.
pub const ARCHIVE_SHA256: &str = "67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288";

/// Directory holding Irori's downloaded `aria2c` binary.
pub fn managed_dir() -> PathBuf {
    Paths::get().tools_dir().join("aria2")
}

/// Archive filename for a Windows machine of `arch` (Rust's `ARCH` names).
pub fn archive_name(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "aarch64" => Some(ARCHIVE_NAME),
        _ => None,
    }
}

/// Whether Irori offers one-click `aria2c` installation on this machine.
pub fn offered() -> bool {
    cfg!(windows) && archive_name(std::env::consts::ARCH).is_some()
}

/// Download and verify the official Windows `aria2c.exe` into [`managed_dir`].
pub async fn install(cancel: &CancellationToken) -> Result<PathBuf> {
    if !offered() {
        bail!("Automatic aria2 installation is only available on Windows");
    }
    let file = archive_name(std::env::consts::ARCH)
        .context("No official aria2 build is available for this architecture")?;
    install_from(SOURCE_BASE, file, ARCHIVE_SHA256, &managed_dir(), cancel).await
}

/// Download `file` from `base`, verify it against `expected_sha256`, and unpack
/// only `aria2c.exe` into `dest/aria2c.exe`. Cleans up temporary files on error.
pub(crate) async fn install_from(
    base: &str,
    file: &str,
    expected_sha256: &str,
    dest: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    let base = base.trim_end_matches('/');
    let staging = dest.join(".download");
    let archive = staging.join(file);
    let download_result = download::download_hashed(
        &format!("{base}/{file}"),
        &archive,
        &ignore_progress(),
        cancel,
        DigestKind::Sha256,
    )
    .await
    .context("Couldn't download aria2");

    let program = dest.join("aria2c.exe");
    let result = match download_result {
        Ok(got) if download::checksum_matches(expected_sha256, &got.digest) => {
            let (archive_path, target_path) = (archive.clone(), program.clone());
            tokio::task::spawn_blocking(move || extract_program(&archive_path, &target_path))
                .await
                .context("aria2 extraction task failed")?
        }
        Ok(_) => Err(anyhow::anyhow!(
            "The downloaded aria2 archive does not match its expected SHA-256 checksum, so it was rejected"
        )),
        Err(err) => Err(err),
    };
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result.map(|()| program)
}

/// Extract ONLY `aria2c.exe` out of `archive` into `program`.
/// The zip entry's own path never controls the output location.
fn extract_program(archive: &Path, program: &Path) -> Result<()> {
    let file =
        std::fs::File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(BufReader::new(file)).context("The aria2 download is damaged")?;
    let is_program = |name: &str| {
        let normalized = name.replace('\\', "/");
        normalized.eq_ignore_ascii_case("aria2c.exe")
            || normalized
                .rsplit('/')
                .next()
                .is_some_and(|base| base.eq_ignore_ascii_case("aria2c.exe"))
    };
    let index = (0..zip.len())
        .find(|&i| {
            zip.by_index(i)
                .is_ok_and(|e| e.is_file() && e.name().is_ok_and(|n| is_program(&n)))
        })
        .context("The aria2 archive does not contain aria2c.exe")?;
    let mut entry = zip
        .by_index(index)
        .context("The aria2 download is damaged")?;
    if let Some(dir) = program.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let partial = program.with_extension("exe.part");
    let copied = (|| -> Result<()> {
        let mut out = std::fs::File::create(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n])?;
        }
        out.sync_all()?;
        Ok(())
    })();
    if let Err(err) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(err).context("Couldn't unpack aria2c.exe");
    }
    if let Err(err) = irori_core::fs_part::finalize_part(&partial, program) {
        let _ = std::fs::remove_file(&partial);
        return Err(err).context("Couldn't finalize aria2c.exe");
    }
    Ok(())
}

/// Discover `aria2c` using user override, managed tools dir, `PATH`,
/// platform package directories, and Flatpak host discovery.
pub async fn discover_aria2(custom_path: &str) -> Aria2Status {
    let bin = program_name();
    let trimmed = custom_path.trim();
    if !trimmed.is_empty() {
        let candidate = irori_core::paths::expand_home(trimmed);
        if is_program(&candidate) {
            return Aria2Status::Ready {
                path: candidate,
                on_host: false,
            };
        }
        let nested = candidate.join(bin);
        if is_program(&nested) {
            return Aria2Status::Ready {
                path: nested,
                on_host: false,
            };
        }
    }

    let managed = managed_dir().join(bin);
    if is_program(&managed) {
        return Aria2Status::Ready {
            path: managed,
            on_host: false,
        };
    }

    if host::is_flatpak() {
        let bundled = PathBuf::from("/app/bin/aria2c");
        if is_program(&bundled) {
            return Aria2Status::Ready {
                path: bundled,
                on_host: false,
            };
        }
    }

    if let Ok(found) = which::which("aria2c")
        && is_program(&found)
    {
        return Aria2Status::Ready {
            path: found,
            on_host: false,
        };
    }

    if let Some(found) = extra_candidate_paths(bin)
        .into_iter()
        .find(|p| is_program(p))
    {
        return Aria2Status::Ready {
            path: found,
            on_host: false,
        };
    }

    if host::is_flatpak()
        && let Some(host_path) = host::find_program("aria2c").await
    {
        return Aria2Status::Ready {
            path: host_path,
            on_host: true,
        };
    }

    Aria2Status::Missing {
        can_auto_install: offered(),
        install_hint: platform_install_hint(),
    }
}

fn program_name() -> &'static str {
    if cfg!(windows) {
        "aria2c.exe"
    } else {
        "aria2c"
    }
}

fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn extra_candidate_paths(bin: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(home) = dirs::home_dir() {
            out.push(home.join("scoop").join("shims").join(bin));
            out.push(
                home.join("scoop")
                    .join("apps")
                    .join("aria2")
                    .join("current")
                    .join(bin),
            );
        }
        if let Some(local) = dirs::data_local_dir() {
            let winget = local.join("Microsoft").join("WinGet");
            out.push(winget.join("Links").join(bin));
            if let Ok(entries) = std::fs::read_dir(winget.join("Packages")) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    if name.starts_with("aria2.aria2") {
                        out.push(entry.path().join(bin));
                        if let Ok(sub) = std::fs::read_dir(entry.path()) {
                            for child in sub.flatten() {
                                out.push(child.path().join(bin));
                            }
                        }
                    }
                }
            }
        }
        out.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin").join(bin));
    } else if cfg!(target_os = "macos") {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
            out.push(Path::new(dir).join(bin));
        }
    } else {
        for dir in ["/usr/bin", "/usr/local/bin"] {
            out.push(Path::new(dir).join(bin));
        }
    }
    out
}

fn platform_install_hint() -> String {
    if cfg!(windows) {
        "Install aria2 from Irori or run: winget install aria2.aria2".into()
    } else if cfg!(target_os = "macos") {
        "Install aria2 with Homebrew: brew install aria2".into()
    } else {
        let paths: &[&str] = if host::is_flatpak() {
            &["/run/host/etc/os-release", "/run/host/usr/lib/os-release"]
        } else {
            &["/etc/os-release", "/usr/lib/os-release"]
        };
        let os_release = paths.iter().find_map(|p| std::fs::read_to_string(p).ok());
        linux_install_hint_for(os_release.as_deref())
    }
}

/// Distro-specific Linux package install hint parsed from `os-release`
/// (`ID`, then each `ID_LIKE`), falling back to a generic package-manager note.
pub(crate) fn linux_install_hint_for(os_release: Option<&str>) -> String {
    match os_release.and_then(linux_aria2_command_for) {
        Some(cmd) => format!("Install aria2 with: {cmd}"),
        None => "Install aria2 with your package manager (e.g. apt, dnf, or pacman install aria2)"
            .into(),
    }
}

pub(crate) fn linux_aria2_command_for(os_release: &str) -> Option<&'static str> {
    let field = |key: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix('=')?;
            Some(value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
        })
    };
    let likes = field("ID_LIKE").unwrap_or_default();
    let ids = field("ID")
        .into_iter()
        .chain(likes.split_whitespace().map(str::to_string));
    ids.into_iter().find_map(|id| {
        Some(match id.as_str() {
            "arch" => "sudo pacman -S aria2",
            "debian" | "ubuntu" => "sudo apt install aria2",
            "fedora" | "rhel" => "sudo dnf install aria2",
            "opensuse" | "suse" | "opensuse-tumbleweed" | "opensuse-leap" => {
                "sudo zypper install aria2"
            }
            "gentoo" => "sudo emerge net-misc/aria2",
            "alpine" => "sudo apk add aria2",
            _ => return None,
        })
    })
}
