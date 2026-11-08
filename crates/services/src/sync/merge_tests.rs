use super::*;
use crate::play_status::PlayStatus;

/// Minutes, as milliseconds after a fixed start.
fn t(minutes: i64) -> Millis {
    1_790_000_000_000 + minutes * 60_000
}

fn rfc(ms: Millis) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339()
}

fn session(game: &str, started: Millis, seconds: u64) -> Session {
    Session {
        game: game.into(),
        started: rfc(started),
        seconds,
        crashed: false,
        exit_code: None,
    }
}

/// One computer, with what it keeps between syncs.
struct Device {
    id: &'static str,
    data: UserData,
    sessions: Vec<Session>,
    registers: Registers,
    known: BTreeSet<String>,
    clock: Millis,
    synced: Option<Millis>,
    published: Shared,
    /// How far this computer's clock is off, in milliseconds.
    skew: Millis,
}

impl Device {
    fn new(id: &'static str) -> Self {
        Self {
            id,
            data: UserData::default(),
            sessions: Vec::new(),
            registers: Registers::default(),
            known: BTreeSet::new(),
            clock: 0,
            synced: None,
            published: Shared::default(),
            skew: 0,
        }
    }

    /// What the periodic tick does between syncs: note changes as they
    /// happen, even with the sync folder out of reach.
    fn tick(&mut self, now: Millis) {
        let dating = Dating::for_now(self.synced, now + self.skew, self.clock);
        record(
            &mut self.registers,
            &self.data,
            &self.known,
            self.id,
            dating,
        );
        self.clock = self.clock.max(self.registers.newest());
    }

    fn sync(&mut self, now: Millis, others: &[&Device]) -> Merged {
        let remotes: Vec<&Shared> = others.iter().map(|d| &d.published).collect();
        let merged = merge(
            Local {
                device: self.id,
                data: &self.data,
                sessions: &self.sessions,
                registers: &self.registers,
                known_playtime: &self.known,
                now: now + self.skew,
                clock: self.clock,
                synced: self.synced,
            },
            &remotes,
        );
        self.data = merged.data.clone();
        self.sessions = merged.sessions.clone();
        self.registers = merged.registers.clone();
        self.known = self.data.playtime.keys().cloned().collect();
        self.clock = merged.clock;
        self.synced = Some(merged.clock);
        self.published = Shared {
            registers: merged.registers.clone(),
            playtime: merged.playtime.clone(),
            sessions: merged.sessions.clone(),
        };
        merged
    }

    fn play(&mut self, game: &str, started: Millis, seconds: u64) {
        self.sessions.push(session(game, started, seconds));
        let r = self.data.playtime.entry(game.into()).or_default();
        let total = r.extra_seconds + seconds;
        r.total_minutes += total / 60;
        r.extra_seconds = total % 60;
        r.last_played = Some(rfc(started));
        r.first_played.get_or_insert(rfc(started));
    }

    fn seconds(&self, game: &str) -> u64 {
        self.data.playtime.get(game).map_or(0, record_seconds)
    }

    fn status(&self, game: &str) -> Option<PlayStatus> {
        self.data.play_status(game)
    }
}

/// Sync `a` then `b` then `a` again, so both have everything.
fn round(a: &mut Device, b: &mut Device, now: Millis) {
    a.sync(now, &[b]);
    b.sync(now + 1, &[a]);
    a.sync(now + 2, &[b]);
}

fn status(s: PlayStatus, since: Millis) -> PlayStatusRecord {
    PlayStatusRecord {
        status: s,
        since: rfc(since),
    }
}

