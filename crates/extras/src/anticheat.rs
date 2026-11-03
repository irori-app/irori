//! AreWeAntiCheatYet data: download, on-disk cache, lookup, and the install
//! gate for games whose anticheat does not work on Linux.
//!
//! AreWeAntiCheatYet rates a game as a whole. Where its anticheat guards only
//! an online mode and the story plays fine under Wine (GTA Online in GTA V),
//! [`KNOWN`] says so: the lookup marks the entry [`AntiCheatInfo::online_only`]
//! and the install gate lets it through.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::AppSettings;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{releases, util};

/// Upstream data for Linux.
pub const DATA_URL: &str =
    "https://raw.githubusercontent.com/Starz0r/AreWeAntiCheatYet/HEAD/games.json";

/// Support status of a game's anticheat on Linux/Proton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AntiCheatStatus {
    Supported,
    Running,
    Working,
    Planned,
    Broken,
    Denied,
    #[serde(other)]
    Unknown,
}

impl AntiCheatStatus {
    /// Multiplayer is expected not to work.
    pub fn is_broken(self) -> bool {
        matches!(
            self,
            AntiCheatStatus::Broken | AntiCheatStatus::Denied | AntiCheatStatus::Unknown
        )
    }
}

/// A note with an optional source link. Upstream writes `[text, url]` pairs;
/// plain strings are accepted too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AntiCheatNote {
    pub text: String,
    pub url: Option<String>,
}

impl<'de> Deserialize<'de> for AntiCheatNote {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            // `[text, url]`; the URL is `null` when absent.
            Pair(Vec<Option<String>>),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Text(text) => AntiCheatNote { text, url: None },
            Raw::Pair(mut v) => {
                let url = (v.len() > 1)
                    .then(|| v.remove(1))
                    .flatten()
                    .filter(|s| !s.is_empty());
                AntiCheatNote {
                    text: v.into_iter().next().flatten().unwrap_or_default(),
                    url,
                }
            }
        })
    }
}

/// A status change with its source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiCheatUpdate {
    pub name: String,
    pub date: String,
    pub reference: String,
}

/// Epic identifiers of an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EpicIds {
    pub namespace: String,
    pub slug: String,
}

/// Store identifiers of an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiCheatStoreIds {
    pub epic: Option<EpicIds>,
    pub steam: Option<String>,
}

/// One AreWeAntiCheatYet entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AntiCheatInfo {
    pub name: String,
    pub slug: String,
    pub status: AntiCheatStatus,
    /// Anticheat products ("Easy Anti-Cheat", "BattlEye", …). Empty when the
    /// anticheat was removed.
    pub anticheats: Vec<String>,
    pub notes: Vec<AntiCheatNote>,
    pub native: bool,
    pub store_ids: AntiCheatStoreIds,
    pub reference: String,
    pub updates: Vec<AntiCheatUpdate>,
    /// The only part of the game the anticheat keeps from running under
    /// Wine ("GTA Online"); the rest works. Irori's own knowledge ([`KNOWN`]),
    /// never in the upstream data.
    #[serde(skip)]
    pub online_only: Option<String>,
}

impl Default for AntiCheatInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            slug: String::new(),
            status: AntiCheatStatus::Unknown,
            anticheats: Vec::new(),
            notes: Vec::new(),
            native: false,
            store_ids: AntiCheatStoreIds::default(),
            reference: String::new(),
            updates: Vec::new(),
            online_only: None,
        }
    }
}

/// A game whose anticheat blocks only its online mode under Wine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineOnly {
    /// The game's names, as stores and AreWeAntiCheatYet write them.
    pub names: &'static [&'static str],
    pub steam_app_ids: &'static [&'static str],
    pub epic_namespaces: &'static [&'static str],
    /// The online mode the anticheat guards, never translated.
    pub mode: &'static str,
    pub anticheat: &'static str,
    /// Where the facts come from.
    pub reference: &'static str,
}

/// Games whose story works under Wine while their online mode doesn't.
pub const KNOWN: &[OnlineOnly] = &[OnlineOnly {
    names: &[
        "Grand Theft Auto V",
        "Grand Theft Auto V Legacy",
        "Grand Theft Auto V Enhanced",
        "Grand Theft Auto V: Premium Edition",
    ],
    steam_app_ids: &["271590", "3240220"],
    epic_namespaces: &["0584d2013f0149a791e7b9bad0eec102"],
    mode: "GTA Online",
    anticheat: "BattlEye",
    reference: "https://support.rockstargames.com/articles/1nenwhZlVrJY6CTFeSS2Fx/grand-theft-auto-online-battleye-faq",
}];

