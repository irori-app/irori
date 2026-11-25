//! Install size accounting and free-space checks for the install sheet.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use irori_core::InstallInfo;
use irori_core::util::format_bytes;
use irori_ui::i18n::{t, t_args};

/// Download and on-disk bytes of an install choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallSize {
    pub download: u64,
    pub disk: u64,
}

impl InstallSize {
    pub fn new(download: u64, disk: u64) -> Self {
        Self { download, disk }
    }

    fn add(&mut self, other: InstallSize) {
        self.download = self.download.saturating_add(other.download);
        self.disk = self.disk.saturating_add(other.disk);
    }

    pub fn is_known(&self) -> bool {
        self.download > 0 || self.disk > 0
    }
}

/// Size of installing the base game in `language` with the `selected` DLCs.
///
/// GOG reports per-language sizes (`language_sizes`, with `"*"` for files
/// shared by every language); those replace the manifest totals. DLC sizes
/// come from `dlc_sizes` (fetched per DLC) and are always added — Heroic
/// left Epic/Amazon DLC sizes out of the total and the disk check.
pub fn install_size(
    info: &InstallInfo,
    language: Option<&str>,
    selected: &BTreeSet<String>,
    dlc_sizes: &HashMap<String, InstallSize>,
) -> InstallSize {
    let mut total = base_size(info, language);
    for dlc in selected {
        if let Some(size) = dlc_sizes.get(dlc) {
            total.add(*size);
        }
    }
    total
}

fn base_size(info: &InstallInfo, language: Option<&str>) -> InstallSize {
    if info.language_sizes.is_empty() {
        return InstallSize::new(info.download_size, info.disk_size);
    }
    let mut size = InstallSize::default();
    let mut matched = false;
    for entry in &info.language_sizes {
        let shared = entry.language == "*";
        let chosen = language.is_some_and(|l| l.eq_ignore_ascii_case(&entry.language));
        if shared || chosen {
            matched |= chosen;
            size.add(InstallSize::new(entry.download_size, entry.disk_size));
        }
    }
    if !matched && !size.is_known() {
        return InstallSize::new(info.download_size, info.disk_size);
    }
    size
}

/// Bytes that a resumed download still needs, from the progress already made.
pub fn remaining(total: u64, percent_done: Option<f32>) -> u64 {
    match percent_done {
        Some(p) if p > 0.0 => {
            let left = (100.0 - p.clamp(0.0, 100.0)) as f64 / 100.0;
            (total as f64 * left).round() as u64
        }
        _ => total,
    }
}

/// Free space on the filesystem holding a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    pub free: u64,
    pub total: u64,
    /// The directory (or its nearest existing ancestor) is writable.
    pub writable: bool,
}

/// Outcome of comparing free space with what an install needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskVerdict {
    /// Size or free space not known yet.
    Unknown,
    Fits {
        left_after: u64,
    },
    /// Not enough space; installing anyway is allowed but flagged.
    TooSmall {
        missing: u64,
    },
    NotWritable,
}

impl DiskVerdict {
    pub fn evaluate(space: Option<DiskSpace>, required: u64) -> Self {
        let Some(space) = space else {
            return DiskVerdict::Unknown;
        };
        if !space.writable {
            return DiskVerdict::NotWritable;
        }
        if required == 0 {
            return DiskVerdict::Unknown;
        }
        if space.free >= required {
            DiskVerdict::Fits {
                left_after: space.free - required,
            }
        } else {
            DiskVerdict::TooSmall {
                missing: required - space.free,
            }
        }
    }

    /// One-line explanation shown under the disk bar, in the interface
    /// language.
    pub fn message(self, space: Option<DiskSpace>) -> String {
        match (self, space) {
            (DiskVerdict::NotWritable, _) => t("game.disk.not_writable").to_string(),
            (DiskVerdict::TooSmall { missing }, _) => {
                t_args("game.disk.too_small", &[("size", &format_bytes(missing))]).to_string()
            }
            (DiskVerdict::Fits { left_after }, Some(s)) => t_args(
                "game.disk.fits",
                &[
                    ("free", &format_bytes(s.free)),
                    ("total", &format_bytes(s.total)),
                    ("left", &format_bytes(left_after)),
                ],
            )
            .to_string(),
            (DiskVerdict::Unknown, Some(s)) => t_args(
                "game.disk.free",
                &[
                    ("free", &format_bytes(s.free)),
                    ("total", &format_bytes(s.total)),
                ],
            )
            .to_string(),
            _ => t("game.disk.checking").to_string(),
        }
    }
}

