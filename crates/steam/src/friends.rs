//! The account's friends: who they are (`ISteamUser/GetFriendList`) and
//! what they are doing (`ISteamUser/GetPlayerSummaries`).
//!
//! Valve documents both with a Web API key; Irori has none to ship, so they
//! are called with the signed-in account's access token, as the library
//! calls are. A refusal (HTTP 401 or 403) here means Steam won't share the
//! list with that token, not that the sign-in stopped working, so it is
//! [`SteamError::NotShared`] rather than [`SteamError::SessionRejected`].

use anyhow::Result;
use serde::Deserialize;

use crate::api::Client;
use crate::error::SteamError;
use crate::library::lenient_u64;

/// `GetPlayerSummaries` answers at most this many accounts at once.
const SUMMARIES_PER_CALL: usize = 100;

/// One account as `GetPlayerSummaries` describes it. Game fields are only
/// there while the account plays and its game details are visible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PlayerSummary {
    #[serde(deserialize_with = "lenient_u64")]
    pub steamid: u64,
    pub personaname: String,
    pub profileurl: String,
    /// 64 × 64 avatar.
    pub avatarmedium: String,
    /// 0 offline, 1 online, 2 busy, 3 away, 4 snooze, 5 looking to trade,
    /// 6 looking to play. A private profile always reads 0.
    #[serde(deserialize_with = "lenient_u64")]
    pub personastate: u64,
    /// The app being played.
    #[serde(deserialize_with = "lenient_u64")]
    pub gameid: u64,
    /// The name of the app being played.
    pub gameextrainfo: String,
    /// `ip:port` of the game server, `0.0.0.0:0` when there is none.
    pub gameserverip: String,
    /// The Steam lobby the account is in.
    #[serde(deserialize_with = "lenient_u64")]
    pub lobbysteamid: u64,
}

impl Client {
    /// SteamID64s of the account's friends.
    pub async fn friend_list(&self, access_token: &str, steamid: u64) -> Result<Vec<u64>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            friendslist: List,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct List {
            friends: Vec<Entry>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Entry {
            #[serde(deserialize_with = "lenient_u64")]
            steamid: u64,
            relationship: String,
        }
        let url = format!("{}/ISteamUser/GetFriendList/v1/", self.endpoints().api);
        let query = [
            ("access_token", access_token.to_string()),
            ("steamid", steamid.to_string()),
            ("relationship", "friend".into()),
            ("format", "json".into()),
        ];
        let answer: Answer = self
            .get_json(url, &query, "your friends list")
            .await
            .map_err(not_shared)?;
        Ok(answer
            .friendslist
            .friends
            .into_iter()
            .filter(|f| f.steamid != 0 && (f.relationship.is_empty() || f.relationship == "friend"))
            .map(|f| f.steamid)
            .collect())
    }

    /// Summaries of `steamids`, asked in batches; accounts Steam doesn't
    /// know are left out.
    pub async fn player_summaries(
        &self,
        access_token: &str,
        steamids: &[u64],
    ) -> Result<Vec<PlayerSummary>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            response: Players,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Players {
            players: Vec<PlayerSummary>,
        }
        let url = format!("{}/ISteamUser/GetPlayerSummaries/v2/", self.endpoints().api);
        let mut out = Vec::with_capacity(steamids.len());
        for batch in steamids.chunks(SUMMARIES_PER_CALL) {
            let ids = batch
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let query = [
                ("access_token", access_token.to_string()),
                ("steamids", ids),
                ("format", "json".into()),
            ];
            let answer: Answer = self
                .get_json(url.clone(), &query, "your friends' status")
                .await
                .map_err(not_shared)?;
            out.extend(
                answer
                    .response
                    .players
                    .into_iter()
                    .filter(|p| p.steamid != 0),
            );
        }
        Ok(out)
    }
}

/// A refused friends call is Steam not sharing it with this sign-in.
fn not_shared(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<SteamError>() {
        Some(SteamError::SessionRejected) => SteamError::NotShared.into(),
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    const ME: u64 = 76561197960265729;

    fn client(server: &TestServer) -> Client {
        Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            assets: server.base(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn reads_friends_and_what_they_play() {
        let server = TestServer::start(|req| {
            let path = req.path.as_str();
            if path.contains("access_token=refused") {
                return Resp::status(403);
            }
            if path.starts_with("/ISteamUser/GetFriendList/v1/?") {
                return Resp::json(json!({"friendslist": {"friends": [
                    {"steamid": "76561197960265730", "relationship": "friend", "friend_since": 1},
                    {"steamid": "76561197960265731", "relationship": "friend", "friend_since": 2},
                    {"steamid": "0", "relationship": "friend"}
                ]}}));
            }
            if path.starts_with("/ISteamUser/GetPlayerSummaries/v2/?") {
                return Resp::json(json!({"response": {"players": [
                    {
                        "steamid": "76561197960265730",
                        "communityvisibilitystate": 3,
                        "personaname": "Ada",
                        "profileurl": "https://steamcommunity.com/id/ada/",
                        "avatarmedium": "https://avatars.steamstatic.com/a_medium.jpg",
                        "personastate": 1,
                        "gameid": "730",
                        "gameextrainfo": "Counter-Strike 2",
                        "gameserverip": "0.0.0.0:0",
                        "lobbysteamid": "109775241047998838"
                    },
                    {"steamid": "76561197960265731", "personaname": "Bo", "personastate": 0}
                ]}}));
            }
            Resp::status(404)
        });
        let api = client(&server);
        let ids = api.friend_list("token", ME).await.unwrap();
        assert_eq!(ids, [76561197960265730, 76561197960265731]);
        let path = &server.requests()[0].path;
        for part in [
            "access_token=token",
            &format!("steamid={ME}"),
            "relationship=friend",
        ] {
            assert!(path.contains(part), "{path} lacks {part}");
        }

        let players = api.player_summaries("token", &ids).await.unwrap();
        assert_eq!(players.len(), 2);
        let ada = &players[0];
        assert_eq!(ada.personaname, "Ada");
        assert_eq!((ada.personastate, ada.gameid), (1, 730));
        assert_eq!(ada.gameextrainfo, "Counter-Strike 2");
        assert_eq!(ada.lobbysteamid, 109775241047998838);
        assert_eq!(players[1].gameid, 0);
        assert!(
            server.requests()[1]
                .path
                .contains("steamids=76561197960265730%2C76561197960265731")
        );

        let e = api.friend_list("refused", ME).await.unwrap_err();
        assert_eq!(e.downcast_ref(), Some(&SteamError::NotShared));
        assert!(!format!("{e:#}").contains("refused"), "{e:#}");
    }

    #[tokio::test]
    async fn asks_for_summaries_in_batches() {
        let server = TestServer::start(|_| Resp::json(json!({"response": {"players": []}})));
        let api = client(&server);
        let ids: Vec<u64> = (1..=250).collect();
        assert!(api.player_summaries("t", &ids).await.unwrap().is_empty());
        assert_eq!(server.requests().len(), 3);
        assert!(api.player_summaries("t", &[]).await.unwrap().is_empty());
        assert_eq!(server.requests().len(), 3);
    }
}
