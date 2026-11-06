//! The web API of Sunshine (or Apollo) on this computer: its applications
//! and covers.
//!
//! Both serve it over HTTPS with a certificate they made themselves, so this
//! client accepts any certificate, and only ever talks to `127.0.0.1`.
//! Requests carry no `Origin` or `Referer`, which Sunshine needs no CSRF
//! token for.
//!
//! Sunshine takes Basic authentication and names applications by their
//! place in its list. Apollo signs in once (`/api/login`, a session cookie it
//! keeps one of: signing in signs any browser out of its web UI) and names
//! applications by `uuid`. The client tries Basic first and Apollo's sign-in
//! when that is refused; a remembered Apollo session is tried before both.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;
use reqwest::{Method, RequestBuilder, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::detect::StreamHost;

/// The web UI's login, kept with `irori_secrets`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SunshineLogin {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for SunshineLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SunshineLogin")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// How requests prove the login.
#[derive(Clone)]
enum Auth {
    Basic,
    /// Apollo's session cookie value.
    Session(String),
}

pub(crate) struct Client {
    http: reqwest::Client,
    base: String,
    login: SunshineLogin,
    auth: Auth,
}

/// Where an application sits in the host's list.
pub(crate) struct Place<'a> {
    pub index: usize,
    pub app: &'a Value,
}

impl Client {
    /// Connect to the host whose web UI listens on `port`; `session` is an
    /// Apollo session from an earlier sync.
    pub(crate) async fn connect(
        port: u16,
        login: SunshineLogin,
        session: Option<String>,
    ) -> Result<Self> {
        Self::connect_at(format!("https://127.0.0.1:{port}"), login, session).await
    }

    /// [`Self::connect`] to `base` (tests point it at a plain HTTP server).
    pub(crate) async fn connect_at(
        base: String,
        login: SunshineLogin,
        session: Option<String>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(irori_core::http::USER_AGENT)
            .danger_accept_invalid_certs(true)
            // A browser without a login is sent to the welcome page.
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .build()
            .context("Could not prepare a connection to Sunshine")?;
        let mut client = Self {
            http,
            base,
            login,
            auth: Auth::Basic,
        };
        if let Some(session) = session {
            client.auth = Auth::Session(session);
            if client.probe().await? == StatusCode::OK {
                return Ok(client);
            }
            client.auth = Auth::Basic;
        }
        match client.probe().await? {
            StatusCode::OK => Ok(client),
            StatusCode::UNAUTHORIZED => {
                client.auth = Auth::Session(client.sign_in().await?);
                Ok(client)
            }
            status => Err(status_error(status, &Value::Null)),
        }
    }

    /// Which host this is.
    pub(crate) fn host(&self) -> StreamHost {
        match self.auth {
            Auth::Basic => StreamHost::Sunshine,
            Auth::Session(_) => StreamHost::Apollo,
        }
    }

    /// The Apollo session to remember for the next sync.
    pub(crate) fn session(&self) -> Option<String> {
        match &self.auth {
            Auth::Basic => None,
            Auth::Session(s) => Some(s.clone()),
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.http.request(method, format!("{}{path}", self.base));
        match &self.auth {
            Auth::Basic => request.basic_auth(&self.login.username, Some(&self.login.password)),
            Auth::Session(cookie) => request.header("Cookie", format!("auth={cookie}")),
        }
    }

    /// The status of listing the applications with the current proof.
    async fn probe(&self) -> Result<StatusCode> {
        let response = self
            .request(Method::GET, "/api/apps")
            .send()
            .await
            .map_err(unreachable)?;
        Ok(response.status())
    }

    /// Apollo's sign-in; the cookie it sets.
    async fn sign_in(&self) -> Result<String> {
        let response = self
            .http
            .post(format!("{}/api/login", self.base))
            .json(&json!({
                "username": self.login.username,
                "password": self.login.password,
            }))
            .send()
            .await
            .map_err(unreachable)?;
        let status = response.status();
        if !status.is_success() {
            // Sunshine has no such page: it refused the Basic login above.
            return Err(status_error(StatusCode::UNAUTHORIZED, &Value::Null));
        }
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                v.split(';')
                    .next()?
                    .trim()
                    .strip_prefix("auth=")
                    .map(str::to_string)
            })
            .filter(|c| !c.is_empty())
            .context("Apollo signed in but gave no session")
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(unreachable)?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(status_error(status, &value));
        }
        Ok(value)
    }

    /// The host's applications, in its order.
    pub(crate) async fn apps(&self) -> Result<Vec<Value>> {
        let value = self.send(Method::GET, "/api/apps", None).await?;
        value
            .get("apps")
            .and_then(Value::as_array)
            .cloned()
            .context("Sunshine sent an unexpected list of applications")
    }

    /// Add `app`, or replace the application at `place`.
    pub(crate) async fn save(&self, mut app: Value, place: Option<Place<'_>>) -> Result<()> {
        let map = app.as_object_mut().context("not an application")?;
        match self.auth {
            Auth::Basic => {
                let index = place.map_or(-1, |p| p.index as i64);
                map.insert("index".into(), index.into());
            }
            Auth::Session(_) => {
                // Apollo replaces the application with the same uuid, and
                // gives one to an application without.
                let uuid = place
                    .and_then(|p| p.app.get("uuid"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                map.insert("uuid".into(), uuid.into());
            }
        }
        self.send(Method::POST, "/api/apps", Some(app)).await?;
        Ok(())
    }

    pub(crate) async fn delete(&self, place: Place<'_>) -> Result<()> {
        match self.auth {
            Auth::Basic => {
                self.send(Method::DELETE, &format!("/api/apps/{}", place.index), None)
                    .await?;
            }
            Auth::Session(_) => {
                let uuid = place
                    .app
                    .get("uuid")
                    .and_then(Value::as_str)
                    .context("Apollo listed an application without a uuid")?;
                self.send(
                    Method::POST,
                    "/api/apps/delete",
                    Some(json!({ "uuid": uuid })),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Store `png` as the cover `key` in the host's covers folder and return
    /// its path there.
    pub(crate) async fn upload_cover(&self, key: &str, png: &[u8]) -> Result<String> {
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let value = self
            .send(
                Method::POST,
                "/api/covers/upload",
                Some(json!({ "key": key, "data": data })),
            )
            .await?;
        value
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("Sunshine didn't say where it kept the cover")
    }
}

fn unreachable(e: reqwest::Error) -> anyhow::Error {
    if e.is_connect() || e.is_timeout() {
        anyhow!("Sunshine isn't running, or its web UI uses another port")
    } else {
        anyhow!("Could not reach Sunshine: {}", e.without_url())
    }
}

fn status_error(status: StatusCode, body: &Value) -> anyhow::Error {
    if status.is_redirection() {
        return anyhow!(
            "Sunshine isn't set up yet: open its web UI and choose a user name and password"
        );
    }
    match status {
        StatusCode::UNAUTHORIZED => anyhow!("Sunshine refused the user name or password"),
        StatusCode::FORBIDDEN => {
            anyhow!("Sunshine's web UI refused a connection from this computer")
        }
        _ => {
            let reason = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| status.to_string());
            anyhow!("Sunshine refused the change: {reason}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_stay_out_of_debug_output() {
        let login = SunshineLogin {
            username: "ana".into(),
            password: "hunter2".into(),
        };
        let text = format!("{login:?}");
        assert!(text.contains("ana") && !text.contains("hunter2"));
    }
}
