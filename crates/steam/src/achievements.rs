//! A game's achievements: what it offers in a language, with each one's
//! global unlock rate (`IPlayerService/GetGameAchievements`, public), and
//! the account's unlocks and progress (`IPlayerService/GetUserAchievements`,
//! with the account's access token). `ISteamUserStats/GetPlayerAchievements`
//! is not used: it takes only a publisher's Web API key.
//!
//! Games may split their achievements into Achievement Sets (the base game,
//! each DLC, seasonal content). `GetGameAchievements` lists the sets in
//! `groups` and names each achievement's set by `groupid`; both are kept.

use anyhow::Result;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::api::Client;

/// Where achievement icons live; the API gives only file names.
pub const ICON_BASE: &str =
    "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps";

/// An achievement a game offers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GameAchievement {
    /// The API name.
    pub internal_name: String,
    /// The number the account's unlocks refer to.
    pub internal_key: u32,
    pub localized_name: String,
    pub localized_desc: String,
    /// Icon file name, relative to [`icon_url`]'s folder.
    pub icon: String,
    pub icon_gray: String,
    pub hidden: bool,
    /// Percentage of players who unlocked it (Steam sends it as text).
    #[serde(deserialize_with = "lenient_f32")]
    pub player_percent_unlocked: f32,
    /// Bounds of the progress it tracks, when it tracks any.
    #[serde(deserialize_with = "lenient_f32")]
    pub min_progress_float: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub max_progress_float: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub min_progress_int: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub max_progress_int: f32,
    /// The Achievement Set it belongs to ([`AchievementGroup::groupid`]);
    /// 0 for a game without sets.
    pub groupid: u32,
}

/// An Achievement Set of a game: the base game's, a DLC's, or another
/// group the developer made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AchievementGroup {
    pub groupid: u32,
    /// Its name in the asked language.
    pub localized_name: String,
    /// The DLC whose achievements these are; 0 for the base game's set.
    pub dlcappid: u32,
    /// Where Steam shows it among the game's sets.
    pub order: u32,
    /// A set only its developers see; never shown to players.
    pub developeronly: bool,
    /// Achievements in the set, and how many of them complete it.
    pub total_achievements: u32,
    pub completion_achievements: u32,
}

/// What `GetGameAchievements` tells about a game: its achievements and the
/// sets they are grouped in (empty for a game without sets).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AchievementSchema {
    pub achievements: Vec<GameAchievement>,
    pub groups: Vec<AchievementGroup>,
}

impl AchievementSchema {
    /// The set `achievement` belongs to, when the game has sets and it is
    /// one players may see.
    pub fn group_of(&self, achievement: &GameAchievement) -> Option<&AchievementGroup> {
        self.groups
            .iter()
            .find(|g| g.groupid == achievement.groupid && !g.developeronly)
    }
}

impl GameAchievement {
    /// How far `unlock` is, `0.0..=1.0`, when this tracks progress.
    pub fn progress(&self, unlock: &UserAchievement) -> Option<f32> {
        let (min, max, value) = if self.max_progress_int > 0.0 {
            (
                self.min_progress_int,
                self.max_progress_int,
                unlock.progress_int,
            )
        } else if self.max_progress_float > 0.0 {
            (
                self.min_progress_float,
                self.max_progress_float,
                unlock.progress_float,
            )
        } else {
            return None;
        };
        (max > min).then(|| ((value - min) / (max - min)).clamp(0.0, 1.0))
    }
}

/// The account's state of one achievement.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UserAchievement {
    pub internal_key: u32,
    pub unlocked: bool,
    /// Unix time of the unlock, 0 when unknown.
    pub unlock_time: u64,
    #[serde(deserialize_with = "lenient_f32")]
    pub progress_int: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub progress_float: f32,
}

/// The address of an achievement icon file of `appid`.
pub fn icon_url(appid: u32, icon: &str) -> String {
    if icon.is_empty() || icon.starts_with("http") {
        return icon.to_string();
    }
    format!("{ICON_BASE}/{appid}/{icon}")
}

