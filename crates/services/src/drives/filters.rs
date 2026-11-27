//! Multi-layer filtering for mounted drives and Linux volume-name lookups
//! (`/dev/disk/by-label` and sysfs block model) without spawning subprocesses.

#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

/// Normalize a candidate volume or hardware model name, rejecting empty values
/// and raw device/path strings.
pub(super) fn clean_volume_name(raw: &str) -> Option<String> {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() || collapsed.starts_with(['/', '\\']) || collapsed.contains(':') {
        return None;
    }
    Some(collapsed)
}

/// Layer 1a: virtual, pseudo, container, and read-only-image filesystems.
pub(super) fn is_virtual_fs(fs: &str) -> bool {
    const VIRTUAL_FS: &[&str] = &[
        "tmpfs",
        "devtmpfs",
        "ramfs",
        "overlay",
        "overlayfs",
        "fuse.fuse-overlayfs",
        "squashfs",
        "erofs",
        "cramfs",
        "efivarfs",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "autofs",
        "devpts",
        "hugetlbfs",
        "mqueue",
        "pstore",
        "tracefs",
        "debugfs",
        "configfs",
        "securityfs",
        "binfmt_misc",
        "bpf",
        "fusectl",
        "rpc_pipefs",
        "nsfs",
        "portal",
        "fuse.portal",
        "fuse.gvfsd-fuse",
        "fuse.lxcfs",
        "fuse.appimage",
        "fuse.revokefs",
        "fuse.docportal",
        "iso9660",
        "udf",
        "devfs",
        "fdesc",
        "volfs",
        "nullfs",
    ];
    let lower = fs.trim().to_ascii_lowercase();
    !lower.is_empty() && VIRTUAL_FS.contains(&lower.as_str())
}

/// Layer 1b: Linux loop, swap, optical, and RAM block devices.
pub(super) fn is_virtual_device(dev: &str) -> bool {
    const VIRTUAL_PREFIXES: &[&str] = &["/dev/loop", "/dev/zram", "/dev/sr", "/dev/ram", "/dev/fd"];
    let trimmed = dev.trim();
    VIRTUAL_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Layer 3: known OS recovery, security, boot, OEM, and backup volume labels.
/// Applied only to auto-detected extra drives, never to user-configured folders.
pub(super) fn is_auxiliary_volume_label(label: &str) -> bool {
    const EXACT: &[&str] = &[
        "system reserved",
        "reservado pelo sistema",
        "réservé au système",
        "system-reserviert",
        "reservado para el sistema",
        "esp",
        "efi",
        "efi system",
        "efi system partition",
        "boot",
        "swap",
        "swapspace",
        "msr",
        "recovery",
        "recuperação",
        "recuperacao",
        "winre",
        "winre_drv",
        "winretools",
        "windows re tools",
        "pbr image",
        "pbr_drv",
        "hp_tools",
        "hp_recovery",
        "lenovo_part",
        "lenovo_recovery",
        "dellutility",
        "factory_image",
        "push button reset",
        "oem",
        "restore",
        "preboot",
        "vm",
        "update",
        "iscpreboot",
        "xart",
        "hardware",
        "time machine",
        "timemachine",
        "timeshift",
    ];
    let lower = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    EXACT.contains(&lower.as_str())
        || lower.contains("time machine")
        || lower.starts_with("com.apple.timemachine")
        || lower.contains("recovery")
        || lower.contains("winre")
        || lower.starts_with("efi ")
        || lower.ends_with(" efi")
}

/// Layer 4: OS, system, security, and snapshot mount paths on POSIX hosts.
pub(super) fn is_system_mount(mount: &str) -> bool {
    const SYSTEM_ROOTS: &[&str] = &[
        "/boot",
        "/efi",
        "/esp",
        "/snap",
        "/sys",
        "/proc",
        "/dev",
        "/var",
        "/tmp",
        "/etc",
        "/usr",
        "/nix",
        "/opt",
        "/srv",
        "/swap",
        "/root",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/lib32",
        "/lost+found",
        "/ostree",
        "/sysroot",
        "/recovery",
        "/cdrom",
        "/selinux",
        "/System",
        "/private",
        "/Library",
        "/Applications",
        "/cores",
        "/net",
        "/Volumes/Recovery",
        "/Volumes/Preboot",
        "/Volumes/VM",
        "/Volumes/Update",
    ];
    if mount == "/"
        || mount == "/run"
        || (mount.starts_with("/run/") && !mount.starts_with("/run/media/"))
        || SYSTEM_ROOTS.iter().any(|root| {
            mount
                .strip_prefix(root)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    {
        return true;
    }
    // Hidden mount segments (such as `/.snapshots`, `/mnt/disk/.snapshots`,
    // `/Volumes/.timemachine`) and macOS Time Machine mounts.
    mount.split('/').any(|seg| {
        seg.starts_with('.')
            || seg
                .to_ascii_lowercase()
                .starts_with("com.apple.timemachine")
    })
}

/// Unescape udev `\xNN` byte escapes used in `/dev/disk/by-label` symlink names.
#[cfg(any(target_os = "linux", test))]
pub(super) fn unescape_udev_label(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1] == b'x'
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 2]), hex_nibble(bytes[i + 3]))
        {
            out.push((hi << 4) | lo);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(any(target_os = "linux", test))]
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Read `/dev/disk/by-label` once into a map from canonical block device path
/// (for example `/dev/nvme1n1p1`) to filesystem label (for example `Asgard`).
#[cfg(target_os = "linux")]
pub(super) fn linux_fs_labels() -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/dev/disk/by-label") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(label) = clean_volume_name(&unescape_udev_label(&name)) else {
            continue;
        };
        if let Ok(target) = std::fs::canonicalize(entry.path()) {
            out.entry(target).or_insert(label);
        }
    }
    out
}

