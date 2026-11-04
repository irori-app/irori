//! Epic achievements from the launcher's GraphQL service: what a game offers
//! (public, by its namespace) with each one's XP and rarity, and what the
//! signed-in player unlocked, with progress towards the rest. Answers are
//! cached for a quarter of an hour.

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use irori_core::Achievement;
use serde_json::Value;

use crate::EpicStore;

/// How long an answer is served without asking Epic.
pub(crate) const MAX_AGE_SECS: u64 = 15 * 60;

/// What the player has of one achievement.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerState {
    /// The unlock date, `Some("")` for an unlock without one.
    pub unlocked: Option<String>,
    /// Progress towards a locked one, `0.0..=1.0`.
    pub progress: Option<f32>,
}
use crate::extra::{escape_gql, graphql, graphql_signed, store_locale};

/// The achievements a game offers, with the product id the player query needs.
fn definitions_query(namespace: &str, locale: &str) -> String {
    format!(
        "{{ Achievement {{ productAchievementsRecordBySandbox(sandboxId: \"{}\", locale: \"{}\") {{ \
         productId achievements {{ achievement {{ name hidden unlockedDisplayName lockedDisplayName \
         unlockedDescription lockedDescription unlockedIconLink lockedIconLink XP rarity {{ percent }} \
         }} }} }} }} }}",
        escape_gql(namespace),
        escape_gql(locale)
    )
}

fn player_query(account_id: &str, product_id: &str) -> String {
    format!(
        "{{ PlayerProfile {{ playerProfile(epicAccountId: \"{}\") {{ productAchievements(productId: \"{}\") {{ \
         __typename ... on PlayerProductAchievementsResponseSuccess {{ data {{ playerAchievements {{ \
         playerAchievement {{ achievementName unlocked unlockDate progress }} }} }} }} }} }} }} }}",
        escape_gql(account_id),
        escape_gql(product_id)
    )
}

