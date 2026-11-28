//! Cross-platform mounted-drive discovery, system/security partition filtering,
//! and human-readable disk labels shared by the Downloads screen and the
//! Install dialog.

mod filters;
#[cfg(test)]
mod tests;

#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use self::filters::{
    clean_volume_name, is_auxiliary_volume_label, is_system_mount, is_virtual_device, is_virtual_fs,
};
#[cfg(target_os = "linux")]
use self::filters::{linux_fs_labels, resolve_linux_volume_name};

/// Minimum total capacity for an auto-detected extra drive (5 GiB).
/// Filters out small EFI, boot, and Windows Recovery Environment partitions.
const MIN_EXTRA_DRIVE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// An additional mounted user drive suggested as an install destination or
/// shown on the Downloads idle screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedDrive {
    /// Label used by the Install dialog's drive chip (for example `D: GamesSSD`,
    /// `/mnt/Asgard`, `/mnt/nvme2 · Asgard`, `/Volumes/ExternalSSD`).
    pub label: String,
    /// Uniform disk label for Downloads and storage pills (for example
    /// `C: System`, `D: Asgard`, `/ · Reletech P400 EVO 2TB`, `/mnt/Asgard`).
    pub disk_label: String,
    /// Suggested default install folder on this drive.
    pub suggested_base: String,
    /// Total capacity of the drive in bytes.
    pub total: u64,
    /// Available space on the drive in bytes.
    pub free: u64,
    /// Mount point of the drive.
    pub mount: PathBuf,
}

/// Batched off-thread drive probe result for the Install dialog: resolved disk
/// labels for each configured target plus filtered extra writable drives,
/// collected from a single `sysinfo::Disks` refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallDriveProbe {
    /// Formatted disk label for each configured folder passed to
    /// [`probe_install_drives`], in matching index order.
    pub configured_labels: Vec<Option<String>>,
    /// Available and total space `(free, total)` in bytes for each configured
    /// folder passed to [`probe_install_drives`], in matching index order.
    pub configured_space: Vec<Option<(u64, u64)>>,
    /// Writable main drives not already represented by configured folders.
    pub extras: Vec<DetectedDrive>,
}

/// One mounted filesystem entry from `sysinfo::Disks`, enriched with an
/// OS-appropriate human-readable volume or hardware name.
#[derive(Debug, Clone)]
pub(crate) struct SystemDisk {
    /// Raw `sysinfo::Disk::name()` (block device path on Linux, volume label on
    /// Windows and macOS). Used for `same_volume` identity checks.
    pub(crate) device_or_name: String,
    /// Human-readable volume label (Windows/macOS volume name, Linux filesystem
    /// label from `/dev/disk/by-label`, or Linux hardware model from sysfs).
    pub(crate) volume_name: Option<String>,
    pub(crate) mount: PathBuf,
    pub(crate) file_system: String,
    pub(crate) total: u64,
    pub(crate) free: u64,
    pub(crate) read_only: bool,
    pub(crate) writable: bool,
}

impl SystemDisk {
    /// Whether both entries represent mounts of the same underlying volume.
    ///
    /// Btrfs subvolumes (`/` and `/home` and `/srv` on `/dev/nvme0n1p2`) and
    /// bind mounts share a device path and total size; counting them apart
    /// would surface the same physical disk multiple times.
    pub(crate) fn same_volume(&self, other: &Self) -> bool {
        let name = self.device_or_name.trim();
        same_mount_path(&self.mount, &other.mount)
            || (!name.is_empty()
                && self.device_or_name == other.device_or_name
                && self.total == other.total
                && (name.starts_with('/') || self.free == other.free))
    }
}

