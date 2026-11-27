//! Hardened RAR archive discovery, pre-list validation, and extraction.
//!
//! Uses `unrar` (including Windows `WinRAR\UnRAR.exe`), `7z`/`7zz`/`7za`, or
//! `bsdtar` (including Windows `System32\tar.exe`), checking macOS Homebrew
//! prefixes and Linux Flatpak host binaries (`host::find_program` +
//! `host::tied_command`). Every archive is pre-listed before extraction so
//! symbolic links, hard links, path traversal, and password-protected archives
//! are rejected before any file is written.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard;
use irori_core::process::{CancellationToken, CommandSpec, ExecResult, RunOptions, run};

use super::{blocking, cancelled};

/// Returned when an archive is password-protected and cannot be unpacked
/// automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordRequired;

impl fmt::Display for PasswordRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("this archive is password-protected")
    }
}

impl std::error::Error for PasswordRequired {}

/// Whether `err` failed because the archive requires a password.
pub fn is_password_required(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<PasswordRequired>())
}

/// A program that unpacks rar archives, with how to list and unpack.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RarTool {
    Unrar { path: PathBuf, host: bool },
    SevenZip { path: PathBuf, host: bool },
    Bsdtar { path: PathBuf, host: bool },
}

impl RarTool {
    fn is_host(&self) -> bool {
        match self {
            Self::Unrar { host, .. } | Self::SevenZip { host, .. } | Self::Bsdtar { host, .. } => {
                *host
            }
        }
    }
}

/// Common macOS Homebrew binary folders (GUI apps launched from Finder do not
/// inherit Homebrew on `PATH`).
const HOMEBREW_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

fn windows_program_files_rar_tool(program_dirs: &[PathBuf]) -> Option<RarTool> {
    program_dirs
        .iter()
        .map(|d| d.join("WinRAR").join("UnRAR.exe"))
        .find(|p| p.is_file())
        .map(|path| RarTool::Unrar { path, host: false })
        .or_else(|| {
            program_dirs
                .iter()
                .map(|d| d.join("7-Zip").join("7z.exe"))
                .find(|p| p.is_file())
                .map(|path| RarTool::SevenZip { path, host: false })
        })
}

fn homebrew_rar_tool_in(dirs: &[&Path]) -> Option<RarTool> {
    let find = |names: &[&str]| {
        names
            .iter()
            .flat_map(|n| dirs.iter().map(move |d| d.join(n)))
            .find(|p| p.is_file())
    };
    find(&["unrar"])
        .map(|path| RarTool::Unrar { path, host: false })
        .or_else(|| find(&["7z", "7zz", "7za"]).map(|path| RarTool::SevenZip { path, host: false }))
        .or_else(|| find(&["bsdtar"]).map(|path| RarTool::Bsdtar { path, host: false }))
}

fn rar_tool() -> Option<RarTool> {
    if let Ok(path) = which::which("unrar") {
        return Some(RarTool::Unrar { path, host: false });
    }
    let program_dirs: Vec<PathBuf> = if cfg!(windows) {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect()
    } else {
        Vec::new()
    };
    if let Some(tool @ RarTool::Unrar { .. }) = windows_program_files_rar_tool(&program_dirs) {
        return Some(tool);
    }
    for name in ["7z", "7zz", "7za"] {
        if let Ok(path) = which::which(name) {
            return Some(RarTool::SevenZip { path, host: false });
        }
    }
    if let Some(tool) = windows_program_files_rar_tool(&program_dirs) {
        return Some(tool);
    }
    if cfg!(target_os = "macos") {
        let dirs: Vec<&Path> = HOMEBREW_BIN_DIRS.iter().map(Path::new).collect();
        if let Some(tool) = homebrew_rar_tool_in(&dirs) {
            return Some(tool);
        }
    }
    if let Ok(path) = which::which("bsdtar") {
        return Some(RarTool::Bsdtar { path, host: false });
    }
    windows_tar(std::env::var_os("SystemRoot"))
        .filter(|p| cfg!(windows) && p.is_file())
        .map(|path| RarTool::Bsdtar { path, host: false })
}

