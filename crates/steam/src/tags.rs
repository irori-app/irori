//! The store's user tags ("Puzzle", "Co-op", "Great Soundtrack"): each
//! app's strongest tags from `IStoreBrowseService/GetItems`, and their names
//! in a language from `IStoreService/GetTagList`. Neither needs a key.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::Client;
use crate::library::lenient_u64;

/// Apps asked for per request.
const BATCH: usize = 100;
/// Tags kept per app, strongest first (the store page shows about as many).
pub const TAGS_PER_APP: usize = 20;

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawItem {
    #[serde(deserialize_with = "lenient_u64")]
    appid: u64,
    tagids: Vec<u32>,
    tags: Vec<RawTag>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTag {
    tagid: u32,
    weight: u64,
}

/// An app's tag ids, strongest first: by the votes `tags` carries, else in
/// `tagids`' order (which the store already ranks).
fn ranked(item: RawItem) -> Vec<u32> {
    if item.tags.is_empty() {
        return item.tagids.into_iter().take(TAGS_PER_APP).collect();
    }
    let mut tags = item.tags;
    tags.sort_by_key(|t| std::cmp::Reverse(t.weight));
    tags.into_iter()
        .map(|t| t.tagid)
        .take(TAGS_PER_APP)
        .collect()
}

impl Client {
    /// The strongest tag ids of `appids`, keyed by app id. Apps the store
    /// doesn't know are left out; one it knows without tags maps to an
    /// empty list.
    pub async fn app_tags(&self, appids: &[u32]) -> Result<HashMap<u32, Vec<u32>>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Items {
            store_items: Vec<Value>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            response: Items,
        }
        let url = format!("{}/IStoreBrowseService/GetItems/v1/", self.endpoints().api);
        let mut all = HashMap::new();
        for chunk in appids.chunks(BATCH) {
            let input = json!({
                "ids": chunk.iter().map(|id| json!({"appid": id})).collect::<Vec<_>>(),
                "context": {"language": "english", "country_code": "US"},
                "data_request": {"include_tag_count": TAGS_PER_APP},
            });
            let answer: Answer = self
                .get_json(
                    url.clone(),
                    &[("input_json", input.to_string())],
                    "the games' store tags",
                )
                .await?;
            all.extend(
                answer
                    .response
                    .store_items
                    .into_iter()
                    .filter_map(|v| serde_json::from_value::<RawItem>(v).ok())
                    .filter_map(|item| {
                        let appid = u32::try_from(item.appid).ok().filter(|&id| id != 0)?;
                        Some((appid, ranked(item)))
                    }),
            );
        }
        Ok(all)
    }

    /// Every tag's name in `language` (a Steam language name, `english`).
    pub async fn tag_names(&self, language: &str) -> Result<HashMap<u32, String>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Tag {
            tagid: u32,
            name: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct List {
            tags: Vec<Tag>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            response: List,
        }
        let url = format!("{}/IStoreService/GetTagList/v1/", self.endpoints().api);
        let answer: Answer = self
            .get_json(
                url,
                &[("language", language.to_owned())],
                "the store's tag names",
            )
            .await?;
        Ok(answer
            .response
            .tags
            .into_iter()
            .filter(|t| !t.name.trim().is_empty())
            .map(|t| (t.tagid, t.name.trim().to_owned()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};

    #[test]
    fn tags_rank_by_votes() {
        let item = RawItem {
            appid: 620,
            tagids: vec![1, 2, 3],
            tags: vec![
                RawTag {
                    tagid: 1,
                    weight: 5,
                },
                RawTag {
                    tagid: 2,
                    weight: 50,
                },
                RawTag {
                    tagid: 3,
                    weight: 9,
                },
            ],
        };
        assert_eq!(ranked(item), [2, 3, 1]);
        let plain = RawItem {
            appid: 620,
            tagids: (0..30).collect(),
            tags: vec![],
        };
        assert_eq!(ranked(plain).len(), TAGS_PER_APP);
    }

    #[tokio::test]
    async fn reads_app_tags_and_names() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/IStoreBrowseService/GetItems/v1/?") {
                Resp::json(json!({"response": {"store_items": [
                    {"appid": 620, "tagids": [4182, 1625], "tags": [
                        {"tagid": 4182, "weight": 604}, {"tagid": 1625, "weight": 588}]},
                    {"appid": 7, "name": "No tags"},
                    {"appid": ["broken"]}
                ]}}))
            } else if req.path.starts_with("/IStoreService/GetTagList/v1/?") {
                assert!(req.path.contains("language=german"));
                Resp::json(json!({"response": {"version_hash": "1", "tags": [
                    {"tagid": 4182, "name": "Einzelspieler"},
                    {"tagid": 1625, "name": " Plattformer "},
                    {"tagid": 9, "name": ""}
                ]}}))
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
        let tags = api.app_tags(&[620, 7]).await.unwrap();
        assert_eq!(tags[&620], [4182, 1625]);
        assert_eq!(tags[&7], Vec::<u32>::new());
        let names = api.tag_names("german").await.unwrap();
        assert_eq!(names[&1625], "Plattformer");
        assert!(!names.contains_key(&9));
    }
}