/// Short, cross-platform drive or mount label for a destination path
/// (`C:\`, `\\server\share`, `/mnt/games`, `/Volumes/SSD`, `/home`, `/`).
pub fn drive_label(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(unc) = unc_share(trimmed) {
        return unc;
    }
    if let Some(letter) = windows_drive_letter(trimmed) {
        return format!("{letter}:\\");
    }
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

/// Format a drive's mount point and optional human-readable volume or hardware
/// name into a concise label:
///
/// - Windows drive letters: `C: System`, `D: Asgard`, or `C:` when unnamed.
/// - Windows UNC shares: `\\server\share`.
/// - POSIX mounts: `<mount> · <name>` when `name` is distinct from the mount's
///   basename, or `<mount>` otherwise (for example `/ · Reletech P400 EVO 2TB`,
///   `/mnt/Asgard`, `/mnt/nvme2 · Asgard`, `/ · Macintosh HD`).
pub fn format_disk_label(mount: &Path, volume_name: Option<&str>) -> String {
    let raw = mount.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let clean_name = volume_name.and_then(clean_volume_name);
    if let Some(unc) = unc_share(trimmed) {
        return unc;
    }
    if let Some(letter) = windows_drive_letter(trimmed) {
        return match clean_name {
            Some(name) => format!("{letter}: {name}"),
            None => format!("{letter}:"),
        };
    }
    let mount_display = if trimmed == "/" {
        "/".to_string()
    } else {
        trimmed.trim_end_matches('/').to_string()
    };
    let basename = mount_display.rsplit('/').next().unwrap_or("");
    match clean_name {
        Some(name) if basename.is_empty() || !name.eq_ignore_ascii_case(basename) => {
            format!("{mount_display} · {name}")
        }
        _ => mount_display,
    }
}

fn windows_drive_letter(raw: &str) -> Option<char> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("\\\\?\\").unwrap_or(trimmed);
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

fn unc_share(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| trimmed.strip_prefix("\\\\?\\unc\\"))
    {
        let mut parts = rest.split(['\\', '/']).filter(|s| !s.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return Some(format!("\\\\{server}\\{share}"));
        }
    }
    if let Some(rest) = trimmed
        .strip_prefix("\\\\")
        .or_else(|| trimmed.strip_prefix("//"))
    {
        if rest.starts_with("?\\") {
            return None;
        }
        let mut parts = rest.split(['\\', '/']).filter(|s| !s.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return Some(format!("\\\\{server}\\{share}"));
        }
    }
    None
}

fn same_mount_path(a: &Path, b: &Path) -> bool {
    let a_raw = a.to_string_lossy();
    let b_raw = b.to_string_lossy();
    if let (Some(la), Some(lb)) = (windows_drive_letter(&a_raw), windows_drive_letter(&b_raw)) {
        return la == lb
            && a_raw.trim().trim_end_matches(['\\', '/']).len() <= 2
            && b_raw.trim().trim_end_matches(['\\', '/']).len() <= 2;
    }
    a == b
}

/// Whether `path` resides under `mount`, handling Windows drive letters and
/// UNC shares even when tested on Unix hosts.
fn path_on_mount(path: &Path, mount: &Path) -> bool {
    let p_raw = path.to_string_lossy();
    let m_raw = mount.to_string_lossy();
    if let (Some(pl), Some(ml)) = (windows_drive_letter(&p_raw), windows_drive_letter(&m_raw)) {
        if pl != ml {
            return false;
        }
        let m_norm = m_raw
            .trim()
            .strip_prefix("\\\\?\\")
            .unwrap_or(m_raw.trim())
            .trim_end_matches(['\\', '/']);
        if m_norm.len() == 2 {
            return true;
        }
        let p_norm = p_raw
            .trim()
            .strip_prefix("\\\\?\\")
            .unwrap_or(p_raw.trim())
            .replace('/', "\\");
        let m_norm = m_norm.replace('/', "\\");
        return p_norm.eq_ignore_ascii_case(&m_norm)
            || (p_norm.len() > m_norm.len()
                && p_norm[..m_norm.len()].eq_ignore_ascii_case(&m_norm)
                && p_norm.as_bytes().get(m_norm.len()) == Some(&b'\\'));
    }
    if let (Some(pu), Some(mu)) = (unc_share(&p_raw), unc_share(&m_raw)) {
        return pu.eq_ignore_ascii_case(&mu);
    }
    path.starts_with(mount)
}