async fn find_rar_tool() -> Option<RarTool> {
    if let Some(tool) = rar_tool() {
        return Some(tool);
    }
    if !irori_core::host::is_flatpak() {
        return None;
    }
    if let Some(path) = irori_core::host::find_program("unrar").await {
        return Some(RarTool::Unrar { path, host: true });
    }
    for name in ["7z", "7zz", "7za"] {
        if let Some(path) = irori_core::host::find_program(name).await {
            return Some(RarTool::SevenZip { path, host: true });
        }
    }
    irori_core::host::find_program("bsdtar")
        .await
        .map(|path| RarTool::Bsdtar { path, host: true })
}

/// The `tar.exe` Windows 10 (1803) and later ship in `System32`: libarchive's
/// bsdtar, which reads rar 4 and 5, so stock Windows unpacks rar mods with no
/// other program. Named by its full path: a `tar` found on `PATH` may be GNU
/// tar (Git for Windows, MSYS2), which can't read rar at all.
fn windows_tar(system_root: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let root = system_root.filter(|r| !r.is_empty())?;
    Some(PathBuf::from(root).join("System32").join("tar.exe"))
}

/// Whether rar archives can be unpacked locally without probing the Flatpak host.
pub fn can_unpack_rar() -> bool {
    rar_tool().is_some()
}

/// Whether rar archives can be unpacked here (including via the host on Flatpak).
pub async fn can_unpack_rar_async() -> bool {
    find_rar_tool().await.is_some()
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

/// Unpack a `.rar` archive into `dest` after pre-listing every entry and
/// rejecting path traversal, symbolic/hard links, and password-protected
/// archives before extraction starts.
pub async fn extract_rar_checked(
    archive: &Path,
    dest: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    let Some(tool) = find_rar_tool().await else {
        bail!("unpacking rar archives needs unrar, 7-Zip or bsdtar; install one and try again");
    };
    let (list, unpack) = rar_commands(&tool, archive, dest);
    let listing = run(&list, RunOptions::default().cancel(cancel.clone())).await?;
    let listing = check_rar_result(&tool, listing, cancel, "the rar archive could not be read")?;
    validate_rar_listing(&tool, &listing.stdout)?;
    let extracted = run(&unpack, RunOptions::default().cancel(cancel.clone())).await?;
    check_rar_result(
        &tool,
        extracted,
        cancel,
        "the rar archive could not be unpacked",
    )?;
    let dest = dest.to_path_buf();
    blocking(move || remove_links(&dest)).await
}

/// One entry parsed from a RAR listing tool's technical output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RarEntry {
    path: String,
    is_link: bool,
    encrypted: bool,
}

fn rar_commands(tool: &RarTool, archive: &Path, dest: &Path) -> (CommandSpec, CommandSpec) {
    let a = archive.as_os_str();
    let (list, unpack) = match tool {
        RarTool::Unrar { path, .. } => (
            CommandSpec::new(path).args(["lt".as_ref(), "-p-".as_ref(), a]),
            // `-p-` prevents interactive password prompts; `-ol-` refuses symlink creation.
            CommandSpec::new(path).args([
                "x".as_ref(),
                "-o+".as_ref(),
                "-y".as_ref(),
                "-p-".as_ref(),
                "-ol-".as_ref(),
                a,
                dest.as_os_str(),
            ]),
        ),
        RarTool::SevenZip { path, .. } => (
            CommandSpec::new(path).args([
                "l".as_ref(),
                "-slt".as_ref(),
                "-ba".as_ref(),
                "-p-".as_ref(),
                a,
            ]),
            // NEVER pass `-snl` or `-snld`: symlinks are rejected before extraction.
            CommandSpec::new(path)
                .arg("x")
                .arg("-y")
                .arg("-p-")
                .arg(format!("-o{}", dest.display()))
                .arg(a),
        ),
        RarTool::Bsdtar { path, .. } => {
            let mut list = CommandSpec::new(path).args(["-tvf".as_ref(), a]);
            if !cfg!(windows) {
                list = list.env("LC_ALL", "C");
            }
            let unpack =
                CommandSpec::new(path).args(["-xf".as_ref(), a, "-C".as_ref(), dest.as_os_str()]);
            (list, unpack)
        }
    };
    if tool.is_host() {
        (
            irori_core::host::tied_command(list),
            irori_core::host::tied_command(unpack),
        )
    } else {
        (list, unpack)
    }
}