impl OnlineOnly {
    fn matches(&self, q: &AntiCheatQuery<'_>) -> bool {
        let title = normalize_title(q.title);
        q.epic_namespace.is_some_and(|ns| {
            self.epic_namespaces
                .iter()
                .any(|n| n.eq_ignore_ascii_case(ns))
        }) || q
            .steam_app_id
            .is_some_and(|id| self.steam_app_ids.contains(&id))
            || (!title.is_empty() && self.names.iter().any(|n| normalize_title(n) == title))
    }
}

/// The game's entry in `db` with Irori's own facts applied. A game in
/// [`KNOWN`] is found even before the data was downloaded, and keeps the
/// upstream rating once that says its anticheat works.
pub fn resolve(db: Option<&AntiCheatDb>, q: &AntiCheatQuery<'_>) -> Option<AntiCheatInfo> {
    let entry = db.and_then(|db| db.lookup(q)).cloned();
    let Some(known) = KNOWN.iter().find(|k| k.matches(q)) else {
        return entry;
    };
    let mut info = entry.unwrap_or_else(|| AntiCheatInfo {
        name: known.names[0].to_string(),
        status: AntiCheatStatus::Denied,
        anticheats: vec![known.anticheat.to_string()],
        reference: known.reference.to_string(),
        ..Default::default()
    });
    if info.status.is_broken() {
        info.online_only = Some(known.mode.to_string());
    }
    Some(info)
}

/// What to look a game up by. Any field may be empty.
#[derive(Debug, Clone, Copy, Default)]
pub struct AntiCheatQuery<'a> {
    pub title: &'a str,
    /// Epic namespace (`GameInfo::namespace`).
    pub epic_namespace: Option<&'a str>,
    pub steam_app_id: Option<&'a str>,
}

/// The parsed data set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntiCheatDb {
    pub entries: Vec<AntiCheatInfo>,
}

fn normalize_title(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl AntiCheatDb {
    pub fn parse(json: &[u8]) -> Result<Self> {
        Ok(Self {
            entries: serde_json::from_slice(json).context("AreWeAntiCheatYet data is not valid")?,
        })
    }

    /// Match by Epic namespace (exact, case-insensitive), then Steam app ID,
    /// then normalised title.
    pub fn lookup(&self, q: &AntiCheatQuery<'_>) -> Option<&AntiCheatInfo> {
        if let Some(ns) = q.epic_namespace.filter(|s| !s.is_empty()) {
            let hit = self.entries.iter().find(|e| {
                e.store_ids
                    .epic
                    .as_ref()
                    .is_some_and(|ep| ep.namespace.eq_ignore_ascii_case(ns))
            });
            if hit.is_some() {
                return hit;
            }
        }
        if let Some(id) = q.steam_app_id.filter(|s| !s.is_empty()) {
            let hit = self
                .entries
                .iter()
                .find(|e| e.store_ids.steam.as_deref() == Some(id));
            if hit.is_some() {
                return hit;
            }
        }
        let title = normalize_title(q.title);
        if title.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| normalize_title(&e.name) == title)
    }
}

fn data_file() -> PathBuf {
    util::cache_dir("anticheat").join("areweanticheatyet.json")
}

static DB: Lazy<RwLock<Option<Arc<AntiCheatDb>>>> = Lazy::new(|| RwLock::new(None));

/// Download the data when the cache is missing or `latest_md5` (from
/// [`releases::ReleasesInfo::anticheat_files`]) differs. Returns whether a new
/// file was written.
pub async fn refresh(latest_md5: Option<&str>) -> Result<bool> {
    let path = data_file();
    if !releases::needs_download(&path, latest_md5).await {
        return Ok(false);
    }
    let text = util::get_text(DATA_URL, Duration::from_secs(30))
        .await
        .context("downloading AreWeAntiCheatYet data")?;
    let db = AntiCheatDb::parse(text.as_bytes())?;
    util::write_atomic(&path, text.as_bytes()).await?;
    *DB.write() = Some(Arc::new(db));
    Ok(true)
}