fn mount_depth(mount: &Path) -> usize {
    let raw = mount.to_string_lossy();
    let trimmed = raw.trim();
    if windows_drive_letter(trimmed).is_some() {
        return trimmed
            .strip_prefix("\\\\?\\")
            .unwrap_or(trimmed)
            .split(['\\', '/'])
            .filter(|s| !s.is_empty())
            .count();
    }
    mount.components().count()
}

/// Read all mounted system disks once. Blocking; call off the UI thread.
pub(crate) fn snapshot_system_disks() -> Vec<SystemDisk> {
    #[cfg(target_os = "linux")]
    let fs_labels = linux_fs_labels();
    #[cfg(target_os = "linux")]
    let mut model_cache: HashMap<PathBuf, Option<String>> = HashMap::new();

    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| {
            let device_or_name = d.name().to_string_lossy().into_owned();
            let mount = d.mount_point().to_path_buf();
            let file_system = d.file_system().to_string_lossy().into_owned();
            let read_only = d.is_read_only();
            let writable = !read_only && irori_core::util::is_dir_writable(&mount);
            #[cfg(target_os = "linux")]
            let volume_name =
                resolve_linux_volume_name(&device_or_name, &fs_labels, &mut model_cache);
            #[cfg(not(target_os = "linux"))]
            let volume_name = clean_volume_name(&device_or_name);
            SystemDisk {
                device_or_name,
                volume_name,
                mount,
                file_system,
                total: d.total_space(),
                free: d.available_space(),
                read_only,
                writable,
            }
        })
        .collect()
}

fn canonical_existing_or_self(path: &Path) -> PathBuf {
    if let Some(existing) = path.ancestors().find(|p| p.is_dir()) {
        let mut resolved =
            std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
        if cfg!(windows)
            && let Some(plain) = resolved.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        {
            resolved = PathBuf::from(plain);
        }
        if let Ok(rest) = path.strip_prefix(existing)
            && !rest.as_os_str().is_empty()
        {
            resolved.push(rest);
        }
        return resolved;
    }
    path.to_path_buf()
}

/// Match `path` to its longest-prefix mount in `disks`.
fn matching_mount<'a>(path: &Path, disks: &'a [SystemDisk]) -> Option<&'a SystemDisk> {
    disks
        .iter()
        .filter(|d| path_on_mount(path, &d.mount))
        .max_by_key(|d| (mount_depth(&d.mount), d.mount.as_os_str().len()))
}

/// Return the topmost mount (fewest path components) representing `disk`'s
/// volume across `disks`, plus the best volume name among those mounts.
fn topmost_volume_info<'a>(
    disk: &'a SystemDisk,
    disks: &'a [SystemDisk],
) -> (&'a SystemDisk, Option<&'a str>) {
    let mut best = disk;
    let mut name = disk.volume_name.as_deref();
    for other in disks {
        if !disk.same_volume(other) {
            continue;
        }
        if mount_depth(&other.mount) < mount_depth(&best.mount)
            || (mount_depth(&other.mount) == mount_depth(&best.mount)
                && other.mount.as_os_str().len() < best.mount.as_os_str().len())
        {
            best = other;
        }
        if name.is_none() {
            name = other.volume_name.as_deref();
        }
    }
    (best, name)
}

/// Resolve a formatted disk label without touching the host filesystem.
fn resolve_disk_label_raw(path: &Path, disks: &[SystemDisk]) -> Option<String> {
    let matched = matching_mount(path, disks)?;
    let (topmost, name) = topmost_volume_info(matched, disks);
    let label = format_disk_label(&topmost.mount, name);
    (!label.is_empty()).then_some(label)
}

