//! Backend facade for Gaming news RSS/Atom feeds and on-demand Reader Mode.
//! Respects offline mode and never contacts external feeds in the background.

use anyhow::Result;
use irori_core::Event;
use irori_extras::news_feeds::{
    self, ArticleContent, FeedArticle, FeedFetchResult, NewsFeedsConfig,
};

use crate::Backend;

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
}