/// Fractions of the disk bar: (already used, this install) in 0..=1.
pub fn disk_bar_fractions(space: DiskSpace, required: u64) -> (f32, f32) {
    if space.total == 0 {
        return (0.0, 0.0);
    }
    let total = space.total as f64;
    let used = (space.total.saturating_sub(space.free)) as f64 / total;
    let install = (required.min(space.free)) as f64 / total;
    (used as f32, install as f32)
}

/// Short, cross-platform drive or mount label for a destination path
/// (`C:\`, `\\server\share`, `/mnt/games`, `/Volumes/SSD`, `/home`, `/`).
pub fn drive_label(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Windows extended-length UNC prefix: `\\?\UNC\server\share\...`
    if let Some(rest) = trimmed
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| trimmed.strip_prefix("\\\\?\\unc\\"))
    {
        let mut parts = rest.split(['\\', '/']).filter(|s| !s.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return format!("\\\\{server}\\{share}");
        }
    }
    // Strip Windows extended-length device prefix: `\\?\C:\...` -> `C:\...`
    let trimmed = trimmed.strip_prefix("\\\\?\\").unwrap_or(trimmed);
    // Windows drive letter: `C:\...`, `d:/...`, `E:`
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let letter = (bytes[0] as char).to_ascii_uppercase();
        return format!("{letter}:\\");
    }
    // Windows UNC share: `\\server\share\...` or `//server/share/...`
    if let Some(rest) = trimmed
        .strip_prefix("\\\\")
        .or_else(|| trimmed.strip_prefix("//"))
    {
        let mut parts = rest.split(['\\', '/']).filter(|s| !s.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return format!("\\\\{server}\\{share}");
        }
    }
    // POSIX absolute paths (Linux / macOS).
    if trimmed.starts_with('/') {
        let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        return match parts.as_slice() {
            ["run", "media", user, vol, ..] => format!("/run/media/{user}/{vol}"),
            ["media", user, vol, ..] => format!("/media/{user}/{vol}"),
            ["media", vol] => format!("/media/{vol}"),
            ["mnt", vol, ..] => format!("/mnt/{vol}"),
            ["Volumes", vol, ..] => format!("/Volumes/{vol}"),
            ["home", ..] => "/home".into(),
            _ => "/".into(),
        };
    }
    trimmed
        .split(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Disk verdict for Modify mode, accounting for space the installed game
/// already occupies on its drive so we never double-count the base install.
pub fn modify_disk_verdict(
    space: Option<DiskSpace>,
    target_disk: u64,
    installed_disk: u64,
    added_dlc_disk: u64,
    removed_dlc_disk: u64,
) -> DiskVerdict {
    let Some(space) = space else {
        return DiskVerdict::Unknown;
    };
    if !space.writable {
        return DiskVerdict::NotWritable;
    }
    let (extra, freed) = if installed_disk > 0 && target_disk > 0 {
        (
            target_disk.saturating_sub(installed_disk),
            installed_disk.saturating_sub(target_disk),
        )
    } else {
        (
            added_dlc_disk.saturating_sub(removed_dlc_disk),
            removed_dlc_disk.saturating_sub(added_dlc_disk),
        )
    };
    if extra > 0 {
        DiskVerdict::evaluate(Some(space), extra)
    } else {
        DiskVerdict::Fits {
            left_after: space.free.saturating_add(freed).min(space.total),
        }
    }
}

/// Normalized equality between two directory paths (case-insensitive on Windows).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.components().collect::<PathBuf>();
    let (a, b) = (norm(a), norm(b));
    #[cfg(windows)]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Minimum total capacity for an auto-detected extra drive (5 GiB).
/// Filters out small Windows recovery/OEM/EFI partitions.
const MIN_EXTRA_DRIVE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// An additional mounted drive suggested as an install destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedDrive {
    pub label: String,
    pub suggested_base: String,
    pub free: u64,
    pub mount: PathBuf,
}