/// The cached data set, loaded from disk once. `None` before the first
/// successful [`refresh`].
pub async fn database() -> Option<Arc<AntiCheatDb>> {
    if let Some(db) = DB.read().clone() {
        return Some(db);
    }
    let bytes = tokio::fs::read(data_file()).await.ok()?;
    let db = match tokio::task::spawn_blocking(move || AntiCheatDb::parse(&bytes)).await {
        Ok(Ok(db)) => Arc::new(db),
        Ok(Err(e)) => {
            tracing::warn!("{e:#}");
            return None;
        }
        Err(_) => return None,
    };
    *DB.write() = Some(db.clone());
    Some(db)
}

/// Look a game up in the cached data, with Irori's own facts applied.
pub async fn lookup(q: &AntiCheatQuery<'_>) -> Option<AntiCheatInfo> {
    resolve(database().await.as_deref(), q)
}

/// Result of the anticheat install check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallGate {
    /// No known problem.
    Allowed,
    /// Broken/denied anticheat, but the user allowed such installs: ask
    /// "Yes (I understand the multiplayer features will not work)" / "No".
    Confirm,
    /// Broken/denied anticheat and installs of such games are disabled
    /// (Settings → Advanced): explain and do not install.
    Blocked,
}

impl InstallGate {
    pub fn blocks(self) -> bool {
        self == InstallGate::Blocked
    }
}

/// Install gate for a game with the given anticheat info. A game whose
/// anticheat only guards an online mode installs freely.
pub fn blocks_install(info: Option<&AntiCheatInfo>, app: &AppSettings) -> InstallGate {
    if cfg!(windows) {
        return InstallGate::Allowed;
    }
    match info {
        Some(i) if i.status.is_broken() && i.online_only.is_none() => {
            if app.allow_installation_broken_anticheat {
                InstallGate::Confirm
            } else {
                InstallGate::Blocked
            }
        }
        _ => InstallGate::Allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {"url":"","name":"Halo: The Master Chief Collection","logo":"","native":false,"status":"Supported",
       "reference":"","anticheats":["Easy Anti-Cheat"],
       "notes":[["All-modes enabled","https://example.org/a"],["Plain",""],["Null url",null]],
       "updates":[{"name":"Steam Deck Support","date":"Apr 6, 2023","reference":"https://example.org/u"}],
       "storeIds":{"steam":"976730"},"slug":"halo-mcc","dateChanged":"2024-01-19T04:40:56.000Z"},
      {"name":"Fortnite","status":"Denied","anticheats":["Easy Anti-Cheat","BattlEye"],"notes":["text only"],
       "native":false,"storeIds":{"epic":{"namespace":"fn","slug":"fortnite"}},"reference":"r","updates":[]},
      {"name":"Future","status":"SomethingNew","anticheats":[],"notes":[],"storeIds":{}}
    ]"#;

    #[test]
    fn maps_upstream_json() {
        let db = AntiCheatDb::parse(FIXTURE.as_bytes()).unwrap();
        assert_eq!(db.entries.len(), 3);
        let halo = &db.entries[0];
        assert_eq!(halo.status, AntiCheatStatus::Supported);
        assert_eq!(halo.notes[0].url.as_deref(), Some("https://example.org/a"));
        assert_eq!(halo.notes[1].url, None);
        assert_eq!(halo.updates[0].name, "Steam Deck Support");
        assert_eq!(db.entries[1].notes[0].text, "text only");
        assert_eq!(db.entries[2].status, AntiCheatStatus::Unknown);
    }

    #[test]
    fn lookup_order() {
        let db = AntiCheatDb::parse(FIXTURE.as_bytes()).unwrap();
        let q = AntiCheatQuery {
            epic_namespace: Some("FN"),
            ..Default::default()
        };
        assert_eq!(db.lookup(&q).unwrap().name, "Fortnite");
        let q = AntiCheatQuery {
            epic_namespace: Some("f"),
            ..Default::default()
        };
        assert!(db.lookup(&q).is_none());
        let q = AntiCheatQuery {
            steam_app_id: Some("976730"),
            ..Default::default()
        };
        assert_eq!(db.lookup(&q).unwrap().slug, "halo-mcc");
        let q = AntiCheatQuery {
            title: "HALO - The Master Chief Collection",
            ..Default::default()
        };
        assert!(db.lookup(&q).is_some());
    }

    #[test]
    fn install_gate() {
        let db = AntiCheatDb::parse(FIXTURE.as_bytes()).unwrap();
        let mut app = AppSettings::default();
        assert_eq!(blocks_install(None, &app), InstallGate::Allowed);
        assert_eq!(
            blocks_install(Some(&db.entries[0]), &app),
            InstallGate::Allowed
        );
        if cfg!(windows) {
            assert_eq!(
                blocks_install(Some(&db.entries[1]), &app),
                InstallGate::Allowed
            );
            return;
        }
        assert!(blocks_install(Some(&db.entries[1]), &app).blocks());
        assert_eq!(
            blocks_install(Some(&db.entries[2]), &app),
            InstallGate::Blocked
        );
        app.allow_installation_broken_anticheat = true;
        assert_eq!(
            blocks_install(Some(&db.entries[1]), &app),
            InstallGate::Confirm
        );
    }
}

