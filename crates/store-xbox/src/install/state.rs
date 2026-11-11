//! Durable update barrier, independent of launcher queue/history state.
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

pub(crate) const INCOMPLETE: &str = ".irori-xbox-update-incomplete";
const LOCK: &str = ".irori-xbox.lock";

pub(crate) fn incomplete(root: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(root.join(INCOMPLETE)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn ensure_ready(root: &Path) -> io::Result<()> {
    if incomplete(root)? {
        return Err(io::Error::other(
            "An update was interrupted. Resume the update before playing.",
        ));
    }
    Ok(())
}

fn no_follow(options: &mut OpenOptions) -> &mut OpenOptions {
    options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
}

/// Hold the same installation lock as streaming, shared, while a game runs:
/// no download or update can change its files meanwhile. Read-only
/// installations remain launchable, but never bypass an existing lock.
pub(crate) fn launch_guard(root: &Path) -> io::Result<Option<File>> {
    launch_guard_with(root, |file| file.try_lock_shared().map_err(io::Error::from))
}

fn launch_guard_with(
    root: &Path,
    shared_lock: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<Option<File>> {
    ensure_ready(root)?;
    let path = root.join(LOCK);
    let file = match no_follow(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600),
    )
    .open(&path)
    {
        Ok(file) => Some(file),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            match no_follow(OpenOptions::new().read(true)).open(&path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    if let Some(file) = &file {
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("invalid installation lock"));
        }
        if let Err(error) = shared_lock(file) {
            if error.kind() == io::ErrorKind::Unsupported
                || matches!(
                    error.raw_os_error(),
                    Some(nix::libc::EOPNOTSUPP | nix::libc::ENOSYS)
                )
            {
                // Existing read-only/unsupported volumes must remain usable.
                // Streaming itself already requires an exclusive folder lock.
                ensure_ready(root)?;
                return Ok(None);
            }
            return Err(io::Error::other(
                "Finish this game's installation before playing it.",
            ));
        }
    }
    ensure_ready(root)?;
    Ok(file)
}

/// Sync this barrier before the first in-place file replacement. Do not remove
/// it on error/cancel/process death, even if the queue entry is later deleted.
pub(crate) fn begin(root: &Path) -> io::Result<()> {
    begin_with_placeholders(root, &[])
}

pub(crate) fn begin_with_placeholders(root: &Path, placeholders: &[String]) -> io::Result<()> {
    let path = root.join(INCOMPLETE);
    let mut file = match no_follow(OpenOptions::new().write(true).create_new(true).mode(0o600))
        .open(&path)
    {
        Ok(mut file) => {
            let bytes =
                serde_json::to_vec(&serde_json::json!({"schema":1,"placeholders":placeholders}))?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(io::Error::other("update placeholder snapshot is too large"));
            }
            file.write_all(&bytes)?;
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            no_follow(OpenOptions::new().read(true)).open(&path)?
        }
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("invalid incomplete-update marker"));
    }
    file.flush()?;
    file.sync_all()?;
    File::open(root)?.sync_all()
}

pub(crate) fn placeholders(root: &Path) -> io::Result<std::collections::HashSet<String>> {
    let file = no_follow(OpenOptions::new().read(true)).open(root.join(INCOMPLETE))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("invalid incomplete-update marker"));
    }
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Ok(Default::default());
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(Default::default());
    };
    if value["schema"] != 1 {
        return Ok(Default::default());
    }
    Ok(value["placeholders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect())
}

/// Publish and sync the new package metadata before allowing any game launch.
pub(crate) fn finish(root: &Path, cache: &Path, final_path: &Path) -> io::Result<()> {
    let file = no_follow(OpenOptions::new().read(true)).open(cache)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("invalid package metadata"));
    }
    file.sync_all()?;
    fs::rename(cache, final_path)?;
    File::open(root)?.sync_all()?;
    if incomplete(root)? {
        fs::remove_file(root.join(INCOMPLETE))?;
        File::open(root)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_update_barrier_survives_failure_and_clears_only_after_metadata_commit() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join(".irori-xbox-tmp.msixvc");
        let final_path = root.path().join(".irori-xbox.msixvc");
        fs::write(&final_path, b"old package").unwrap();
        begin(root.path()).unwrap();
        assert!(ensure_ready(root.path()).is_err());
        assert!(finish(root.path(), &cache, &final_path).is_err());
        assert!(incomplete(root.path()).unwrap());
        assert_eq!(fs::read(&final_path).unwrap(), b"old package");
        begin(root.path()).unwrap();
        fs::write(&cache, b"new package").unwrap();
        finish(root.path(), &cache, &final_path).unwrap();
        assert!(!incomplete(root.path()).unwrap());
        ensure_ready(root.path()).unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"new package");
    }

    #[test]
    fn streaming_update_launch_lock_excludes_writers_and_never_bypasses_an_active_download() {
        let root = tempfile::tempdir().unwrap();
        let reader = launch_guard(root.path()).unwrap();
        let file = OpenOptions::new()
            .write(true)
            .open(root.path().join(LOCK))
            .unwrap();
        assert!(file.try_lock().is_err());
        drop(reader);
        file.try_lock().unwrap();
        assert!(launch_guard(root.path()).is_err());
        drop(file);
        assert!(launch_guard(root.path()).is_ok());
    }

    #[test]
    fn streaming_update_barrier_and_locks_reject_symlinks_without_touching_targets() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), b"keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(INCOMPLETE)).unwrap();
        assert!(ensure_ready(root.path()).is_err());
        assert!(begin(root.path()).is_err());
        fs::remove_file(root.path().join(INCOMPLETE)).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(LOCK)).unwrap();
        assert!(launch_guard(root.path()).is_err());
        assert_eq!(fs::read(outside.path()).unwrap(), b"keep");
    }

    #[test]
    fn streaming_update_launch_preserves_read_only_and_unsupported_lock_installations() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o555)).unwrap();
        assert!(launch_guard(root.path()).is_ok());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        for errno in [nix::libc::EOPNOTSUPP, nix::libc::ENOSYS] {
            assert!(
                launch_guard_with(root.path(), |_| Err(io::Error::from_raw_os_error(errno)))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(launch_guard_with(root.path(), |_| Err(io::ErrorKind::WouldBlock.into())).is_err());
        begin(root.path()).unwrap();
        assert!(
            launch_guard_with(root.path(), |_| Err(io::ErrorKind::Unsupported.into())).is_err()
        );
    }
}