#[derive(Debug, Clone)]
struct DiskEntry {
    name: String,
    mount: PathBuf,
    total: u64,
    free: u64,
    read_only: bool,
}

fn is_system_mount(mount: &str) -> bool {
    const SYSTEM_EXACT: &[&str] = &[
        "/", "/boot", "/efi", "/snap", "/sys", "/proc", "/dev", "/var", "/tmp", "/etc", "/usr",
        "/nix", "/opt", "/System", "/private", "/run",
    ];
    const SYSTEM_PREFIXES: &[&str] = &[
        "/boot/",
        "/efi/",
        "/snap/",
        "/sys/",
        "/proc/",
        "/dev/",
        "/var/",
        "/tmp/",
        "/etc/",
        "/usr/",
        "/nix/",
        "/opt/",
        "/System/",
        "/private/",
    ];
    SYSTEM_EXACT.contains(&mount)
        || SYSTEM_PREFIXES.iter().any(|p| mount.starts_with(p))
        || (mount.starts_with("/run/") && !mount.starts_with("/run/media/"))
}

fn extra_drives_from_list(disks: &[DiskEntry], existing_paths: &[PathBuf]) -> Vec<DetectedDrive> {
    let mut seen: HashSet<String> = existing_paths
        .iter()
        .map(|p| drive_label(p).to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let mut out = Vec::new();
    for disk in disks {
        if disk.read_only || disk.total < MIN_EXTRA_DRIVE_BYTES {
            continue;
        }
        let mount_raw = disk.mount.to_string_lossy();
        let mount_str = mount_raw.trim();
        if mount_str.is_empty() {
            continue;
        }
        if mount_str.starts_with('/') && is_system_mount(mount_str) {
            continue;
        }
        let short = drive_label(&disk.mount);
        if short.is_empty() || !seen.insert(short.to_ascii_lowercase()) {
            continue;
        }
        let suggested_base = if mount_str.contains('\\') || mount_str.contains(':') {
            format!("{}\\Games\\Irori", mount_str.trim_end_matches(['\\', '/']))
        } else {
            format!("{}/Games/Irori", mount_str.trim_end_matches('/'))
        };
        let name = disk.name.trim();
        let label = if !name.is_empty()
            && name.chars().count() <= 18
            && !name.starts_with(['/', '\\'])
            && !name.contains(':')
            && !name.eq_ignore_ascii_case(&short)
        {
            format!("{name} ({short})")
        } else {
            short
        };
        out.push(DetectedDrive {
            label,
            suggested_base,
            free: disk.free,
            mount: disk.mount.clone(),
        });
    }
    out
}

/// Detect writable mounted drives not already represented by `existing_paths`.
/// Blocking: run off the UI thread.
pub fn detect_extra_drives(existing_paths: &[PathBuf]) -> Vec<DetectedDrive> {
    let entries: Vec<DiskEntry> = sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| std::fs::read_dir(d.mount_point()).is_ok())
        .map(|d| DiskEntry {
            name: d.name().to_string_lossy().into_owned(),
            mount: d.mount_point().to_path_buf(),
            total: d.total_space(),
            free: d.available_space(),
            read_only: d.is_read_only(),
        })
        .collect();
    extra_drives_from_list(&entries, existing_paths)
}

/// Read-only available-space query for `path` without touching the filesystem
/// with a write probe (used for library folder chips).
pub fn free_space(path: &Path) -> anyhow::Result<u64> {
    use anyhow::Context as _;
    let dir = existing_ancestor(path).with_context(|| {
        t_args(
            "game.disk.path_missing",
            &[("path", &path.display().to_string())],
        )
    })?;
    fs4::available_space(&dir).with_context(|| {
        t_args(
            "game.disk.read_failed",
            &[("path", &dir.display().to_string())],
        )
    })
}

