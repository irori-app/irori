//! The signed-in account's Steam friends, with their online state and the
//! game they are playing, for the Friends window. Read only on request
//! (the window asks while it is open) and kept for [`MAX_AGE`].
//!
//! Actions are `steam://` links the Steam client handles itself: its friends
//! list (where "Invite to game" lives while a game runs; Steam has no link
//! that invites), a chat with one friend, and joining a friend's lobby
//! (`steam://joinlobby/<app>/<lobby>/<friend>`, the link of Steam's own
//! "Join game" button) or game server (`steam://connect/<ip:port>`) when
//! Steam reports one.

use std::cmp::Ordering;
use std::net::SocketAddrV4;
use std::time::{Duration, Instant};

use irori_steam_api::PlayerSummary;

/// How long a read list is shown before Steam is asked again.
pub const MAX_AGE: Duration = Duration::from_secs(60);

/// What a friend is doing, as Steam reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FriendState {
    Online,
    Busy,
    Away,
    Offline,
}

impl FriendState {
    fn from_persona(state: u64) -> Self {
        match state {
            0 => Self::Offline,
            2 => Self::Busy,
            3 | 4 => Self::Away,
            _ => Self::Online,
        }
    }
}

/// One friend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamFriend {
    /// SteamID64.
    pub steamid: u64,
    pub name: String,
    /// The avatar, an `https` address; empty when Steam sent none.
    pub avatar: String,
    /// The profile page, an `https` address; empty when Steam sent none.
    pub profile: String,
    pub state: FriendState,
    /// The game being played: its Steam app ID (0 for a non-Steam game)
    /// and its name.
    pub playing: Option<(u32, String)>,
    /// How to join them, when Steam reports a lobby or a server.
    pub join: Option<FriendLink>,
}

/// A `steam://` link of the friends list, built only from numbers Steam
/// sent, so nothing else can reach the Steam client through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendLink {
    /// Steam's friends list.
    FriendsList,
    /// A chat with one friend.
    Message(u64),
    /// A friend's lobby in a game.
    JoinLobby { app: u32, lobby: u64, friend: u64 },
    /// The game server a friend plays on.
    Connect(SocketAddrV4),
}

impl FriendLink {
    pub fn url(self) -> String {
        match self {
            Self::FriendsList => "steam://open/friends".into(),
            Self::Message(friend) => format!("steam://friends/message/{friend}"),
            Self::JoinLobby { app, lobby, friend } => {
                format!("steam://joinlobby/{app}/{lobby}/{friend}")
            }
            Self::Connect(addr) => format!("steam://connect/{addr}"),
        }
    }
}

/// The account's friends as last read.
#[derive(Debug, Clone, Default)]
pub struct Friends {
    /// Playing first, then online, then the rest; by name within each.
    pub list: Vec<SteamFriend>,
    /// When Steam was asked.
    pub read_at: Option<Instant>,
}

impl Friends {
    pub fn is_fresh(&self) -> bool {
        self.read_at.is_some_and(|t| t.elapsed() < MAX_AGE)
    }

    /// Friends playing the Steam app `app`.
    pub fn playing(&self, app: u32) -> impl Iterator<Item = &SteamFriend> {
        self.list
            .iter()
            .filter(move |f| matches!(f.playing, Some((a, _)) if a == app && a != 0))
    }
}

/// An `https` address, else nothing: avatars and profiles are opened and
/// downloaded, so nothing else is kept.
fn web(url: String) -> String {
    if url.starts_with("https://") {
        url
    } else {
        String::new()
    }
}

/// The friends list from Steam's summaries, sorted.
pub(crate) fn from_summaries(players: Vec<PlayerSummary>) -> Vec<SteamFriend> {
    let mut list: Vec<SteamFriend> = players.into_iter().map(friend).collect();
    list.sort_by(order);
    list
}

