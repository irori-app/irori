//! Steam achievements of a game. The definitions (names in the UI
//! language, icons, global unlock rates) are public; the unlocks come from
//! the signed-in account, else from Steam's own files on this computer
//! ([`stats_file`]), which also stand in for the definitions offline.
//! A game's Achievement Sets (base game, each DLC) carry over as each
//! achievement's group; Steam's local files don't name them, so offline
//! answers have none.
//! Answers are cached for a quarter of an hour.

use std::collections::HashMap;

use anyhow::Result;
use irori_core::util::rfc3339_from_unix;
use irori_core::{Achievement, AchievementGroup};
use irori_steam_api::achievements::icon_url;
use irori_steam_api::{AchievementSchema, UserAchievement};

use crate::SteamStore;
use crate::stats_file::{self, steam_language};

/// How long an answer is served without asking Steam.
pub(crate) const MAX_AGE_SECS: u64 = 15 * 60;

/// SteamID64 of account id 0; account ids name the local stats files.
const STEAMID64_BASE: u64 = 76_561_197_960_265_728;

/// Merge Steam's definitions with the account's unlocks.
pub(crate) fn merge(
    appid: u32,
    schema: &AchievementSchema,
    unlocks: Option<&[UserAchievement]>,
    local: &HashMap<String, Option<String>>,
) -> Vec<Achievement> {
    let by_key: HashMap<u32, &UserAchievement> = unlocks
        .unwrap_or_default()
        .iter()
        .map(|u| (u.internal_key, u))
        .collect();
    schema
        .achievements
        .iter()
        .map(|d| {
            let mut a = Achievement {
                id: d.internal_name.clone(),
                key: d.internal_name.clone(),
                visible: !d.hidden,
                name: d.localized_name.clone(),
                description: d.localized_desc.clone(),
                image_unlocked: icon_url(appid, &d.icon),
                image_locked: icon_url(appid, &d.icon_gray),
                rarity: d.player_percent_unlocked,
                group: schema.group_of(d).map(|g| AchievementGroup {
                    id: g.groupid.to_string(),
                    name: g.localized_name.clone(),
                    dlc_app_id: (g.dlcappid > 0).then(|| g.dlcappid.to_string()),
                    order: g.order,
                }),
                ..Default::default()
            };
            match (unlocks, by_key.get(&d.internal_key)) {
                (Some(_), Some(u)) => {
                    if u.unlocked {
                        a.unlocked_at = Some(
                            rfc3339_from_unix(u.unlock_time as i64)
                                .filter(|_| u.unlock_time > 0)
                                .unwrap_or_default(),
                        );
                    } else {
                        a.progress = d.progress(u);
                    }
                }
                (Some(_), None) => {}
                (None, _) => {
                    if let Some(at) = local.get(&d.internal_name) {
                        a.unlocked_at = Some(at.clone().unwrap_or_default());
                    }
                }
            }
            a
        })
        .collect()
}

impl SteamStore {
    /// A game's achievements; see the module docs.
    pub(crate) async fn game_achievements(
        &self,
        app_name: &str,
        language: &str,
    ) -> Result<Vec<Achievement>> {
        let Ok(appid) = app_name.parse::<u32>() else {
            return Ok(Vec::new());
        };
        let cached = self.achievement_cache.get(app_name, language).await;
        if let Some(c) = cached.as_ref().filter(|c| c.fresh) {
            return Ok(c.list.clone());
        }
        let steam_language = steam_language(language);
        let session = self.live_session().await;
        let account = session
            .as_ref()
            .and_then(|s| s.steamid().ok())
            .map(|id| id.saturating_sub(STEAMID64_BASE) as u32);
        let steam_dir = self.steam_dir();
        let app = app_name.to_string();
        let local = tokio::task::spawn_blocking(move || {
            stats_file::local_achievements(&steam_dir, &app, account, steam_language)
        })
        .await
        .unwrap_or_default();

        let client = self.client()?;
        let token = session.as_ref().map(|s| s.access_token.as_str());
        let schema = match client.game_achievements(token, appid, steam_language).await {
            Ok(schema) => schema,
            Err(e) => {
                tracing::debug!("Steam achievements of {app_name}: {e:#}");
                if let Some(c) = cached {
                    return Ok(c.list);
                }
                if !local.is_empty() {
                    return Ok(local);
                }
                return Err(e);
            }
        };
        if schema.achievements.is_empty() {
            // Steam's web schema lags behind some games; its files may not.
            return Ok(local);
        }
        let unlocks = match &session {
            Some(s) => match client
                .user_achievements(&s.access_token, s.steamid()?, appid)
                .await
            {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!("Steam unlocks of {app_name}: {e:#}");
                    None
                }
            },
            None => None,
        };
        let local_unlocks: HashMap<String, Option<String>> = local
            .iter()
            .filter(|a| a.unlocked_at.is_some())
            .map(|a| {
                (
                    a.id.clone(),
                    a.unlocked_at.clone().filter(|d| !d.is_empty()),
                )
            })
            .collect();
        let list = merge(appid, &schema, unlocks.as_deref(), &local_unlocks);
        self.achievement_cache.put(app_name, language, &list).await;
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_steam_api::GameAchievement;

    fn def(name: &str, key: u32, percent: f32) -> GameAchievement {
        GameAchievement {
            internal_name: name.into(),
            internal_key: key,
            localized_name: name.to_lowercase(),
            icon: format!("{name}.jpg"),
            player_percent_unlocked: percent,
            ..Default::default()
        }
    }

    #[test]
    fn merges_account_or_local_unlocks() {
        let defs = AchievementSchema {
            achievements: vec![def("A", 1, 50.0), def("B", 2, 5.0)],
            groups: Vec::new(),
        };
        let unlocks = vec![UserAchievement {
            internal_key: 1,
            unlocked: true,
            unlock_time: 1_700_000_000,
            ..Default::default()
        }];
        let list = merge(620, &defs, Some(&unlocks), &HashMap::new());
        assert_eq!(list[0].unlocked_at.as_deref(), Some("2023-11-14T22:13:20Z"));
        assert!(list[1].unlocked_at.is_none());
        assert_eq!(list[1].rarity, 5.0);
        assert!(list[0].image_unlocked.ends_with("/620/A.jpg"));
        assert!(list.iter().all(|a| a.group.is_none()));

        let local = HashMap::from([("B".to_string(), None)]);
        let list = merge(620, &defs, None, &local);
        assert!(list[0].unlocked_at.is_none());
        assert_eq!(list[1].unlocked_at.as_deref(), Some(""));
    }

    #[test]
    fn carries_achievement_sets() {
        let answer = serde_json::json!({"response": {
            "achievements": [
                {"internal_name": "BASE", "internal_key": 1, "groupid": 1},
                {"internal_name": "DLC", "internal_key": 2, "groupid": 2}
            ],
            "groups": [
                {"groupid": 1, "localized_name": "Base game", "order": 0},
                {"groupid": 2, "localized_name": "The Island", "dlcappid": 1500, "order": 1}
            ]
        }});
        let schema = irori_steam_api::achievements::parse_game(&answer);
        let list = merge(620, &schema, None, &HashMap::new());
        assert_eq!(
            list[0].group,
            Some(AchievementGroup {
                id: "1".into(),
                name: "Base game".into(),
                dlc_app_id: None,
                order: 0,
            })
        );
        let dlc = list[1].group.as_ref().unwrap();
        assert_eq!(
            (dlc.name.as_str(), dlc.dlc_app_id.as_deref(), dlc.order),
            ("The Island", Some("1500"), 1)
        );
    }
}
