//! RetroAchievements client, built on its public Web API
//! (`retroachievements.org/API`) and the "connect" login emulators use.
//!
//! * [`Client`]: the account behind a username and Web API key, a console's
//!   games with their ROM hashes, and a game's achievements with the
//!   player's progress; [`Client::connect_login`] turns a password into the
//!   token emulators sign in with.
//! * [`hash`]: the hash RetroAchievements identifies a game file by, the
//!   same one its emulators compute (rcheevos), for the systems where it is
//!   a matter of reading the file.
//! * [`console_for`]: RetroAchievements' console id and hash method for a
//!   system id.
//!
//! The Web API key travels in query strings, so every transport error is
//! stripped of its address before it can reach a message or a log, and the
//! key never appears in `Debug` output.
//!
//! Nothing here knows about Irori: no paths, settings or events.

#![forbid(unsafe_code)]

mod api;
mod consoles;
mod disc;
pub mod hash;
pub mod model;

pub use api::{Client, Credentials, Endpoints, RaError, SITE_BASE};
pub use consoles::{Console, console_for};
pub use model::{
    ConnectLogin, GameEntry, GameProgress, RaAchievement, UserProfile, badge_url, locked_badge_url,
};

/// Where a user finds their Web API key (signed in on the site).
pub const API_KEY_URL: &str = "https://retroachievements.org/settings";

/// Whether `key` looks like a Web API key: RetroAchievements' keys are 32
/// letters and digits.
pub fn is_api_key(key: &str) -> bool {
    let key = key.trim();
    key.len() == 32 && key.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    #[test]
    fn recognises_api_keys() {
        assert!(super::is_api_key("AbCdEf0123456789AbCdEf0123456789"));
        assert!(super::is_api_key("  AbCdEf0123456789AbCdEf0123456789\n"));
        assert!(!super::is_api_key("short"));
        assert!(!super::is_api_key("AbCdEf0123456789AbCdEf012345678!"));
    }
}