/// Plain-language summary shown alongside the segmented disk bar.
pub fn bar_summary(verdict: DiskVerdict, space: Option<DiskSpace>, required: u64) -> String {
    match (verdict, space) {
        (DiskVerdict::NotWritable, _) => t("game.disk.not_writable").to_string(),
        (DiskVerdict::TooSmall { missing }, _) => {
            t_args("game.disk.too_small", &[("size", &format_bytes(missing))]).to_string()
        }
        (DiskVerdict::Fits { left_after }, Some(_)) => t_args(
            "game.disk.bar_fits",
            &[
                ("install", &format_bytes(required)),
                ("left", &format_bytes(left_after)),
            ],
        )
        .to_string(),
        (DiskVerdict::Unknown, Some(s)) => t_args(
            "game.disk.free",
            &[
                ("free", &format_bytes(s.free)),
                ("total", &format_bytes(s.total)),
            ],
        )
        .to_string(),
        _ => t("game.disk.checking").to_string(),
    }
}

/// The nearest existing directory at or above `path` (installs usually
/// target a folder that does not exist yet).
pub fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// Query free space for `path`. Blocking (a `statvfs` call); run it off the
/// UI thread.
pub fn disk_space(path: &Path) -> anyhow::Result<DiskSpace> {
    use anyhow::Context as _;
    let dir = existing_ancestor(path).with_context(|| {
        t_args(
            "game.disk.path_missing",
            &[("path", &path.display().to_string())],
        )
    })?;
    let free = fs4::available_space(&dir).with_context(|| {
        t_args(
            "game.disk.read_failed",
            &[("path", &dir.display().to_string())],
        )
    })?;
    let total = fs4::total_space(&dir)?;
    #[cfg(unix)]
    let writable = nix::unistd::access(&dir, nix::unistd::AccessFlags::W_OK).is_ok();
    // Windows directory read-only attributes do not represent ACL write access.
    // Probe creation with a unique, automatically removed file instead.
    #[cfg(windows)]
    let writable = tempfile::Builder::new()
        .prefix(".irori-write-test-")
        .tempfile_in(&dir)
        .is_ok();
    Ok(DiskSpace {
        free,
        total,
        writable,
    })
}

