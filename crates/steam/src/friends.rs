//! The account's friends: who they are (`IFriendsListService/GetFriendsList`)
//! and what they are doing (`IPlayerService/GetPlayerLinkDetails`).
//!
//! Both are Steam's service interfaces, which take the signed-in account's
//! access token as the library calls do. The older `ISteamUser` calls
//! (`GetFriendList`, `GetPlayerSummaries`) accept only a Web API key, which
//! Irori has none of to ship, and answer every token with HTTP 400. A
//! refusal here means Steam won't share the list with that token, not that
//! the sign-in stopped working, so it is [`SteamError::NotShared`] rather
//! than [`SteamError::SessionRejected`].

use anyhow::Result;
use base64::Engine as _;
use serde::Deserialize;

use crate::api::Client;
use crate::error::SteamError;
use crate::library::lenient_u64;

/// Accounts asked for in one `GetPlayerLinkDetails` call.
const SUMMARIES_PER_CALL: usize = 100;

/// `EFriendRelationship::Friend`.
const FRIEND: u64 = 3;

/// The avatar Steam shows for an account without one.
const DEFAULT_AVATAR: &str = "fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb";

/// One account as the friends window shows it. Game fields are only there
/// while the account plays and its game details are visible.
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

/// `GetPlayerLinkDetails`' account: what anyone may see, and what this
/// account may see of a friend.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkDetails {
    public_data: PublicData,
    private_data: PrivateData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PublicData {
    #[serde(deserialize_with = "lenient_u64")]
    steamid: u64,
    /// The avatar's SHA-1, base64.
    sha_digest_avatar: String,
    persona_name: String,
    profile_url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PrivateData {
    #[serde(deserialize_with = "lenient_u64")]
    persona_state: u64,
    #[serde(deserialize_with = "lenient_u64")]
    game_id: u64,
    /// IPv4 address as a number, network order.
    #[serde(deserialize_with = "lenient_u64")]
    game_server_ip_address: u64,
    #[serde(deserialize_with = "lenient_u64")]
    game_server_port: u64,
    game_extra_info: String,
    #[serde(deserialize_with = "lenient_u64")]
    lobby_steam_id: u64,
}

impl LinkDetails {
    fn into_summary(self) -> PlayerSummary {
        let p = self.public_data;
        let q = self.private_data;
        let server = match (u32::try_from(q.game_server_ip_address), q.game_server_port) {
            (Ok(ip), port @ 1..=65535) if ip != 0 => {
                format!("{}:{port}", std::net::Ipv4Addr::from(ip))
            }
            _ => "0.0.0.0:0".to_string(),
        };
        let profileurl = match p.profile_url.trim() {
            "" => format!("https://steamcommunity.com/profiles/{}/", p.steamid),
            custom => format!("https://steamcommunity.com/id/{custom}/"),
        };
        PlayerSummary {
            steamid: p.steamid,
            personaname: p.persona_name,
            profileurl,
            avatarmedium: avatar_url(&p.sha_digest_avatar),
            personastate: q.persona_state,
            gameid: q.game_id,
            gameextrainfo: q.game_extra_info,
            gameserverip: server,
            lobbysteamid: q.lobby_steam_id,
        }
    }
}

/// The 64 × 64 avatar named by its base64 SHA-1, Steam's default one when
/// there is none.
fn avatar_url(digest: &str) -> String {
    let hex = base64::engine::general_purpose::STANDARD
        .decode(digest.trim())
        .ok()
        .filter(|b| b.len() == 20 && b.iter().any(|&x| x != 0))
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
        .unwrap_or_else(|| DEFAULT_AVATAR.to_string());
    format!("https://avatars.steamstatic.com/{hex}_medium.jpg")
}

impl Client {
    /// SteamID64s of the friends of the account the token signs in, without
    /// `steamid` (the account itself).
    pub async fn friend_list(&self, access_token: &str, steamid: u64) -> Result<Vec<u64>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Answer {
            response: Response,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Response {
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
            ulfriendid: u64,
            #[serde(deserialize_with = "lenient_u64")]
            efriendrelationship: u64,
        }
        let url = format!(
            "{}/IFriendsListService/GetFriendsList/v1/",
            self.endpoints().api
        );
        let query = [("access_token", access_token.to_string())];
        let answer: Answer = self
            .get_json(url, &query, "your friends list")
            .await
            .map_err(not_shared)?;
        Ok(answer
            .response
            .friendslist
            .friends
            .into_iter()
            .filter(|f| f.efriendrelationship == FRIEND && f.ulfriendid != 0)
            .map(|f| f.ulfriendid)
            .filter(|&id| id != steamid)
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
            response: Accounts,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Accounts {
            accounts: Vec<LinkDetails>,
        }
        let url = format!(
            "{}/IPlayerService/GetPlayerLinkDetails/v1/",
            self.endpoints().api
        );
        let mut out = Vec::with_capacity(steamids.len());
        for batch in steamids.chunks(SUMMARIES_PER_CALL) {
            let keys: Vec<String> = (0..batch.len()).map(|i| format!("steamids[{i}]")).collect();
            let mut query = vec![("access_token", access_token.to_string())];
            query.extend(
                keys.iter()
                    .zip(batch)
                    .map(|(key, id)| (key.as_str(), id.to_string())),
            );
            let answer: Answer = self
                .get_json(url.clone(), &query, "your friends' status")
                .await
                .map_err(not_shared)?;
            out.extend(
                answer
                    .response
                    .accounts
                    .into_iter()
                    .map(LinkDetails::into_summary)
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
            if path.starts_with("/IFriendsListService/GetFriendsList/v1/?") {
                return Resp::json(json!({"response": {"friendslist": {
                    "bincremental": false,
                    "friends": [
                        {"ulfriendid": "76561197960265730", "efriendrelationship": 3},
                        {"ulfriendid": "76561197960265731", "efriendrelationship": 3},
                        // Blocked, a pending invite, nobody, and the account itself.
                        {"ulfriendid": "76561197960265732", "efriendrelationship": 1},
                        {"ulfriendid": "76561197960265733", "efriendrelationship": 2},
                        {"ulfriendid": "0", "efriendrelationship": 3},
                        {"ulfriendid": ME.to_string(), "efriendrelationship": 3}
                    ]
                }}}));
            }
            if path.starts_with("/IPlayerService/GetPlayerLinkDetails/v1/?") {
                return Resp::json(json!({"response": {"accounts": [
                    {
                        "public_data": {
                            "steamid": "76561197960265730",
                            "visibility_state": 3,
                            "sha_digest_avatar": "q83vEjRWeJCrze8SNFZ4kKvN7xI=",
                            "persona_name": "Ada",
                            "profile_url": "ada"
                        },
                        "private_data": {
                            "persona_state": 1,
                            "game_id": "730",
                            "game_extra_info": "Counter-Strike 2",
                            "game_server_ip_address": 3405803781u32,
                            "game_server_port": 27015,
                            "lobby_steam_id": "109775241047998838"
                        }
                    },
                    {
                        "public_data": {
                            "steamid": "76561197960265731",
                            "sha_digest_avatar": "AAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                            "persona_name": "Bo"
                        }
                    },
                    {"public_data": {"persona_name": "nobody"}}
                ]}}));
            }
            Resp::status(404)
        });
        let api = client(&server);
        let ids = api.friend_list("token", ME).await.unwrap();
        assert_eq!(ids, [76561197960265730, 76561197960265731]);
        assert!(server.requests()[0].path.contains("access_token=token"));

        let players = api.player_summaries("token", &ids).await.unwrap();
        assert_eq!(players.len(), 2);
        let ada = &players[0];
        assert_eq!(ada.personaname, "Ada");
        assert_eq!(ada.profileurl, "https://steamcommunity.com/id/ada/");
        assert_eq!(
            ada.avatarmedium,
            "https://avatars.steamstatic.com/abcdef1234567890abcdef1234567890abcdef12_medium.jpg"
        );
        assert_eq!((ada.personastate, ada.gameid), (1, 730));
        assert_eq!(ada.gameextrainfo, "Counter-Strike 2");
        assert_eq!(ada.gameserverip, "203.0.113.5:27015");
        assert_eq!(ada.lobbysteamid, 109775241047998838);
        let bo = &players[1];
        assert_eq!((bo.personastate, bo.gameid), (0, 0));
        assert_eq!(bo.gameserverip, "0.0.0.0:0");
        assert_eq!(
            bo.profileurl,
            "https://steamcommunity.com/profiles/76561197960265731/"
        );
        assert!(
            bo.avatarmedium.contains(DEFAULT_AVATAR),
            "{}",
            bo.avatarmedium
        );
        let path = &server.requests()[1].path;
        for part in [
            "steamids%5B0%5D=76561197960265730",
            "steamids%5B1%5D=76561197960265731",
        ] {
            assert!(path.contains(part), "{path} lacks {part}");
        }

        let e = api.friend_list("refused", ME).await.unwrap_err();
        assert_eq!(e.downcast_ref(), Some(&SteamError::NotShared));
        assert!(!format!("{e:#}").contains("refused"), "{e:#}");
    }

    #[tokio::test]
    async fn asks_for_summaries_in_batches() {
        let server = TestServer::start(|_| Resp::json(json!({"response": {}})));
        let api = client(&server);
        let ids: Vec<u64> = (1..=250).collect();
        assert!(api.player_summaries("t", &ids).await.unwrap().is_empty());
        assert_eq!(server.requests().len(), 3);
        assert!(server.requests()[2].path.contains("steamids%5B49%5D=250"));
        assert!(api.player_summaries("t", &[]).await.unwrap().is_empty());
        assert_eq!(server.requests().len(), 3);
    }
}
