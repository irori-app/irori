//! Shared HTTP client. One connection pool for the whole process.
//!
//! Both clients follow the user's network policy ([`crate::net`]): they
//! resolve names through [`crate::net::OfflineAwareResolver`], so nothing
//! leaves this computer while Irori works offline, and take their timeouts
//! from `iroriNetworkTimeout`.

use std::collections::HashMap;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::net;

pub const USER_AGENT: &str = concat!("Irori/", env!("CARGO_PKG_VERSION"));

/// A browser-like user agent for store web APIs that reject unknown clients.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// What a client is built from: its timeouts, read from the policy, and
/// whether Irori works offline: switching gives a client of its own, so no
/// connection pooled while online carries a request made offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Timeouts {
    connect: Duration,
    total: Duration,
    offline: bool,
}

/// Clients already built, by their timeouts. Callers hold `&'static`
/// references, so a client is never dropped; a new one is built only for
/// policies not seen before, which only a settings change brings, so this
/// stays a handful of entries.
type Built = Mutex<HashMap<Timeouts, &'static reqwest::Client>>;

static CLIENTS: Lazy<Built> = Lazy::new(Default::default);
static DOWNLOAD_CLIENTS: Lazy<Built> = Lazy::new(Default::default);

fn built(
    cache: &Built,
    timeouts: Timeouts,
    make: impl FnOnce(Timeouts) -> reqwest::Client,
) -> &'static reqwest::Client {
    let mut cache = cache.lock();
    let client: &&'static reqwest::Client = cache
        .entry(timeouts)
        .or_insert_with(|| Box::leak(Box::new(make(timeouts))));
    client
}

/// The process-wide client (must be used from within the Tokio runtime).
pub fn client() -> &'static reqwest::Client {
    let timeouts = Timeouts {
        connect: net::connect_timeout(),
        total: net::request_timeout(),
        offline: net::working_offline(),
    };
    built(&CLIENTS, timeouts, |t| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .dns_resolver(net::resolver())
            .connect_timeout(t.connect)
            .timeout(t.total)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(2)
            .build()
            .expect("http client")
    })
}

/// GET a URL and decode JSON, failing on non-2xx statuses.
pub async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    Ok(client()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Client for large transfers: no total timeout, only connect and per-read
/// idle timeouts, so multi-gigabyte downloads are not cut off.
pub fn download_client() -> &'static reqwest::Client {
    let timeouts = Timeouts {
        connect: net::connect_timeout().max(Duration::from_secs(15)),
        // The idle time allowed between reads.
        total: net::timeout_or(Duration::from_secs(60)),
        offline: net::working_offline(),
    };
    built(&DOWNLOAD_CLIENTS, timeouts, |t| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .dns_resolver(net::resolver())
            .connect_timeout(t.connect)
            .read_timeout(t.total)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .build()
            .expect("download client")
    })
}
