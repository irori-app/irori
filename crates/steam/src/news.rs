//! Official game news and patch notes from Steam's public
//! `ISteamNews/GetNewsForApp/v2` endpoint. Needs no key or Steam sign-in.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api::Client;
use crate::library::lenient_u64;

/// Maximum announcements requested for one app in a single call.
pub const MAX_NEWS_COUNT: u32 = 50;

/// Coarse category of a Steam announcement item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewsKind {
    PatchNotes,
    #[default]
    News,
}

/// One news item or patch note returned by Steam for an app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamNewsItem {
    pub gid: String,
    pub title: String,
    pub url: String,
    pub is_external_url: bool,
    pub author: String,
    pub contents: String,
    pub feedlabel: String,
    pub date: u64,
    pub feedname: String,
    pub appid: u32,
    pub tags: Vec<String>,
    pub kind: NewsKind,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawItem {
    gid: String,
    title: String,
    url: String,
    is_external_url: bool,
    author: String,
    contents: String,
    feedlabel: String,
    #[serde(deserialize_with = "lenient_u64")]
    date: u64,
    feedname: String,
    #[serde(deserialize_with = "lenient_u64")]
    appid: u64,
    tags: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAppNews {
    newsitems: Vec<RawItem>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEnvelope {
    appnews: RawAppNews,
}

/// Classify a Steam announcement into patch notes vs general news.
pub fn classify_kind(title: &str, tags: &[String]) -> NewsKind {
    if tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("patchnotes") || t.eq_ignore_ascii_case("patch_notes"))
    {
        return NewsKind::PatchNotes;
    }
    let lower = title.to_ascii_lowercase();
    let patch_word = [
        "patch",
        "hotfix",
        "changelog",
        "bugfix",
        "bug fix",
        "release notes",
        "update notes",
        "calibration",
        "maintenance update",
        "balance update",
    ]
    .iter()
    .any(|kw| lower.contains(kw));
    if patch_word || has_version_token(&lower) {
        NewsKind::PatchNotes
    } else {
        NewsKind::News
    }
}

/// True when `lower` contains a version token such as `v1.4`, `1.6.16`, or `update 2.1`.
fn has_version_token(lower: &str) -> bool {
    lower.split_whitespace().any(|raw| {
        let tok = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
        let rest = tok.strip_prefix('v').unwrap_or(tok);
        let mut parts = rest.split('.');
        let Some(major) = parts.next() else {
            return false;
        };
        let Some(minor) = parts.next() else {
            return false;
        };
        if major.is_empty()
            || minor.is_empty()
            || !major.bytes().all(|b| b.is_ascii_digit())
            || !minor.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
        // Require either a leading `v` (`v1.2`) or a third component (`1.6.16`)
        // so dates or decimals in prose ("2.5 million") stay News.
        tok.starts_with('v')
            || parts
                .next()
                .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

impl Client {
    /// Fetch official developer announcements and patch notes for `appid`.
    ///
    /// Uses `feeds=steam_community_announcements` so the feed stays focused on
    /// first-party developer posts rather than third-party press syndication.
    /// Unknown or unlisted apps (HTTP 403/404) return an empty list; HTTP 429
    /// surfaces [`crate::SteamError::RateLimited`].
    pub async fn app_news(
        &self,
        appid: u32,
        count: u32,
        max_length: u32,
    ) -> Result<Vec<SteamNewsItem>> {
        let count = count.clamp(1, MAX_NEWS_COUNT);
        let url = format!("{}/ISteamNews/GetNewsForApp/v2/", self.endpoints().api);
        let query = [
            ("appid", appid.to_string()),
            ("count", count.to_string()),
            ("maxlength", max_length.to_string()),
            ("feeds", "steam_community_announcements".to_string()),
        ];
        let answer: Option<RawEnvelope> = self
            .get_json_allowing(url, &query, "the game's news", &[403, 404])
            .await?;
        let Some(env) = answer else {
            return Ok(Vec::new());
        };
        let mut items: Vec<SteamNewsItem> = env
            .appnews
            .newsitems
            .into_iter()
            .filter(|raw| !raw.gid.is_empty() && !raw.title.trim().is_empty())
            .map(|raw| {
                let kind = classify_kind(&raw.title, &raw.tags);
                let item_app = u32::try_from(raw.appid).unwrap_or(appid);
                SteamNewsItem {
                    gid: raw.gid,
                    title: raw.title,
                    url: raw.url,
                    is_external_url: raw.is_external_url,
                    author: raw.author,
                    contents: raw.contents,
                    feedlabel: raw.feedlabel,
                    date: raw.date,
                    feedname: raw.feedname,
                    appid: if item_app == 0 { appid } else { item_app },
                    tags: raw.tags,
                    kind,
                }
            })
            .collect();
        items.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.gid.cmp(&a.gid)));
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SteamError;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    #[test]
    fn classifies_patch_notes_and_general_news() {
        assert_eq!(
            classify_kind("Something new", &["patchnotes".into()]),
            NewsKind::PatchNotes
        );
        assert_eq!(
            classify_kind("Hotfix #31: Cross-Play fixes", &[]),
            NewsKind::PatchNotes
        );
        assert_eq!(
            classify_kind("Stardew Valley 1.6.16", &[]),
            NewsKind::PatchNotes
        );
        assert_eq!(
            classify_kind("Release v2.0 is live", &[]),
            NewsKind::PatchNotes
        );
        assert_eq!(
            classify_kind("Celebrating 2.5 million players!", &[]),
            NewsKind::News
        );
        assert_eq!(
            classify_kind("New Hades II Merchandise!", &["mod_reviewed".into()]),
            NewsKind::News
        );
    }

    #[tokio::test]
    async fn fetches_news_handles_absent_and_rate_limit() {
        let server = TestServer::start(|req| {
            if req.path.contains("appid=1086940") {
                assert!(req.path.contains("feeds=steam_community_announcements"));
                Resp::json(json!({
                    "appnews": {
                        "appid": 1086940,
                        "newsitems": [
                            {
                                "gid": "100",
                                "title": "Community Spotlight",
                                "url": "https://store.steampowered.com/news/app/1086940",
                                "is_external_url": false,
                                "author": "Larian",
                                "contents": "[h2]Hello[/h2] world",
                                "feedlabel": "Community Announcements",
                                "date": 1700000100,
                                "feedname": "steam_community_announcements",
                                "appid": 1086940,
                                "tags": ["mod_reviewed"]
                            },
                            {
                                "gid": "200",
                                "title": "Hotfix #36 Now Live!",
                                "url": "https://store.steampowered.com/news/app/1086940",
                                "is_external_url": false,
                                "author": "Larian",
                                "contents": "Fixed Act 3 crash",
                                "feedlabel": "Community Announcements",
                                "date": 1700000900,
                                "feedname": "steam_community_announcements",
                                "appid": 1086940,
                                "tags": ["patchnotes"]
                            }
                        ]
                    }
                }))
            } else if req.path.contains("appid=429") {
                Resp::status(429)
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            login: server.base(),
            assets: server.base(),
        })
        .unwrap();

        let items = api.app_news(1086940, 10, 0).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].gid, "200");
        assert_eq!(items[0].kind, NewsKind::PatchNotes);
        assert_eq!(items[1].gid, "100");
        assert_eq!(items[1].kind, NewsKind::News);

        let missing = api.app_news(999999, 5, 0).await.unwrap();
        assert!(missing.is_empty());

        let err = api.app_news(429, 5, 0).await.unwrap_err();
        assert_eq!(
            err.downcast_ref::<SteamError>(),
            Some(&SteamError::RateLimited)
        );
    }
}