#[test]
fn first_sync_keeps_everything_both_devices_had() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data.favourites.insert("gog:1".into());
    a.data.ratings.insert("gog:1".into(), 4);
    a.data
        .categories
        .insert("RPG".into(), ["gog:1".to_string()].into());
    b.data.favourites.insert("gog:2".into());
    b.data
        .categories
        .insert("RPG".into(), ["gog:2".to_string()].into());
    b.data.backlog_goals.insert(2026, 12);
    // Both set a status before syncing; the later one is kept.
    a.data
        .play_status
        .insert("gog:3".into(), status(PlayStatus::Playing, t(-100)));
    b.data
        .play_status
        .insert("gog:3".into(), status(PlayStatus::Beaten, t(-50)));

    round(&mut a, &mut b, t(0));
    for d in [&a, &b] {
        assert_eq!(
            d.data.favourites,
            ["gog:1", "gog:2"].map(String::from).into()
        );
        assert_eq!(d.data.ratings["gog:1"], 4);
        assert_eq!(d.data.categories["RPG"].len(), 2);
        assert_eq!(d.data.backlog_goals[&2026], 12);
        assert_eq!(d.status("gog:3"), Some(PlayStatus::Beaten));
    }
}

#[test]
fn edits_to_different_games_both_arrive() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    round(&mut a, &mut b, t(0));

    a.data.ratings.insert("gog:1".into(), 5);
    a.data
        .play_status
        .insert("gog:1".into(), status(PlayStatus::Beaten, t(10)));
    a.tick(t(10));
    b.data.favourites.insert("gog:2".into());
    b.data.hidden.insert("gog:3".into());
    b.tick(t(11));

    let merged = a.sync(t(20), &[&b]);
    assert!(merged.conflicts.is_empty());
    let merged = b.sync(t(21), &[&a]);
    assert!(merged.conflicts.is_empty());
    a.sync(t(22), &[&b]);
    for d in [&a, &b] {
        assert_eq!(d.data.ratings["gog:1"], 5);
        assert_eq!(d.status("gog:1"), Some(PlayStatus::Beaten));
        assert!(d.data.favourites.contains("gog:2"));
        assert!(d.data.hidden.contains("gog:3"));
    }
}

#[test]
fn the_later_edit_of_the_same_value_wins_and_the_other_side_hears_of_it() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data.ratings.insert("gog:1".into(), 3);
    round(&mut a, &mut b, t(0));

    a.data.ratings.insert("gog:1".into(), 5);
    a.tick(t(10));
    b.data.ratings.insert("gog:1".into(), 2);
    b.tick(t(12));

    // a syncs first, but b's change was made later.
    let at_a = a.sync(t(20), &[&b]);
    assert!(at_a.conflicts.is_empty(), "b hasn't published yet");
    let at_b = b.sync(t(21), &[&a]);
    assert!(at_b.conflicts.is_empty(), "b's edit is the later one");
    let at_a = a.sync(t(22), &[&b]);
    assert_eq!(
        at_a.conflicts,
        [Conflict {
            field: ConflictField::Rating,
            subject: "gog:1".into(),
            category: None,
            kept_from: "b".into(),
            kept_at: t(12),
        }]
    );
    assert_eq!(a.data.ratings["gog:1"], 2);
    assert_eq!(b.data.ratings["gog:1"], 2);

    // Settled: nothing changes and no conflict is reported again.
    let again = a.sync(t(30), &[&b]);
    assert_eq!(again.changed, 0);
    assert!(again.conflicts.is_empty());
}

#[test]
fn a_change_made_after_seeing_another_wins_despite_a_slow_clock() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    // a's clock runs an hour fast.
    a.skew = 60 * 60_000;
    round(&mut a, &mut b, t(0));

    a.data
        .play_status
        .insert("gog:1".into(), status(PlayStatus::Playing, t(70)));
    a.tick(t(10));
    a.sync(t(11), &[&b]);
    b.sync(t(12), &[&a]);
    assert_eq!(b.status("gog:1"), Some(PlayStatus::Playing));

    // Twenty minutes later by the real clock, b finishes the game. Its
    // clock says it is still earlier than a's change.
    b.data
        .play_status
        .insert("gog:1".into(), status(PlayStatus::Beaten, t(30)));
    b.tick(t(30));
    b.sync(t(31), &[&a]);
    let merged = a.sync(t(32), &[&b]);
    assert!(merged.conflicts.is_empty());
    assert_eq!(a.status("gog:1"), Some(PlayStatus::Beaten));
    assert_eq!(b.status("gog:1"), Some(PlayStatus::Beaten));
}

