//! [`Client`]: the HTTP transport shared by the web session renewal and
//! library calls.
//!
//! Access tokens and Web API keys travel in query strings or cookies, so every
//! transport error is stripped of its URL before it can reach a message or a
//! log.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::de::DeserializeOwned;

use crate::error::SteamError;

/// Steam's Web API server.
pub const API_BASE: &str = "https://api.steampowered.com";

/// Steam's store server (app details and web login).
pub const STORE_BASE: &str = "https://store.steampowered.com";

/// Steam's web login and JWT refresh server.
pub const LOGIN_BASE: &str = "https://login.steampowered.com";

/// Steam's CDN for store art (`IStoreBrowseService` asset paths are
/// relative to it).
pub const ASSETS_BASE: &str = "https://shared.cloudflare.steamstatic.com/store_item_assets";

/// Server base addresses (overridable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// The Web API, `https://api.steampowered.com`.
    pub api: String,
    /// The store, `https://store.steampowered.com`.
    pub store: String,
    /// The web login server, `https://login.steampowered.com`.
    pub login: String,
    /// The store art CDN, [`ASSETS_BASE`].
    pub assets: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: API_BASE.into(),
            store: STORE_BASE.into(),
            login: LOGIN_BASE.into(),
            assets: ASSETS_BASE.into(),
        }
    }
}

/// A Steam HTTP client. Holds no credentials: every call takes the auth it
/// needs. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    no_redirect_http: reqwest::Client,
    endpoints: Endpoints,
}

impl Client {
    /// A client for Steam's own servers.
    pub fn new() -> Result<Self> {
        Self::with_endpoints(Endpoints::default())
    }

    /// A client for other servers (a test server).
    pub fn with_endpoints(endpoints: Endpoints) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Irori/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .context("Could not set up the connection to Steam")?;
        let no_redirect_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .context("Could not set up the connection to Steam")?;
        Ok(Self {
            http,
            no_redirect_http,
            endpoints: Endpoints {
                api: endpoints.api.trim_end_matches('/').to_string(),
                store: endpoints.store.trim_end_matches('/').to_string(),
                login: endpoints.login.trim_end_matches('/').to_string(),
                assets: endpoints.assets.trim_end_matches('/').to_string(),
            },
        })
    }

    pub(crate) fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub(crate) fn no_redirect_http(&self) -> &reqwest::Client {
        &self.no_redirect_http
    }

    /// GET a JSON document; `what` names it in error messages.
    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        url: String,
        query: &[(&str, String)],
        what: &str,
    ) -> Result<T> {
        self.get_json_allowing(url, query, what, &[])
            .await?
            .with_context(|| format!("Steam sent no answer for {what}"))
    }

    /// [`Self::get_json`], with `None` for an HTTP status in `absent`: the
    /// ones Steam answers when there is nothing to send.
    pub(crate) async fn get_json_allowing<T: DeserializeOwned>(
        &self,
        url: String,
        query: &[(&str, String)],
        what: &str,
        absent: &[u16],
    ) -> Result<Option<T>> {
        let resp = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach Steam")?;
        let status = resp.status();
        if absent.contains(&status.as_u16()) {
            return Ok(None);
        }
        if !status.is_success() {
            if let Some(e) = SteamError::from_http_status(status.as_u16()) {
                return Err(e.into());
            }
            bail!(
                "Steam answered with HTTP status {} for {what}.",
                status.as_u16()
            );
        }
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Steam's answer was cut off")?;
        serde_json::from_slice(&body)
            .map(Some)
            .with_context(|| format!("Steam sent an unexpected answer for {what}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_endpoint_slashes() {
        let c = Client::with_endpoints(Endpoints {
            api: "http://a/".into(),
            store: "http://s//".into(),
            login: "http://l/".into(),
            assets: "http://c/".into(),
        })
        .unwrap();
        assert_eq!(c.endpoints().api, "http://a");
        assert_eq!(c.endpoints().store, "http://s");
        assert_eq!(c.endpoints().login, "http://l");
        assert_eq!(c.endpoints().assets, "http://c");
        assert_eq!(Endpoints::default().api, API_BASE);
        assert_eq!(Endpoints::default().login, LOGIN_BASE);
    }
}