fn is_password_failure(tool: &RarTool, res: &ExecResult) -> bool {
    if matches!(tool, RarTool::Unrar { .. }) && res.code == Some(11) {
        return true;
    }
    let combined = format!("{}\n{}", res.stdout, res.stderr).to_ascii_lowercase();
    [
        "wrong password",
        "encrypted archive",
        "enter password",
        "password is incorrect",
        "missing password",
        "passphrase",
        "is encrypted",
        "encrypted file",
        "header is encrypted",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
}

fn check_rar_result(
    tool: &RarTool,
    res: ExecResult,
    cancel: &CancellationToken,
    context: &str,
) -> Result<ExecResult> {
    if res.aborted || cancel.is_cancelled() {
        return Err(cancelled());
    }
    if !res.success() {
        if is_password_failure(tool, &res) {
            return Err(anyhow::Error::new(PasswordRequired));
        }
        return res.check().with_context(|| context.to_string());
    }
    Ok(res)
}

fn validate_rar_listing(tool: &RarTool, stdout: &str) -> Result<Vec<String>> {
    let entries = parse_rar_listing(tool, stdout)?;
    if entries.is_empty() {
        bail!("the rar archive is empty");
    }
    for entry in &entries {
        if entry.encrypted {
            return Err(anyhow::Error::new(PasswordRequired));
        }
        if entry.is_link {
            bail!(
                "the rar archive contains a symbolic link, which is not allowed: {}",
                entry.path
            );
        }
        if archive_guard::relative_entry_path(Path::new(&entry.path.replace('\\', "/"))).is_none() {
            bail!("the archive holds an unsafe path: {}", entry.path);
        }
    }
    Ok(entries.into_iter().map(|e| e.path).collect())
}

fn parse_rar_listing(tool: &RarTool, stdout: &str) -> Result<Vec<RarEntry>> {
    match tool {
        RarTool::SevenZip { .. } => Ok(parse_seven_zip_slt(stdout)),
        RarTool::Unrar { .. } => Ok(parse_unrar_lt(stdout)),
        RarTool::Bsdtar { .. } => parse_bsdtar_tvf(stdout),
    }
}

/// Whether an `Attributes =` / `Attributes:` field marks a Unix symlink
/// (`lrwxrwxrwx`) or Windows reparse point (`L`).
fn attrs_indicate_link(val: &str) -> bool {
    val.split_whitespace().any(|tok| {
        (tok.len() >= 10 && tok.starts_with('l'))
            || (tok.bytes().all(|b| b"DRHSALNTCOIE._".contains(&b)) && tok.contains('L'))
    })
}

fn parse_seven_zip_slt(stdout: &str) -> Vec<RarEntry> {
    let mut entries = Vec::new();
    let mut current: Option<RarEntry> = None;
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line == "----------" {
            // Discard any archive-level header block preceding `----------`.
            entries.clear();
            current = None;
            continue;
        }
        if line.trim().is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("Path = ") {
            if let Some(prev) = current.take() {
                entries.push(prev);
            }
            current = Some(RarEntry {
                path: path.to_string(),
                is_link: false,
                encrypted: false,
            });
        } else if let Some(entry) = current.as_mut() {
            let link_field = line
                .strip_prefix("Symbolic Link = ")
                .or_else(|| line.strip_prefix("Hard Link = "))
                .or_else(|| line.strip_prefix("Link = "))
                .is_some_and(|v| !v.trim().is_empty())
                || line
                    .strip_prefix("Attributes = ")
                    .is_some_and(attrs_indicate_link)
                || line
                    .strip_prefix("Mode = ")
                    .is_some_and(|v| v.trim().starts_with('l'));
            if link_field {
                entry.is_link = true;
            } else if line
                .strip_prefix("Encrypted = ")
                .is_some_and(|v| v.trim() == "+")
            {
                entry.encrypted = true;
            }
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

fn parse_unrar_lt(stdout: &str) -> Vec<RarEntry> {
    let mut entries = Vec::new();
    let mut current: Option<RarEntry> = None;
    for raw_line in stdout.lines() {
        let trimmed = raw_line.trim_end_matches('\r').trim_start();
        if trimmed.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("Name: ") {
            if let Some(prev) = current.take() {
                entries.push(prev);
            }
            current = Some(RarEntry {
                path: name.to_string(),
                is_link: false,
                encrypted: false,
            });
        } else if let Some(entry) = current.as_mut() {
            let link_type = trimmed.strip_prefix("Type: ").is_some_and(|ty| {
                let lower = ty.trim().to_ascii_lowercase();
                lower.contains("link") || lower.contains("junction") || lower.contains("reparse")
            });
            let link_field = link_type
                || trimmed
                    .strip_prefix("Target: ")
                    .is_some_and(|v| !v.trim().is_empty())
                || trimmed
                    .strip_prefix("Attributes: ")
                    .is_some_and(attrs_indicate_link);
            if link_field {
                entry.is_link = true;
            } else if trimmed
                .strip_prefix("Flags: ")
                .is_some_and(|v| v.to_ascii_lowercase().contains("encrypted"))
            {
                entry.encrypted = true;
            }
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

fn after_n_tokens(s: &str, count: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut i = 0;
    for _ in 0..count {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        while bytes.get(i).is_some_and(|b| !b.is_ascii_whitespace()) {
            i += 1;
        }
    }
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    s.get(i..).filter(|rest| !rest.is_empty())
}

fn parse_bsdtar_tvf(stdout: &str) -> Result<Vec<RarEntry>> {
    let mut entries = Vec::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Some(kind) = line.chars().next() else {
            continue;
        };
        let is_link = match kind {
            '-' | 'd' => false,
            'l' | 'h' => true,
            other => bail!("the rar archive contains an unsupported entry type ({other}): {line}"),
        };
        let Some(name_and_target) = after_n_tokens(line, 8) else {
            if is_link {
                entries.push(RarEntry {
                    path: line.to_string(),
                    is_link: true,
                    encrypted: false,
                });
                continue;
            }
            bail!("unexpected rar listing entry: {line}");
        };
        let path = if is_link {
            name_and_target
                .split_once(" -> ")
                .or_else(|| name_and_target.split_once(" link to "))
                .map_or(name_and_target, |(name, _)| name)
        } else {
            name_and_target
        };
        if kind == 'd' && path.trim_matches(['/', '\\']) == "." {
            continue;
        }
        entries.push(RarEntry {
            path: path.to_string(),
            is_link,
            encrypted: false,
        });
    }
    Ok(entries)
}

/// Defense in depth: remove any link an unpacking program still created.
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
    if seven_zip {
        parse_seven_zip_slt(stdout)
            .into_iter()
            .map(|e| e.path)
            .collect()
    } else {
        let parsed = parse_unrar_lt(stdout);
        if !parsed.is_empty() {
            parsed.into_iter().map(|e| e.path).collect()
        } else {
            stdout
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_args(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

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

    #[test]
    fn rar_commands_enforce_security_flags() {
        let (archive, dest) = (Path::new("/tmp/game.rar"), Path::new("/tmp/out"));

        let sz = RarTool::SevenZip {
            path: PathBuf::from("7z"),
            host: false,
        };
        let (sz_list, sz_unpack) = rar_commands(&sz, archive, dest);
        assert_eq!(
            spec_args(&sz_list),
            ["l", "-slt", "-ba", "-p-", "/tmp/game.rar"]
        );
        let sz_u = spec_args(&sz_unpack);
        assert!(sz_u.contains(&"-p-".to_string()));
        assert!(!sz_u.iter().any(|a| a == "-snl" || a == "-snld"));

        let unrar = RarTool::Unrar {
            path: PathBuf::from("unrar"),
            host: false,
        };
        let (u_list, u_unpack) = rar_commands(&unrar, archive, dest);
        assert_eq!(spec_args(&u_list), ["lt", "-p-", "/tmp/game.rar"]);
        let u_u = spec_args(&u_unpack);
        assert!(u_u.contains(&"-p-".to_string()) && u_u.contains(&"-ol-".to_string()));

        let tar = RarTool::Bsdtar {
            path: PathBuf::from("bsdtar"),
            host: false,
        };
        assert_eq!(
            spec_args(&rar_commands(&tar, archive, dest).0),
            ["-tvf", "/tmp/game.rar"]
        );
    }

    #[test]
    fn rar_listing_rejects_symlinks_traversal_and_passwords_for_all_tools() {
        let sz = RarTool::SevenZip {
            path: PathBuf::from("7z"),
            host: false,
        };
        assert_eq!(
            validate_rar_listing(
                &sz,
                "Path = Game/game.exe\nSize = 100\nAttributes = ....A\nEncrypted = -\n"
            )
            .unwrap(),
            vec!["Game/game.exe"]
        );
        assert!(
            validate_rar_listing(
                &sz,
                "Path = Game/link\nAttributes = L_ lrwxrwxrwx\nSymbolic Link = ../../etc/passwd\n"
            )
            .is_err()
        );
        assert!(validate_rar_listing(&sz, "Path = Game/junc\nAttributes = D...L\n").is_err());
        assert!(is_password_required(
            &validate_rar_listing(&sz, "Path = Game/secret.exe\nEncrypted = +\n").unwrap_err()
        ));

        let unrar = RarTool::Unrar {
            path: PathBuf::from("unrar"),
            host: false,
        };
        assert_eq!(
            validate_rar_listing(
                &unrar,
                "        Name: Game/My Game.exe\n        Type: File\n  Attributes: ..A....\n"
            )
            .unwrap(),
            vec!["Game/My Game.exe"]
        );
        assert!(
            validate_rar_listing(
                &unrar,
                "        Name: Game/link\n        Type: Symlink\n      Target: ../../etc/passwd\n"
            )
            .is_err()
        );
        assert!(is_password_required(
            &validate_rar_listing(
                &unrar,
                "        Name: Game/secret.exe\n        Type: File\n       Flags: encrypted\n"
            )
            .unwrap_err()
        ));
        assert!(is_password_required(
            &check_rar_result(
                &unrar,
                ExecResult {
                    code: Some(11),
                    ..Default::default()
                },
                &CancellationToken::new(),
                "read"
            )
            .unwrap_err()
        ));

        let bsdtar = RarTool::Bsdtar {
            path: PathBuf::from("bsdtar"),
            host: false,
        };
        assert_eq!(
            validate_rar_listing(
                &bsdtar,
                "-rw-rw-r--  0 0      0           2 nov 27 16:14 dir/hello world.txt\n"
            )
            .unwrap(),
            vec!["dir/hello world.txt"]
        );
        assert!(
            validate_rar_listing(
                &bsdtar,
                "lrwxrwxrwx  0 0      0           0 Jan  1  2024 evil -> ../../etc/passwd\n"
            )
            .is_err()
        );
        assert!(
            validate_rar_listing(
                &bsdtar,
                "hrw-r--r--  0 0      0           0 Jan  1  2024 hard link to a.txt\n"
            )
            .is_err()
        );
        assert!(
            validate_rar_listing(
                &bsdtar,
                "-rw-r--r--  0 0      0           5 Jan  1  2024 ../escape.txt\n"
            )
            .is_err()
        );
    }

    #[test]
    fn detects_winrar_and_homebrew_rar_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let pf = tmp.path().join("Program Files");
        let unrar_exe = pf.join("WinRAR").join("UnRAR.exe");
        std::fs::create_dir_all(unrar_exe.parent().unwrap()).unwrap();
        std::fs::write(&unrar_exe, b"MZ").unwrap();
        assert_eq!(
            windows_program_files_rar_tool(&[pf]),
            Some(RarTool::Unrar {
                path: unrar_exe,
                host: false,
            })
        );

        let brew_arm = tmp.path().join("opt/homebrew/bin");
        let brew_intel = tmp.path().join("usr/local/bin");
        std::fs::create_dir_all(&brew_arm).unwrap();
        std::fs::create_dir_all(&brew_intel).unwrap();
        let seven_zz = brew_arm.join("7zz");
        std::fs::write(&seven_zz, b"bin").unwrap();
        assert_eq!(
            homebrew_rar_tool_in(&[&brew_arm, &brew_intel]),
            Some(RarTool::SevenZip {
                path: seven_zz,
                host: false,
            })
        );
    }
}