fn friend(p: PlayerSummary) -> SteamFriend {
    let name = p.gameextrainfo.trim().to_string();
    let app = u32::try_from(p.gameid).unwrap_or(0);
    let playing = (!name.is_empty() || app != 0).then_some((app, name));
    let server = p
        .gameserverip
        .trim()
        .parse::<SocketAddrV4>()
        .ok()
        .filter(|a| !a.ip().is_unspecified() && a.port() != 0);
    let join = match (playing.as_ref(), p.lobbysteamid, server) {
        (Some((app, _)), lobby, _) if *app != 0 && lobby != 0 => Some(FriendLink::JoinLobby {
            app: *app,
            lobby,
            friend: p.steamid,
        }),
        (Some(_), _, Some(addr)) => Some(FriendLink::Connect(addr)),
        _ => None,
    };
    let state = FriendState::from_persona(p.personastate);
    SteamFriend {
        steamid: p.steamid,
        name: if p.personaname.trim().is_empty() {
            p.steamid.to_string()
        } else {
            p.personaname.trim().to_string()
        },
        avatar: web(p.avatarmedium),
        profile: web(p.profileurl),
        state,
        playing,
        join,
    }
}

fn order(a: &SteamFriend, b: &SteamFriend) -> Ordering {
    let rank = |f: &SteamFriend| match (&f.playing, f.state) {
        (Some(_), _) => 0,
        (None, FriendState::Online | FriendState::Busy) => 1,
        (None, FriendState::Away) => 2,
        (None, FriendState::Offline) => 3,
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.steamid.cmp(&b.steamid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(json: serde_json::Value) -> PlayerSummary {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn sorts_players_then_online_then_offline() {
        let list = from_summaries(vec![
            summary(serde_json::json!({"steamid": "3", "personaname": "zed", "personastate": 0})),
            summary(serde_json::json!({"steamid": "4", "personaname": "Amy", "personastate": 3})),
            summary(serde_json::json!({"steamid": "1", "personaname": "Bo", "personastate": 1})),
            summary(serde_json::json!({
                "steamid": "2", "personaname": "Cy", "personastate": 1,
                "gameid": "620", "gameextrainfo": "Portal 2"
            })),
            summary(serde_json::json!({"steamid": "5", "personastate": 1})),
        ]);
        let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Cy", "5", "Bo", "Amy", "zed"]);
        assert_eq!(list[0].playing, Some((620, "Portal 2".into())));
        assert_eq!(list[3].state, FriendState::Away);
        let friends = Friends {
            list,
            read_at: Some(Instant::now()),
        };
        assert!(friends.is_fresh());
        assert_eq!(friends.playing(620).count(), 1);
        assert_eq!(friends.playing(0).count(), 0);
    }

    #[test]
    fn offers_to_join_only_what_steam_reports() {
        let lobby = friend(summary(serde_json::json!({
            "steamid": "76561197960265730", "personastate": 1,
            "gameid": "730", "gameextrainfo": "Counter-Strike 2",
            "gameserverip": "0.0.0.0:0", "lobbysteamid": "109775241047998838"
        })));
        assert_eq!(
            lobby.join.unwrap().url(),
            "steam://joinlobby/730/109775241047998838/76561197960265730"
        );
        let server = friend(summary(serde_json::json!({
            "steamid": "2", "personastate": 1, "gameid": "440",
            "gameextrainfo": "Team Fortress 2", "gameserverip": "203.0.113.5:27015"
        })));
        assert_eq!(
            server.join.unwrap().url(),
            "steam://connect/203.0.113.5:27015"
        );
        let solo = friend(summary(serde_json::json!({
            "steamid": "3", "personastate": 1, "gameid": "620",
            "gameextrainfo": "Portal 2", "gameserverip": "0.0.0.0:0"
        })));
        assert_eq!(solo.join, None);
        let idle = friend(summary(serde_json::json!({
            "steamid": "4", "personastate": 1, "lobbysteamid": "5",
            "gameserverip": "203.0.113.5:27015; rm -rf /"
        })));
        assert_eq!((idle.playing, idle.join), (None, None));
        assert_eq!(FriendLink::FriendsList.url(), "steam://open/friends");
        assert_eq!(FriendLink::Message(42).url(), "steam://friends/message/42");
    }

    #[test]
    fn keeps_only_web_addresses() {
        let f = friend(summary(serde_json::json!({
            "steamid": "1",
            "avatarmedium": "javascript:alert(1)",
            "profileurl": "https://steamcommunity.com/id/one/"
        })));
        assert_eq!(f.avatar, "");
        assert_eq!(f.profile, "https://steamcommunity.com/id/one/");
    }
}