#[test]
fn removals_reach_other_devices_and_stay_removed() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data.favourites.insert("gog:1".into());
    a.data.ratings.insert("gog:1".into(), 4);
    a.data
        .play_status
        .insert("gog:1".into(), status(PlayStatus::PlanToPlay, t(-5)));
    a.data
        .categories
        .insert("Couch".into(), ["gog:1".to_string()].into());
    a.data.backlog_goals.insert(2026, 10);
    round(&mut a, &mut b, t(0));
    assert!(b.data.favourites.contains("gog:1"));

    b.data.favourites.remove("gog:1");
    b.data.ratings.remove("gog:1");
    b.data.play_status.remove("gog:1");
    b.data.categories.remove("Couch");
    b.data.backlog_goals.remove(&2026);
    b.tick(t(10));
    round(&mut b, &mut a, t(20));
    // And again, so nothing comes back from an older copy.
    round(&mut a, &mut b, t(30));
    for d in [&a, &b] {
        assert!(d.data.favourites.is_empty());
        assert!(d.data.ratings.is_empty());
        assert!(d.data.play_status.is_empty());
        assert!(d.data.categories.is_empty());
        assert!(d.data.backlog_goals.is_empty());
    }
}

#[test]
fn a_category_deleted_on_one_device_survives_for_a_game_added_later() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data
        .categories
        .insert("Couch".into(), ["gog:1".to_string()].into());
    round(&mut a, &mut b, t(0));

    a.data.categories.remove("Couch");
    a.tick(t(10));
    b.data
        .categories
        .get_mut("Couch")
        .unwrap()
        .insert("gog:2".into());
    b.tick(t(15));
    round(&mut a, &mut b, t(20));
    round(&mut a, &mut b, t(30));
    for d in [&a, &b] {
        assert_eq!(
            d.data.categories["Couch"],
            ["gog:2".to_string()].into(),
            "{}",
            d.id
        );
    }
}

#[test]
fn renaming_a_category_moves_its_games() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data
        .categories
        .insert("Rpg".into(), ["gog:1".to_string(), "gog:2".into()].into());
    round(&mut a, &mut b, t(0));
    let games = a.data.categories.remove("Rpg").unwrap();
    a.data.categories.insert("RPG".into(), games);
    a.tick(t(5));
    round(&mut a, &mut b, t(10));
    assert_eq!(b.data.categories.keys().collect::<Vec<_>>(), ["RPG"]);
    assert_eq!(b.data.categories["RPG"].len(), 2);
}

#[test]
fn data_of_games_one_device_doesnt_have_is_kept() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    // A game a no longer owns (or deleted) still has b's notes on it.
    b.data.ratings.insert("legendary:Gone".into(), 5);
    b.play("legendary:Gone", t(-30), 1200);
    round(&mut a, &mut b, t(0));
    round(&mut a, &mut b, t(10));
    for d in [&a, &b] {
        assert_eq!(d.data.ratings["legendary:Gone"], 5);
        assert_eq!(d.seconds("legendary:Gone"), 1200);
    }
}

