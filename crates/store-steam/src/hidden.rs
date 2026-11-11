//! The games a user hid in Steam, read from the collections the Steam
//! client keeps on this computer.
//!
//! Since Steam's 2019 library, collections (the hidden one included) live in
//! Steam's cloud and are mirrored in
//! `userdata/<account>/config/cloudstorage/cloud-storage-namespace-1.json`:
//! a list of `[key, entry]` pairs whose `user-collections.hidden` entry holds
//! the collection as JSON text (`{"id": "hidden", "added": [ids],
//! "removed": [ids]}`). Changes not yet sent to Steam's cloud wait in
//! `cloud-storage-namespace-1.modified.json`, whose entries win.
//!
//! Steam's Web API has no call that lists them, so a signed-in account sees
//! its hidden games only when Steam on this computer keeps that account's
//! files. Hidden games stay in the library: Irori hides them with its own
//! mechanism (`GameInfo::store_hidden`), where the user's own choice wins.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use irori_runtime::vdf;
use serde_json::Value;

/// Steam's offset between a SteamID64 and the account ID that names the
/// user's `userdata` folder.
const STEAMID64_BASE: u64 = 76_561_197_960_265_728;

/// The key of the hidden collection.
const HIDDEN_KEY: &str = "user-collections.hidden";

