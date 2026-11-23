//! [`SteamError`]: the refusals callers act on, from Steam's HTTP statuses.

/// Steam refused a request. Callers match on the variant; the message is
/// worded for the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SteamError {
    /// HTTP 429: too many requests.
    #[error(
        "Steam is refusing requests for now because there were too many. Wait a while and try again."
    )]
    RateLimited,
    /// The saved sign-in or API key no longer works (HTTP 401/403, or a
    /// refused web session refresh). Reconnecting fixes it; retrying does not.
    #[error("Steam no longer accepts the saved sign-in. Sign in to Steam again.")]
    SessionRejected,
    /// Steam refused to share something with this sign-in (HTTP 401 or 403
    /// on a call that the sign-in may not make), though the sign-in itself
    /// still works.
    #[error("Steam doesn't share this with Irori's sign-in.")]
    NotShared,
}

impl SteamError {
    /// The error for an HTTP status that carries a meaning of its own.
    pub fn from_http_status(status: u16) -> Option<Self> {
        match status {
            401 | 403 => Some(Self::SessionRejected),
            429 => Some(Self::RateLimited),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_http_statuses() {
        assert_eq!(
            SteamError::from_http_status(401),
            Some(SteamError::SessionRejected)
        );
        assert_eq!(
            SteamError::from_http_status(403),
            Some(SteamError::SessionRejected)
        );
        assert_eq!(
            SteamError::from_http_status(429),
            Some(SteamError::RateLimited)
        );
        assert_eq!(SteamError::from_http_status(500), None);
    }
}