#[cfg(test)]
mod known_tests {
    use super::*;

    const GTA: &str = r#"[{"name":"Grand Theft Auto V","status":"Denied","anticheats":["BattlEye"],
       "storeIds":{"steam":"271590","epic":{"namespace":"0584d2013f0149a791e7b9bad0eec102","slug":"gta"}},
       "updates":[{"name":"BattlEye added","date":"","reference":"https://example.org/battleye"}]},
      {"name":"Fortnite","status":"Denied","anticheats":["Easy Anti-Cheat"],"storeIds":{}}]"#;

    #[test]
    fn gta_online_is_blocked_but_the_story_installs() {
        let db = AntiCheatDb::parse(GTA.as_bytes()).unwrap();
        let app = AppSettings::default();
        for q in [
            AntiCheatQuery {
                title: "Grand Theft Auto V Legacy",
                ..Default::default()
            },
            AntiCheatQuery {
                title: "Anything",
                epic_namespace: Some("0584D2013F0149A791E7B9BAD0EEC102"),
                steam_app_id: None,
            },
            AntiCheatQuery {
                title: "Grand Theft Auto V Enhanced",
                steam_app_id: Some("3240220"),
                ..Default::default()
            },
        ] {
            let info = resolve(Some(&db), &q).expect("known");
            assert_eq!(info.online_only.as_deref(), Some("GTA Online"), "{q:?}");
            assert_eq!(info.status, AntiCheatStatus::Denied);
            assert_eq!(info.anticheats, ["BattlEye"]);
            assert_eq!(blocks_install(Some(&info), &app), InstallGate::Allowed);
        }
        // Known before the data was ever downloaded.
        let offline = resolve(
            None,
            &AntiCheatQuery {
                title: "Grand Theft Auto V",
                ..Default::default()
            },
        )
        .expect("known");
        assert_eq!(offline.online_only.as_deref(), Some("GTA Online"));
        assert!(
            offline
                .reference
                .starts_with("https://support.rockstargames.com/")
        );

        // Other games are rated as upstream says.
        let fortnite = resolve(
            Some(&db),
            &AntiCheatQuery {
                title: "Fortnite",
                ..Default::default()
            },
        )
        .expect("listed");
        assert_eq!(fortnite.online_only, None);
        if !cfg!(windows) {
            assert!(blocks_install(Some(&fortnite), &app).blocks());
        }
        assert_eq!(
            resolve(
                None,
                &AntiCheatQuery {
                    title: "Fortnite",
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn a_working_upstream_rating_wins() {
        let fixed = GTA.replace(r#""Denied""#, r#""Running""#);
        let db = AntiCheatDb::parse(fixed.as_bytes()).unwrap();
        let info = resolve(
            Some(&db),
            &AntiCheatQuery {
                title: "Grand Theft Auto V",
                ..Default::default()
            },
        )
        .expect("listed");
        assert_eq!(info.status, AntiCheatStatus::Running);
        assert_eq!(info.online_only, None);
    }
}

#[cfg(test)]
mod real_data {
    /// Parses the user's real Heroic copy when present (skipped otherwise).
    #[test]
    fn parses_heroic_copy() {
        let Some(home) = dirs::home_dir() else { return };
        let p = home.join(".config/heroic/areweanticheatyet.json");
        if let Ok(bytes) = std::fs::read(&p) {
            super::AntiCheatDb::parse(&bytes).unwrap();
        }
    }
}