#[test]
fn playtime_counts_every_session_once() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    // Both start from the same restored backup: one session, 10 minutes.
    for d in [&mut a, &mut b] {
        d.play("gog:1", t(-100), 600);
    }
    round(&mut a, &mut b, t(0));
    assert_eq!(a.seconds("gog:1"), 600);
    assert_eq!(b.seconds("gog:1"), 600);

    // The desktop plays an hour, the Deck half an hour.
    a.play("gog:1", t(10), 3600);
    b.play("gog:1", t(20), 1800);
    for minute in [100, 200, 300] {
        round(&mut a, &mut b, t(minute));
    }
    for d in [&a, &b] {
        assert_eq!(d.seconds("gog:1"), 600 + 3600 + 1800, "{}", d.id);
        assert_eq!(d.sessions.len(), 3);
        let r = &d.data.playtime["gog:1"];
        assert_eq!(parse_millis(r.last_played.as_deref().unwrap()), Some(t(20)));
        assert_eq!(
            parse_millis(r.first_played.as_deref().unwrap()),
            Some(t(-100))
        );
    }
}

#[test]
fn store_totals_and_imports_count_once_beside_sessions() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    // a imported 10 hours from Heroic, without sessions.
    a.data.playtime.insert(
        "legendary:Fowl".into(),
        PlaytimeRecord {
            total_minutes: 600,
            ..Default::default()
        },
    );
    b.play("legendary:Fowl", t(5), 1800);
    round(&mut a, &mut b, t(10));
    assert_eq!(a.seconds("legendary:Fowl"), 600 * 60 + 1800);
    assert_eq!(b.seconds("legendary:Fowl"), 600 * 60 + 1800);

    // The store then reports a total covering the Deck's session too: it
    // is taken as is, not added to.
    b.data
        .playtime
        .get_mut("legendary:Fowl")
        .unwrap()
        .total_minutes = 640;
    b.tick(t(20));
    round(&mut a, &mut b, t(30));
    round(&mut a, &mut b, t(40));
    assert_eq!(a.seconds("legendary:Fowl"), 640 * 60);
    assert_eq!(b.seconds("legendary:Fowl"), 640 * 60);
}

#[test]
fn removed_playtime_stays_removed_until_the_game_is_played_again() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    for d in [&mut a, &mut b] {
        d.data.playtime.insert(
            "legendary:Dlc".into(),
            PlaytimeRecord {
                total_minutes: 30,
                ..Default::default()
            },
        );
    }
    round(&mut a, &mut b, t(0));
    // a's store says this was a DLC's time and drops it.
    a.data.playtime.remove("legendary:Dlc");
    a.tick(t(5));
    round(&mut a, &mut b, t(10));
    round(&mut a, &mut b, t(20));
    assert!(!a.data.playtime.contains_key("legendary:Dlc"));
    assert!(!b.data.playtime.contains_key("legendary:Dlc"));

    b.play("legendary:Dlc", t(30), 120);
    b.tick(t(30));
    round(&mut b, &mut a, t(40));
    assert_eq!(a.seconds("legendary:Dlc"), 120);
    assert_eq!(b.seconds("legendary:Dlc"), 120);
}

#[test]
fn custom_art_from_files_stays_on_its_computer() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data.overrides.insert(
        "gog:1".into(),
        GameOverrides {
            title: Some("Mine".into()),
            art_cover: Some("file:///home/me/cover.png".into()),
            art_background: Some("https://example.com/bg.jpg".into()),
            ..Default::default()
        },
    );
    round(&mut a, &mut b, t(0));
    let at_b = &b.data.overrides["gog:1"];
    assert_eq!(at_b.title.as_deref(), Some("Mine"));
    assert_eq!(at_b.art_cover, None);
    assert_eq!(
        at_b.art_background.as_deref(),
        Some("https://example.com/bg.jpg")
    );
    assert_eq!(
        a.data.overrides["gog:1"].art_cover.as_deref(),
        Some("file:///home/me/cover.png")
    );

    // b renames it; a keeps its own cover file.
    b.data.overrides.get_mut("gog:1").unwrap().title = Some("Ours".into());
    b.tick(t(5));
    round(&mut b, &mut a, t(10));
    let at_a = &a.data.overrides["gog:1"];
    assert_eq!(at_a.title.as_deref(), Some("Ours"));
    assert_eq!(at_a.art_cover.as_deref(), Some("file:///home/me/cover.png"));
}

