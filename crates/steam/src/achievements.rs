//! A game's achievements: what it offers in a language, with each one's
//! global unlock rate (`IPlayerService/GetGameAchievements`, public), and
//! the account's unlocks and progress (`IPlayerService/GetUserAchievements`,
//! with the account's access token). `ISteamUserStats/GetPlayerAchievements`
//! is not used: it takes only a publisher's Web API key.

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

impl Client {
    /// The achievements `appid` offers, in Steam's `language` (`english`,
    /// `german`…). Public; an access token is sent when there is one.
    /// Empty for a game without any.
    pub async fn game_achievements(
        &self,
        access_token: Option<&str>,
        appid: u32,
        language: &str,
    ) -> Result<Vec<GameAchievement>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            achievements: Vec<GameAchievement>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Envelope {
            response: Answer,
        }
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
        let answer: Envelope = self
            .get_json(url, &query, "the game's achievements")
            .await?;
        Ok(answer.response.achievements)
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
        let defs = c.game_achievements(None, 620, "german").await.unwrap();
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
}
