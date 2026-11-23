//! Deleting one backup of a game from Ludusavi's backup folder. Ludusavi's
//! command line can lock and comment backups but not delete them, so its
//! `mapping.yaml` (the game's list of backups) is edited here: the entry is
//! cut out, every other key kept as Ludusavi wrote it, and the file
//! replaced atomically before the backup's own folder or zip is removed.
//!
//! A differential backup lives inside its full backup's `children`, so
//! deleting a full backup deletes its differential ones too. Locked backups
//! and the old single-backup layout (`.`) are never deleted.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_yaml_ng::Value;

const MAPPING: &str = "mapping.yaml";

/// Cut `id` out of the mapping `yaml`. Returns the new mapping and the
/// names of the backups removed with it (the backup and its children).
pub fn cut(yaml: &str, id: &str) -> Result<(String, Vec<String>)> {
    if id == "." || id.is_empty() || id.contains(['/', '\\']) || id == ".." {
        bail!("This backup can’t be deleted from Irori");
    }
    let mut root: Value =
        serde_yaml_ng::from_str(yaml).context("Ludusavi’s list of backups can’t be read")?;
    let backups = root
        .get_mut("backups")
        .and_then(Value::as_sequence_mut)
        .context("Ludusavi’s list of backups has no backups")?;
    let name_of = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_string);
    let locked = |v: &Value| v.get("locked").and_then(Value::as_bool).unwrap_or(false);
    let mut removed = Vec::new();
    if let Some(ix) = backups
        .iter()
        .position(|b| name_of(b).as_deref() == Some(id))
    {
        let full = &backups[ix];
        let children: Vec<&Value> = full
            .get("children")
            .and_then(Value::as_sequence)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        if locked(full) || children.iter().any(|c| locked(c)) {
            bail!("The backup is locked. Unlock it first.");
        }
        removed.push(id.to_string());
        removed.extend(children.iter().filter_map(|c| name_of(c)));
        backups.remove(ix);
    } else {
        let mut found = false;
        for full in backups.iter_mut() {
            let Some(children) = full.get_mut("children").and_then(Value::as_sequence_mut) else {
                continue;
            };
            if let Some(ix) = children
                .iter()
                .position(|c| name_of(c).as_deref() == Some(id))
            {
                if locked(&children[ix]) {
                    bail!("The backup is locked. Unlock it first.");
                }
                children.remove(ix);
                removed.push(id.to_string());
                found = true;
                break;
            }
        }
        if !found {
            bail!("Ludusavi has no backup {id}");
        }
    }
    let out = serde_yaml_ng::to_string(&root)?;
    Ok((out, removed))
}

/// Delete backup `id` from `folder` (a game's folder of backups). Returns
/// the backups removed (it and its differential ones).
pub fn delete(folder: &Path, id: &str) -> Result<Vec<String>> {
    let mapping = folder.join(MAPPING);
    let yaml = std::fs::read_to_string(&mapping)
        .with_context(|| format!("Cannot read {}", mapping.display()))?;
    let (out, removed) = cut(&yaml, id)?;
    irori_core::json_store::write_bytes(&mapping, out.as_bytes())
        .with_context(|| format!("Cannot write {}", mapping.display()))?;
    for name in &removed {
        let path: PathBuf = folder.join(name);
        let result = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        result.with_context(|| format!("Cannot remove {}", path.display()))?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPPING_YAML: &str = r#"name: Celeste
drives:
  drive-0: /home/u
backups:
  - name: backup-20241102T101500Z
    when: "2024-11-02T10:15:00Z"
    os: linux
    files:
      /home/u/a.sav:
        hash: abc
        size: 3
    registry:
      hash: ~
    children:
      - name: backup-20241103T101500Z-diff
        when: "2024-11-03T10:15:00Z"
        files: {}
      - name: backup-20241104T101500Z-diff
        when: "2024-11-04T10:15:00Z"
        locked: true
        files: {}
  - name: backup-20241110T101500Z.zip
    when: "2024-11-10T10:15:00Z"
    comment: Before the boss
    files: {}
    futureKey: kept
"#;

    #[test]
    fn cuts_a_differential_backup_and_keeps_everything_else() {
        let (out, removed) = cut(MAPPING_YAML, "backup-20241103T101500Z-diff").unwrap();
        assert_eq!(removed, ["backup-20241103T101500Z-diff"]);
        let v: Value = serde_yaml_ng::from_str(&out).unwrap();
        assert_eq!(v["backups"][0]["children"].as_sequence().unwrap().len(), 1);
        assert_eq!(v["backups"][1]["futureKey"].as_str(), Some("kept"));
        assert_eq!(v["drives"]["drive-0"].as_str(), Some("/home/u"));
    }

    #[test]
    fn a_locked_backup_or_one_with_locked_children_stays() {
        assert!(cut(MAPPING_YAML, "backup-20241104T101500Z-diff").is_err());
        assert!(cut(MAPPING_YAML, "backup-20241102T101500Z").is_err());
        assert!(cut(MAPPING_YAML, ".").is_err());
        assert!(cut(MAPPING_YAML, "../etc").is_err());
        assert!(cut(MAPPING_YAML, "backup-nope").is_err());
    }

    #[test]
    fn deletes_a_full_backup_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::write(folder.join(MAPPING), MAPPING_YAML).unwrap();
        std::fs::write(folder.join("backup-20241110T101500Z.zip"), b"zip").unwrap();
        std::fs::create_dir_all(folder.join("backup-20241102T101500Z/drive-0")).unwrap();
        let removed = delete(folder, "backup-20241110T101500Z.zip").unwrap();
        assert_eq!(removed, ["backup-20241110T101500Z.zip"]);
        assert!(!folder.join("backup-20241110T101500Z.zip").exists());
        assert!(folder.join("backup-20241102T101500Z").exists());
        let left = std::fs::read_to_string(folder.join(MAPPING)).unwrap();
        assert!(!left.contains("backup-20241110T101500Z.zip"));
        assert!(left.contains("backup-20241102T101500Z"));
    }
}
