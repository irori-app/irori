//! Sync and scheduled backups through a real backend and folders.

use super::merge::{PlaytimeShare, Reg};
use super::*;
use crate::tests::isolate_paths;
use crate::{BackupKind, BackupParts};

const GAME: &str = "gog:sync-test";

fn other_device() -> DeviceFile {
    let reg = |value| Reg {
        value,
        at: 1_790_000_000_000,
        by: "deck".into(),
        over: 0,
    };
    let mut shared = Shared::default();
    shared.registers.favourites.insert(GAME.into(), reg(true));
    shared.registers.ratings.insert(
        GAME.into(),
        Reg {
            value: Some(4),
            at: 1_790_000_000_000,
            by: "deck".into(),
            over: 0,
        },
    );
    shared.sessions.push(sessions::Session {
        game: GAME.into(),
        started: "2026-10-30T20:00:00Z".into(),
        seconds: 5400,
        crashed: false,
        exit_code: None,
    });
    shared
        .playtime
        .insert(GAME.into(), PlaytimeShare::default());
    DeviceFile {
        format: FORMAT.into(),
        version: VERSION,
        device: SyncDevice {
            id: "deck".into(),
            name: "Steam Deck".into(),
            os: "linux".into(),
            irori_version: "0.1.0".into(),
            updated: "2026-10-30T22:00:00Z".into(),
        },
        shared,
    }
}

#[test]
fn syncs_with_another_device_through_a_folder() {
    isolate_paths();
    let b = Backend::start_headless(|_, _| vec![]).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let dir = folder.path().join(target::DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("deck.json"),
        serde_json::to_vec(&other_device()).unwrap(),
    )
    .unwrap();
    // A device running a newer Irori is left alone, and said so.
    let mut newer = other_device();
    newer.version = VERSION + 1;
    newer.device.id = "laptop".into();
    newer.device.name = "Laptop".into();
    std::fs::write(dir.join("laptop.json"), serde_json::to_vec(&newer).unwrap()).unwrap();
    std::fs::write(dir.join("broken.json"), "{ not json").unwrap();

    b.settings.update_app(|a| {
        a.sync.mode = SyncMode::Folder;
        a.sync.folder = folder.path().to_string_lossy().into_owned();
        a.sync.device_name = "Desk".into();
    });
    let report = b.runtime().block_on(b.sync_now()).unwrap();
    assert_eq!(report.error, None);
    assert_eq!(
        report
            .devices
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["Steam Deck"]
    );
    assert!(report.newer_version);
    assert_eq!(report.skipped, ["broken.json", "Laptop"]);
    assert_eq!(report.sessions_added, 1);

    let data = b.userdata.snapshot();
    assert!(data.favourites.contains(GAME));
    assert_eq!(data.ratings[GAME], 4);
    assert_eq!(data.playtime[GAME].total_minutes, 90);
    assert!(
        sessions::read_all()
            .iter()
            .any(|s| s.game == GAME && s.seconds == 5400)
    );

    // This device's own file, with what it now knows.
    let status = b.sync_status();
    let own: DeviceFile = serde_json::from_slice(
        &std::fs::read(dir.join(format!("{}.json", status.device_id))).unwrap(),
    )
    .unwrap();
    assert_eq!(own.device.name, "Desk");
    assert!(own.shared.registers.favourites[GAME].value);
    assert_eq!(status.last_success, Some(report.at.clone()));

    // Nothing new: nothing changes and the playtime isn't added again.
    let again = b.runtime().block_on(b.sync_now()).unwrap();
    assert_eq!((again.changed, again.sessions_added), (0, 0));
    assert_eq!(b.userdata.snapshot().playtime[GAME].total_minutes, 90);

    // A folder that isn't there fails, and says why.
    b.settings.update_app(|a| {
        a.sync.folder = folder.path().join("gone").to_string_lossy().into_owned();
    });
    assert!(b.runtime().block_on(b.sync_now()).is_err());
    let status = b.sync_status();
    assert!(
        status
            .last
            .unwrap()
            .error
            .unwrap()
            .contains("isn’t available")
    );
    // The devices seen last time stay listed.
    b.settings.update_app(|a| a.sync.mode = SyncMode::Off);
}

#[test]
fn scheduled_backups_land_in_the_chosen_folder_without_this_computers_setup() {
    isolate_paths();
    let b = Backend::start_headless(|_, _| vec![]).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let chosen = folder.path().to_string_lossy().into_owned();
    b.settings.update_app(|a| {
        a.backups.folder = chosen.clone();
        a.backups.keep = 2;
    });
    let mut made = Vec::new();
    for _ in 0..3 {
        made.push(b.runtime().block_on(b.back_up_now()).unwrap());
        // Names are made to the second.
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }
    let listed = b.scheduled_backups();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|e| e.kind == BackupKind::Scheduled));
    assert!(!made[0].exists() && made[2].exists());
    let status = b.backup_status();
    assert!(status.last_ok.is_some() && status.last_error.is_none());

    // Where this computer backs up and syncs never travels.
    let backup = Backend::read_backup(&made[2]).unwrap();
    let settings = backup.settings.clone().unwrap();
    assert!(settings.get("iroriBackups").is_none());
    assert!(settings.get("iroriSync").is_none());
    b.restore_backup(
        &backup,
        BackupParts {
            settings: true,
            ..BackupParts::ALL
        },
    )
    .unwrap();
    assert_eq!(b.settings.app().backups.folder, chosen);

    // A chosen folder that isn't there is reported, not created.
    let gone = folder.path().join("unplugged");
    b.settings
        .update_app(|a| a.backups.folder = gone.to_string_lossy().into_owned());
    assert!(b.runtime().block_on(b.back_up_now()).is_err());
    assert!(!gone.exists());
    assert!(b.backup_status().last_error.is_some());
    b.settings.update_app(|a| a.backups.folder = String::new());
}
