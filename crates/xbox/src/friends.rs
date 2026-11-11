//! The account's Xbox friends, through the Xbox Live services Microsoft
//! documents in its Xbox Live REST reference: the people the account
//! follows (`social.xboxlive.com`, `GET /users/xuid(<xuid>)/people`), their
//! gamertags and pictures (`profile.xboxlive.com`,
//! `POST /users/batch/profile/settings`) and whether they are online and
//! what they play (`userpresence.xboxlive.com`, `POST /users/batch`). All
//! three read with the account's XSTS token; nothing is written.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use crate::Session;
use crate::api::Client;

/// Users asked about in one batch request.
const BATCH: usize = 100;

/// Shell titles presence reports for someone who isn't playing anything.
const SHELL_TITLES: &[&str] = &["Home", "Xbox App", "Xbox", "Online"];

/// One person the account follows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Person {
    pub xuid: String,
    /// They follow the account back (a friend both ways).
    pub follows_back: bool,
    /// The account marked them a favourite.
    pub favorite: bool,
    pub gamertag: String,
    /// Their gamerpic, an `https` address; empty when none was sent.
    pub picture: String,
    pub presence: Presence,
}

/// Whether someone is online, as presence reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnlineState {
    Online,
    Away,
    #[default]
    Offline,
}

