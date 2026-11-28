//! Backend facade for Gaming news RSS/Atom feeds, Reader Mode, and keyless
//! Steam weekly releases. Respects offline mode and never polls in the background.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use irori_core::{Event, Paths, json_store};
use irori_extras::news_feeds::{
    self, ArticleContent, FeedArticle, FeedFetchResult, NewsFeedsConfig,
};
use irori_steam_api::{ReleaseFeedKind, SteamError, StoreReleaseItem};
use serde::{Deserialize, Serialize};

use crate::Backend;

/// Steam weekly release cache stays fresh for 6 hours.
const RELEASES_TTL_SECS: u64 = 6 * 60 * 60;
/// Number of releases requested per section (New this week / Coming soon).
const RELEASES_PER_SECTION: u32 = 24;

/// Combined New this week + Coming soon snapshot for `Stores -> News -> Releases`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleasesSnapshot {
    pub new_this_week: Vec<StoreReleaseItem>,
    pub coming_soon: Vec<StoreReleaseItem>,
    pub offline: bool,
    pub rate_limited: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CachedReleases {
    fetched_at: u64,
    new_this_week: Vec<StoreReleaseItem>,
    coming_soon: Vec<StoreReleaseItem>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn releases_country(setting: &str) -> String {
    let trimmed = setting.trim();
    if trimmed.len() == 2 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        trimmed.to_ascii_uppercase()
    } else {
        "US".to_string()
    }
}

fn releases_cache_path(country: &str) -> PathBuf {
    Paths::get()
        .cache
        .join("news")
        .join(format!("releases-{}.json", country.to_ascii_lowercase()))
}

fn read_cached_releases(country: &str) -> Option<CachedReleases> {
    json_store::read(&releases_cache_path(country))
        .ok()
        .flatten()
}

fn write_cached_releases(country: &str, payload: &CachedReleases) {
    if let Err(e) = json_store::write(&releases_cache_path(country), payload) {
        tracing::warn!("saving Steam releases cache: {e:#}");
    }
}

fn is_releases_fresh(cached: &CachedReleases, now: u64) -> bool {
    now.saturating_sub(cached.fetched_at) < Duration::from_secs(RELEASES_TTL_SECS).as_secs()
}

impl Backend {
    /// Read the saved RSS/Atom feed preferences (`<config>/news-feeds.json`).
    pub fn news_feeds_config(&self) -> NewsFeedsConfig {
        news_feeds::load_config()
    }

    /// Save updated RSS/Atom feed preferences and notify the UI.
    pub fn save_news_feeds_config(&self, cfg: NewsFeedsConfig) -> Result<()> {
        news_feeds::save_config(&cfg)?;
        self.bus.emit(Event::NewsChanged);
        Ok(())
    }

    /// Read cached gaming news articles from disk without any network call.
    pub async fn cached_gaming_news(&self) -> Vec<FeedArticle> {
        let cfg = news_feeds::load_config();
        news_feeds::fetch_cached_feeds(&cfg).await
    }

    /// Refresh enabled gaming news RSS/Atom feeds. Returns cached articles when
    /// offline or in isolated test mode.
    pub async fn refresh_gaming_news(&self, force: bool) -> FeedFetchResult {
        let cfg = news_feeds::load_config();
        if self.working_offline() || self.isolated {
            return FeedFetchResult {
                articles: news_feeds::fetch_cached_feeds(&cfg).await,
                failed_sources: Vec::new(),
            };
        }
        let result = news_feeds::fetch_feeds(&cfg, force).await;
        self.bus.emit(Event::NewsChanged);
        result
    }

    /// Load readable HTML for one article on demand, falling back to its RSS
    /// summary immediately when offline or if the site refuses extraction.
    pub async fn read_gaming_article(&self, article: &FeedArticle) -> ArticleContent {
        if self.working_offline() || self.isolated {
            return ArticleContent {
                body_html: article.body_html.clone(),
                image_url: article.image_url.clone(),
                used_reader_mode: false,
            };
        }
        news_feeds::fetch_reader_article(
            &article.url,
            &article.body_html,
            article.image_url.as_deref(),
        )
        .await
    }

    /// Read cached Steam weekly releases from disk without any network call.
    pub fn cached_store_releases(&self) -> ReleasesSnapshot {
        let country = releases_country(&self.settings.app().deals_region);
        let cached = read_cached_releases(&country).unwrap_or_default();
        ReleasesSnapshot {
            new_this_week: cached.new_this_week,
            coming_soon: cached.coming_soon,
            offline: self.working_offline(),
            rate_limited: false,
        }
    }

    /// Refresh Steam New this week + Coming soon releases with 6-hour disk
    /// cache. Preserves cached releases offline or on HTTP 429.
    pub async fn refresh_store_releases(&self, force: bool) -> Result<ReleasesSnapshot> {
        let country = releases_country(&self.settings.app().deals_region);
        let cached = read_cached_releases(&country);
        let now = now_unix();

        if !force
            && let Some(c) = cached.as_ref()
            && is_releases_fresh(c, now)
        {
            return Ok(ReleasesSnapshot {
                new_this_week: c.new_this_week.clone(),
                coming_soon: c.coming_soon.clone(),
                offline: false,
                rate_limited: false,
            });
        }

        if self.working_offline() || self.isolated {
            let c = cached.unwrap_or_default();
            return Ok(ReleasesSnapshot {
                new_this_week: c.new_this_week,
                coming_soon: c.coming_soon,
                offline: self.working_offline(),
                rate_limited: false,
            });
        }

        let client = self.steam_links.client()?;
        let (new_res, soon_res) = futures::join!(
            client.store_releases(
                ReleaseFeedKind::NewThisWeek,
                &country,
                "english",
                RELEASES_PER_SECTION,
            ),
            client.store_releases(
                ReleaseFeedKind::ComingSoon,
                &country,
                "english",
                RELEASES_PER_SECTION,
            ),
        );

        let rate_limited = [&new_res, &soon_res].iter().any(|r| {
            r.as_ref()
                .is_err_and(|e| e.downcast_ref::<SteamError>() == Some(&SteamError::RateLimited))
        });

        match (new_res, soon_res) {
            (Ok(new_this_week), Ok(coming_soon)) => {
                let payload = CachedReleases {
                    fetched_at: now,
                    new_this_week: new_this_week.clone(),
                    coming_soon: coming_soon.clone(),
                };
                write_cached_releases(&country, &payload);
                Ok(ReleasesSnapshot {
                    new_this_week,
                    coming_soon,
                    offline: false,
                    rate_limited: false,
                })
            }
            (new_opt, soon_opt) => {
                let fallback = cached.unwrap_or_default();
                let new_this_week = new_opt.unwrap_or(fallback.new_this_week);
                let coming_soon = soon_opt.unwrap_or(fallback.coming_soon);
                if new_this_week.is_empty() && coming_soon.is_empty() && !rate_limited {
                    anyhow::bail!("failed to load Steam store releases");
                }
                Ok(ReleasesSnapshot {
                    new_this_week,
                    coming_soon,
                    offline: false,
                    rate_limited,
                })
            }
        }
    }
}