/// The `userdata` folder of the user whose hidden games count: the signed-in
/// account's (never another user's, whose choices aren't this account's),
/// else the user Steam remembers as the latest on this computer, else the
/// one whose settings were written last.
pub(crate) fn user_dir(steam_dir: &Path, steamid: Option<&str>) -> Option<PathBuf> {
    let userdata = steam_dir.join("userdata");
    if let Some(steamid) = steamid {
        let account = account_id(steamid)?;
        let dir = userdata.join(account.to_string());
        return dir.is_dir().then_some(dir);
    }
    if let Some(dir) = most_recent_user(steam_dir)
        .map(|account| userdata.join(account.to_string()))
        .filter(|d| d.is_dir())
    {
        return Some(dir);
    }
    std::fs::read_dir(&userdata)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .parse::<u64>()
                .is_ok_and(|n| n > 0)
        })
        .filter_map(|e| {
            let modified = std::fs::metadata(e.path().join("config").join("localconfig.vdf"))
                .ok()?
                .modified()
                .ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn account_id(steamid: &str) -> Option<u64> {
    steamid
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_sub(STEAMID64_BASE)
}

/// The account `config/loginusers.vdf` marks `MostRecent`.
fn most_recent_user(steam_dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(steam_dir.join("config").join("loginusers.vdf")).ok()?;
    let doc = vdf::parse(&text);
    doc.get("users")?
        .entries()
        .iter()
        .find_map(|(steamid, user)| {
            let recent = user
                .get("MostRecent")
                .and_then(vdf::Vdf::as_str)
                .is_some_and(|v| v.trim() == "1");
            recent.then(|| account_id(steamid)).flatten()
        })
}

/// The app IDs the user of `user_dir` hid in Steam. `None` when Steam keeps
/// no readable collections there, so Irori can't tell. Blocking.
pub(crate) fn hidden_apps(user_dir: &Path) -> Option<HashSet<String>> {
    let folder = user_dir.join("config").join("cloudstorage");
    let read = |name: &str| {
        let text = std::fs::read_to_string(folder.join(name)).ok()?;
        let entries = parse_namespace(&text);
        if entries.is_none() {
            tracing::debug!("unreadable Steam collections in {}", folder.display());
        }
        entries
    };
    let base = read("cloud-storage-namespace-1.json");
    let modified = read("cloud-storage-namespace-1.modified.json");
    if base.is_none() && modified.is_none() {
        return None;
    }
    let mut entries = base.unwrap_or_default();
    entries.extend(modified.unwrap_or_default());
    Some(
        entries
            .get(HIDDEN_KEY)
            .map(collection_apps)
            .unwrap_or_default(),
    )
}

/// The entries of a namespace file by key; `None` when it isn't one.
pub(crate) fn parse_namespace(text: &str) -> Option<HashMap<String, Value>> {
    let list: Vec<Value> = serde_json::from_str(text).ok()?;
    Some(
        list.into_iter()
            .filter_map(|pair| {
                let mut pair = match pair {
                    Value::Array(pair) if pair.len() == 2 => pair,
                    _ => return None,
                };
                let entry = pair.pop()?;
                let key = pair.pop()?.as_str()?.to_string();
                entry.is_object().then_some((key, entry))
            })
            .collect(),
    )
}

/// The apps of one collection entry: added and not removed again. A deleted
/// entry or one Irori can't read holds none.
pub(crate) fn collection_apps(entry: &Value) -> HashSet<String> {
    if entry.get("is_deleted").and_then(Value::as_bool) == Some(true) {
        return HashSet::new();
    }
    let Some(collection) = entry
        .get("value")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return HashSet::new();
    };
    let ids = |field: &str| -> HashSet<String> {
        collection
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter(|id| *id > 0 && *id < 1 << 31)
            .map(|id| id.to_string())
            .collect()
    };
    let removed = ids("removed");
    ids("added")
        .into_iter()
        .filter(|id| !removed.contains(id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace(hidden: &str) -> String {
        let value = serde_json::to_string(hidden).unwrap();
        format!(
            r#"[
  ["user-collections.favorite", {{"key": "user-collections.favorite", "timestamp": 1700000000,
     "value": "{{\"id\":\"favorite\",\"added\":[620],\"removed\":[]}}", "version": "12"}}],
  ["{HIDDEN_KEY}", {{"key": "{HIDDEN_KEY}", "timestamp": 1700000001,
     "value": {value}, "version": "13", "conflictResolutionMethod": "custom",
     "strMethodId": "union-collections", "someNewField": [1, 2]}}],
  ["showcases.1", {{"key": "showcases.1", "timestamp": 1, "is_deleted": true, "version": "2"}}],
  "something a newer Steam writes"
]"#
        )
    }

    #[test]
    fn reads_the_hidden_collection() {
        let entries = parse_namespace(&namespace(
            r#"{"id":"hidden","name":"","added":[70,400,620,0,-3,"x"],"removed":[400]}"#,
        ))
        .unwrap();
        let mut ids: Vec<String> = collection_apps(&entries[HIDDEN_KEY]).into_iter().collect();
        ids.sort();
        assert_eq!(ids, ["620", "70"]);
        assert!(collection_apps(&entries["showcases.1"]).is_empty());
        assert!(parse_namespace("{}").is_none());
        assert!(parse_namespace("garbage").is_none());
        // A collection whose text isn't JSON hides nothing.
        let broken = serde_json::json!({"value": "not json"});
        assert!(collection_apps(&broken).is_empty());
    }

    #[test]
    fn local_changes_win_over_the_synced_file() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("userdata/7");
        assert_eq!(hidden_apps(&user), None, "no files: Irori can't tell");
        let folder = user.join("config/cloudstorage");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("cloud-storage-namespace-1.json"),
            namespace(r#"{"id":"hidden","added":[70],"removed":[]}"#),
        )
        .unwrap();
        assert_eq!(hidden_apps(&user), Some(HashSet::from(["70".to_string()])));
        std::fs::write(
            folder.join("cloud-storage-namespace-1.modified.json"),
            namespace(r#"{"id":"hidden","added":[220],"removed":[]}"#),
        )
        .unwrap();
        assert_eq!(hidden_apps(&user), Some(HashSet::from(["220".to_string()])));
        // A user who hid nothing: readable, with no hidden games.
        std::fs::write(folder.join("cloud-storage-namespace-1.json"), "[]").unwrap();
        std::fs::remove_file(folder.join("cloud-storage-namespace-1.modified.json")).unwrap();
        assert_eq!(hidden_apps(&user), Some(HashSet::new()));
    }

    #[test]
    fn picks_the_signed_in_or_latest_user() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path();
        for account in ["5", "7", "0"] {
            std::fs::create_dir_all(steam.join("userdata").join(account).join("config")).unwrap();
        }
        let steamid = |account: u64| (STEAMID64_BASE + account).to_string();
        // The signed-in account's folder, never another user's.
        assert_eq!(
            user_dir(steam, Some(&steamid(7))),
            Some(steam.join("userdata/7"))
        );
        assert_eq!(user_dir(steam, Some(&steamid(9))), None);
        assert_eq!(user_dir(steam, Some("garbage")), None);
        // Without one: the user whose settings were written last…
        std::fs::write(steam.join("userdata/5/config/localconfig.vdf"), "x").unwrap();
        assert_eq!(user_dir(steam, None), Some(steam.join("userdata/5")));
        // …unless Steam marks another as the latest.
        std::fs::create_dir_all(steam.join("config")).unwrap();
        std::fs::write(
            steam.join("config/loginusers.vdf"),
            format!(
                "\"users\" {{ \"{}\" {{ \"MostRecent\" \"0\" }} \"{}\" {{ \"MostRecent\" \"1\" }} }}",
                steamid(5),
                steamid(7)
            ),
        )
        .unwrap();
        assert_eq!(user_dir(steam, None), Some(steam.join("userdata/7")));
        assert_eq!(user_dir(&steam.join("missing"), None), None);
    }
}