/// Resolve a formatted disk label (`C: System`, `/ · Reletech P400 EVO 2TB`,
/// `/mnt/Asgard`, `/ · Macintosh HD`) for `path` against a `SystemDisk` slice.
pub(crate) fn resolve_disk_label(path: &Path, disks: &[SystemDisk]) -> Option<String> {
    let resolved = canonical_existing_or_self(path);
    resolve_disk_label_raw(&resolved, disks).or_else(|| resolve_disk_label_raw(path, disks))
}

fn resolve_disk_space_raw(path: &Path, disks: &[SystemDisk]) -> Option<(u64, u64)> {
    let matched = matching_mount(path, disks)?;
    let (topmost, _) = topmost_volume_info(matched, disks);
    Some((topmost.free, topmost.total))
}

/// Resolve `(free, total)` bytes for `path` against a `SystemDisk` slice.
pub(crate) fn resolve_disk_space(path: &Path, disks: &[SystemDisk]) -> Option<(u64, u64)> {
    let resolved = canonical_existing_or_self(path);
    resolve_disk_space_raw(&resolved, disks).or_else(|| resolve_disk_space_raw(path, disks))
}

fn suggested_base_for_mount(mount_str: &str) -> String {
    if mount_str.contains('\\') || mount_str.contains(':') {
        format!("{}\\Games\\Irori", mount_str.trim_end_matches(['\\', '/']))
    } else {
        format!("{}/Games/Irori", mount_str.trim_end_matches('/'))
    }
}

fn short_mount_key(mount: &Path, mount_str: &str) -> String {
    let label = drive_label(mount);
    if mount_str.starts_with('/') && label == "/" {
        mount_str.trim_end_matches('/').to_string()
    } else {
        label
    }
}

/// Core of extra-drive detection with an injectable path canonicalizer so
/// offline tests stay decoupled from the host filesystem.
fn extra_drives_with_resolver(
    disks: &[SystemDisk],
    existing_paths: &[PathBuf],
    home_dir: Option<(&Path, bool)>,
    resolve: impl Fn(&Path) -> PathBuf,
) -> Vec<DetectedDrive> {
    let mut covered_volumes: Vec<&SystemDisk> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();

    for path in existing_paths {
        let resolved = resolve(path);
        if let Some(matched) =
            matching_mount(&resolved, disks).or_else(|| matching_mount(path, disks))
        {
            covered_volumes.push(matched);
        }
        let key = drive_label(path).to_ascii_lowercase();
        if !key.is_empty() {
            seen_keys.insert(key);
        }
    }

    // Prefer shallower mount points when a single volume has multiple mounts
    // (for example a primary mount at `/mnt/Asgard` and a deeper bind mount),
    // keeping OS order for mounts at the same depth.
    let mut ordered: Vec<&SystemDisk> = disks.iter().collect();
    ordered.sort_by_key(|d| mount_depth(&d.mount));

    let mut out = Vec::new();
    for disk in ordered {
        // Layer 1: read-only, virtual/pseudo filesystems, virtual block devices.
        if disk.read_only
            || is_virtual_fs(&disk.file_system)
            || is_virtual_device(&disk.device_or_name)
        {
            continue;
        }
        // Layer 2: real user writability.
        if !disk.writable {
            continue;
        }
        // Layer 3: minimum capacity and security/recovery/auxiliary labels.
        if disk.total < MIN_EXTRA_DRIVE_BYTES {
            continue;
        }
        if disk
            .volume_name
            .as_deref()
            .is_some_and(is_auxiliary_volume_label)
        {
            continue;
        }
        let mount_raw = disk.mount.to_string_lossy();
        let mount_str = mount_raw.trim();
        if mount_str.is_empty() {
            continue;
        }
        // Layer 4: system, security, and snapshot mount paths on POSIX hosts.
        if mount_str.starts_with('/') {
            if is_system_mount(mount_str) {
                continue;
            }
            if let Some(base) = mount_str.trim_end_matches('/').rsplit('/').next()
                && !base.is_empty()
                && is_auxiliary_volume_label(base)
            {
                continue;
            }
        }
        // Layer 5: deduplicate by underlying volume and normalized mount key.
        if covered_volumes.iter().any(|c| c.same_volume(disk)) {
            continue;
        }
        let short = short_mount_key(&disk.mount, mount_str);
        if short.is_empty() || !seen_keys.insert(short.to_ascii_lowercase()) {
            continue;
        }
        covered_volumes.push(disk);

        let disk_label = format_disk_label(&disk.mount, disk.volume_name.as_deref());
        let suggested_base = suggested_base_for_mount(mount_str);
        out.push(DetectedDrive {
            label: disk_label.clone(),
            disk_label,
            suggested_base,
            total: disk.total,
            free: disk.free,
            mount: disk.mount.clone(),
        });
    }

    // If the user moved their default install folder onto a secondary drive
    // (such as `/mnt/Asgard/Games`), their home directory's disk (`/` or
    // `/home`) is still a main writable drive even though the root mount
    // itself is owned by root on Unix.
    if let Some((home, home_writable)) = home_dir
        && home_writable
    {
        let resolved_home = resolve(home);
        if let Some(matched) =
            matching_mount(&resolved_home, disks).or_else(|| matching_mount(home, disks))
            && !matched.read_only
            && !is_virtual_fs(&matched.file_system)
            && !is_virtual_device(&matched.device_or_name)
            && matched.total >= MIN_EXTRA_DRIVE_BYTES
            && !covered_volumes.iter().any(|c| c.same_volume(matched))
        {
            let (topmost, name) = topmost_volume_info(matched, disks);
            let disk_label = format_disk_label(&topmost.mount, name);
            let home_str = home.to_string_lossy();
            let suggested_base = suggested_base_for_mount(home_str.trim());
            out.push(DetectedDrive {
                label: disk_label.clone(),
                disk_label,
                suggested_base,
                total: topmost.total,
                free: topmost.free,
                mount: topmost.mount.clone(),
            });
        }
    }

    out
}

