//! RetroAchievements' answers. The Web API is PHP: numbers sometimes come
//! as strings, an empty map as `[]`, and dates as `YYYY-MM-DD HH:MM:SS` in
//! UTC; these types accept all of that.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Where badge images live.
pub const MEDIA_BASE: &str = "https://media.retroachievements.org";

/// The image of an unlocked achievement.
pub fn badge_url(badge: &str) -> String {
    format!("{MEDIA_BASE}/Badge/{badge}.png")
}

/// The greyed image of a locked achievement.
pub fn locked_badge_url(badge: &str) -> String {
    format!("{MEDIA_BASE}/Badge/{badge}_lock.png")
}

/// The account a Web API key belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserProfile {
    pub user: String,
    /// Path of the avatar on the site (`/UserPic/name.png`).
    pub user_pic: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_points: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_softcore_points: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub total_true_points: u64,
}

/// A game in a console's list, with the ROM hashes it is known by.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct GameEntry {
    #[serde(rename = "ID", deserialize_with = "lenient_u64")]
    pub id: u64,
    pub title: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_achievements: u64,
    pub hashes: Vec<String>,
}

/// A game's achievements and the player's progress in it
/// (`API_GetGameInfoAndUserProgress.php`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct GameProgress {
    #[serde(rename = "ID", deserialize_with = "lenient_u64")]
    pub id: u64,
    pub title: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_distinct_players: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_distinct_players_hardcore: u64,
    #[serde(deserialize_with = "achievement_map")]
    pub achievements: Vec<RaAchievement>,
}

/// One achievement of a set.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RaAchievement {
    #[serde(rename = "ID", deserialize_with = "lenient_u64")]
    pub id: u64,
    pub title: String,
    pub description: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub points: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub true_ratio: u64,
    pub badge_name: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub display_order: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_awarded: u64,
    #[serde(deserialize_with = "lenient_u64")]
    pub num_awarded_hardcore: u64,
    /// `progression`, `win_condition`, `missable` or none.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// `YYYY-MM-DD HH:MM:SS` (UTC), when unlocked.
    pub date_earned: Option<String>,
    pub date_earned_hardcore: Option<String>,
}

/// What the emulators' login answers. The token signs emulators in, so
/// `Debug` leaves it out.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ConnectLogin {
    pub success: bool,
    pub user: String,
    pub token: String,
    pub error: Option<String>,
}

impl std::fmt::Debug for ConnectLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectLogin")
            .field("success", &self.success)
            .field("user", &self.user)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// An RFC 3339 timestamp from RetroAchievements' UTC date.
pub fn rfc3339(date: &str) -> Option<String> {
    let date = date.trim();
    if date.is_empty() {
        return None;
    }
    if date.contains('T') {
        return Some(date.to_string());
    }
    let (day, time) = date.split_once(' ')?;
    Some(format!("{day}T{time}Z"))
}

pub(crate) fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

/// The achievements map keyed by id (or `[]` for none), in display order.
fn achievement_map<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RaAchievement>, D::Error> {
    let mut list: Vec<RaAchievement> = match Value::deserialize(d)? {
        Value::Object(map) => map
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_value(v).ok())
            .collect(),
        Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    };
    list.sort_by_key(|a| (a.display_order, a.id));
    Ok(list)
}

/// Hashes to game ids, lower case.
pub fn hash_index(games: &[GameEntry]) -> BTreeMap<String, u64> {
    games
        .iter()
        .flat_map(|g| g.hashes.iter().map(|h| (h.to_ascii_lowercase(), g.id)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_progress_with_loose_numbers() {
        let v = json!({
            "ID": 1, "Title": "Sonic the Hedgehog", "NumDistinctPlayers": "2000",
            "NumDistinctPlayersHardcore": 800,
            "Achievements": {
                "9": {"ID": 9, "Title": "Ring Collector", "Description": "Collect 100 rings",
                      "Points": "5", "TrueRatio": 7, "BadgeName": "250336", "DisplayOrder": 2,
                      "NumAwarded": 1500, "NumAwardedHardcore": 600, "type": null,
                      "DateEarned": "2023-01-02 03:04:05"},
                "4": {"ID": 4, "Title": "Green Hill", "Description": "Clear Act 1",
                      "Points": 3, "BadgeName": "250335", "DisplayOrder": 1,
                      "NumAwarded": "1900", "NumAwardedHardcore": "700", "type": "progression",
                      "DateEarned": "2023-01-01 00:00:00", "DateEarnedHardcore": "2023-01-01 00:00:00"}
            }
        });
        let p: GameProgress = serde_json::from_value(v).unwrap();
        assert_eq!(p.num_distinct_players, 2000);
        assert_eq!(p.achievements.len(), 2);
        assert_eq!(p.achievements[0].id, 4);
        assert_eq!(p.achievements[0].num_awarded, 1900);
        assert_eq!(p.achievements[0].kind.as_deref(), Some("progression"));
        assert_eq!(p.achievements[1].points, 5);
        assert!(p.achievements[1].date_earned_hardcore.is_none());

        let empty: GameProgress =
            serde_json::from_value(json!({"ID": 2, "Achievements": []})).unwrap();
        assert!(empty.achievements.is_empty());
    }

    #[test]
    fn converts_dates_and_indexes_hashes() {
        assert_eq!(
            rfc3339("2023-01-02 03:04:05").as_deref(),
            Some("2023-01-02T03:04:05Z")
        );
        assert_eq!(rfc3339(""), None);
        let games: Vec<GameEntry> = serde_json::from_value(json!([
            {"ID": 1, "Title": "A", "NumAchievements": 10, "Hashes": ["ABCDEF", "123456"]},
            {"ID": "2", "Title": "B", "Hashes": []}
        ]))
        .unwrap();
        let index = hash_index(&games);
        assert_eq!(index.get("abcdef"), Some(&1));
        assert_eq!(index.len(), 2);
        assert_eq!(
            badge_url("250336"),
            "https://media.retroachievements.org/Badge/250336.png"
        );
        assert!(locked_badge_url("1").ends_with("/Badge/1_lock.png"));
    }
}
