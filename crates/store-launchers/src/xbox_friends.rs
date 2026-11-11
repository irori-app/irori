//! The Xbox account's friends for the Friends window: the people it
//! follows, with their gamertag, picture, online state and the game they
//! play. Read only while the window asks, kept in memory for [`MAX_AGE`],
//! and dropped when the account signs out. Nothing is written to Xbox.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use irori_xbox_api::{OnlineState, Person};

/// How long a read list is shown before Xbox is asked again.
pub const MAX_AGE: Duration = Duration::from_secs(60);

/// One Xbox friend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XboxFriend {
    pub xuid: String,
    pub gamertag: String,
    /// Their gamerpic, an `https` address; empty when Xbox sent none.
    pub picture: String,
    pub state: OnlineState,
    /// The game they play.
    pub playing: Option<String>,
    /// They follow the account back.
    pub follows_back: bool,
    pub favorite: bool,
}

/// The account's friends as last read.
#[derive(Debug, Clone, Default)]
pub struct XboxFriends {
    /// Playing first, then online, away and offline; favourites first
    /// within each, then by gamertag.
    pub list: Vec<XboxFriend>,
    /// When Xbox was asked.
    pub read_at: Option<Instant>,
}

impl XboxFriends {
    pub fn is_fresh(&self) -> bool {
        self.read_at.is_some_and(|t| t.elapsed() < MAX_AGE)
    }
}

/// The friends list from what Xbox sent, sorted.
pub(crate) fn from_people(people: Vec<Person>) -> Vec<XboxFriend> {
    let mut list: Vec<XboxFriend> = people
        .into_iter()
        .map(|p| XboxFriend {
            gamertag: if p.gamertag.is_empty() {
                p.xuid.clone()
            } else {
                p.gamertag
            },
            xuid: p.xuid,
            picture: if p.picture.starts_with("https://") {
                p.picture
            } else {
                String::new()
            },
            state: p.presence.state,
            playing: p.presence.playing,
            follows_back: p.follows_back,
            favorite: p.favorite,
        })
        .collect();
    list.sort_by(order);
    list
}

fn order(a: &XboxFriend, b: &XboxFriend) -> Ordering {
    let rank = |f: &XboxFriend| match (&f.playing, f.state) {
        (Some(_), _) => 0,
        (None, OnlineState::Online) => 1,
        (None, OnlineState::Away) => 2,
        (None, OnlineState::Offline) => 3,
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| b.favorite.cmp(&a.favorite))
        .then_with(|| a.gamertag.to_lowercase().cmp(&b.gamertag.to_lowercase()))
        .then_with(|| a.xuid.cmp(&b.xuid))
}

#[cfg(test)]
mod tests {
    use irori_xbox_api::Presence;

    use super::*;

    fn person(xuid: &str, tag: &str, state: OnlineState, playing: Option<&str>) -> Person {
        Person {
            xuid: xuid.into(),
            gamertag: tag.into(),
            presence: Presence {
                state,
                playing: playing.map(str::to_string),
            },
            ..Default::default()
        }
    }

    #[test]
    fn sorts_players_then_online_then_the_rest() {
        let mut fav = person("4", "zed", OnlineState::Online, None);
        fav.favorite = true;
        fav.picture = "javascript:alert(1)".into();
        let list = from_people(vec![
            person("1", "amy", OnlineState::Offline, None),
            person("2", "Bo", OnlineState::Online, None),
            person("3", "", OnlineState::Away, None),
            fav,
            person("5", "Cy", OnlineState::Online, Some("Halo Infinite")),
        ]);
        let tags: Vec<&str> = list.iter().map(|f| f.gamertag.as_str()).collect();
        assert_eq!(tags, ["Cy", "zed", "Bo", "3", "amy"]);
        assert_eq!(list[1].picture, "");
        let friends = XboxFriends {
            list,
            read_at: Some(Instant::now()),
        };
        assert!(friends.is_fresh());
        assert!(!XboxFriends::default().is_fresh());
    }
}
