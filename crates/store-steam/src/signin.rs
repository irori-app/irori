//! Connecting a Steam account to Irori.
//!
//! Two safe connection paths are offered:
//!
//! 1. Valve's official web sign-in page (`https://store.steampowered.com/login/`)
//!    in Irori's sign-in window. Irori never sees the password or submits
//!    credentials to `IAuthenticationService`.
//! 2. A personal Steam Web API key from `https://steamcommunity.com/dev/apikey`
//!    plus the user's `SteamID64`, creating no Steam login session at all.

use anyhow::{Result, bail};
use irori_core::AppSettings;

pub use crate::account::{api_key_login_input, web_login_input};
pub use irori_steam_api::SteamError;

/// Official Steam store web sign-in page.
pub const STEAM_LOGIN_URL: &str = "https://store.steampowered.com/login/";

/// Official page where users register or view their personal Steam Web API key.
pub const STEAM_API_KEY_URL: &str = "https://steamcommunity.com/dev/apikey";

/// Official Steam privacy settings page (Game Details must be Public for API keys).
pub const STEAM_PRIVACY_URL: &str = "https://steamcommunity.com/my/edit/settings";

const MIN_STEAMID64: u64 = 76_561_197_960_265_728;

/// Auto-detect the most recent SteamID64 from Steam's local
/// `config/loginusers.vdf` on this computer. Blocking filesystem read.
pub fn detected_local_steamid(settings: &AppSettings) -> Option<String> {
    let steam_dir = irori_runtime::locations::steam_path(settings);
    crate::scan::most_recent_steamid(&steam_dir)
}

/// Parse a user-entered `SteamID64` or Steam numeric profile URL
/// (`https://steamcommunity.com/profiles/<steamid64>/`).
pub fn parse_steamid_input(raw: &str) -> Result<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Enter your 17-digit SteamID64.");
    }
    let candidate = if let Some(rest) = trimmed
        .strip_prefix("https://steamcommunity.com/profiles/")
        .or_else(|| trimmed.strip_prefix("http://steamcommunity.com/profiles/"))
        .or_else(|| trimmed.strip_prefix("steamcommunity.com/profiles/"))
    {
        rest.split(['/', '?', '#']).next().unwrap_or("")
    } else {
        trimmed
    };
    let Ok(id) = candidate.parse::<u64>() else {
        bail!("Enter a 17-digit SteamID64 (or a steamcommunity.com/profiles/... link).");
    };
    if id < MIN_STEAMID64 {
        bail!("That doesn't look like a valid SteamID64.");
    }
    Ok(id)
}

/// Trim and validate a 32-character hexadecimal Steam Web API key.
pub fn validate_api_key(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.len() != 32 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Steam Web API keys are 32 hexadecimal characters (0-9, A-F).");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steamid64_and_profile_urls() {
        assert_eq!(
            parse_steamid_input("76561197960265729").unwrap(),
            76561197960265729
        );
        assert_eq!(
            parse_steamid_input("  https://steamcommunity.com/profiles/76561197960265729/  ")
                .unwrap(),
            76561197960265729
        );
        assert_eq!(
            parse_steamid_input("steamcommunity.com/profiles/76561197960265729?l=english").unwrap(),
            76561197960265729
        );
        assert!(parse_steamid_input("").is_err());
        assert!(parse_steamid_input("12345").is_err());
        assert!(parse_steamid_input("https://steamcommunity.com/id/gaben").is_err());
    }

    #[test]
    fn validates_32_hex_api_keys() {
        let key = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(validate_api_key(&format!("  {key}\n")).unwrap(), key);
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key("short").is_err());
        assert!(validate_api_key("0123456789abcdef0123456789ABCDEG").is_err());
    }

    #[test]
    fn detects_most_recent_local_steamid() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(dir.path().join("steamapps")).unwrap();
        std::fs::write(
            config.join("loginusers.vdf"),
            r#"
            "users"
            {
                "76561197960265729"
                {
                    "PersonaName" "First"
                    "MostRecent" "0"
                }
                "76561197960265730"
                {
                    "PersonaName" "Second"
                    "MostRecent" "1"
                }
            }
            "#,
        )
        .unwrap();
        let settings = AppSettings {
            default_steam_path: dir.path().display().to_string(),
            ..Default::default()
        };
        assert_eq!(
            detected_local_steamid(&settings).as_deref(),
            Some("76561197960265730")
        );
    }
}
