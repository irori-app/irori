//! How a game is received on Steam: the review summary the store page shows
//! (`/appreviews`, every language and purchase) and how many people play it
//! right now (`ISteamUserStats/GetNumberOfCurrentPlayers`). Neither needs a
//! key or an account.

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::api::Client;
use crate::library::lenient_u64;

/// The store's review summary of an app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewSummary {
    /// Steam's verdict, 1 (overwhelmingly negative) to 9 (overwhelmingly
    /// positive); 0 when there are too few reviews for one.
    pub score: u8,
    pub positive: u64,
    pub total: u64,
}

impl ReviewSummary {
    /// The share of positive reviews, 0–100, when there are any.
    pub fn percent(&self) -> Option<u8> {
        (self.total > 0).then(|| {
            let share = (self.positive.min(self.total) * 100 + self.total / 2) / self.total;
            u8::try_from(share).unwrap_or(100)
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QuerySummary {
    #[serde(deserialize_with = "lenient_u64")]
    review_score: u64,
    #[serde(deserialize_with = "lenient_u64")]
    total_positive: u64,
    #[serde(deserialize_with = "lenient_u64")]
    total_reviews: u64,
}

/// The summary in an `/appreviews` answer; `None` when Steam said it failed
/// or sent no summary.
fn summary_of(answer: &Value) -> Option<ReviewSummary> {
    if answer.get("success").and_then(Value::as_i64) != Some(1) {
        return None;
    }
    let raw: QuerySummary = serde_json::from_value(answer.get("query_summary")?.clone()).ok()?;
    Some(ReviewSummary {
        score: u8::try_from(raw.review_score.min(9)).unwrap_or_default(),
        positive: raw.total_positive,
        total: raw.total_reviews,
    })
}

impl Client {
    /// The review summary of `appid`, over every language and purchase type;
    /// `None` for an app the store has no reviews of.
    pub async fn review_summary(&self, appid: u32) -> Result<Option<ReviewSummary>> {
        let url = format!("{}/appreviews/{appid}", self.endpoints().store);
        let query = [
            ("json", "1".to_string()),
            ("language", "all".to_string()),
            ("purchase_type", "all".to_string()),
            ("num_per_page", "0".to_string()),
        ];
        let answer: Value = self.get_json(url, &query, "the game's reviews").await?;
        Ok(summary_of(&answer))
    }

    /// How many people play `appid` right now; `None` when Steam doesn't
    /// count it (an app it doesn't know answers 404).
    pub async fn current_players(&self, appid: u32) -> Result<Option<u64>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Response {
            #[serde(deserialize_with = "lenient_u64")]
            player_count: u64,
            #[serde(deserialize_with = "lenient_u64")]
            result: u64,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            response: Response,
        }
        let url = format!(
            "{}/ISteamUserStats/GetNumberOfCurrentPlayers/v1/",
            self.endpoints().api
        );
        let answer: Option<Answer> = self
            .get_json_allowing(
                url,
                &[("appid", appid.to_string())],
                "the game's player count",
                &[404],
            )
            .await?;
        Ok(answer
            .filter(|a| a.response.result == 1)
            .map(|a| a.response.player_count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    #[test]
    fn percent_rounds_and_needs_reviews() {
        let r = |positive, total| ReviewSummary {
            score: 8,
            positive,
            total,
        };
        assert_eq!(r(0, 0).percent(), None);
        assert_eq!(r(2, 3).percent(), Some(67));
        assert_eq!(r(9, 9).percent(), Some(100));
        // A count that disagrees with itself never passes 100.
        assert_eq!(r(12, 9).percent(), Some(100));
    }

    #[tokio::test]
    async fn reads_reviews_and_players() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/appreviews/620?") {
                assert!(req.path.contains("language=all"));
                assert!(req.path.contains("num_per_page=0"));
                Resp::json(json!({"success": 1, "query_summary": {
                    "num_reviews": 0, "review_score": 9,
                    "review_score_desc": "Overwhelmingly Positive",
                    "total_positive": "990", "total_negative": 10, "total_reviews": 1000
                }, "reviews": []}))
            } else if req.path.starts_with("/appreviews/7?") {
                Resp::json(json!({"success": 2}))
            } else if req
                .path
                .starts_with("/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=620")
            {
                Resp::json(json!({"response": {"player_count": 4321, "result": 1}}))
            } else if req
                .path
                .starts_with("/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=8")
            {
                Resp::json(json!({"response": {"result": 42}}))
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            assets: server.base(),
        })
        .unwrap();
        let reviews = api.review_summary(620).await.unwrap().unwrap();
        assert_eq!(
            reviews,
            ReviewSummary {
                score: 9,
                positive: 990,
                total: 1000
            }
        );
        assert_eq!(reviews.percent(), Some(99));
        assert_eq!(api.review_summary(7).await.unwrap(), None);
        assert_eq!(api.current_players(620).await.unwrap(), Some(4321));
        assert_eq!(api.current_players(8).await.unwrap(), None);
        assert_eq!(api.current_players(9).await.unwrap(), None);
    }
}
