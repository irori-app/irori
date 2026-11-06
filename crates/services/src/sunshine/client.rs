//! Sunshine's web API on this computer: its applications and covers.
//!
//! Sunshine serves it over HTTPS with a certificate it made itself, so this
//! client accepts any certificate, and only ever talks to `127.0.0.1`.
//! Requests carry no `Origin` or `Referer`, which Sunshine needs no CSRF
//! token for.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use reqwest::{Method, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Sunshine's web UI login, kept with `irori_secrets`.
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

pub(crate) struct Client {
    http: reqwest::Client,
    base: String,
    login: SunshineLogin,
}

impl Client {
    /// A client of the Sunshine whose web UI listens on `port`.
    pub(crate) fn new(port: u16, login: SunshineLogin) -> Result<Self> {
        Self::at(format!("https://127.0.0.1:{port}"), login)
    }

    /// A client of `base` (tests point it at a plain HTTP server).
    pub(crate) fn at(base: String, login: SunshineLogin) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(irori_core::http::USER_AGENT)
            .danger_accept_invalid_certs(true)
            // Sunshine sends a browser without a login to its welcome page.
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .build()
            .context("Could not prepare a connection to Sunshine")?;
        Ok(Self { http, base, login })
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth(&self.login.username, Some(&self.login.password));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                anyhow!("Sunshine isn't running, or its web UI uses another port")
            } else {
                anyhow!("Could not reach Sunshine: {}", e.without_url())
            }
        })?;
        let status = response.status();
        if status.is_redirection() {
            bail!("Sunshine isn't set up yet: open its web UI and choose a user name and password");
        }
        if status == StatusCode::UNAUTHORIZED {
            bail!("Sunshine refused the user name or password");
        }
        if status == StatusCode::FORBIDDEN {
            bail!("Sunshine's web UI refused a connection from this computer");
        }
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let reason = value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| status.to_string());
            bail!("Sunshine refused the change: {reason}");
        }
        Ok(value)
    }

    /// Sunshine's applications, in its order.
    pub(crate) async fn apps(&self) -> Result<Vec<Value>> {
        let value = self.send(Method::GET, "/api/apps", None).await?;
        value
            .get("apps")
            .and_then(Value::as_array)
            .cloned()
            .context("Sunshine sent an unexpected list of applications")
    }

    /// Add `app` (`index` `None`) or replace the application at `index`.
    pub(crate) async fn save(&self, mut app: Value, index: Option<usize>) -> Result<()> {
        let index = index.map_or(-1, |i| i as i64);
        app.as_object_mut()
            .context("not an application")?
            .insert("index".into(), index.into());
        self.send(Method::POST, "/api/apps", Some(app)).await?;
        Ok(())
    }

    pub(crate) async fn delete(&self, index: usize) -> Result<()> {
        self.send(Method::DELETE, &format!("/api/apps/{index}"), None)
            .await?;
        Ok(())
    }

    /// Store `png` as the cover `key` in Sunshine's covers folder and return
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
