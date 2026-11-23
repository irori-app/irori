//! Store media for games of other stores from the Steam app they are linked
//! to (the per-game `iroriSteamAppId`). Steam's store page has screenshots
//! for nearly every PC game, so a GOG, Epic or added game whose own page
//! has none borrows them; what the game's own store shows always wins.
//!
//! Store pages are asked only when a game page opens, through the keyless
//! `appdetails` endpoint, and kept in memory for the run: an answer per app
//! and language, a failure for a few minutes so an unreachable store isn't
//! asked again on every page.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use irori_core::settings::GameSettings;
use irori_core::{ExtraInfo, GameId, Runner, Video};
use irori_steam_api::Client;
use irori_steam_api::library::AppDetails;
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};

use crate::Backend;

/// How long a store page that couldn't be read is not asked for again.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

static CLIENT: Lazy<RwLock<Option<Client>>> = Lazy::new(|| {
    RwLock::new(match Client::new() {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("Steam client for store media: {e:#}");
            None
        }
    })
});

enum Page {
    /// The store's answer; `None` when Steam has no page for the app.
    Known(Option<Arc<AppDetails>>),
    Failed(Instant),
}

static PAGES: Lazy<Mutex<HashMap<(u32, &'static str), Page>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Ask another server than Steam's (tests).
#[cfg(test)]
pub(crate) fn set_endpoints(endpoints: irori_steam_api::Endpoints) {
    *CLIENT.write() = Client::with_endpoints(endpoints).ok();
    PAGES.lock().clear();
}

/// The Steam app a game is linked to: the first word of its
/// `iroriSteamAppId`, when that is a positive number.
pub(crate) fn linked_app(settings: &GameSettings) -> Option<u32> {
    settings
        .steam_app_id
        .split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|id| *id > 0)
}

/// Fill what `info` lacks from Steam's store page `details`.
pub(crate) fn fill(info: &mut ExtraInfo, details: &AppDetails) {
    if info.screenshots.is_empty() {
        info.screenshots = details
            .screenshots
            .iter()
            .map(|s| s.path_full.trim().to_string())
            .filter(|p| p.starts_with("https://"))
            .collect();
    }
    if info.videos.is_empty() {
        let mut videos: Vec<(bool, Video)> = details
            .movies
            .iter()
            .filter_map(|m| {
                let video = Video {
                    name: m.name.trim().to_string(),
                    thumbnail: m.thumbnail_url(),
                    url: m.stream_url()?,
                };
                Some((m.highlight, video))
            })
            .collect();
        videos.sort_by_key(|(highlight, _)| !highlight);
        info.videos = videos.into_iter().map(|(_, v)| v).collect();
    }
}

/// Whether `info` still lacks something Steam's page could give.
fn lacks_media(info: &ExtraInfo) -> bool {
    info.screenshots.is_empty() || info.videos.is_empty()
}

/// Steam's store page of `appid` in `language`, from memory when known.
async fn store_page(appid: u32, language: &'static str) -> Option<Arc<AppDetails>> {
    match PAGES.lock().get(&(appid, language)) {
        Some(Page::Known(page)) => return page.clone(),
        Some(Page::Failed(at)) if at.elapsed() < RETRY_AFTER => return None,
        _ => {}
    }
    let client = CLIENT.read().clone()?;
    let page = match client.app_details(appid, language).await {
        Ok(page) => Page::Known(page.map(Arc::new)),
        Err(e) => {
            tracing::debug!("Steam store page of {appid}: {e:#}");
            Page::Failed(Instant::now())
        }
    };
    let found = match &page {
        Page::Known(p) => p.clone(),
        Page::Failed(_) => None,
    };
    PAGES.lock().insert((appid, language), page);
    found
}

impl Backend {
    /// Fill `info`'s missing media from the Steam app `id`'s game is linked
    /// to. Steam's own games have their page already; Steam being
    /// unreachable never fails the store's page.
    pub(crate) async fn fill_from_steam(&self, id: &GameId, info: &mut ExtraInfo) {
        if id.runner == Runner::Steam || !lacks_media(info) {
            return;
        }
        let Some(appid) = linked_app(&self.settings.game(id)) else {
            return;
        };
        let language =
            irori_steam_api::achievements::steam_language(&self.settings.app().ui_language);
        if let Some(page) = store_page(appid, language).await {
            fill(info, &page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_linked_app() {
        let with = |text: &str| GameSettings {
            steam_app_id: text.into(),
            ..Default::default()
        };
        assert_eq!(linked_app(&with("1091500")), Some(1091500));
        assert_eq!(linked_app(&with(" 220 Half-Life 2")), Some(220));
        assert_eq!(linked_app(&with("")), None);
        assert_eq!(linked_app(&with("0")), None);
        assert_eq!(linked_app(&with("none")), None);
    }

    #[test]
    fn fills_only_what_the_store_lacks() {
        let details: AppDetails = serde_json::from_value(serde_json::json!({
            "screenshots": [
                {"path_thumbnail": "https://cdn/t1.jpg", "path_full": "https://cdn/1.jpg"},
                {"path_full": ""},
                {"path_full": "file:///etc/passwd"}
            ],
            "movies": [
                {"name": "Gameplay", "mp4": {"480": "https://cdn/g.mp4"}},
                {"name": "Launch", "highlight": true, "hls_h264": "https://cdn/l.m3u8"}
            ]
        }))
        .unwrap();
        let mut empty = ExtraInfo::default();
        fill(&mut empty, &details);
        assert_eq!(empty.screenshots, ["https://cdn/1.jpg"]);
        let names: Vec<&str> = empty.videos.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Launch", "Gameplay"]);
        let mut own = ExtraInfo {
            screenshots: vec!["https://gog/1.jpg".into()],
            ..Default::default()
        };
        fill(&mut own, &details);
        assert_eq!(own.screenshots, ["https://gog/1.jpg"]);
        assert_eq!(own.videos.len(), 2);
    }
}