/// Steam's name for an interface language code (`de`, `pt_BR`,
/// `zh_Hans`…): `english`, `german`, `schinese`…; English for any other.
pub fn steam_language(ui: &str) -> &'static str {
    match ui {
        "cs" => "czech",
        "de" => "german",
        "es" => "spanish",
        "fr" => "french",
        "it" => "italian",
        "nl" => "dutch",
        "pl" => "polish",
        "pt" => "portuguese",
        "pt_BR" => "brazilian",
        "ro" => "romanian",
        "sv" => "swedish",
        "tr" => "turkish",
        "ru" => "russian",
        "uk" => "ukrainian",
        "ja" => "japanese",
        "ko" => "koreana",
        "zh_Hans" => "schinese",
        "zh_Hant" => "tchinese",
        _ => "english",
    }
}

impl Client {
    /// The achievements `appid` offers, in Steam's `language` (`english`,
    /// `german`…). Public; an access token is sent when there is one.
    /// Empty for a game without any.
    pub async fn game_achievements(
        &self,
        access_token: Option<&str>,
        appid: u32,
        language: &str,
    ) -> Result<AchievementSchema> {
        let url = format!(
            "{}/IPlayerService/GetGameAchievements/v1/",
            self.endpoints().api
        );
        let mut query = vec![
            ("appid", appid.to_string()),
            ("language", language.to_string()),
        ];
        if let Some(token) = access_token {
            query.push(("access_token", token.to_string()));
        }
        let answer: Value = self
            .get_json(url, &query, "the game's achievements")
            .await?;
        Ok(parse_game(&answer))
    }

    /// The account's unlocks and progress in `appid`. `None` when Steam
    /// shares none (it answers `{"response": {}}`).
    pub async fn user_achievements(
        &self,
        access_token: &str,
        steamid: u64,
        appid: u32,
    ) -> Result<Option<Vec<UserAchievement>>> {
        let url = format!(
            "{}/IPlayerService/GetUserAchievements/v1/",
            self.endpoints().api
        );
        let query = [
            ("access_token", access_token.to_string()),
            ("steamid", steamid.to_string()),
            ("appid", appid.to_string()),
        ];
        let answer: Option<Value> = self
            .get_json_allowing(url, &query, "your achievements", &[400, 404])
            .await?;
        Ok(answer.as_ref().and_then(parse_user))
    }
}

/// The achievements and sets of a `GetGameAchievements` answer; empty when
/// it has none.
pub fn parse_game(v: &Value) -> AchievementSchema {
    v.get("response")
        .and_then(|r| serde_json::from_value(r.clone()).ok())
        .unwrap_or_default()
}

/// The unlocks of a `GetUserAchievements` answer.
pub fn parse_user(v: &Value) -> Option<Vec<UserAchievement>> {
    let list = v.pointer("/response/achievements")?;
    serde_json::from_value(list.clone()).ok()
}

