//! The media of a Steam store page, read for games of other stores linked
//! to a Steam app: its screenshots and trailers. Steam's store page has
//! them for nearly every PC game, so a GOG, Epic or added game whose own
//! page has none borrows them; what the game's own store shows always wins.
//!
//! The page itself is fetched and kept by the Steam link
//! ([`crate::steam_link`]), one fetch per linked app, which also fills the
//! description, genres and release date a page lacks.

use irori_core::{AgeRating, RatingSystem, Video, age_rating};
use irori_steam_api::library::AppDetails;

/// The page's full-size screenshots (`https` only).
pub(crate) fn screenshots_of(details: &AppDetails) -> Vec<String> {
    details
        .screenshots
        .iter()
        .map(|s| s.path_full.trim().to_string())
        .filter(|p| p.starts_with("https://"))
        .collect()
}

/// The page's trailers that can play, the highlighted ones first.
pub(crate) fn videos_of(details: &AppDetails) -> Vec<Video> {
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
    videos.into_iter().map(|(_, v)| v).collect()
}

/// The page's age ratings by board (unknown boards such as `steam_germany`
/// are left out).
pub(crate) fn age_ratings_of(details: &AppDetails) -> Vec<AgeRating> {
    details
        .ratings
        .iter()
        .filter_map(|(board, r)| {
            let system = RatingSystem::from_key(board)?;
            let age = u8::try_from(r.required_age).ok().filter(|a| *a > 0);
            AgeRating::new(
                system,
                &r.rating,
                age_rating::split_descriptors(&r.descriptors, false),
                age,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_screenshots_and_trailers() {
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
        assert_eq!(screenshots_of(&details), ["https://cdn/1.jpg"]);
        let names: Vec<String> = videos_of(&details).into_iter().map(|v| v.name).collect();
        assert_eq!(names, ["Launch", "Gameplay"]);
    }

    #[test]
    fn reads_known_age_ratings() {
        let details: AppDetails = serde_json::from_value(serde_json::json!({
            "ratings": {
                "esrb": {"rating": "m", "descriptors": "Blood and Gore\r\nIntense Violence", "required_age": "17"},
                "pegi": {"rating": "18", "descriptors": "Violence", "required_age": "18"},
                "dejus": {"rating": "18", "descriptors": "Violência Extrema", "required_age": "18"},
                "steam_germany": {"rating": "18", "required_age": "18"}
            }
        }))
        .unwrap();
        let ratings = age_ratings_of(&details);
        let titles: Vec<String> = ratings.iter().map(AgeRating::title).collect();
        assert_eq!(titles, ["ClassInd 18", "ESRB M", "PEGI 18"]);
        assert_eq!(
            ratings[1].descriptors,
            ["Blood and Gore", "Intense Violence"]
        );
    }
}