/// The product id and achievements of a definitions answer; `None` for a
/// game without achievements.
pub fn parse_definitions(v: &Value) -> Option<(String, Vec<Achievement>)> {
    let record = v.pointer("/data/Achievement/productAchievementsRecordBySandbox")?;
    let product = record.get("productId")?.as_str()?.to_string();
    let s = |a: &Value, k: &str| {
        a.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let list: Vec<Achievement> = record
        .get("achievements")?
        .as_array()?
        .iter()
        .filter_map(|entry| entry.get("achievement"))
        .map(|a| {
            let name = s(a, "unlockedDisplayName");
            let description = s(a, "unlockedDescription");
            Achievement {
                id: s(a, "name"),
                key: s(a, "name"),
                visible: !a.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                name: if name.is_empty() {
                    s(a, "lockedDisplayName")
                } else {
                    name
                },
                description: if description.is_empty() {
                    s(a, "lockedDescription")
                } else {
                    description
                },
                image_unlocked: s(a, "unlockedIconLink"),
                image_locked: s(a, "lockedIconLink"),
                rarity: a
                    .pointer("/rarity/percent")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
                points: a.get("XP").and_then(Value::as_u64).unwrap_or(0) as u32,
                ..Default::default()
            }
        })
        .collect();
    (!list.is_empty()).then_some((product, list))
}

/// The player's state of each achievement, by name, from a player answer.
pub fn parse_player(v: &Value) -> Result<HashMap<String, PlayerState>> {
    let progress = v
        .pointer("/data/PlayerProfile/playerProfile/productAchievements")
        .context("Epic sent no achievement progress")?;
    if progress.get("__typename").and_then(Value::as_str)
        != Some("PlayerProductAchievementsResponseSuccess")
    {
        bail!("Epic did not share your achievement progress");
    }
    // A player who never played the game has no record yet.
    let Some(list) = progress
        .pointer("/data/playerAchievements")
        .and_then(Value::as_array)
    else {
        return Ok(HashMap::new());
    };
    Ok(list
        .iter()
        .filter_map(|e| e.get("playerAchievement"))
        .filter_map(|a| {
            let name = a.get("achievementName")?.as_str()?.to_string();
            let unlocked = (a.get("unlocked").and_then(Value::as_bool) == Some(true)).then(|| {
                a.get("unlockDate")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            });
            // Epic sends a fraction; some records carry a percentage.
            let progress = a
                .get("progress")
                .and_then(Value::as_f64)
                .map(|p| if p > 1.0 { p / 100.0 } else { p })
                .map(|p| p.clamp(0.0, 1.0) as f32)
                .filter(|p| *p > 0.0 && unlocked.is_none());
            Some((name, PlayerState { unlocked, progress }))
        })
        .collect())
}

/// Mark the player's unlocks and progress on the definitions.
pub fn apply_player(list: &mut [Achievement], player: &HashMap<String, PlayerState>) {
    for a in list {
        if let Some(state) = player.get(&a.id) {
            // An unlock without a date still counts as unlocked.
            a.unlocked_at = state.unlocked.clone();
            a.progress = state.progress;
        }
    }
}

impl EpicStore {
    /// A game's achievements with the signed-in player's unlocks. Empty for
    /// games without achievements.
    pub(crate) async fn game_achievements(
        &self,
        app_name: &str,
        language: &str,
    ) -> Result<Vec<Achievement>> {
        let Some(namespace) = self
            .state
            .read()
            .games
            .get(app_name)
            .and_then(|g| g.namespace.clone())
        else {
            return Ok(Vec::new());
        };
        let cached = self.achievement_cache.get(app_name, language).await;
        if let Some(c) = cached.as_ref().filter(|c| c.fresh) {
            return Ok(c.list.clone());
        }
        let fetched = async {
            let defs = graphql(definitions_query(&namespace, &store_locale(language))).await?;
            let Some((product, mut list)) = parse_definitions(&defs) else {
                return Ok(Vec::new());
            };
            let session = self.session().await?;
            let player =
                graphql_signed(player_query(&session.account_id, &product), &session).await?;
            apply_player(&mut list, &parse_player(&player)?);
            anyhow::Ok(list)
        }
        .await;
        match fetched {
            Ok(list) => {
                self.achievement_cache.put(app_name, language, &list).await;
                Ok(list)
            }
            Err(e) => match cached {
                Some(c) => {
                    tracing::debug!("Epic achievements of {app_name}: {e:#}");
                    Ok(c.list)
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_definitions_and_unlocks() {
        let defs = json!({"data": {"Achievement": {"productAchievementsRecordBySandbox": {
        "productId": "0de2", "achievements": [
            {"achievement": {"name": "Act1", "hidden": true, "unlockedDisplayName": "The Wait",
             "lockedDisplayName": "???", "unlockedDescription": "Complete Act I",
             "lockedDescription": "", "unlockedIconLink": "https://u", "lockedIconLink": "https://l",
             "XP": 50, "rarity": {"percent": 7}}},
            {"achievement": {"name": "Act2", "hidden": false, "unlockedDisplayName": "",
             "lockedDisplayName": "The Ascent", "unlockedDescription": "",
             "lockedDescription": "Complete Act II", "rarity": {"percent": 4.5}}}
        ]}}}});
        let (product, list) = parse_definitions(&defs).unwrap();
        assert_eq!(product, "0de2");
        assert_eq!(list[0].name, "The Wait");
        assert!(!list[0].visible);
        assert_eq!(list[0].rarity, 7.0);
        assert_eq!(list[1].name, "The Ascent");
        assert_eq!(list[1].description, "Complete Act II");
        assert_eq!(list[0].points, 50);

        let player = json!({"data": {"PlayerProfile": {"playerProfile": {"productAchievements": {
        "__typename": "PlayerProductAchievementsResponseSuccess",
        "data": {"playerAchievements": [
            {"playerAchievement": {"achievementName": "Act1", "unlocked": true,
              "unlockDate": "2022-05-22T07:39:26.283Z"}},
            {"playerAchievement": {"achievementName": "Act2", "unlocked": false, "progress": 0.25}}
        ]}}}}}});
        let player = parse_player(&player).unwrap();
        assert_eq!(
            player["Act1"].unlocked.as_deref(),
            Some("2022-05-22T07:39:26.283Z")
        );
        let mut list = list;
        apply_player(&mut list, &player);
        assert!(list[0].is_unlocked());
        assert!(!list[1].is_unlocked());
        assert_eq!(list[1].progress, Some(0.25));
    }

    #[test]
    fn games_without_achievements_and_refusals() {
        let none = json!({"data": {"Achievement": {"productAchievementsRecordBySandbox":
            {"productId": null, "achievements": null}}}});
        assert!(parse_definitions(&none).is_none());
        let refused = json!({"data": {"PlayerProfile": {"playerProfile": {"productAchievements":
            {"__typename": "ServiceError"}}}}});
        assert!(parse_player(&refused).is_err());
        let never_played = json!({"data": {"PlayerProfile": {"playerProfile": {"productAchievements":
            {"__typename": "PlayerProductAchievementsResponseSuccess", "data": null}}}}});
        assert!(parse_player(&never_played).unwrap().is_empty());
    }
}