fn lenient_f32<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use crate::error::SteamError;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    fn client(server: &TestServer) -> Client {
        Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            assets: server.base(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn reads_definitions_and_unlocks() {
        let server = TestServer::start(|req| {
            let p = req.path.as_str();
            if p.starts_with("/IPlayerService/GetGameAchievements/v1/?")
                && p.contains("appid=620")
                && p.contains("language=german")
            {
                Resp::json(json!({"response": {"achievements": [
                    {"internal_name": "ACH.WAKE_UP", "internal_key": 1,
                     "localized_name": "Weckruf", "localized_desc": "Überlebe",
                     "icon": "a.jpg", "icon_gray": "b.jpg", "hidden": false,
                     "player_percent_unlocked": "71.2", "progress_type": 0},
                    {"internal_name": "ACH.COLLECT", "internal_key": 2,
                     "localized_name": "Sammler", "localized_desc": "",
                     "icon": "c.jpg", "icon_gray": "d.jpg", "hidden": true,
                     "player_percent_unlocked": "3.5", "min_progress_int": 0,
                     "max_progress_int": 50}
                ]}}))
            } else if p.starts_with("/IPlayerService/GetUserAchievements/v1/?") {
                if p.contains("access_token=revoked") {
                    return Resp::status(401);
                }
                if p.contains("appid=10") {
                    return Resp::json(json!({"response": {}}));
                }
                Resp::json(json!({"response": {"achievements": [
                    {"internal_key": 1, "unlocked": true, "unlock_time": 1700000000},
                    {"internal_key": 2, "unlocked": false, "progress_int": 20}
                ]}}))
            } else {
                Resp::status(404)
            }
        });
        let c = client(&server);
        let schema = c.game_achievements(None, 620, "german").await.unwrap();
        assert!(schema.groups.is_empty());
        let defs = schema.achievements;
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].localized_name, "Weckruf");
        assert!((defs[0].player_percent_unlocked - 71.2).abs() < 0.01);
        assert!(defs[1].hidden);
        let mine = c.user_achievements("token", 1, 620).await.unwrap().unwrap();
        assert!(mine[0].unlocked && !mine[1].unlocked);
        assert_eq!(mine[0].unlock_time, 1_700_000_000);
        assert_eq!(defs[1].progress(&mine[1]), Some(0.4));
        assert_eq!(defs[0].progress(&mine[0]), None);
        assert!(c.user_achievements("token", 1, 10).await.unwrap().is_none());
        let e = c.user_achievements("revoked", 1, 620).await.unwrap_err();
        assert_eq!(
            e.downcast_ref::<SteamError>(),
            Some(&SteamError::SessionRejected)
        );
        assert_eq!(icon_url(620, "a.jpg"), format!("{ICON_BASE}/620/a.jpg"));
    }

    #[test]
    fn keeps_achievement_sets() {
        // Shaped like Steam's answer for a game with a base set and a DLC set.
        let answer = json!({"response": {
            "achievements": [
                {"internal_name": "BASE_1", "internal_key": 1, "localized_name": "First",
                 "player_percent_unlocked": "60.0", "groupid": 1},
                {"internal_name": "DLC_1", "internal_key": 2, "localized_name": "Island",
                 "player_percent_unlocked": "4.0", "groupid": 2},
                {"internal_name": "DEV_1", "internal_key": 3, "groupid": 3},
                {"internal_name": "LOOSE", "internal_key": 4, "groupid": 9}
            ],
            "schema_version": 3,
            "groups": [
                {"groupid": 2, "localized_name": "The Island", "dlcappid": 1500,
                 "order": 1, "ispublic": true, "total_achievements": 1,
                 "completion_achievements": 1},
                {"groupid": 1, "localized_name": "Base game", "dlcappid": 0,
                 "order": 0, "ispublic": true, "total_achievements": 1,
                 "completion_achievements": 1},
                {"groupid": 3, "localized_name": "Debug", "developeronly": true}
            ]
        }});
        let schema = parse_game(&answer);
        assert_eq!(schema.achievements.len(), 4);
        assert_eq!(schema.groups.len(), 3);
        let set = |i: usize| schema.group_of(&schema.achievements[i]);
        let base = set(0).unwrap();
        assert_eq!(
            (base.localized_name.as_str(), base.dlcappid),
            ("Base game", 0)
        );
        let dlc = set(1).unwrap();
        assert_eq!(
            (dlc.localized_name.as_str(), dlc.dlcappid, dlc.order),
            ("The Island", 1500, 1)
        );
        assert_eq!(dlc.total_achievements, 1);
        assert!(set(2).is_none(), "developer-only sets stay out");
        assert!(set(3).is_none(), "an unknown set is no set");
        assert_eq!(
            parse_game(&json!({"response": {}})),
            AchievementSchema::default()
        );
    }
}
