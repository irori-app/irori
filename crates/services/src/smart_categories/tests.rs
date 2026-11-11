use std::collections::HashMap;
use std::sync::Arc;

use chrono::{FixedOffset, NaiveDate};
use irori_core::game::PlaytimeRecord;
use irori_core::{GameId, GameInfo, GameMode, Runner};
use serde_json::json;

use super::*;
use crate::play_status::{PlayStatus, PlayStatusRecord};
use crate::{LibrarySnapshot, UserData};

fn game(runner: Runner, app: &str, title: &str) -> GameInfo {
    GameInfo {
        runner: Some(runner),
        app_name: app.into(),
        title: title.into(),
        ..Default::default()
    }
}

fn library(games: Vec<GameInfo>, user: &UserData) -> LibrarySnapshot {
    let games = games.into_iter().map(|g| Arc::new(g.finalize())).collect();
    LibrarySnapshot::new(1, crate::copies::group(games, user, true), HashMap::new())
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// The ids of the games `category` holds, sorted.
fn members(category: SmartCategory, lib: &LibrarySnapshot, user: &UserData) -> Vec<String> {
    members_with(category, lib, user, &HashMap::new())
}

fn members_with(
    category: SmartCategory,
    lib: &LibrarySnapshot,
    user: &UserData,
    hltb: &HashMap<String, u32>,
) -> Vec<String> {
    let mut user = user.clone();
    user.smart_categories.insert("s".into(), category);
    let key = IndexKey {
        today: Some(day(2026, 11, 10)),
        ..Default::default()
    };
    let index = CategoryIndex::build(key, lib, &user, hltb, &FixedOffset::east_opt(0).unwrap());
    let mut ids: Vec<String> = lib
        .all_games()
        .map(|g| g.id())
        .filter(|id| index.contains("s", id))
        .map(|id| id.key())
        .collect();
    ids.sort();
    ids
}

fn category(matching: Match, rules: Vec<Rule>, leave_out: Vec<Rule>) -> SmartCategory {
    SmartCategory::new("Test", matching, rules, leave_out)
}

/// A rule a newer Irori wrote.
fn future_rule() -> Condition {
    serde_json::from_value(json!({"rule": "moonPhase", "phase": "full"})).unwrap()
}

fn three_games() -> (LibrarySnapshot, UserData) {
    let user = UserData::default();
    let mut a = game(Runner::Gog, "1", "Alpha");
    a.is_installed = true;
    let b = game(Runner::Legendary, "2", "Bravo");
    let c = game(Runner::Steam, "3", "Charlie");
    (library(vec![a, b, c], &user), user)
}

#[test]
fn not_keeps_an_unknown_answer_unknown() {
    assert_eq!(Truth::Unknown.not(), Truth::Unknown);
    assert_eq!(Truth::Yes.not(), Truth::No);
    assert_eq!(Truth::Unknown.and(Truth::No), Truth::No);
    assert_eq!(Truth::Unknown.and(Truth::Yes), Truth::Unknown);
    assert_eq!(Truth::Unknown.or(Truth::Yes), Truth::Yes);
    assert_eq!(Truth::Unknown.or(Truth::No), Truth::Unknown);
}

#[test]
fn a_category_depending_on_an_unknown_rule_shows_no_games() {
    let (lib, user) = three_games();
    let mut all = category(
        Match::All,
        vec![Rule::Installed { installed: true }],
        vec![],
    );
    all.rules.push(future_rule());
    assert!(members(all, &lib, &user).is_empty());

    // Leaving out an unknown rule is unknown too, never "everything".
    let mut left_out = category(Match::All, vec![], vec![]);
    left_out.leave_out.push(future_rule());
    assert!(members(left_out, &lib, &user).is_empty());

    // An unknown match mode decides nothing either.
    let mut mode = category(
        Match::All,
        vec![Rule::Installed { installed: true }],
        vec![],
    );
    mode.matching = Match::Unknown(json!("mostOf"));
    assert!(members(mode, &lib, &user).is_empty());
}

#[test]
fn an_any_with_a_known_match_still_matches() {
    let (lib, user) = three_games();
    let mut any = category(
        Match::Any,
        vec![Rule::Installed { installed: true }],
        vec![],
    );
    any.rules.push(future_rule());
    assert_eq!(members(any.clone(), &lib, &user), ["gog:1"]);
    assert!(!any.is_readable());
}

#[test]
fn empty_all_and_empty_any_hold_every_game() {
    let (lib, user) = three_games();
    let every = ["gog:1", "legendary:2", "steam:3"];
    assert_eq!(
        members(category(Match::All, vec![], vec![]), &lib, &user),
        every
    );
    assert_eq!(
        members(category(Match::Any, vec![], vec![]), &lib, &user),
        every
    );
}

#[test]
fn an_empty_any_with_leave_out_rows_shows_the_rest_not_nothing() {
    let (lib, user) = three_games();
    let rest = category(
        Match::Any,
        vec![],
        vec![Rule::Store {
            stores: [Runner::Steam].into(),
        }],
    );
    assert_eq!(members(rest.clone(), &lib, &user), ["gog:1", "legendary:2"]);
    // It reads back with the same mode, so the editor shows "any" again.
    let text = serde_json::to_string(&rest).unwrap();
    let back: SmartCategory = serde_json::from_str(&text).unwrap();
    assert_eq!(back.matching, Match::Any);
    assert_eq!(back, rest);
}

#[test]
fn unknown_keys_stay_on_the_category_and_on_every_condition() {
    let written = json!({
        "name": "Weekend",
        "match": "all",
        "rules": [
            {"rule": "installed", "installed": true, "weight": 2},
            {"rule": "moonPhase", "phase": "full"},
            "a row that isn't an object",
        ],
        "leaveOut": [{"rule": "favourite", "since": "2027"}],
        "pinned": true,
        "colour": "teal",
        "icon": "sun",
    });
    let read: SmartCategory = serde_json::from_value(written.clone()).unwrap();
    assert_eq!(read.name, "Weekend");
    assert!(read.look.pinned);
    assert_eq!(read.look.colour.as_deref(), Some("teal"));
    assert_eq!(
        read.rules[0].rule,
        Some(Rule::Installed { installed: true })
    );
    assert_eq!(read.rules[0].extra["weight"], 2);
    assert!(read.rules[1].rule.is_none());
    assert_eq!(read.extra["icon"], "sun");
    assert_eq!(serde_json::to_value(&read).unwrap(), written);

    // Editing a row keeps what a newer Irori added to it.
    let mut edited = read.clone();
    edited.rules[0] = read.rules[0].with_rule(Rule::Installed { installed: false });
    edited.name = "Weekends".into();
    let value = serde_json::to_value(&edited).unwrap();
    assert_eq!(value["rules"][0]["weight"], 2);
    assert_eq!(value["rules"][0]["installed"], false);
    assert_eq!(value["icon"], "sun");
}

#[test]
fn an_entry_this_version_cant_read_is_kept_whole_and_holds_nothing() {
    let written = json!({"name": "Odd", "rules": {"tree": []}, "pinned": true});
    let read: SmartCategory = serde_json::from_value(written.clone()).unwrap();
    assert!(read.is_unreadable());
    assert_eq!(read.name, "Odd");
    assert!(read.look.pinned);
    assert_eq!(serde_json::to_value(&read).unwrap(), written);
    // Reading it again gives the same.
    let again: SmartCategory =
        serde_json::from_value(serde_json::to_value(&read).unwrap()).unwrap();
    assert_eq!(again, read);
    let (lib, user) = three_games();
    assert!(members(read.clone(), &lib, &user).is_empty());

    // Its pin and colour can still change, and stay in what is written.
    let look = CategoryLook {
        colour: Some("red".into()),
        ..Default::default()
    };
    let recoloured = read.with_look(look.clone());
    let value = serde_json::to_value(&recoloured).unwrap();
    assert_eq!(value["colour"], "red");
    assert!(value.get("pinned").is_none());
    let back: SmartCategory = serde_json::from_value(value).unwrap();
    assert_eq!(back.look, look);
}

#[test]
fn days_ago_counts_local_calendar_days_not_utc_dates() {
    let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
    let minus_five = FixedOffset::west_opt(5 * 3600).unwrap();
    let today = day(2026, 11, 10);
    // 23:30 UTC on the 9th is already the 10th two hours east.
    assert_eq!(days_ago("2026-11-09T23:30:00Z", today, &plus_two), Some(0));
    // 02:00 UTC on the 10th is still the 9th five hours west.
    assert_eq!(
        days_ago("2026-11-10T02:00:00Z", today, &minus_five),
        Some(1)
    );
    assert_eq!(
        days_ago("2026-11-03T12:00:00+02:00", today, &plus_two),
        Some(7)
    );
    // Plain dates read as written.
    assert_eq!(days_ago("2026-11-01", today, &minus_five), Some(9));
    assert_eq!(days_ago("not a date", today, &plus_two), None);
}

#[test]
fn last_played_rules_go_by_the_local_day() {
    let mut user = UserData::default();
    user.playtime.insert(
        "gog:1".into(),
        PlaytimeRecord {
            total_minutes: 30,
            last_played: Some("2026-11-09T23:30:00Z".into()),
            ..Default::default()
        },
    );
    let lib = library(vec![game(Runner::Gog, "1", "Alpha")], &user);
    user.smart_categories.insert(
        "today".into(),
        category(
            Match::All,
            vec![Rule::LastPlayed {
                min_days: None,
                max_days: Some(0),
            }],
            vec![],
        ),
    );
    let key = IndexKey {
        today: Some(day(2026, 11, 10)),
        ..Default::default()
    };
    let id = GameId::new(Runner::Gog, "1");
    let east = CategoryIndex::build(
        key,
        &lib,
        &user,
        &HashMap::new(),
        &FixedOffset::east_opt(2 * 3600).unwrap(),
    );
    assert!(east.contains("today", &id));
    let utc = CategoryIndex::build(
        key,
        &lib,
        &user,
        &HashMap::new(),
        &FixedOffset::east_opt(0).unwrap(),
    );
    assert!(!utc.contains("today", &id));
}

#[test]
fn every_rule_kind_reads_its_part_of_the_game() {
    let mut user = UserData::default();
    let mut a = game(Runner::Gog, "1", "Hollow Knight");
    a.store_tags = vec!["Metroidvania".into()];
    a.game_modes = vec![GameMode::Single];
    a.genres = vec!["Action".into()];
    a.is_installed = true;
    let mut b = game(Runner::Steam, "2", "Portal 2");
    b.game_modes = vec![GameMode::Coop, GameMode::Single];
    b.store_tags = vec!["Puzzle".into(), "Co-op".into()];
    let mut c = game(Runner::Emulation, "3", "Chrono Trigger");
    c.system = Some("snes".into());
    c.genres = vec!["RPG ".into()];
    user.favourites.insert("steam:2".into());
    user.play_status.insert(
        "emulation:3".into(),
        PlayStatusRecord {
            status: PlayStatus::Beaten,
            since: "2026-01-01T00:00:00Z".into(),
        },
    );
    user.playtime.insert(
        "gog:1".into(),
        PlaytimeRecord {
            total_minutes: 600,
            last_played: Some("2026-10-01T10:00:00Z".into()),
            ..Default::default()
        },
    );
    user.categories
        .insert("Couch".into(), ["steam:2".to_string()].into());
    let lib = library(vec![a, b, c], &user);
    let hltb = HashMap::from([("gog:1".to_string(), 27), ("steam:2".to_string(), 8)]);
    let one =
        |rule: Rule| members_with(category(Match::All, vec![rule], vec![]), &lib, &user, &hltb);

    let stores = [Runner::Steam, Runner::Emulation].into();
    assert_eq!(one(Rule::Store { stores }), ["emulation:3", "steam:2"]);
    assert_eq!(
        one(Rule::Tags {
            tags: vec!["co op".into()]
        }),
        ["steam:2"]
    );
    assert_eq!(
        one(Rule::Mode {
            modes: [GameMode::Coop].into()
        }),
        ["steam:2"]
    );
    assert_eq!(
        one(Rule::Status {
            statuses: [EffectiveStatus::Beaten, EffectiveStatus::Played].into()
        }),
        ["emulation:3", "gog:1"]
    );
    assert_eq!(
        one(Rule::AgeRating {
            max_age: 12,
            include_unrated: true
        })
        .len(),
        3
    );
    assert!(
        one(Rule::AgeRating {
            max_age: 12,
            include_unrated: false
        })
        .is_empty()
    );
    assert_eq!(one(Rule::Installed { installed: true }), ["gog:1"]);
    assert_eq!(
        one(Rule::Playtime {
            min_minutes: Some(60),
            max_minutes: None
        }),
        ["gog:1"]
    );
    assert_eq!(
        one(Rule::Playtime {
            min_minutes: None,
            max_minutes: Some(0)
        }),
        ["emulation:3", "steam:2"]
    );
    assert_eq!(
        one(Rule::LastPlayed {
            min_days: Some(30),
            max_days: None
        }),
        ["gog:1"]
    );
    assert_eq!(
        one(Rule::Hltb {
            min_hours: None,
            max_hours: Some(10)
        }),
        ["steam:2"]
    );
    assert_eq!(
        one(Rule::System {
            systems: ["snes".to_string()].into()
        }),
        ["emulation:3"]
    );
    assert_eq!(
        one(Rule::Category {
            names: ["Couch".to_string()].into()
        }),
        ["steam:2"]
    );
    assert!(
        one(Rule::Category {
            names: ["Gone".to_string()].into()
        })
        .is_empty()
    );
    assert_eq!(
        one(Rule::Query {
            query: "hollow".into()
        }),
        ["gog:1"]
    );
    assert_eq!(
        one(Rule::Query {
            query: "mode:coop".into()
        }),
        ["steam:2"]
    );
    assert_eq!(one(Rule::Favourite {}), ["steam:2"]);
    assert_eq!(
        one(Rule::Genre {
            genres: vec!["rpg".into()]
        }),
        ["emulation:3"]
    );
}

#[test]
fn an_entry_is_in_a_category_when_any_of_its_copies_is() {
    let user = UserData::default();
    let mut epic = game(Runner::Legendary, "p2", "Portal 2");
    epic.is_installed = true;
    let steam = game(Runner::Steam, "620", "Portal 2");
    let lib = library(vec![epic, steam, game(Runner::Gog, "1", "Celeste")], &user);
    let on_steam = category(
        Match::All,
        vec![Rule::Store {
            stores: [Runner::Steam].into(),
        }],
        vec![],
    );
    assert_eq!(
        members(on_steam, &lib, &user),
        ["legendary:p2", "steam:620"]
    );
}

#[test]
fn counts_leave_hidden_games_out() {
    let mut user = UserData::default();
    user.hidden.insert("legendary:2".into());
    user.categories.insert(
        "Both".into(),
        ["gog:1".to_string(), "legendary:2".to_string()].into(),
    );
    user.smart_categories
        .insert("all".into(), category(Match::All, vec![], vec![]));
    let lib = library(
        vec![
            game(Runner::Gog, "1", "Alpha"),
            game(Runner::Legendary, "2", "Bravo"),
        ],
        &user,
    );
    let index = CategoryIndex::build(
        IndexKey::default(),
        &lib,
        &user,
        &HashMap::new(),
        &chrono::Local,
    );
    assert_eq!(index.count(&CategoryRef::Smart("all".into())), 1);
    assert_eq!(index.count(&CategoryRef::Manual("Both".into())), 1);
    // Still in it: the library hides it as it hides it everywhere.
    assert!(index.contains("all", &GameId::new(Runner::Legendary, "2")));
}

#[test]
fn ten_thousand_games_are_indexed_in_one_quick_pass() {
    let mut user = UserData::default();
    let games: Vec<GameInfo> = (0..10_000)
        .map(|i| {
            let mut g = game(
                Runner::ALL[i % Runner::ALL.len()],
                &i.to_string(),
                &format!("Game {i}"),
            );
            g.is_installed = i % 3 == 0;
            g.store_tags = vec![format!("tag{}", i % 50)];
            g
        })
        .collect();
    for i in (0..10_000).step_by(7) {
        user.playtime.insert(
            format!("{}:{i}", Runner::ALL[i % Runner::ALL.len()].id()),
            PlaytimeRecord {
                total_minutes: i as u64,
                last_played: Some("2026-11-01T12:00:00Z".into()),
                ..Default::default()
            },
        );
    }
    let lib = library(games, &user);
    let rules = [
        vec![Rule::Installed { installed: true }],
        vec![Rule::Tags {
            tags: vec!["tag7".into()],
        }],
        vec![Rule::Playtime {
            min_minutes: Some(600),
            max_minutes: None,
        }],
        vec![Rule::LastPlayed {
            min_days: None,
            max_days: Some(30),
        }],
        vec![Rule::Query {
            query: "game 12".into(),
        }],
    ];
    for (i, rules) in rules.into_iter().enumerate() {
        user.smart_categories
            .insert(format!("s{i}"), category(Match::Any, rules, vec![]));
    }
    let started = std::time::Instant::now();
    let index = CategoryIndex::build(
        IndexKey::default(),
        &lib,
        &user,
        &HashMap::new(),
        &chrono::Local,
    );
    let took = started.elapsed();
    assert_eq!(index.count(&CategoryRef::Smart("s0".into())), 3334);
    // Unoptimized test builds included, well under what a frame or two
    // allows at this size; it runs once per change, never per frame.
    assert!(took.as_secs_f64() < 5.0, "indexing took {took:?}");
}

#[test]
fn renaming_a_manual_category_follows_it_in_rules() {
    let mut smart = category(
        Match::All,
        vec![Rule::Category {
            names: ["RPG".to_string()].into(),
        }],
        vec![Rule::Category {
            names: ["Other".to_string()].into(),
        }],
    );
    assert!(smart.rename_category("RPG", "Role-playing"));
    assert_eq!(
        smart.rules[0].rule,
        Some(Rule::Category {
            names: ["Role-playing".to_string()].into()
        })
    );
    assert!(!smart.rename_category("RPG", "Again"));
}
