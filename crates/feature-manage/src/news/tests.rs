use std::collections::HashSet;

use irori_core::{GameId, Runner};
use irori_services::{GameNewsEntry, NewsKind, SteamNewsItem};

use super::my_games::{TimeBucket, filter_entries, group_rows};

fn sample_entry(
    game: &str,
    app_id: u32,
    gid: &str,
    title: &str,
    date: u64,
    kind: NewsKind,
) -> GameNewsEntry {
    GameNewsEntry {
        game_id: GameId::new(Runner::Steam, app_id.to_string()),
        title: game.to_string(),
        store: Runner::Steam,
        via_steam: false,
        app_id,
        is_installed: true,
        item: SteamNewsItem {
            gid: gid.to_string(),
            title: title.to_string(),
            url: format!("https://store.steampowered.com/news/app/{app_id}/view/{gid}"),
            author: "Dev".to_string(),
            contents: "Body".to_string(),
            feedlabel: "Community Announcements".to_string(),
            feedname: "steam_community_announcements".to_string(),
            appid: app_id,
            is_external_url: false,
            tags: Vec::new(),
            date,
            kind,
        },
    }
}

#[test]
fn time_bucket_classifies_today_yesterday_week_and_earlier() {
    let now = 1_760_000_000_u64;
    assert_eq!(
        TimeBucket::from_timestamps(now - 300, now),
        TimeBucket::Today
    );
    assert_eq!(
        TimeBucket::from_timestamps(now - 90_000, now),
        TimeBucket::Yesterday
    );
    assert_eq!(
        TimeBucket::from_timestamps(now - 3 * 86_400, now),
        TimeBucket::ThisWeek
    );
    assert_eq!(
        TimeBucket::from_timestamps(now - 10 * 86_400, now),
        TimeBucket::Earlier
    );
}

#[test]
fn filter_entries_matches_kind_and_case_insensitive_query() {
    let entries = vec![
        sample_entry(
            "Hades II",
            1145350,
            "1",
            "Patch 1.2 Notes",
            1000,
            NewsKind::PatchNotes,
        ),
        sample_entry(
            "Hades II",
            1145350,
            "2",
            "Soundtrack Release",
            900,
            NewsKind::News,
        ),
        sample_entry(
            "Cyberpunk 2077",
            1091500,
            "3",
            "Hotfix 2.21",
            800,
            NewsKind::PatchNotes,
        ),
    ];

    assert_eq!(filter_entries(&entries, None, ""), vec![0, 1, 2]);
    assert_eq!(
        filter_entries(&entries, Some(NewsKind::PatchNotes), ""),
        vec![0, 2]
    );
    assert_eq!(
        filter_entries(&entries, Some(NewsKind::News), "hades"),
        vec![1]
    );
    assert_eq!(filter_entries(&entries, None, "hotfix"), vec![2]);
}

#[test]
fn group_rows_collapses_consecutive_hotfixes_and_expands() {
    let now = 1_760_000_000_u64;
    let entries = vec![
        sample_entry(
            "Hades II",
            1145350,
            "g3",
            "Hotfix 3",
            now - 60,
            NewsKind::PatchNotes,
        ),
        sample_entry(
            "Hades II",
            1145350,
            "g2",
            "Hotfix 2",
            now - 120,
            NewsKind::PatchNotes,
        ),
        sample_entry(
            "Hades II",
            1145350,
            "g1",
            "Hotfix 1",
            now - 180,
            NewsKind::PatchNotes,
        ),
        sample_entry(
            "Cyberpunk 2077",
            1091500,
            "c1",
            "Patch 2.2",
            now - 240,
            NewsKind::PatchNotes,
        ),
        sample_entry(
            "Hades II",
            1145350,
            "g0",
            "Older post",
            now - 300,
            NewsKind::News,
        ),
    ];
    let filtered: Vec<usize> = (0..entries.len()).collect();
    let mut expanded = HashSet::new();

    let rows = group_rows(&entries, &filtered, now, &expanded);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].entry_idx, 0);
    assert_eq!(rows[0].collapsed_siblings, vec![1, 2]);
    assert!(!rows[0].expanded);
    assert_eq!(rows[1].entry_idx, 3);
    assert!(rows[1].collapsed_siblings.is_empty());
    // Non-consecutive Hades II post after Cyberpunk stays its own row.
    assert_eq!(rows[2].entry_idx, 4);
    assert!(rows[2].collapsed_siblings.is_empty());

    expanded.insert(rows[0].group_key.clone());
    let rows_expanded = group_rows(&entries, &filtered, now, &expanded);
    assert!(rows_expanded[0].expanded);
}
