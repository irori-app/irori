//! Shared HTTP plumbing for the Store APIs: the `StoreApi` handle with its
//! endpoints, the error type every Store call returns, bounded body reads and
//! the Microsoft correlation vector.

use std::sync::Arc;
use std::time::Duration;

/// Base URLs of every service the Store APIs call. `Default` is production;
/// tests point all of them at a local server with [`Endpoints::local`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// `https://displaycatalog.mp.microsoft.com` (public product metadata).
    pub displaycatalog: String,
    /// `https://packagespc.xboxlive.com` (package files and CDN URLs).
    pub packagespc: String,
    /// `https://licensing.mp.microsoft.com` (content licences, licence tokens).
    pub licensing: String,
    /// `https://beige.xboxservices.com` (PC Game App Front Door: entitlements,
    /// subscriptions, offline-PC designation).
    pub pcgafd: String,
    /// `https://catalog.gamepass.com` (public Game Pass collections).
    pub gamepass: String,
    /// `https://apps.microsoft.com` (Store search and recommendation lists).
    pub store_web: String,
    /// `https://titlestorage.xboxlive.com` (cloud saves).
    pub titlestorage: String,
    /// `https://title.mgt.xboxlive.com` (title-management endpoint table).
    pub title_mgt: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            displaycatalog: "https://displaycatalog.mp.microsoft.com".into(),
            packagespc: "https://packagespc.xboxlive.com".into(),
            licensing: "https://licensing.mp.microsoft.com".into(),
            pcgafd: "https://beige.xboxservices.com".into(),
            gamepass: "https://catalog.gamepass.com".into(),
            store_web: "https://apps.microsoft.com".into(),
            titlestorage: "https://titlestorage.xboxlive.com".into(),
            title_mgt: "https://title.mgt.xboxlive.com".into(),
        }
    }
}

impl Endpoints {
    /// Every service at one base URL (a local test server).
    pub fn local(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self {
            displaycatalog: base.clone(),
            packagespc: base.clone(),
            licensing: base.clone(),
            pcgafd: base.clone(),
            gamepass: base.clone(),
            store_web: base.clone(),
            titlestorage: base.clone(),
            title_mgt: base,
        }
    }
}

/// Handle for the Store APIs: an HTTP client plus the endpoints it talks to.
/// Cheap to clone. Holds no credentials; every call takes the token it needs.
#[derive(Clone)]
pub struct StoreApi {
    pub(crate) client: reqwest::Client,
    pub(crate) endpoints: Arc<Endpoints>,
}

impl std::fmt::Debug for StoreApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreApi")
            .field("endpoints", &self.endpoints)
            .finish_non_exhaustive()
    }
}

impl StoreApi {
    /// The production services.
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_endpoints(client, Endpoints::default())
    }

    pub fn with_endpoints(client: reqwest::Client, endpoints: Endpoints) -> Self {
        Self {
            client,
            endpoints: Arc::new(endpoints),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }
}

/// Why a Store call failed. Messages are user-facing and never carry tokens,
/// response bodies or account identifiers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// HTTP 401/403 on a call where that means the token was refused: sign in
    /// again (or get a fresh token) and retry.
    #[error("Microsoft refused the sign-in for this request. Sign in again.")]
    Rejected,
    #[error("Microsoft did not answer in time. Check your connection and try again.")]
    Timeout,
    #[error("Microsoft could not be reached. Check your connection and try again.")]
    Unreachable,
    #[error("Microsoft is rate-limiting requests (HTTP 429). Wait before trying again.")]
    RateLimited,
    #[error("Microsoft returned a server error (HTTP {0}). Try again later.")]
    Server(u16),
    #[error("Microsoft returned HTTP {0}.")]
    Http(u16),
    #[error("Microsoft has no such item.")]
    NotFound,
    #[error("The Microsoft response exceeded its size limit.")]
    TooLarge,
    /// The response was readable but not what the protocol promises. The
    /// reason is a fixed word for diagnostics.
    #[error("Microsoft returned an unrecognised response ({0}).")]
    Malformed(&'static str),
    /// The caller passed an argument the protocol cannot carry.
    #[error("Invalid Store request: {0}.")]
    Invalid(&'static str),
    /// Content licensing answered and refused this account or content (not
    /// owned, not covered by the account's Game Pass tier, HTTP 403).
    #[error(
        "Microsoft did not grant a license for this account and package. Check the purchased edition, subscription and Store region."
    )]
    NotEntitled,
    /// Content licensing refused this device: the Store device limit.
    #[error(
        "Microsoft Store device limit reached. Open https://account.microsoft.com/devices/content, unlink a device you no longer use, then try again."
    )]
    DeviceLimit,
}

