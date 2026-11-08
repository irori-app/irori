//! Discord Rich Presence: "Playing <game>" while a game Irori started runs,
//! through Discord's local IPC socket (`discord-ipc-N`; the
//! `discord-rich-presence` crate finds it in `XDG_RUNTIME_DIR` and the
//! Flatpak and Snap folders, or as the Windows named pipe).
//!
//! One worker task owns the connection. It wakes only when a session starts
//! or ends, and while a game runs and Discord can't be reached it tries again
//! every [`RETRY`], so a Discord started after the game still shows it.
//! Nothing runs, and no socket is open, while no game is shown. The IPC is
//! blocking, so every call to it runs on the blocking pool. A launch never
//! waits for Discord.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc as _, DiscordIpcClient};
use irori_core::{GameId, GameInfo, Runner};
use parking_lot::Mutex;
use tokio::sync::watch;

/// How long to wait before looking for Discord again while a game runs.
const RETRY: Duration = Duration::from_secs(60);

/// The art asset of Irori's Discord application shown as the small image.
const IRORI_ASSET: &str = "irori";

/// Discord's limit for an activity's texts and a button's address.
const MAX_TEXT: usize = 128;
const MAX_URL: usize = 512;

/// What Discord shows for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    /// The Discord application the activity belongs to.
    pub application: String,
    pub title: String,
    /// The store, for games that come from one.
    pub store: Option<&'static str>,
    /// Unix time in milliseconds.
    pub started_ms: i64,
    /// An `https` image of the game, which Discord shows as the large image.
    pub cover: Option<String>,
    /// The game's store page, offered as a button.
    pub store_url: Option<String>,
}

impl Shown {
    pub(crate) fn new(application: String, id: &GameId, game: &GameInfo, started_ms: i64) -> Self {
        let store = match id.runner {
            Runner::Sideload | Runner::Emulation | Runner::Launchers => None,
            runner => Some(runner.store_name()),
        };
        let web = |url: &Option<String>, max: usize| {
            url.as_deref()
                .map(str::trim)
                .filter(|u| u.starts_with("https://") && u.len() <= max)
                .map(str::to_string)
        };
        Self {
            application,
            title: clip(game.title.trim(), MAX_TEXT),
            store,
            started_ms,
            // Discord fetches external images through its media proxy; local
            // files and plain http can't be shown.
            cover: web(&game.art_square, MAX_URL)
                .or_else(|| web(&game.art_cover, MAX_URL))
                .or_else(|| web(&game.art_icon, MAX_URL)),
            store_url: web(&game.store_url, MAX_URL),
        }
    }

    /// The activity Discord is sent.
    fn activity(&self) -> Activity<'_> {
        let mut assets = Assets::new().small_image(IRORI_ASSET).small_text("Irori");
        if let Some(cover) = &self.cover {
            assets = assets
                .large_image(cover.as_str())
                .large_text(self.title.as_str());
        }
        let mut activity = Activity::new()
            .activity_type(ActivityType::Playing)
            .details(self.title.as_str())
            .timestamps(Timestamps::new().start(self.started_ms))
            .assets(assets);
        if let Some(store) = self.store {
            activity = activity.state(store);
        }
        if let (Some(url), Some(store)) = (&self.store_url, self.store) {
            // A store's name labels its own page in every language.
            activity = activity.buttons(vec![Button::new(store, url.as_str())]);
        }
        activity
    }
}

/// `text` cut to at most `max` bytes on a character boundary.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// The sessions Discord could show, latest last. The latest one is shown;
/// when it ends, the one before it comes back.
#[derive(Debug, Default)]
struct Sessions(Vec<(GameId, Shown)>);

impl Sessions {
    fn start(&mut self, id: GameId, shown: Shown) {
        self.0.retain(|(g, _)| g != &id);
        self.0.push((id, shown));
    }

    fn end(&mut self, id: &GameId) {
        self.0.retain(|(g, _)| g != id);
    }

    fn top(&self) -> Option<Shown> {
        self.0.last().map(|(_, s)| s.clone())
    }
}

/// Rich Presence for the backend's sessions. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Presence {
    sessions: Arc<Mutex<Sessions>>,
    want: Arc<watch::Sender<Option<Shown>>>,
    worker: Arc<AtomicBool>,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            want: Arc::new(watch::channel(None).0),
            worker: Arc::default(),
        }
    }
}

impl Presence {
    /// Show `shown` for `id` from now on. Must run inside the Tokio runtime;
    /// returns at once.
    pub(crate) fn playing(&self, id: GameId, shown: Shown) {
        let top = {
            let mut sessions = self.sessions.lock();
            sessions.start(id, shown);
            sessions.top()
        };
        self.want.send_replace(top);
        if !self.worker.swap(true, Ordering::SeqCst) {
            tokio::spawn(run(self.want.subscribe()));
        }
    }