/// Pure core of extra-drive detection, decoupled from real OS state for
/// cross-platform offline testing.
#[cfg(test)]
pub(crate) fn extra_drives_from_system(
    disks: &[SystemDisk],
    existing_paths: &[PathBuf],
    home_dir: Option<(&Path, bool)>,
) -> Vec<DetectedDrive> {
    extra_drives_with_resolver(disks, existing_paths, home_dir, Path::to_path_buf)
}

/// Detect writable extra drives against an already-refreshed `SystemDisk`
/// snapshot, following symlinks and checking the current user's home directory.
pub(crate) fn extra_drives_for_snapshot(
    disks: &[SystemDisk],
    existing_paths: &[PathBuf],
) -> Vec<DetectedDrive> {
    let home = dirs::home_dir();
    let home_ref = home
        .as_deref()
        .map(|p| (p, irori_core::util::is_dir_writable(p)));
    extra_drives_with_resolver(disks, existing_paths, home_ref, canonical_existing_or_self)
}

/// Detect writable main mounted drives not already represented by
/// `existing_paths`. Blocking: run off the UI thread.
pub fn detect_extra_drives(existing_paths: &[PathBuf]) -> Vec<DetectedDrive> {
    let disks = snapshot_system_disks();
    extra_drives_for_snapshot(&disks, existing_paths)
}

/// Probe configured install folders and extra writable drives in one
/// `sysinfo::Disks` refresh. Blocking: run off the UI thread.
pub fn probe_install_drives(configured_paths: &[PathBuf]) -> InstallDriveProbe {
    let disks = snapshot_system_disks();
    let configured_labels = configured_paths
        .iter()
        .map(|p| resolve_disk_label(p, &disks))
        .collect();
    let configured_space = configured_paths
        .iter()
        .map(|p| resolve_disk_space(p, &disks))
        .collect();
    let extras = extra_drives_for_snapshot(&disks, configured_paths);
    InstallDriveProbe {
        configured_labels,
        configured_space,
        extras,
    }
}