/// `Install · 12.3 GiB`, or `Install` while the size is unknown.
pub fn install_button_label(verb: &str, download: u64) -> String {
    if download == 0 {
        verb.to_string()
    } else {
        format!("{verb} · {}", format_bytes(download))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::LanguageSize;

    fn info() -> InstallInfo {
        InstallInfo {
            download_size: 100,
            disk_size: 200,
            ..Default::default()
        }
    }

    #[test]
    fn manifest_totals_plus_selected_dlcs() {
        let mut dlcs = HashMap::new();
        dlcs.insert("a".to_string(), InstallSize::new(10, 20));
        dlcs.insert("b".to_string(), InstallSize::new(1, 2));
        let selected: BTreeSet<String> = ["a".to_string()].into();
        assert_eq!(
            install_size(&info(), None, &selected, &dlcs),
            InstallSize::new(110, 220)
        );
        // A selected DLC whose size is unknown adds nothing.
        let selected: BTreeSet<String> = ["c".to_string()].into();
        assert_eq!(
            install_size(&info(), None, &selected, &dlcs),
            InstallSize::new(100, 200)
        );
    }

    #[test]
    fn per_language_sizes() {
        let mut i = info();
        i.language_sizes = vec![
            LanguageSize {
                language: "*".into(),
                download_size: 50,
                disk_size: 60,
            },
            LanguageSize {
                language: "en-US".into(),
                download_size: 5,
                disk_size: 6,
            },
            LanguageSize {
                language: "de-DE".into(),
                download_size: 7,
                disk_size: 8,
            },
        ];
        let none = BTreeSet::new();
        let sizes = HashMap::new();
        assert_eq!(
            install_size(&i, Some("de-de"), &none, &sizes),
            InstallSize::new(57, 68)
        );
        // Unknown language: shared files only.
        assert_eq!(
            install_size(&i, Some("fr-FR"), &none, &sizes),
            InstallSize::new(50, 60)
        );
        i.language_sizes.remove(0);
        // No shared entry and no match: fall back to the manifest totals.
        assert_eq!(
            install_size(&i, Some("fr-FR"), &none, &sizes),
            InstallSize::new(100, 200)
        );
    }

    #[test]
    fn verdicts() {
        crate::i18n::register();
        let space = DiskSpace {
            free: 1000,
            total: 4000,
            writable: true,
        };
        assert_eq!(DiskVerdict::evaluate(None, 10), DiskVerdict::Unknown);
        assert_eq!(DiskVerdict::evaluate(Some(space), 0), DiskVerdict::Unknown);
        assert_eq!(
            DiskVerdict::evaluate(Some(space), 400),
            DiskVerdict::Fits { left_after: 600 }
        );
        assert_eq!(
            DiskVerdict::evaluate(Some(space), 1500),
            DiskVerdict::TooSmall { missing: 500 }
        );
        let ro = DiskSpace {
            writable: false,
            ..space
        };
        assert_eq!(DiskVerdict::evaluate(Some(ro), 1), DiskVerdict::NotWritable);
        assert!(
            DiskVerdict::TooSmall { missing: 2048 }
                .message(Some(space))
                .contains("2.0 KiB")
        );
    }

    #[test]
    fn bar_fractions() {
        let space = DiskSpace {
            free: 1000,
            total: 4000,
            writable: true,
        };
        assert_eq!(disk_bar_fractions(space, 400), (0.75, 0.1));
        // The install segment never exceeds the free part.
        assert_eq!(disk_bar_fractions(space, 9000), (0.75, 0.25));
        assert_eq!(
            disk_bar_fractions(DiskSpace { total: 0, ..space }, 1),
            (0.0, 0.0)
        );
    }

    #[test]
    fn remaining_after_resume() {
        assert_eq!(remaining(1000, None), 1000);
        assert_eq!(remaining(1000, Some(25.0)), 750);
        assert_eq!(remaining(1000, Some(150.0)), 0);
    }

    #[test]
    fn labels() {
        assert_eq!(install_button_label("Install", 0), "Install");
        assert_eq!(
            install_button_label("Install", 3 * 1024 * 1024 * 1024),
            "Install · 3.0 GiB"
        );
    }

    #[cfg(windows)]
    #[test]
    fn readonly_directory_attribute_does_not_block_installation() {
        let dir = tempfile::tempdir().unwrap();
        let original = std::fs::metadata(dir.path()).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(dir.path(), readonly).unwrap();
        let result = disk_space(dir.path());
        std::fs::set_permissions(dir.path(), original).unwrap();
        assert!(result.unwrap().writable);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn real_disk_space() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not/yet/created");
        assert_eq!(existing_ancestor(&missing).as_deref(), Some(dir.path()));
        let space = disk_space(&missing).unwrap();
        assert!(space.total >= space.free);
        assert!(space.writable);
    }

    #[test]
    fn drive_labels_are_short_and_cross_platform() {
        assert_eq!(drive_label(Path::new("")), "");
        assert_eq!(
            drive_label(Path::new(r"C:\Users\eduardo.junior\Games\Irori")),
            r"C:\"
        );
        assert_eq!(drive_label(Path::new("d:/Games/BG3")), r"D:\");
        assert_eq!(drive_label(Path::new("e:")), r"E:\");
        assert_eq!(
            drive_label(Path::new(r"\\nas\games\library")),
            r"\\nas\games"
        );
        assert_eq!(
            drive_label(Path::new("/mnt/games/Irori/Cyberpunk 2077")),
            "/mnt/games"
        );
        assert_eq!(
            drive_label(Path::new("/media/deck/SDCard/Games")),
            "/media/deck/SDCard"
        );
        assert_eq!(
            drive_label(Path::new("/run/media/deck/SDCard/Games")),
            "/run/media/deck/SDCard"
        );
        assert_eq!(
            drive_label(Path::new("/media/usb/Games")),
            "/media/usb/Games"
        );
        assert_eq!(drive_label(Path::new("/media/usb")), "/media/usb");
        assert_eq!(
            drive_label(Path::new("/Volumes/ExternalSSD/Games")),
            "/Volumes/ExternalSSD"
        );
        assert_eq!(drive_label(Path::new("/home/user/Games/Irori")), "/home");
        assert_eq!(drive_label(Path::new("/opt/games")), "/");
        assert_eq!(drive_label(Path::new("  C:\\Games  ")), r"C:\");
        assert_eq!(
            drive_label(Path::new(r"\\?\C:\Users\eduardo\Games\Irori")),
            r"C:\"
        );
        assert_eq!(
            drive_label(Path::new(r"\\?\UNC\nas\games\library")),
            r"\\nas\games"
        );
    }

    #[test]
    fn modify_verdict_does_not_double_count_installed_game() {
        let gib = 1024 * 1024 * 1024;
        let space = DiskSpace {
            free: 100 * gib,
            total: 500 * gib,
            writable: true,
        };
        // 140 GiB game already installed on a drive with 100 GiB free, no changes:
        // must NOT report "Short by 40 GiB".
        assert_eq!(
            modify_disk_verdict(Some(space), 140 * gib, 140 * gib, 0, 0),
            DiskVerdict::Fits {
                left_after: 100 * gib
            }
        );
        // Adding a 20 GiB DLC needs only 20 GiB extra.
        assert_eq!(
            modify_disk_verdict(Some(space), 160 * gib, 140 * gib, 20 * gib, 0),
            DiskVerdict::Fits {
                left_after: 80 * gib
            }
        );
        // Adding a 120 GiB DLC exceeds the 100 GiB free by 20 GiB.
        assert_eq!(
            modify_disk_verdict(Some(space), 260 * gib, 140 * gib, 120 * gib, 0),
            DiskVerdict::TooSmall { missing: 20 * gib }
        );
        // Removing a 30 GiB DLC increases free space after update.
        assert_eq!(
            modify_disk_verdict(Some(space), 110 * gib, 140 * gib, 0, 30 * gib),
            DiskVerdict::Fits {
                left_after: 130 * gib
            }
        );
        // When the store didn't record install_size_bytes (0), fall back to DLC delta.
        assert_eq!(
            modify_disk_verdict(Some(space), 140 * gib, 0, 0, 0),
            DiskVerdict::Fits {
                left_after: 100 * gib
            }
        );
        assert_eq!(
            modify_disk_verdict(Some(space), 145 * gib, 0, 5 * gib, 0),
            DiskVerdict::Fits {
                left_after: 95 * gib
            }
        );
        assert_eq!(
            modify_disk_verdict(Some(space), 145 * gib, 0, 120 * gib, 0),
            DiskVerdict::TooSmall { missing: 20 * gib }
        );
        // Read-only drive still reports NotWritable.
        let ro = DiskSpace {
            writable: false,
            ..space
        };
        assert_eq!(
            modify_disk_verdict(Some(ro), 140 * gib, 140 * gib, 0, 0),
            DiskVerdict::NotWritable
        );
        assert_eq!(
            modify_disk_verdict(None, 140 * gib, 140 * gib, 0, 0),
            DiskVerdict::Unknown
        );
    }

    #[test]
    fn same_dir_normalizes_components() {
        assert!(same_dir(Path::new("/mnt/games/."), Path::new("/mnt/games")));
        assert!(!same_dir(Path::new("/mnt/games"), Path::new("/mnt/other")));
        #[cfg(windows)]
        assert!(same_dir(
            Path::new(r"c:\Games\Irori\"),
            Path::new(r"C:\Games\Irori")
        ));
    }

    #[test]
    fn free_space_reads_without_creating_files() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = free_space(&dir.path().join("child")).unwrap();
        assert!(bytes > 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn bar_summaries_cover_every_verdict() {
        crate::i18n::register();
        let gib = 1024 * 1024 * 1024;
        let space = DiskSpace {
            free: 100 * gib,
            total: 500 * gib,
            writable: true,
        };
        let fits = bar_summary(
            DiskVerdict::Fits {
                left_after: 80 * gib,
            },
            Some(space),
            20 * gib,
        );
        assert!(fits.contains("20.0 GiB"), "{fits}");
        assert!(fits.contains("80.0 GiB"), "{fits}");

        let short = bar_summary(DiskVerdict::TooSmall { missing: 12 * gib }, Some(space), 0);
        assert!(short.contains("12.0 GiB"), "{short}");

        let ro = bar_summary(DiskVerdict::NotWritable, Some(space), 10 * gib);
        assert!(!ro.is_empty());

        let unknown_known_space = bar_summary(DiskVerdict::Unknown, Some(space), 0);
        assert!(
            unknown_known_space.contains(&format_bytes(space.free)),
            "{unknown_known_space}"
        );
        assert!(
            unknown_known_space.contains(&format_bytes(space.total)),
            "{unknown_known_space}"
        );

        let checking = bar_summary(DiskVerdict::Unknown, None, 0);
        assert!(!checking.is_empty());
    }

    #[test]
    fn extra_drives_filters_system_mounts_and_dedups_existing() {
        let gib = 1024 * 1024 * 1024;
        let disks = vec![
            DiskEntry {
                name: "System".into(),
                mount: PathBuf::from(r"C:\"),
                total: 500 * gib,
                free: 80 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "GamesSSD".into(),
                mount: PathBuf::from(r"D:\"),
                total: 1000 * gib,
                free: 420 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "Recovery".into(),
                mount: PathBuf::from(r"E:\"),
                total: 50 * gib,
                free: 10 * gib,
                read_only: true,
            },
            DiskEntry {
                name: "WindowsRE".into(),
                mount: PathBuf::from(r"F:\"),
                total: 4 * gib,
                free: 512 * 1024 * 1024,
                read_only: false,
            },
            DiskEntry {
                name: "boot".into(),
                mount: PathBuf::from("/boot/efi"),
                total: 20 * gib,
                free: 10 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "snap".into(),
                mount: PathBuf::from("/snap/core22/1"),
                total: 20 * gib,
                free: 0,
                read_only: false,
            },
            DiskEntry {
                name: "run".into(),
                mount: PathBuf::from("/run/user/1000"),
                total: 20 * gib,
                free: 10 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "".into(),
                mount: PathBuf::from("/mnt/games"),
                total: 1000 * gib,
                free: 600 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "SDCard".into(),
                mount: PathBuf::from("/run/media/deck/SDCard"),
                total: 512 * gib,
                free: 200 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "Data".into(),
                mount: PathBuf::from("/System/Volumes/Data"),
                total: 500 * gib,
                free: 120 * gib,
                read_only: false,
            },
            DiskEntry {
                name: "ExternalSSD".into(),
                mount: PathBuf::from("/Volumes/ExternalSSD"),
                total: 2000 * gib,
                free: 900 * gib,
                read_only: false,
            },
        ];

        let existing = vec![PathBuf::from(r"C:\Users\eduardo\Games\Irori")];
        let detected = extra_drives_from_list(&disks, &existing);
        let labels: Vec<&str> = detected.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                r"GamesSSD (D:\)",
                "/mnt/games",
                "SDCard (/run/media/deck/SDCard)",
                "ExternalSSD (/Volumes/ExternalSSD)",
            ]
        );
        assert_eq!(detected[0].suggested_base, r"D:\Games\Irori");
        assert_eq!(detected[1].suggested_base, "/mnt/games/Games/Irori");
        assert_eq!(
            detected[3].suggested_base,
            "/Volumes/ExternalSSD/Games/Irori"
        );

        // If D:\ already hosts a configured library folder, don't duplicate it.
        let with_d = vec![
            PathBuf::from(r"C:\Games\Irori"),
            PathBuf::from(r"d:\MyLibrary"),
        ];
        let detected2 = extra_drives_from_list(&disks, &with_d);
        assert!(!detected2.iter().any(|d| d.label.contains(r"D:\")));
    }
}