    /// The session of `id` ended: show the one before it, or nothing.
    pub(crate) fn ended(&self, id: &GameId) {
        let top = {
            let mut sessions = self.sessions.lock();
            sessions.end(id);
            sessions.top()
        };
        self.want.send_replace(top);
    }
}

/// The connection to Discord, with the application it was opened for.
type Connection = Option<(String, DiscordIpcClient)>;

async fn run(mut want: watch::Receiver<Option<Shown>>) {
    let mut connection: Connection = None;
    loop {
        let wanted = want.borrow_and_update().clone();
        let (kept, done) = tokio::task::spawn_blocking(move || apply(connection, wanted))
            .await
            .unwrap_or((None, false));
        connection = kept;
        if done {
            if want.changed().await.is_err() {
                return;
            }
        } else {
            tokio::select! {
                changed = want.changed() => if changed.is_err() { return },
                _ = tokio::time::sleep(RETRY) => {}
            }
        }
    }
}

/// Bring Discord in line with `wanted`. Returns the connection to keep and
/// whether Discord shows what was wanted.
fn apply(connection: Connection, wanted: Option<Shown>) -> (Connection, bool) {
    let Some(shown) = wanted else {
        if let Some((_, mut client)) = connection {
            let _ = client.clear_activity();
            let _ = client.close();
        }
        return (None, true);
    };
    let mut client = match connection {
        Some((app, client)) if app == shown.application => client,
        other => {
            if let Some((_, mut old)) = other {
                let _ = old.close();
            }
            let mut client = DiscordIpcClient::new(&shown.application);
            if let Err(e) = client.connect() {
                tracing::debug!("Discord isn't reachable: {e}");
                return (None, false);
            }
            client
        }
    };
    match client.set_activity(shown.activity()) {
        Ok(()) => (Some((shown.application.clone(), client)), true),
        Err(e) => {
            tracing::debug!("Discord presence: {e}");
            let _ = client.close();
            (None, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(title: &str) -> GameInfo {
        GameInfo {
            title: title.into(),
            ..GameInfo::default()
        }
    }

    fn id(runner: Runner, app: &str) -> GameId {
        GameId::new(runner, app)
    }

    #[test]
    fn shows_the_game_with_its_cover_and_store_page() {
        let mut info = game("Hades");
        info.art_cover = Some("https://images.gog.com/hades_cover.jpg".into());
        info.art_square = Some("file:///home/me/hades.png".into());
        info.store_url = Some("https://www.gog.com/game/hades".into());
        let shown = Shown::new("42".into(), &id(Runner::Gog, "1"), &info, 1_700_000_000_000);
        assert_eq!(
            shown.cover.as_deref(),
            Some("https://images.gog.com/hades_cover.jpg"),
            "local files can't be shown"
        );
        let json = serde_json::to_value(shown.activity()).unwrap();
        assert_eq!(json["details"], "Hades");
        assert_eq!(json["state"], "GOG");
        assert_eq!(json["type"], 0);
        assert_eq!(json["timestamps"]["start"], 1_700_000_000_000i64);
        assert_eq!(
            json["assets"]["large_image"],
            "https://images.gog.com/hades_cover.jpg"
        );
        assert_eq!(json["assets"]["small_image"], "irori");
        assert_eq!(json["buttons"][0]["label"], "GOG");
        assert_eq!(json["buttons"][0]["url"], "https://www.gog.com/game/hades");
    }

    #[test]
    fn added_games_show_only_their_title() {
        let mut info = game("My game");
        info.store_url = Some("https://example.com".into());
        info.art_cover = Some("http://example.com/cover.png".into());
        let shown = Shown::new("42".into(), &id(Runner::Sideload, "x"), &info, 0);
        let json = serde_json::to_value(shown.activity()).unwrap();
        assert_eq!(json["details"], "My game");
        assert!(json.get("state").is_none());
        assert!(json.get("buttons").is_none());
        assert!(json["assets"].get("large_image").is_none());
    }

    #[test]
    fn long_titles_are_cut_on_a_character() {
        let title = "é".repeat(100);
        let shown = Shown::new("1".into(), &id(Runner::Gog, "1"), &game(&title), 0);
        assert!(shown.title.len() <= MAX_TEXT);
        assert!(shown.title.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_latest_session_shows_and_the_previous_one_returns() {
        let a = id(Runner::Gog, "a");
        let b = id(Runner::Steam, "b");
        let shown = |t: &str| Shown::new("1".into(), &a, &game(t), 0);
        let mut s = Sessions::default();
        assert_eq!(s.top(), None);
        s.start(a.clone(), shown("A"));
        s.start(b.clone(), shown("B"));
        assert_eq!(s.top().unwrap().title, "B");
        s.end(&b);
        assert_eq!(s.top().unwrap().title, "A");
        s.end(&b);
        assert_eq!(s.top().unwrap().title, "A");
        s.end(&a);
        assert_eq!(s.top(), None);
    }
}