impl StoreError {
    /// Microsoft was not reached or answered with a temporary failure; a
    /// retry later (or an offline fallback) is reasonable.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Unreachable | Self::RateLimited | Self::Server(_)
        )
    }

    pub(crate) fn from_reqwest(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else {
            Self::Unreachable
        }
    }

    /// Status mapping for calls where 401/403 means the token was refused.
    pub(crate) fn from_status(status: u16) -> Self {
        match status {
            401 | 403 => Self::Rejected,
            404 => Self::NotFound,
            429 => Self::RateLimited,
            500..=599 => Self::Server(status),
            _ => Self::Http(status),
        }
    }
}

pub(crate) type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Sends `request` with `timeout` as the whole-request deadline.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<reqwest::Response> {
    request
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| StoreError::from_reqwest(&error))
}

/// Reads a whole body, failing once it passes `limit` bytes (announced or
/// actual), so a hostile or broken server cannot exhaust memory.
pub(crate) async fn read_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(StoreError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| StoreError::from_reqwest(&error))?
    {
        if chunk.len() > limit - body.len() {
            return Err(StoreError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Checks the status (401/403 → `Rejected`), then reads and parses JSON
/// within `limit` bytes.
pub(crate) async fn json_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    limit: usize,
) -> Result<T> {
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(StoreError::from_status(status));
    }
    let body = read_limited(response, limit).await?;
    serde_json::from_slice(&body).map_err(|_| StoreError::Malformed("json"))
}

/// A 12-character Store product ID (`9NBLGGH4R315`).
pub fn is_product_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// A two-letter upper-case market (`US`).
pub fn is_market(market: &str) -> bool {
    market.len() == 2 && market.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// The current time in Unix seconds.
pub fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};

    #[tokio::test]
    async fn bodies_are_bounded_by_announcement_and_by_bytes_read() {
        let server = TestServer::start(|req| match req.path.as_str() {
            "/big" => Resp {
                status: 200,
                body: vec![b'a'; 2048],
                ..Default::default()
            },
            _ => Resp {
                status: 200,
                body: vec![b'a'; 16],
                ..Default::default()
            },
        });
        let client = reqwest::Client::new();
        let big = client.get(server.url("/big")).send().await.unwrap();
        assert_eq!(read_limited(big, 1024).await, Err(StoreError::TooLarge));
        let small = client.get(server.url("/small")).send().await.unwrap();
        assert_eq!(read_limited(small, 16).await.unwrap().len(), 16);
    }

    #[test]
    fn status_mapping_and_transience() {
        assert_eq!(StoreError::from_status(401), StoreError::Rejected);
        assert_eq!(StoreError::from_status(403), StoreError::Rejected);
        assert_eq!(StoreError::from_status(404), StoreError::NotFound);
        assert!(StoreError::from_status(503).is_transient());
        assert!(StoreError::from_status(429).is_transient());
        assert!(!StoreError::from_status(418).is_transient());
        assert!(!StoreError::Rejected.is_transient());
    }

    #[test]
    fn identifiers() {
        assert!(is_product_id("9NBLGGH4R315"));
        assert!(!is_product_id("9nblggh4r315"));
        assert!(!is_product_id("9NBLGGH4R31"));
        assert!(is_market("US") && !is_market("us") && !is_market("USA"));
    }
}