/// Look up the hardware disk model for a Linux block device via sysfs
/// (`/sys/class/block/<dev>/device/model`, its parent block device for
/// partitions, or a single `slaves/` hop for `dm-*` / LUKS volumes).
#[cfg(target_os = "linux")]
fn linux_block_model(device_path: &Path) -> Option<String> {
    let canonical_dev =
        std::fs::canonicalize(device_path).unwrap_or_else(|_| device_path.to_path_buf());
    let dev_name = canonical_dev.file_name()?.to_str()?;
    let sys_entry = std::fs::canonicalize(Path::new("/sys/class/block").join(dev_name)).ok()?;
    read_sysfs_model(&sys_entry)
        .or_else(|| sys_entry.parent().and_then(read_sysfs_model))
        .or_else(|| {
            let slaves = std::fs::read_dir(sys_entry.join("slaves")).ok()?;
            for slave in slaves.flatten() {
                let Ok(slave_sys) = std::fs::canonicalize(slave.path()) else {
                    continue;
                };
                if let Some(model) = read_sysfs_model(&slave_sys)
                    .or_else(|| slave_sys.parent().and_then(read_sysfs_model))
                {
                    return Some(model);
                }
            }
            None
        })
}

#[cfg(target_os = "linux")]
fn read_sysfs_model(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("device/model")).ok()?;
    clean_volume_name(&raw)
}

#[cfg(target_os = "linux")]
pub(super) fn resolve_linux_volume_name(
    device_raw: &str,
    fs_labels: &HashMap<PathBuf, String>,
    model_cache: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    if !device_raw.starts_with('/') {
        return None;
    }
    let dev_path = Path::new(device_raw);
    let canonical = std::fs::canonicalize(dev_path).unwrap_or_else(|_| dev_path.to_path_buf());
    if let Some(label) = fs_labels.get(&canonical) {
        return Some(label.clone());
    }
    model_cache
        .entry(canonical.clone())
        .or_insert_with(|| linux_block_model(&canonical))
        .clone()
}
