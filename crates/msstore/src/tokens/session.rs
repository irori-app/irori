//! Freshness checks for network sessions, without changing raw credential reads.
use crate::models::secrets::Token;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Current,
    Expired,
    Invalid,
}

pub fn state(token: &Token, now: DateTime<Utc>) -> State {
    let Token::Legacy(token) = token else {
        return State::Invalid;
    };
    let (Ok(created), Ok(expires)) = (
        DateTime::parse_from_rfc3339(&token.lifetime.created),
        DateTime::parse_from_rfc3339(&token.lifetime.expires),
    ) else {
        return State::Invalid;
    };
    if expires <= created || created > now + Duration::minutes(5) {
        return State::Invalid;
    }
    if expires <= now {
        State::Expired
    } else {
        State::Current
    }
}

/// Serializes device-token renewals: concurrent requests wait for one
/// renewal and then reuse its token. Irori runs once per config root and owns
/// its credentials, so an in-process lock is enough (Xodus locks a file
/// because several of its processes share the system keyring).
pub(crate) async fn renewal_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static RENEWAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    RENEWAL.lock().await
}