/// Someone's presence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presence {
    pub state: OnlineState,
    /// The game they play, when it isn't the console's home or the Xbox
    /// app.
    pub playing: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PeopleAnswer {
    people: Vec<RawPerson>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawPerson {
    xuid: String,
    is_favorite: bool,
    is_following_caller: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ProfileAnswer {
    profile_users: Vec<ProfileUser>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProfileUser {
    id: String,
    settings: Vec<ProfileSetting>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProfileSetting {
    id: String,
    value: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPresence {
    xuid: String,
    state: String,
    devices: Vec<RawDevice>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDevice {
    titles: Vec<RawTitle>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTitle {
    name: String,
    state: String,
    placement: String,
}

fn people(answer: PeopleAnswer) -> Vec<Person> {
    answer
        .people
        .into_iter()
        .filter(|p| !p.xuid.is_empty() && p.xuid.bytes().all(|b| b.is_ascii_digit()))
        .map(|p| Person {
            xuid: p.xuid,
            follows_back: p.is_following_caller,
            favorite: p.is_favorite,
            ..Default::default()
        })
        .collect()
}

/// Gamertag (the modern one when set) and picture per xuid.
fn profiles(answer: ProfileAnswer) -> HashMap<String, (String, String)> {
    answer
        .profile_users
        .into_iter()
        .map(|u| {
            let get = |id: &str| {
                u.settings
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.value.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            let gamertag = get("ModernGamertag").or_else(|| get("Gamertag"));
            let picture = get("GameDisplayPicRaw")
                .map(|p| crate::model::https(&p))
                .filter(|p| p.starts_with("https://"));
            (
                u.id.clone(),
                (gamertag.unwrap_or_default(), picture.unwrap_or_default()),
            )
        })
        .collect()
}

fn presence(raw: RawPresence) -> (String, Presence) {
    let state = match raw.state.as_str() {
        "Online" => OnlineState::Online,
        "Away" => OnlineState::Away,
        _ => OnlineState::Offline,
    };
    let playing = (state != OnlineState::Offline)
        .then(|| {
            raw.devices
                .iter()
                .flat_map(|d| &d.titles)
                .filter(|t| t.state.eq_ignore_ascii_case("active"))
                .filter(|t| !t.placement.eq_ignore_ascii_case("background"))
                .map(|t| t.name.trim())
                .find(|n| !n.is_empty() && !SHELL_TITLES.contains(n))
                .map(str::to_string)
        })
        .flatten();
    (raw.xuid, Presence { state, playing })
}

impl Client {
    /// The people the account follows, with their gamertags, pictures and
    /// presence. Read only.
    pub async fn friends(&self, session: &Session, language: &str) -> Result<Vec<Person>> {
        let url = format!(
            "{}/users/xuid({})/people",
            self.endpoints.social, session.xuid
        );
        let answer: PeopleAnswer = self
            .data_with_contract(self.http.get(url), session, language, "your friends", "1")
            .await?;
        let mut list = people(answer);
        let xuids: Vec<String> = list.iter().map(|p| p.xuid.clone()).collect();
        let mut names = HashMap::new();
        let mut states = HashMap::new();
        for chunk in xuids.chunks(BATCH) {
            let answer: ProfileAnswer = self
                .data_with_contract(
                    self.http
                        .post(format!(
                            "{}/users/batch/profile/settings",
                            self.endpoints.profile
                        ))
                        .json(&serde_json::json!({
                            "userIds": chunk,
                            "settings": ["Gamertag", "ModernGamertag", "GameDisplayPicRaw"],
                        })),
                    session,
                    language,
                    "your friends' profiles",
                    "2",
                )
                .await?;
            names.extend(profiles(answer));
            let answer: Vec<RawPresence> = self
                .data_with_contract(
                    self.http
                        .post(format!("{}/users/batch", self.endpoints.presence))
                        .json(&serde_json::json!({
                            "users": chunk,
                            "level": "title",
                            "onlineOnly": false,
                        })),
                    session,
                    language,
                    "your friends' presence",
                    "3",
                )
                .await?;
            states.extend(answer.into_iter().map(presence));
        }
        for person in &mut list {
            if let Some((gamertag, picture)) = names.remove(&person.xuid) {
                person.gamertag = gamertag;
                person.picture = picture;
            }
            if let Some(p) = states.remove(&person.xuid) {
                person.presence = p;
            }
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use irori_depot::test_server::{Resp, TestServer};

    use super::*;
    use crate::Endpoints;

    fn session() -> Session {
        Session {
            user_hash: "hash-1".into(),
            token: "xsts-token".into(),
            xuid: "2500000000000001".into(),
            gamertag: "Pilot".into(),
            not_after: i64::MAX,
        }
    }

    /// The three services, answering in the shapes Microsoft documents.
    fn server() -> TestServer {
        TestServer::start(|req| {
            assert_eq!(
                req.header("Authorization"),
                Some("XBL3.0 x=hash-1;xsts-token")
            );
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/users/xuid(2500000000000001)/people") => {
                    assert_eq!(req.header("x-xbl-contract-version"), Some("1"));
                    Resp::json(serde_json::json!({"totalCount": 4, "people": [
                        {"xuid": "2533274800000001", "isFavorite": true, "isFollowingCaller": true,
                         "socialNetworks": ["LegacyXboxLive"]},
                        {"xuid": "2533274800000002", "isFavorite": false, "isFollowingCaller": false},
                        {"xuid": "2533274800000003", "isFavorite": false, "isFollowingCaller": true},
                        {"xuid": "../people", "isFavorite": false, "isFollowingCaller": true}
                    ]}))
                }
                ("POST", "/users/batch/profile/settings") => {
                    assert_eq!(req.header("x-xbl-contract-version"), Some("2"));
                    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                    assert_eq!(body["userIds"].as_array().unwrap().len(), 3);
                    Resp::json(serde_json::json!({"profileUsers": [
                        {"id": "2533274800000001", "settings": [
                            {"id": "Gamertag", "value": "Old Tag"},
                            {"id": "ModernGamertag", "value": "Wanderer"},
                            {"id": "GameDisplayPicRaw", "value": "http://images-eds.xboxlive.com/image?url=a"}]},
                        {"id": "2533274800000002", "settings": [
                            {"id": "Gamertag", "value": "Racer"},
                            {"id": "GameDisplayPicRaw", "value": "javascript:alert(1)"}]}
                    ]}))
                }
                ("POST", "/users/batch") => {
                    assert_eq!(req.header("x-xbl-contract-version"), Some("3"));
                    Resp::json(serde_json::json!([
                        {"xuid": "2533274800000001", "state": "Online", "devices": [
                            {"type": "Scarlett", "titles": [
                                {"id": "750323071", "name": "Home", "placement": "Background", "state": "Active"},
                                {"id": "1234", "name": "Forza Horizon 5", "placement": "Full", "state": "Active"}]}]},
                        {"xuid": "2533274800000002", "state": "Online", "devices": [
                            {"type": "WindowsOneCore", "titles": [
                                {"id": "1", "name": "Xbox App", "placement": "Full", "state": "Active"}]}]},
                        {"xuid": "2533274800000003", "state": "Offline"}
                    ]))
                }
                _ => Resp::status(404),
            }
        })
    }

    #[tokio::test]
    async fn reads_friends_with_profiles_and_presence() {
        let s = server();
        let client = Client::with_endpoints("c", Endpoints::all_at(&s.base())).unwrap();
        let friends = client.friends(&session(), "en-US").await.unwrap();
        assert_eq!(friends.len(), 3, "an xuid that isn't a number is dropped");
        let first = &friends[0];
        assert_eq!(first.gamertag, "Wanderer");
        assert!(first.favorite && first.follows_back);
        assert_eq!(first.picture, "https://images-eds.xboxlive.com/image?url=a");
        assert_eq!(first.presence.state, OnlineState::Online);
        assert_eq!(first.presence.playing.as_deref(), Some("Forza Horizon 5"));
        let second = &friends[1];
        assert_eq!(
            (second.gamertag.as_str(), second.picture.as_str()),
            ("Racer", "")
        );
        assert_eq!(second.presence.playing, None, "the Xbox app isn't a game");
        let third = &friends[2];
        assert_eq!(third.gamertag, "");
        assert_eq!(third.presence, Presence::default());
    }
}
