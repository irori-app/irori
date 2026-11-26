//! The network policy every HTTP client of Irori's follows: whether the user
//! asked Irori to work offline (`iroriNetworkMode`) and how long a request
//! may take (`iroriNetworkTimeout`).
//!
//! Settings set it once at start and on every change; it lives here as plain
//! atomics so protocol code and the shared clients read it without reaching
//! for the settings. Working offline is enforced where names are resolved
//! ([`OfflineAwareResolver`]): a client built with it never reaches a host
//! other than this computer, whatever code made the request.

use std::net::{IpAddr, ToSocketAddrs as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How Irori decides whether it may use the network (`iroriNetworkMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// Probe the connection and follow what it finds.
    #[default]
    Auto,
    /// Take the connection as working without probing it (for networks
    /// that block the probe's address).
    Online,
    /// Never reach the internet: no store refreshes, downloads, update
    /// checks or lookups. Cached lists stand in.
    Offline,
}

/// Irori's own request timeout when the user set none.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The shortest and longest timeouts `iroriNetworkTimeout` may ask for, in
/// seconds; shorter ones would fail on any ordinary slow answer.
pub const MIN_TIMEOUT_SECS: u32 = 10;
pub const MAX_TIMEOUT_SECS: u32 = 600;

static OFFLINE: AtomicBool = AtomicBool::new(false);
static TIMEOUT_SECS: AtomicU32 = AtomicU32::new(0);

/// Applies the user's settings: `mode` and `timeout_secs` (0 for Irori's
/// own). Returns whether anything changed.
pub fn configure(mode: NetworkMode, timeout_secs: u32) -> bool {
    let offline = mode == NetworkMode::Offline;
    let timeout_secs = match timeout_secs {
        0 => 0,
        secs => secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS),
    };
    let was_offline = OFFLINE.swap(offline, Ordering::Relaxed);
    let was_timeout = TIMEOUT_SECS.swap(timeout_secs, Ordering::Relaxed);
    was_offline != offline || was_timeout != timeout_secs
}

/// Whether the user asked Irori to work offline.
pub fn working_offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

/// How long one request may take: the user's `iroriNetworkTimeout`, else
/// [`DEFAULT_TIMEOUT`].
pub fn request_timeout() -> Duration {
    timeout_or(DEFAULT_TIMEOUT)
}

/// The user's timeout, else `default` (for clients with timeouts of their
/// own, such as downloads waiting on a slow read).
pub fn timeout_or(default: Duration) -> Duration {
    match TIMEOUT_SECS.load(Ordering::Relaxed) {
        0 => default,
        secs => Duration::from_secs(u64::from(secs)),
    }
}

/// How long connecting may take: a sixth of the request timeout, at least
/// 10 s, so a raised timeout helps slow handshakes too.
pub fn connect_timeout() -> Duration {
    (request_timeout() / 6).max(Duration::from_secs(10))
}

/// Why a request was refused while working offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingOffline;

impl std::fmt::Display for WorkingOffline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Irori is working offline. Turn off “Work offline” to connect.")
    }
}

impl std::error::Error for WorkingOffline {}

impl WorkingOffline {
    /// Whether `err` (or its cause chain) is a refusal to go online.
    pub fn of(err: &anyhow::Error) -> bool {
        // reqwest keeps the resolver's error deep in its source chain,
        // which `chain` walks.
        err.chain().any(|e| e.is::<WorkingOffline>())
    }
}

/// Fails with [`WorkingOffline`] while the user asked Irori to work offline.
pub fn ensure_online() -> anyhow::Result<()> {
    if working_offline() {
        return Err(WorkingOffline.into());
    }
    Ok(())
}

/// Whether `host` names this computer, which working offline still reaches
/// (Sunshine, Steam's debugging port, comet).
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Resolves names the way the system does, except that while working
/// offline every host but this computer fails with [`WorkingOffline`].
#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineAwareResolver;

impl reqwest::dns::Resolve for OfflineAwareResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if working_offline() && !is_local_host(&host) {
                return Err(Box::new(WorkingOffline) as Box<dyn std::error::Error + Send + Sync>);
            }
            // getaddrinfo blocks, as in reqwest's own resolver.
            let addrs = tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??;
            Ok(Box::new(addrs) as reqwest::dns::Addrs)
        })
    }
}

/// A resolver for `reqwest::ClientBuilder::dns_resolver`.
pub fn resolver() -> Arc<OfflineAwareResolver> {
    Arc::new(OfflineAwareResolver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts() {
        for host in [
            "localhost",
            "LOCALHOST",
            "irori.localhost",
            "127.0.0.1",
            "[::1]",
            "::1",
        ] {
            assert!(is_local_host(host), "{host}");
        }
        for host in [
            "example.com",
            "192.168.1.4",
            "10.0.0.1",
            "localhost.example.com",
        ] {
            assert!(!is_local_host(host), "{host}");
        }
    }

    #[test]
    fn mode_reads_and_defaults() {
        let mode: NetworkMode = serde_json::from_str(r#""offline""#).unwrap();
        assert_eq!(mode, NetworkMode::Offline);
        assert_eq!(NetworkMode::default(), NetworkMode::Auto);
    }

    #[test]
    fn working_offline_errors_are_found_in_chains() {
        let err = anyhow::Error::new(WorkingOffline).context("refreshing GOG");
        assert!(WorkingOffline::of(&err));
        assert!(!WorkingOffline::of(&anyhow::anyhow!("timed out")));
    }

    // One test owns the global policy, so parallel tests never see it
    // half-set.
    #[test]
    fn policy_and_resolver() {
        configure(NetworkMode::Auto, 0);
        assert_eq!(request_timeout(), DEFAULT_TIMEOUT);
        assert_eq!(connect_timeout(), Duration::from_secs(10));
        assert!(configure(NetworkMode::Auto, 300));
        assert_eq!(request_timeout(), Duration::from_secs(300));
        assert_eq!(connect_timeout(), Duration::from_secs(50));
        assert!(!configure(NetworkMode::Online, 300), "online isn't offline");
        configure(NetworkMode::Auto, 5000);
        assert_eq!(request_timeout(), Duration::from_secs(600));
        configure(NetworkMode::Auto, 3);
        assert_eq!(request_timeout(), Duration::from_secs(10));

        configure(NetworkMode::Offline, 0);
        assert!(ensure_online().is_err());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let resolve = |host: &str| {
            let name: reqwest::dns::Name = host.parse().unwrap();
            rt.block_on(reqwest::dns::Resolve::resolve(&OfflineAwareResolver, name))
        };
        let refused = resolve("example.com").err().expect("refused offline");
        assert!(refused.is::<WorkingOffline>());
        assert!(resolve("localhost").is_ok());
        configure(NetworkMode::Auto, 0);
        assert!(ensure_online().is_ok());
    }
}
