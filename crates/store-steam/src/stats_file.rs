//! Achievements Steam keeps on this computer, for players without a Steam
//! account in Irori: `appcache/stats/UserGameStatsSchema_<app>.bin` (what
//! a game offers, in every language) and `UserGameStats_<account>_<app>.bin`
//! (what the account unlocked, and when). Both are binary KeyValues.
//!
//! Achievements are bits of statistics whose `type` is 4: the schema names
//! each bit; the player's file holds the statistic's bit field in `data`
//! and each bit's unlock time under `AchievementTimes`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use irori_core::Achievement;

/// The icon address of an achievement icon file.
fn icon_url(app_id: &str, icon: &str) -> String {
    irori_steam_api::achievements::icon_url(app_id.parse().unwrap_or(0), icon)
}

/// A binary KeyValues value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kv {
    Map(BTreeMap<String, Kv>),
    Str(String),
    Int(i64),
    Float(f32),
}

impl Kv {
    pub fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Kv::Map(m) => m.get(key).or_else(|| {
                m.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v)
            }),
            _ => None,
        }
    }

    pub fn map(&self) -> Option<&BTreeMap<String, Kv>> {
        match self {
            Kv::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Kv::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a number, from an integer or a numeric string.
    pub fn int(&self) -> Option<i64> {
        match self {
            Kv::Int(i) => Some(*i),
            Kv::Float(f) => Some(*f as i64),
            Kv::Str(s) => s.trim().parse().ok(),
            Kv::Map(_) => None,
        }
    }
}

/// Parse a binary KeyValues document into its root map.
pub(crate) fn parse(data: &[u8]) -> Option<Kv> {
    let mut at = 0;
    let map = parse_map(data, &mut at, 0)?;
    Some(Kv::Map(map))
}

fn parse_map(data: &[u8], at: &mut usize, depth: usize) -> Option<BTreeMap<String, Kv>> {
    if depth > 32 {
        return None;
    }
    let mut map = BTreeMap::new();
    while *at < data.len() {
        let kind = data[*at];
        *at += 1;
        if kind == 0x08 || kind == 0x0b {
            return Some(map);
        }
        let key = cstr(data, at)?;
        let value = match kind {
            0x00 => Kv::Map(parse_map(data, at, depth + 1)?),
            0x01 => Kv::Str(cstr(data, at)?),
            0x02 | 0x04 | 0x06 => Kv::Int(i32::from_le_bytes(take::<4>(data, at)?) as i64),
            0x03 => Kv::Float(f32::from_le_bytes(take::<4>(data, at)?)),
            0x07 | 0x0a => Kv::Int(i64::from_le_bytes(take::<8>(data, at)?)),
            _ => return None,
        };
        map.insert(key, value);
    }
    Some(map)
}

fn cstr(data: &[u8], at: &mut usize) -> Option<String> {
    let end = data[*at..].iter().position(|b| *b == 0)? + *at;
    let s = String::from_utf8_lossy(&data[*at..end]).into_owned();
    *at = end + 1;
    Some(s)
}

fn take<const N: usize>(data: &[u8], at: &mut usize) -> Option<[u8; N]> {
    let bytes = data.get(*at..*at + N)?.try_into().ok()?;
    *at += N;
    Some(bytes)
}

pub(crate) use irori_steam_api::achievements::steam_language;

/// A localized text: `language`, else English, else any.
fn localized(v: Option<&Kv>, language: &str) -> String {
    match v {
        Some(Kv::Str(s)) => s.clone(),
        Some(m @ Kv::Map(map)) => m
            .get(language)
            .or_else(|| m.get("english"))
            .and_then(Kv::str)
            .or_else(|| map.values().find_map(Kv::str))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// Achievements a schema file offers, in its order, each keyed by
/// `(statistic, bit)` for the player's file.
pub(crate) fn schema_achievements(
    schema: &Kv,
    app_id: &str,
    language: &str,
) -> Vec<((String, u32), Achievement)> {
    let root = schema
        .map()
        .and_then(|m| m.get(app_id).or_else(|| m.values().next()));
    let Some(stats) = root.and_then(|r| r.get("stats")).and_then(Kv::map) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut ids: Vec<(&String, &Kv)> = stats.iter().collect();
    ids.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(u64::MAX));
    for (stat_id, stat) in ids {
        let kind = stat.get("type");
        let is_bits =
            kind.and_then(Kv::int) == Some(4) || kind.and_then(Kv::str) == Some("ACHIEVEMENTS");
        if !is_bits {
            continue;
        }
        let Some(bits) = stat.get("bits").and_then(Kv::map) else {
            continue;
        };
        let mut bits: Vec<(&String, &Kv)> = bits.iter().collect();
        bits.sort_by_key(|(k, _)| k.parse::<u32>().unwrap_or(u32::MAX));
        for (bit_key, bit) in bits {
            let bit_no = bit
                .get("bit")
                .and_then(Kv::int)
                .or_else(|| bit_key.parse().ok())
                .unwrap_or(0) as u32;
            let name = bit.get("name").and_then(Kv::str).unwrap_or_default();
            let display = bit.get("display");
            let field = |k: &str| display.and_then(|d| d.get(k));
            let hidden = field("hidden").and_then(Kv::int).unwrap_or(0) != 0;
            out.push((
                (stat_id.clone(), bit_no),
                Achievement {
                    id: name.to_string(),
                    key: name.to_string(),
                    visible: !hidden,
                    name: localized(field("name"), language),
                    description: localized(field("desc"), language),
                    image_unlocked: icon_url(
                        app_id,
                        field("icon").and_then(Kv::str).unwrap_or_default(),
                    ),
                    image_locked: icon_url(
                        app_id,
                        field("icon_gray").and_then(Kv::str).unwrap_or_default(),
                    ),
                    ..Default::default()
                },
            ));
        }
    }
    out
}

/// Mark the achievements the player's stats file says are unlocked.
pub(crate) fn apply_player(list: &mut [((String, u32), Achievement)], player: &Kv) {
    // The stats sit under the file's one root key ("cache").
    let root = player
        .map()
        .and_then(|m| m.get("cache").or_else(|| m.values().next()));
    let Some(root) = root else {
        return;
    };
    for ((stat, bit), a) in list.iter_mut() {
        let Some(s) = root.get(stat) else {
            continue;
        };
        let data = s.get("data").and_then(Kv::int).unwrap_or(0) as u64;
        if data & (1u64 << (*bit as u64 % 64)) == 0 {
            continue;
        }
        let time = s
            .get("AchievementTimes")
            .and_then(|t| t.get(&bit.to_string()))
            .and_then(Kv::int)
            .unwrap_or(0);
        // An unlock without a time still counts as unlocked.
        a.unlocked_at = Some(
            (time > 0)
                .then(|| irori_core::util::rfc3339_from_unix(time))
                .flatten()
                .unwrap_or_default(),
        );
    }
}

/// The schema file of an app, if Steam cached one.
pub(crate) fn schema_path(steam_dir: &Path, app_id: &str) -> PathBuf {
    steam_dir
        .join("appcache/stats")
        .join(format!("UserGameStatsSchema_{app_id}.bin"))
}

/// The player's stats file of an app: the given account's, else the most
/// recently written one.
pub(crate) fn player_path(steam_dir: &Path, app_id: &str, account: Option<u32>) -> Option<PathBuf> {
    let dir = steam_dir.join("appcache/stats");
    if let Some(account) = account {
        let p = dir.join(format!("UserGameStats_{account}_{app_id}.bin"));
        if p.exists() {
            return Some(p);
        }
    }
    let suffix = format!("_{app_id}.bin");
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("UserGameStats_") && name.ends_with(&suffix)
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// The achievements of an app from Steam's local files; empty when Steam
/// has no schema for it.
pub(crate) fn local_achievements(
    steam_dir: &Path,
    app_id: &str,
    account: Option<u32>,
    language: &str,
) -> Vec<Achievement> {
    let Some(schema) = std::fs::read(schema_path(steam_dir, app_id))
        .ok()
        .and_then(|d| parse(&d))
    else {
        return Vec::new();
    };
    let mut list = schema_achievements(&schema, app_id, language);
    if let Some(player) = player_path(steam_dir, app_id, account)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|d| parse(&d))
    {
        apply_player(&mut list, &player);
    }
    list.into_iter().map(|(_, a)| a).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes binary KeyValues.
    pub struct Writer(pub Vec<u8>);

    impl Writer {
        pub fn new() -> Self {
            Self(Vec::new())
        }
        fn key(&mut self, kind: u8, key: &str) -> &mut Self {
            self.0.push(kind);
            self.0.extend(key.as_bytes());
            self.0.push(0);
            self
        }
        pub fn open(&mut self, key: &str) -> &mut Self {
            self.key(0, key)
        }
        pub fn close(&mut self) -> &mut Self {
            self.0.push(8);
            self
        }
        pub fn str(&mut self, key: &str, v: &str) -> &mut Self {
            self.key(1, key);
            self.0.extend(v.as_bytes());
            self.0.push(0);
            self
        }
        pub fn int(&mut self, key: &str, v: i32) -> &mut Self {
            self.key(2, key);
            self.0.extend(v.to_le_bytes());
            self
        }
        pub fn done(&mut self) -> Vec<u8> {
            self.0.push(8);
            std::mem::take(&mut self.0)
        }
    }

    pub fn schema_fixture() -> Vec<u8> {
        let mut w = Writer::new();
        w.open("620")
            .str("gamename", "Portal 2")
            .open("stats")
            .open("1")
            .int("type", 4)
            .str("id", "1")
            .open("bits")
            .open("0")
            .str("name", "ACH.SURVIVE_CONTAINER_RIDE")
            .int("bit", 0)
            .open("display")
            .open("name")
            .str("english", "Wake Up Call")
            .str("german", "Weckruf")
            .close()
            .open("desc")
            .str("english", "Survive the manual curiosity core switch")
            .close()
            .str("hidden", "0")
            .str("icon", "abc.jpg")
            .str("icon_gray", "abc_gray.jpg")
            .close()
            .close()
            .open("1")
            .str("name", "ACH.SECRET")
            .int("bit", 1)
            .open("display")
            .open("name")
            .str("english", "Secret")
            .close()
            .str("hidden", "1")
            .close()
            .close()
            .close()
            .close()
            .open("2")
            .int("type", 1)
            .str("name", "kills")
            .close()
            .close()
            .close();
        w.done()
    }

    pub fn player_fixture() -> Vec<u8> {
        let mut w = Writer::new();
        w.open("cache")
            .int("crc", 1)
            .open("1")
            .int("data", 0b10)
            .open("AchievementTimes")
            .int("1", 1_700_000_000)
            .close()
            .close()
            .close();
        w.done()
    }

    #[test]
    fn reads_schema_and_unlocks() {
        let schema = parse(&schema_fixture()).unwrap();
        let mut list = schema_achievements(&schema, "620", "german");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1.name, "Weckruf");
        assert_eq!(
            list[0].1.description,
            "Survive the manual curiosity core switch"
        );
        assert!(list[0].1.image_unlocked.ends_with("/apps/620/abc.jpg"));
        assert!(!list[1].1.visible);
        apply_player(&mut list, &parse(&player_fixture()).unwrap());
        assert!(list[0].1.unlocked_at.is_none());
        assert_eq!(
            list[1].1.unlocked_at.as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
    }

    #[test]
    fn finds_the_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let stats = dir.path().join("appcache/stats");
        std::fs::create_dir_all(&stats).unwrap();
        assert!(local_achievements(dir.path(), "620", None, "english").is_empty());
        std::fs::write(stats.join("UserGameStatsSchema_620.bin"), schema_fixture()).unwrap();
        std::fs::write(stats.join("UserGameStats_42_620.bin"), player_fixture()).unwrap();
        let list = local_achievements(dir.path(), "620", Some(7), "english");
        assert_eq!(list.len(), 2);
        assert!(list[1].unlocked_at.is_some());
        assert!(parse(&[1, b'a']).is_none());
    }
}
