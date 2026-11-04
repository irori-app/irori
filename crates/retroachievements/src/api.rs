//! [`Client`]: RetroAchievements' Web API (`/API/API_*.php`, signed with the
//! user's Web API key in `y`) and the emulators' login
//! (`/dorequest.php?r=login2`).

use std::fmt;
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::de::DeserializeOwned;

use crate::model::{ConnectLogin, GameEntry, GameProgress, UserProfile};

/// RetroAchievements' site, which serves the Web API.
pub const SITE_BASE: &str = "https://retroachievements.org";

/// Server base addresses (overridable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub site: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            site: SITE_BASE.into(),
        }
    }
}

/// A username and its Web API key. `Debug` leaves the key out.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub api_key: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// RetroAchievements refused a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RaError {
    /// The Web API key (or the password, for the emulators' login) was
    /// refused; only new credentials fix it.
    #[error("RetroAchievements refused the sign-in: {0}")]
    Unauthorized(String),
    #[error("RetroAchievements is busy; try again in a minute")]
    TooManyRequests,
    #[error("RetroAchievements answered with HTTP status {0}")]
    Status(u16),
}

/// A RetroAchievements client. Holds no credentials; cheap to clone.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    site: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        Self::with_endpoints(Endpoints::default())
    }

    /// A client for another server (a test server).
    pub fn with_endpoints(endpoints: Endpoints) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Irori/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .context("Could not set up the connection to RetroAchievements")?;
        Ok(Self {
            http,
            site: endpoints.site.trim_end_matches('/').to_string(),
        })
    }

    async fn web<T: DeserializeOwned>(
        &self,
        method: &str,
        creds: &Credentials,
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut params = vec![("y", creds.api_key.trim().to_string())];
        params.extend(query.iter().cloned());
        let resp = self
            .http
            .get(format!("{}/API/{method}.php", self.site))
            .query(&params)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach RetroAchievements")?;
        let status = resp.status().as_u16();
        tracing::debug!(method, status, "retroachievements call");
        match status {
            200..=299 => {}
            401 | 403 => {
                return Err(
                    RaError::Unauthorized("the Web API key was not accepted".into()).into(),
                );
            }
            429 => return Err(RaError::TooManyRequests.into()),
            s => return Err(RaError::Status(s).into()),
        }
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("RetroAchievements' answer was cut off")?;
        serde_json::from_slice(&body)
            .with_context(|| format!("RetroAchievements sent an unexpected answer for {method}"))
    }

    /// The account of `creds`; fails with [`RaError::Unauthorized`] when the
    /// key is not this user's.
    pub async fn user_profile(&self, creds: &Credentials) -> Result<UserProfile> {
        let profile: UserProfile = self
            .web(
                "API_GetUserProfile",
                creds,
                &[("u", creds.username.trim().to_string())],
            )
            .await?;
        if profile.user.is_empty() {
            return Err(RaError::Unauthorized(format!(
                "there is no user named {}",
                creds.username.trim()
            ))
            .into());
        }
        Ok(profile)
    }

    /// The games of a console that have achievements, with their hashes.
    pub async fn game_list(&self, creds: &Credentials, console: u32) -> Result<Vec<GameEntry>> {
        self.web(
            "API_GetGameList",
            creds,
            &[
                ("i", console.to_string()),
                ("f", "1".into()),
                ("h", "1".into()),
            ],
        )
        .await
    }

    /// A game's achievements with the player's unlocks.
    pub async fn game_progress(&self, creds: &Credentials, game: u64) -> Result<GameProgress> {
        self.web(
            "API_GetGameInfoAndUserProgress",
            creds,
            &[
                ("g", game.to_string()),
                ("u", creds.username.trim().to_string()),
                ("a", "1".into()),
            ],
        )
        .await
    }

    /// Sign in the way emulators do, for the token they keep. The password
    /// goes in the request body, never the address.
    pub async fn connect_login(&self, username: &str, password: &str) -> Result<ConnectLogin> {
        let resp = self
            .http
            .post(format!("{}/dorequest.php", self.site))
            .form(&[("r", "login2"), ("u", username.trim()), ("p", password)])
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach RetroAchievements")?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("RetroAchievements' answer was cut off")?;
        let login: ConnectLogin = serde_json::from_slice(&body).map_err(|_| match status {
            429 => anyhow::Error::from(RaError::TooManyRequests),
            s => RaError::Status(s).into(),
        })?;
        if !login.success || login.token.is_empty() {
            let why = login
                .error
                .clone()
                .unwrap_or_else(|| "the username or password is wrong".into());
            return Err(RaError::Unauthorized(why).into());
        }
        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    const KEY: &str = "AbCdEf0123456789AbCdEf0123456789";

    fn creds() -> Credentials {
        Credentials {
            username: "player".into(),
            api_key: KEY.into(),
        }
    }

    fn client(server: &TestServer) -> Client {
        Client::with_endpoints(Endpoints {
            site: server.base(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn signs_requests_and_reads_answers() {
        let server = TestServer::start(|req| {
            let p = req.path.as_str();
            if !p.contains(&format!("y={KEY}")) && p.starts_with("/API/") {
                return Resp::status(401);
            }
            if p.starts_with("/API/API_GetUserProfile.php") && p.contains("u=player") {
                Resp::json(
                    json!({"User": "player", "TotalPoints": "1200", "UserPic": "/UserPic/player.png"}),
                )
            } else if p.starts_with("/API/API_GetGameList.php")
                && p.contains("i=12")
                && p.contains("h=1")
            {
                Resp::json(json!([{"ID": 11, "Title": "Spyro", "Hashes": ["aa"]}]))
            } else if p.starts_with("/API/API_GetGameInfoAndUserProgress.php") && p.contains("g=11")
            {
                Resp::json(json!({"ID": 11, "NumDistinctPlayers": 4, "Achievements": {
                    "1": {"ID": 1, "Title": "Gems", "Points": 10, "NumAwarded": 2}}}))
            } else if p.starts_with("/dorequest.php") {
                Resp::json(json!({"Success": true, "User": "player", "Token": "tok"}))
            } else {
                Resp::status(404)
            }
        });
        let c = client(&server);
        let profile = c.user_profile(&creds()).await.unwrap();
        assert_eq!(profile.total_points, 1200);
        let list = c.game_list(&creds(), 12).await.unwrap();
        assert_eq!(list[0].hashes, ["aa"]);
        let progress = c.game_progress(&creds(), 11).await.unwrap();
        assert_eq!(progress.achievements[0].num_awarded, 2);
        let login = c.connect_login("player", "secret").await.unwrap();
        assert_eq!(login.token, "tok");
        assert!(!format!("{login:?}").contains("tok"));
        assert!(!format!("{:?}", creds()).contains(KEY));
    }

    #[tokio::test]
    async fn refusals_are_typed_and_keep_the_key_out() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/dorequest.php") {
                Resp::json(json!({"Success": false, "Error": "Invalid username/password."}))
            } else {
                Resp::status(401)
            }
        });
        let c = client(&server);
        let e = c.user_profile(&creds()).await.unwrap_err();
        assert!(matches!(
            e.downcast_ref::<RaError>(),
            Some(RaError::Unauthorized(_))
        ));
        assert!(!format!("{e:#}").contains(KEY));
        let e = c.connect_login("player", "wrong").await.unwrap_err();
        assert!(format!("{e:#}").contains("Invalid username/password"));
        assert!(!format!("{e:#}").contains("wrong"));
    }

    #[tokio::test]
    async fn an_empty_profile_is_an_unknown_user() {
        let server = TestServer::start(|_| Resp::json(json!({})));
        let e = client(&server).user_profile(&creds()).await.unwrap_err();
        assert!(matches!(
            e.downcast_ref::<RaError>(),
            Some(RaError::Unauthorized(_))
        ));
    }
}
