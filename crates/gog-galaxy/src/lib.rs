//! GOG Galaxy's communication service, provided by comet running inside
//! Irori. Games built with the Galaxy SDK connect to it on
//! `127.0.0.1:9977` for achievements, statistics and leaderboards. Every local
//! program can reach that port, so it only serves games Irori started, and
//! never the account's own token. What those games unlock is told to Irori
//! as it happens ([`Service::unlocks`]).

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod peer;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use comet::api;
use comet::api::gog::overlay::OverlayPeerMessage;
use comet::api::notification_pusher::{NotificationPusherClient, PusherEvent};
use comet::api::structs::{Token, UserInfo};
use comet::constants;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

/// Port the Galaxy SDK connects to.
pub const PORT: u16 = 9977;

/// The signed-in Galaxy account.
#[derive(Clone)]
pub struct Account {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub username: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// A running service. Dropping it stops the service.
pub struct Service {
    shutdown: CancellationToken,
    allowed_clients: constants::AllowedClients,
    overlay: broadcast::Sender<(u32, OverlayPeerMessage)>,
}

/// An achievement a game unlocked through the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlocked {
    /// The Galaxy client id the game signed in with.
    pub client_id: String,
    /// GOG's id of the achievement (`achievement_id` in its web API).
    pub id: String,
    pub key: String,
    pub name: String,
    /// RFC 3339; `None` when the game gave no time.
    pub unlocked_at: Option<String>,
}

/// The unlocks games make through the service, as they happen.
pub struct Unlocks {
    rx: broadcast::Receiver<(u32, OverlayPeerMessage)>,
    stop: CancellationToken,
}

impl Unlocks {
    /// The next unlock; `None` once the service stopped.
    pub async fn next(&mut self) -> Option<Unlocked> {
        loop {
            let message = tokio::select! {
                _ = self.stop.cancelled() => return None,
                message = self.rx.recv() => message,
            };
            match message {
                Ok((
                    _,
                    OverlayPeerMessage::Unlocked {
                        client_id,
                        achievement,
                    },
                )) => {
                    return Some(Unlocked {
                        client_id,
                        id: achievement.achievement_id().to_owned(),
                        key: achievement.achievement_key().to_owned(),
                        name: achievement.name().to_owned(),
                        unlocked_at: achievement.date_unlocked().clone(),
                    });
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

impl Service {
    /// What games unlock through the service from now on.
    pub fn unlocks(&self) -> Unlocks {
        Unlocks {
            rx: self.overlay.subscribe(),
            stop: self.shutdown.clone(),
        }
    }

    /// Let a game Irori is starting sign in, by its Galaxy client id. It stays
    /// allowed until the service stops, with the last game.
    pub async fn allow_client(&self, client_id: String) {
        self.allowed_clients.lock().await.insert(client_id);
    }

    /// Stop accepting games and close every connection.
    pub fn stop(&self) {
        self.shutdown.cancel();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Start the service on [`PORT`]. Fails when the port is taken (another
/// Galaxy service, or a comet started elsewhere).
pub async fn start(account: Account) -> Result<Service> {
    let listener = TcpListener::bind(("127.0.0.1", PORT))
        .await
        .with_context(|| format!("Port {PORT} is in use; another Galaxy service may be running"))?;
    let http = reqwest::Client::builder()
        .user_agent(format!(
            "GOGGalaxyCommunicationService/2.0.13.27 (Windows_32bit) installation_source/gog Irori/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;
    let user = Arc::new(UserInfo {
        username: account.username.clone(),
        galaxy_user_id: account.user_id.clone(),
    });
    let tokens: constants::TokenStorage = Arc::new(Mutex::new(HashMap::new()));
    tokens.lock().await.insert(
        constants::GALAXY_CLIENT_ID.to_string(),
        Token::new(account.access_token.clone(), account.refresh_token.clone()),
    );
    let allowed_clients: constants::AllowedClients = Arc::new(Mutex::new(HashSet::new()));
    let shutdown = CancellationToken::new();

    // The Galaxy peer library games load (comet keeps it in its own data folder).
    let peer_http = http.clone();
    let peer_stop = shutdown.clone();
    tokio::spawn(async move {
        for attempt in 0..10u64 {
            tokio::select! {
                _ = peer_stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(attempt * 5)) => {}
            }
            let res = api::gog::components::get_component(
                &peer_http,
                comet::paths::REDISTS_STORAGE.clone(),
                api::gog::components::Platform::Windows,
                api::gog::components::Component::Peer,
            )
            .await;
            if res.is_ok() {
                return;
            }
        }
        tracing::warn!("Galaxy: could not download the peer library");
    });

    let (topics, _) = tokio::sync::broadcast::channel::<PusherEvent>(20);
    let pusher_topics = topics.clone();
    let pusher_stop = shutdown.clone();
    let access_token = account.access_token.clone();
    tokio::spawn(async move {
        if let Some(mut pusher) =
            NotificationPusherClient::new(&access_token, pusher_topics, pusher_stop).await
        {
            pusher.handle_loop().await;
        }
    });

    let (overlay, _) = broadcast::channel::<(u32, OverlayPeerMessage)>(32);
    let host_overlay = overlay.clone();
    let stop = shutdown.clone();
    let allowed = allowed_clients.clone();
    tokio::spawn(async move {
        tracing::info!("Galaxy service listening on port {PORT}");
        loop {
            let (socket, _) = tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!("Galaxy service: {e}");
                        continue;
                    }
                },
            };
            let (http, tokens, allowed, user) =
                (http.clone(), tokens.clone(), allowed.clone(), user.clone());
            let (topics, overlay, stop) = (topics.subscribe(), overlay.clone(), stop.clone());
            tokio::spawn(async move {
                #[cfg(target_os = "linux")]
                if !peer::same_user(&socket).await {
                    tracing::warn!("Galaxy service: refused a connection from another user");
                    return;
                }
                api::handlers::entry_point(
                    socket, http, tokens, allowed, user, topics, overlay, stop,
                )
                .await;
            });
        }
        tracing::info!("Galaxy service stopped");
    });
    Ok(Service {
        shutdown,
        allowed_clients,
        overlay: host_overlay,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn achievement(key: &str) -> Arc<comet::api::gog::achievements::Achievement> {
        Arc::new(comet::api::gog::achievements::Achievement::new(
            "38".into(),
            key.into(),
            "Name".into(),
            String::new(),
            String::new(),
            String::new(),
            true,
            Some("2026-05-01T12:00:00+00:00".into()),
            1.0,
            String::new(),
            String::new(),
        ))
    }

    #[tokio::test]
    async fn tells_unlocks_and_ends_with_the_service() {
        let (overlay, _) = broadcast::channel(8);
        let stop = CancellationToken::new();
        let mut unlocks = Unlocks {
            rx: overlay.subscribe(),
            stop: stop.clone(),
        };
        // What the overlay pipe reads is no unlock of the host's.
        overlay
            .send((1, OverlayPeerMessage::Achievement(achievement("other"))))
            .unwrap();
        overlay
            .send((
                1,
                OverlayPeerMessage::Unlocked {
                    client_id: "53652320436400298".into(),
                    achievement: achievement("k"),
                },
            ))
            .unwrap();
        assert_eq!(
            unlocks.next().await,
            Some(Unlocked {
                client_id: "53652320436400298".into(),
                id: "38".into(),
                key: "k".into(),
                name: "Name".into(),
                unlocked_at: Some("2026-05-01T12:00:00+00:00".into()),
            })
        );
        stop.cancel();
        assert_eq!(unlocks.next().await, None);
    }
}
