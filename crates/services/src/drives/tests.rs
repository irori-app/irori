use std::path::{Path, PathBuf};

use super::filters::unescape_udev_label;
use super::{
    SystemDisk, canonical_existing_or_self, drive_label, extra_drives_from_system,
    extra_drives_with_resolver, format_disk_label, resolve_disk_label, resolve_disk_label_raw,
};

const GIB: u64 = 1024 * 1024 * 1024;

fn sys_disk(
    device_or_name: &str,
    volume_name: Option<&str>,
    mount: &str,
    fs: &str,
    total_gib: u64,
    free_gib: u64,
) -> SystemDisk {
    SystemDisk {
        device_or_name: device_or_name.into(),
        volume_name: volume_name.map(str::to_string),
        mount: PathBuf::from(mount),
        file_system: fs.into(),
        total: total_gib * GIB,
        free: free_gib * GIB,
        read_only: false,
        writable: true,
    }
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
fn formats_disk_labels_uniformly_across_oses() {
    // Windows: `C: Name`, `D: Name`, unnamed `E:`, UNC share.
    assert_eq!(
        format_disk_label(Path::new(r"C:\"), Some("Windows")),
        "C: Windows"
    );
    assert_eq!(
        format_disk_label(Path::new(r"d:\"), Some("Asgard")),
        "D: Asgard"
    );
    assert_eq!(format_disk_label(Path::new(r"E:\"), None), "E:");
    assert_eq!(
        format_disk_label(Path::new(r"\\?\C:\"), Some("OS")),
        "C: OS"
    );
    assert_eq!(
        format_disk_label(Path::new(r"\\nas\games"), Some("ignored")),
        r"\\nas\games"
    );
    // Linux: root + model, matching basename avoids repetition, distinct label joins.
    assert_eq!(
        format_disk_label(Path::new("/"), Some("Reletech P400 EVO 2TB")),
        "/ · Reletech P400 EVO 2TB"
    );
    assert_eq!(
        format_disk_label(Path::new("/mnt/Asgard"), Some("Asgard")),
        "/mnt/Asgard"
    );
    assert_eq!(
        format_disk_label(Path::new("/mnt/nvme2"), Some("Asgard")),
        "/mnt/nvme2 · Asgard"
    );
    assert_eq!(
        format_disk_label(Path::new("/mnt/Asgard"), Some("/dev/nvme1n1p1")),
        "/mnt/Asgard"
    );
    // macOS: root + Macintosh HD, external volume avoids basename duplication.
    assert_eq!(
        format_disk_label(Path::new("/"), Some("Macintosh HD")),
        "/ · Macintosh HD"
    );
    assert_eq!(
        format_disk_label(Path::new("/Volumes/ExternalSSD"), Some("ExternalSSD")),
        "/Volumes/ExternalSSD"
    );
}

#[test]
fn unescapes_udev_by_label_hex_sequences() {
    assert_eq!(unescape_udev_label("Asgard"), "Asgard");
    assert_eq!(unescape_udev_label(r"My\x20Games\x20SSD"), "My Games SSD");
    assert_eq!(unescape_udev_label(r"Jogos\xc3\xa9"), "Jogosé");
    // Malformed escapes pass through unchanged.
    assert_eq!(unescape_udev_label(r"Drive\xZZ"), r"Drive\xZZ");
}

#[test]
fn btrfs_subvolumes_and_root_owned_mounts_never_leak_as_extra_drives() {
    let mut root = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/",
        "btrfs",
        1900,
        1350,
    );
    root.writable = false;
    let mut home = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/home",
        "btrfs",
        1900,
        1350,
    );
    home.writable = false;
    // Simulate `/srv`, `/swap`, and `/.snapshots` even if marked writable (e.g. running as root).
    let srv = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/srv",
        "btrfs",
        1900,
        1350,
    );
    let swap = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/swap",
        "btrfs",
        1900,
        1350,
    );
    let snapshots = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/.snapshots",
        "btrfs",
        1900,
        1350,
    );
    let boot = sys_disk("/dev/nvme0n1p1", Some("EFI"), "/boot", "vfat", 4, 3);
    let asgard = sys_disk(
        "/dev/nvme1n1p1",
        Some("Asgard"),
        "/mnt/Asgard",
        "ext4",
        938,
        177,
    );

    let disks = vec![root, home, srv, swap, snapshots, boot, asgard];
    let existing = vec![PathBuf::from("/home/edward/Games/Heroic")];

    // Default folder resolves to topmost mount `/` + hardware model.
    assert_eq!(
        resolve_disk_label_raw(Path::new("/home/edward/Games/Heroic"), &disks).as_deref(),
        Some("/ · Reletech P400 EVO 2TB")
    );

    let detected = extra_drives_from_system(&disks, &existing, None);
    assert_eq!(detected.len(), 1);
    assert_eq!(detected[0].mount, Path::new("/mnt/Asgard"));
    assert_eq!(detected[0].disk_label, "/mnt/Asgard");
    assert_eq!(detected[0].suggested_base, "/mnt/Asgard/Games/Irori");
    assert_eq!(detected[0].total, 938 * GIB);
    assert_eq!(detected[0].free, 177 * GIB);

    // Once `/mnt/Asgard/Games` is added as a configured library folder, it is not duplicated.
    let both = vec![
        PathBuf::from("/home/edward/Games/Heroic"),
        PathBuf::from("/mnt/Asgard/Games"),
    ];
    assert!(extra_drives_from_system(&disks, &both, None).is_empty());
}

#[test]
fn filters_security_recovery_virtual_and_non_writable_volumes_across_oses() {
    let mut ro = sys_disk("ReadOnly", Some("ReadOnly"), r"E:\", "ntfs", 500, 100);
    ro.read_only = true;
    let mut unwriteable = sys_disk("/dev/sdb1", Some("Data"), "/mnt/rootonly", "ext4", 500, 200);
    unwriteable.writable = false;

    let disks = vec![
        sys_disk("System", Some("System"), r"C:\", "ntfs", 500, 80),
        sys_disk("GamesSSD", Some("GamesSSD"), r"D:\", "ntfs", 1000, 420),
        ro,
        // Small Windows RE partition (< 5 GiB).
        sys_disk("WindowsRE", Some("WindowsRE"), r"F:\", "ntfs", 4, 1),
        // Large OEM / security / recovery partitions (>= 5 GiB) with drive letters.
        sys_disk(
            "System Reserved",
            Some("System Reserved"),
            r"G:\",
            "ntfs",
            12,
            8,
        ),
        sys_disk(
            "Reservado pelo Sistema",
            Some("Reservado pelo Sistema"),
            r"H:\",
            "ntfs",
            12,
            8,
        ),
        sys_disk("HP_RECOVERY", Some("HP_RECOVERY"), r"I:\", "ntfs", 20, 5),
        sys_disk("WinRE_DRV", Some("WinRE_DRV"), r"J:\", "ntfs", 10, 4),
        // Linux virtual filesystems, virtual block devices, and system/snapshot mounts.
        sys_disk("/dev/loop0", None, "/snap/core22/1", "squashfs", 20, 0),
        sys_disk("/dev/zram0", Some("zram0"), "/mnt/zram", "ext4", 32, 30),
        sys_disk("overlay", None, "/mnt/overlay", "overlay", 500, 200),
        sys_disk(
            "fuse-overlayfs",
            None,
            "/mnt/psd",
            "fuse.fuse-overlayfs",
            20,
            10,
        ),
        sys_disk("tmpfs", None, "/mnt/ram", "tmpfs", 32, 30),
        sys_disk("/dev/sda1", Some("EFI"), "/boot/efi", "vfat", 20, 10),
        sys_disk("/dev/sda2", None, "/run/user/1000", "ext4", 20, 10),
        sys_disk(
            "/dev/sda3",
            Some("Recovery"),
            "/mnt/Recovery",
            "ntfs3",
            20,
            10,
        ),
        sys_disk("/dev/sda4", None, "/mnt/disk/.snapshots", "btrfs", 500, 200),
        unwriteable,
        // Real user mounts across Linux and macOS.
        sys_disk("/dev/sdc1", None, "/mnt/games", "ext4", 1000, 600),
        sys_disk(
            "/dev/mmcblk0p1",
            Some("SDCard"),
            "/run/media/deck/SDCard",
            "ext4",
            512,
            200,
        ),
        sys_disk(
            "Data",
            Some("Data"),
            "/System/Volumes/Data",
            "apfs",
            500,
            120,
        ),
        sys_disk(
            "Preboot",
            Some("Preboot"),
            "/Volumes/Preboot",
            "apfs",
            500,
            120,
        ),
        sys_disk(
            "Time Machine",
            Some("Time Machine"),
            "/Volumes/Time Machine",
            "apfs",
            2000,
            900,
        ),
        sys_disk(
            "ExternalSSD",
            Some("ExternalSSD"),
            "/Volumes/ExternalSSD",
            "apfs",
            2000,
            900,
        ),
        // Custom root-level mounts keep distinct labels instead of colliding on `/`.
        sys_disk("/dev/sdd1", Some("Archive"), "/games", "ext4", 1000, 500),
        sys_disk("/dev/sde1", None, "/data", "xfs", 1000, 700),
    ];

    let existing = vec![PathBuf::from(r"C:\Users\eduardo\Games\Irori")];
    let detected = extra_drives_from_system(&disks, &existing, None);
    let labels: Vec<&str> = detected.iter().map(|d| d.disk_label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "D: GamesSSD",
            "/games · Archive",
            "/data",
            "/mnt/games",
            "/Volumes/ExternalSSD",
            "/run/media/deck/SDCard",
        ]
    );
    assert_eq!(detected[0].suggested_base, r"D:\Games\Irori");
    assert_eq!(detected[1].suggested_base, "/games/Games/Irori");
    assert_eq!(detected[3].suggested_base, "/mnt/games/Games/Irori");
    assert_eq!(
        detected[4].suggested_base,
        "/Volumes/ExternalSSD/Games/Irori"
    );

    // If `D:\` already hosts a configured library folder, don't duplicate it.
    let with_d = vec![
        PathBuf::from(r"C:\Games\Irori"),
        PathBuf::from(r"d:\MyLibrary"),
    ];
    let detected2 = extra_drives_from_system(&disks, &with_d, None);
    assert!(!detected2.iter().any(|d| d.disk_label.starts_with("D:")));
}

#[test]
fn surfaces_home_disk_when_default_install_moved_to_secondary_drive() {
    let mut root = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/",
        "btrfs",
        1900,
        1350,
    );
    root.writable = false;
    let mut home = sys_disk(
        "/dev/nvme0n1p2",
        Some("Reletech P400 EVO 2TB"),
        "/home",
        "btrfs",
        1900,
        1350,
    );
    home.writable = false;
    let asgard = sys_disk(
        "/dev/nvme1n1p1",
        Some("Asgard"),
        "/mnt/Asgard",
        "ext4",
        938,
        177,
    );
    let disks = vec![root, home, asgard];
    let existing = vec![PathBuf::from("/mnt/Asgard/Games")];

    let detected =
        extra_drives_from_system(&disks, &existing, Some((Path::new("/home/edward"), true)));
    assert_eq!(detected.len(), 1);
    assert_eq!(detected[0].mount, Path::new("/"));
    assert_eq!(detected[0].disk_label, "/ · Reletech P400 EVO 2TB");
    assert_eq!(detected[0].suggested_base, "/home/edward/Games/Irori");
}

#[test]
fn canonical_existing_or_self_preserves_missing_suffix_for_mount_matching() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical_tmp = canonical_existing_or_self(tmp.path());
    let missing_secondary_games = tmp.path().join("Secondary").join("Games");
    let canonical_secondary_mount = canonical_tmp.join("Secondary");

    // Non-existent subdirectories must stay attached after canonicalizing the
    // nearest existing ancestor so mount matching does not collapse to `/`.
    assert_eq!(
        canonical_existing_or_self(&missing_secondary_games),
        canonical_secondary_mount.join("Games")
    );

    let mut root = sys_disk(
        "/dev/nvme0n1p2",
        Some("RootDisk"),
        &canonical_tmp.to_string_lossy(),
        "btrfs",
        1900,
        1350,
    );
    root.writable = false;
    let secondary = sys_disk(
        "/dev/nvme1n1p1",
        Some("Secondary"),
        &canonical_secondary_mount.to_string_lossy(),
        "ext4",
        938,
        177,
    );
    let disks = vec![root, secondary];

    assert_eq!(
        resolve_disk_label(&missing_secondary_games, &disks),
        Some(format_disk_label(
            &canonical_secondary_mount,
            Some("Secondary")
        ))
    );

    let home_dir = canonical_tmp.join("home").join("player");
    let detected = extra_drives_with_resolver(
        &disks,
        std::slice::from_ref(&missing_secondary_games),
        Some((&home_dir, true)),
        canonical_existing_or_self,
    );
    assert_eq!(detected.len(), 1);
    assert_eq!(detected[0].mount, canonical_tmp);

    #[cfg(unix)]
    {
        let real_target = tmp.path().join("real_target");
        std::fs::create_dir(&real_target).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();
        assert_eq!(
            canonical_existing_or_self(&link.join("NewGames")),
            canonical_tmp.join("real_target").join("NewGames")
        );
    }
}
