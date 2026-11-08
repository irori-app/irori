//! What the Xbox services answer, as far as Irori reads it.
//!
//! Shapes follow Microsoft's Xbox Live REST reference (Achievement JSON,
//! version 2) and, for the title history and user statistics, which that
//! reference no longer lists, the answers recorded by the open-source
//! xbox-webapi clients. Every field is optional on reading: the services
//! leave out what a title doesn't have, and sometimes send `null`.

use serde::{Deserialize, Deserializer, Serialize};

/// `null` reads as the type's default.
fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One title of the account's title history (`titlehub`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Title {
    #[serde(deserialize_with = "nullable")]
    pub title_id: String,
    /// The package family name of a Microsoft Store (packaged) title.
    pub pfn: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    /// "Game", "App"…
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub kind: String,
    /// Where it was played: "PC", "Win32", "XboxOne", "XboxSeries"…
    #[serde(deserialize_with = "nullable")]
    pub devices: Vec<String>,
    pub display_image: Option<String>,
    /// The Microsoft Store product id, when the decoration names it.
    pub product_id: Option<String>,
    pub achievement: Option<TitleAchievements>,
    #[serde(deserialize_with = "nullable")]
    pub images: Vec<Image>,
    pub title_history: Option<TitleHistory>,
    pub detail: Option<Detail>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TitleAchievements {
    pub current_achievements: u32,
    pub total_achievements: u32,
    pub current_gamerscore: u32,
    pub total_gamerscore: u32,
    pub progress_percentage: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Image {
    #[serde(deserialize_with = "nullable")]
    pub url: String,
    /// "Poster", "BoxArt", "SuperHeroArt", "TitledHeroArt", "Logo",
    /// "Screenshot"…
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TitleHistory {
    /// RFC 3339.
    pub last_time_played: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Detail {
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub developer_name: Option<String>,
    pub publisher_name: Option<String>,
    pub release_date: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub genres: Vec<String>,
}

impl Title {
    /// A game played on a Windows PC from the Microsoft Store or the Xbox
    /// app: one with a package family name. Titles played through another
    /// launcher ("Win32") have none and aren't the Xbox app's.
    pub fn is_pc_game(&self) -> bool {
        self.kind.eq_ignore_ascii_case("game")
            && self.devices.iter().any(|d| d.eq_ignore_ascii_case("pc"))
            && self.pfn.as_deref().is_some_and(|p| !p.is_empty())
    }

    /// The first image of the first kind in `kinds` the title has, over
    /// HTTPS.
    pub fn image(&self, kinds: &[&str]) -> Option<String> {
        kinds.iter().find_map(|kind| {
            self.images
                .iter()
                .find(|i| i.kind.eq_ignore_ascii_case(kind) && !i.url.is_empty())
                .map(|i| https(&i.url))
        })
    }

    pub fn last_played(&self) -> Option<&str> {
        self.title_history
            .as_ref()
            .and_then(|h| h.last_time_played.as_deref())
            .filter(|t| !t.is_empty() && !t.starts_with("0001"))
    }
}

/// Microsoft's image servers answer over HTTPS too; the title history
/// still names some of them with `http://`.
pub fn https(url: &str) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

/// The title history's answer.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TitleHub {
    #[serde(deserialize_with = "nullable")]
    pub titles: Vec<Title>,
}

/// An achievement (version 2).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Achievement {
    #[serde(deserialize_with = "nullable")]
    pub id: String,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    /// "Achieved", "InProgress", "NotStarted".
    #[serde(deserialize_with = "nullable")]
    pub progress_state: String,
    pub progression: Option<Progression>,
    #[serde(deserialize_with = "nullable")]
    pub media_assets: Vec<MediaAsset>,
    pub is_secret: bool,
    #[serde(deserialize_with = "nullable")]
    pub description: String,
    #[serde(deserialize_with = "nullable")]
    pub locked_description: String,
    #[serde(deserialize_with = "nullable")]
    pub rewards: Vec<Reward>,
    /// Not in Microsoft's reference, but sent by the service.
    pub rarity: Option<Rarity>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Progression {
    #[serde(deserialize_with = "nullable")]
    pub requirements: Vec<Requirement>,
    pub time_unlocked: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Requirement {
    pub current: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaAsset {
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub kind: String,
    #[serde(deserialize_with = "nullable")]
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reward {
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub kind: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rarity {
    pub current_category: Option<String>,
    pub current_percentage: Option<f32>,
}

impl Achievement {
    pub fn is_unlocked(&self) -> bool {
        self.progress_state.eq_ignore_ascii_case("achieved")
    }

    /// When it was unlocked (RFC 3339); `None` while locked, and for the
    /// zero date the service sends for some old unlocks.
    pub fn unlock_time(&self) -> Option<&str> {
        self.progression
            .as_ref()
            .and_then(|p| p.time_unlocked.as_deref())
            .filter(|t| self.is_unlocked() && !t.is_empty() && !t.starts_with("0001"))
    }

    /// The gamerscore it gives.
    pub fn gamerscore(&self) -> u32 {
        self.rewards
            .iter()
            .filter(|r| r.kind.eq_ignore_ascii_case("gamerscore"))
            .filter_map(|r| r.value.as_deref()?.trim().parse::<u32>().ok())
            .sum()
    }

    pub fn icon(&self) -> Option<String> {
        self.media_assets
            .iter()
            .find(|m| m.kind.eq_ignore_ascii_case("icon") && !m.url.is_empty())
            .map(|m| https(&m.url))
    }

    /// How far towards it the player is, for one that is in progress and
    /// counts something.
    pub fn progress(&self) -> Option<f32> {
        if self.is_unlocked() {
            return None;
        }
        let r = self.progression.as_ref()?.requirements.first()?;
        let current: f32 = r.current.as_deref()?.trim().parse().ok()?;
        let target: f32 = r.target.as_deref()?.trim().parse().ok()?;
        (target > 0.0).then(|| (current / target).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AchievementPage {
    #[serde(deserialize_with = "nullable")]
    pub achievements: Vec<Achievement>,
    pub paging_info: Option<PagingInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PagingInfo {
    pub continuation_token: Option<String>,
}

/// The `MinutesPlayed` statistic in a user statistics batch answer: in
/// the answer's own lists, or in its groups'.
pub fn minutes_played(answer: &serde_json::Value) -> Option<u64> {
    fn lists(v: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
        v.get("statlistscollection")
            .and_then(|l| l.as_array())
            .into_iter()
            .flatten()
    }
    let groups = answer
        .get("groups")
        .and_then(|g| g.as_array())
        .into_iter()
        .flatten();
    lists(answer)
        .chain(groups.flat_map(lists))
        .filter_map(|list| list.get("stats")?.as_array())
        .flatten()
        .filter(|s| {
            s.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("MinutesPlayed"))
        })
        .find_map(|s| match s.get("value")? {
            serde_json::Value::String(v) => v.trim().parse::<f64>().ok(),
            v => v.as_f64(),
        })
        .filter(|m| m.is_finite() && *m >= 0.0)
        .map(|m| m as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a title history answer; values are made up.
    const TITLES: &str = r#"{
      "xuid": "2500000000000001",
      "titles": [
        {
          "titleId": "1700000001",
          "pfn": "Studio.SkyRacer_abcdefghjkmnp",
          "name": "Sky Racer",
          "type": "Game",
          "devices": ["PC", "XboxSeries"],
          "displayImage": "http://store-images.s-microsoft.com/image/apps.1.display",
          "achievement": {"currentAchievements": 3, "totalAchievements": 20,
            "currentGamerscore": 60, "totalGamerscore": 1000,
            "progressPercentage": 6.0, "sourceVersion": 2},
          "images": [
            {"url": "http://store-images.s-microsoft.com/image/apps.1.box", "type": "BoxArt"},
            {"url": "https://store-images.s-microsoft.com/image/apps.1.poster", "type": "Poster"},
            {"url": "https://store-images.s-microsoft.com/image/apps.1.hero", "type": "SuperHeroArt"}
          ],
          "titleHistory": {"lastTimePlayed": "2026-10-08T01:55:12.4411936Z", "visible": true, "canHide": false},
          "detail": {"developerName": "Studio", "publisherName": "Studio Publishing",
            "description": "Race the sky.", "genres": null, "releaseDate": "2025-05-01T00:00:00Z"}
        },
        {"titleId": "1700000002", "pfn": null, "name": "Battle Thing", "type": "Game",
         "devices": ["Win32"], "images": null, "titleHistory": null},
        {"titleId": "1700000003", "pfn": "Studio.Console_abcdefghjkmnp", "name": "Console Only",
         "type": "Game", "devices": ["XboxOne"]},
        {"titleId": "1700000004", "pfn": "Microsoft.App_8wekyb3d8bbwe", "name": "An App",
         "type": "App", "devices": ["PC"]}
      ]
    }"#;

    #[test]
    fn reads_the_title_history() {
        let hub: TitleHub = serde_json::from_str(TITLES).unwrap();
        assert_eq!(hub.titles.len(), 4);
        let sky = &hub.titles[0];
        assert!(sky.is_pc_game());
        assert_eq!(sky.title_id, "1700000001");
        assert_eq!(sky.last_played(), Some("2026-10-08T01:55:12.4411936Z"));
        assert_eq!(
            sky.image(&["Poster", "BoxArt"]).as_deref(),
            Some("https://store-images.s-microsoft.com/image/apps.1.poster")
        );
        assert_eq!(
            sky.image(&["BoxArt"]).as_deref(),
            Some("https://store-images.s-microsoft.com/image/apps.1.box")
        );
        assert_eq!(sky.achievement.as_ref().unwrap().current_gamerscore, 60);
        let detail = sky.detail.as_ref().unwrap();
        assert_eq!(detail.publisher_name.as_deref(), Some("Studio Publishing"));
        assert!(detail.genres.is_empty());
        // Played through another launcher, on a console, or not a game.
        assert!(hub.titles[1..].iter().all(|t| !t.is_pc_game()));
        assert_eq!(hub.titles[1].last_played(), None);
    }

    /// Shaped like Microsoft's sample answer (Achievement JSON, v2).
    const ACHIEVEMENTS: &str = r#"{
      "achievements": [
        {"id": "1", "serviceConfigId": "00000000-0000-0000-0000-000000000001", "name": "First Flight",
         "progressState": "Achieved",
         "progression": {"achievementState": "Achieved", "requirements": null,
           "timeUnlocked": "2026-01-17T03:19:00.3087016Z"},
         "mediaAssets": [{"name": "icon", "type": "Icon", "url": "http://images-eds.xboxlive.com/1"}],
         "isSecret": false, "description": "Fly once.", "lockedDescription": "Fly.",
         "rewards": [{"name": null, "description": null, "value": "10", "type": "Gamerscore", "valueType": "Int"},
                     {"name": "Hat", "value": "1", "type": "InApp", "valueType": "String"}],
         "rarity": {"currentCategory": "Common", "currentPercentage": 71.5}},
        {"id": "2", "name": "Long Haul", "progressState": "InProgress",
         "progression": {"requirements": [{"id": "x", "current": "25", "target": "100"}],
           "timeUnlocked": "0001-01-01T00:00:00Z"},
         "mediaAssets": [], "isSecret": true, "description": "Fly 100 km.",
         "lockedDescription": "Keep flying.", "rewards": [{"value": "50", "type": "Gamerscore"}]}
      ],
      "pagingInfo": {"continuationToken": null, "totalRecords": 2}
    }"#;

    #[test]
    fn reads_achievements() {
        let page: AchievementPage = serde_json::from_str(ACHIEVEMENTS).unwrap();
        let [first, long] = page.achievements.as_slice() else {
            panic!("two achievements")
        };
        assert!(first.is_unlocked());
        assert_eq!(first.unlock_time(), Some("2026-01-17T03:19:00.3087016Z"));
        assert_eq!(first.gamerscore(), 10);
        assert_eq!(
            first.icon().as_deref(),
            Some("https://images-eds.xboxlive.com/1")
        );
        assert_eq!(
            first.rarity.as_ref().unwrap().current_percentage,
            Some(71.5)
        );
        assert_eq!(first.progress(), None);
        assert!(!long.is_unlocked() && long.is_secret);
        assert_eq!(long.unlock_time(), None);
        assert_eq!(long.progress(), Some(0.25));
        assert_eq!(long.gamerscore(), 50);
        assert!(page.paging_info.unwrap().continuation_token.is_none());
    }

    #[test]
    fn finds_minutes_played_in_a_batch_answer() {
        let top = serde_json::json!({
            "groups": [],
            "statlistscollection": [{"arrangebyfield": "xuid", "arrangebyfieldid": "1",
                "stats": [{"xuid": "1", "scid": "s", "name": "MinutesPlayed", "type": "Integer", "value": "754"}]}]
        });
        assert_eq!(minutes_played(&top), Some(754));
        let grouped = serde_json::json!({
            "groups": [{"name": "Hero", "titleid": "1", "statlistscollection": [{"stats": [
                {"name": "DistanceSailed", "value": "92173"},
                {"name": "minutesplayed", "value": 12.0}]}]}],
            "statlistscollection": [{"stats": []}]
        });
        assert_eq!(minutes_played(&grouped), Some(12));
        assert_eq!(
            minutes_played(&serde_json::json!({"statlistscollection": []})),
            None
        );
    }
}