#[test]
fn game_modes_the_user_set_travel_with_the_custom_title() {
    use irori_core::GameMode;
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data.overrides.insert(
        "steam:620".into(),
        GameOverrides {
            game_modes: Some(vec![GameMode::Coop, GameMode::Local]),
            ..Default::default()
        },
    );
    round(&mut a, &mut b, t(0));
    assert_eq!(
        b.data.overrides["steam:620"].game_modes,
        Some(vec![GameMode::Coop, GameMode::Local])
    );
    // "None of these" is a choice too, and wins when made later.
    b.data.overrides.get_mut("steam:620").unwrap().game_modes = Some(Vec::new());
    b.tick(t(5));
    round(&mut b, &mut a, t(10));
    assert_eq!(a.data.overrides["steam:620"].game_modes, Some(Vec::new()));
}

#[test]
fn a_category_change_lost_to_a_later_one_is_a_conflict() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.data
        .categories
        .insert("Couch".into(), ["gog:1".to_string()].into());
    round(&mut a, &mut b, t(0));
    a.data.categories.get_mut("Couch").unwrap().remove("gog:1");
    a.tick(t(10));
    b.data.favourites.insert("gog:9".into());
    b.data
        .categories
        .get_mut("Couch")
        .unwrap()
        .insert("gog:1".into());
    // b removes and adds it back later: its "in Couch" is the newer edit.
    b.data.categories.get_mut("Couch").unwrap().remove("gog:1");
    b.tick(t(11));
    b.data
        .categories
        .get_mut("Couch")
        .unwrap()
        .insert("gog:1".into());
    b.tick(t(12));
    b.sync(t(13), &[&a]);
    let merged = a.sync(t(14), &[&b]);
    assert_eq!(merged.conflicts.len(), 1);
    assert_eq!(merged.conflicts[0].field, ConflictField::CategoryGame);
    assert_eq!(merged.conflicts[0].category.as_deref(), Some("Couch"));
    assert!(a.data.categories["Couch"].contains("gog:1"));
}

#[test]
fn sessions_written_with_other_offsets_are_the_same_session() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.play("gog:1", t(0), 60);
    let mut same = session("gog:1", t(0), 60);
    same.started = chrono::DateTime::from_timestamp_millis(t(0))
        .unwrap()
        .with_timezone(&chrono::FixedOffset::east_opt(2 * 3600).unwrap())
        .to_rfc3339();
    b.sessions.push(same);
    b.data.playtime.insert(
        "gog:1".into(),
        PlaytimeRecord {
            total_minutes: 1,
            ..Default::default()
        },
    );
    round(&mut a, &mut b, t(5));
    assert_eq!(a.sessions.len(), 1);
    assert_eq!(a.seconds("gog:1"), 60);
}

#[test]
fn three_devices_converge() {
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    let mut c = Device::new("c");
    a.data.favourites.insert("gog:1".into());
    b.play("gog:2", t(1), 300);
    c.data.ratings.insert("gog:2".into(), 3);
    c.play("gog:2", t(2), 60);
    for minute in [10, 20] {
        a.sync(t(minute), &[&b, &c]);
        b.sync(t(minute + 1), &[&a, &c]);
        c.sync(t(minute + 2), &[&a, &b]);
    }
    for d in [&a, &b, &c] {
        assert!(d.data.favourites.contains("gog:1"));
        assert_eq!(d.data.ratings["gog:2"], 3);
        assert_eq!(d.seconds("gog:2"), 360);
    }
}

#[test]
fn files_from_older_versions_read_with_defaults() {
    let shared: Shared =
        serde_json::from_str(r#"{"registers":{"favourites":{"gog:1":{"value":true,"at":5}}}}"#)
            .unwrap();
    assert_eq!(shared.registers.favourites["gog:1"].by, "");
    assert!(shared.sessions.is_empty());
    assert_eq!(shared.registers.newest(), 5);
}
