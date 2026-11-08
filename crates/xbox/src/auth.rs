//! The Microsoft account part of the sign-in: PKCE, the authorization
//! address and reading the code back from what the user (or the sign-in
//! window) hands over.
//!
//! The desktop redirect `https://login.live.com/oauth20_desktop.srf` is a
//! blank Microsoft page; after signing in its address carries `code=…`.

use std::fmt;

use anyhow::{Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

/// Where Microsoft sends the browser after sign-in, for desktop apps.
pub const REDIRECT_URL: &str = "https://login.live.com/oauth20_desktop.srf";
/// The scopes asked for: Xbox sign-in, and a refresh token.
pub const SCOPES: &str = "Xboxlive.signin Xboxlive.offline_access";
/// The authorization code in the desktop redirect's address (first group).
pub const CAPTURE: &str = r"oauth20_desktop\.srf\?(?:[^#\s]*&)?code=([^&#\s]+)";

/// A PKCE pair (RFC 7636, method S256). `Debug` leaves the verifier out.
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

impl Pkce {
    /// A fresh random verifier: 64 hexadecimal digits from two random
    /// UUIDs (244 random bits), within RFC 7636's 43 to 128 characters.
    pub fn new() -> Self {
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        Self::from_verifier(verifier)
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// The authorization code in `input`: the desktop redirect's address, a
/// page text holding it, or the bare code.
pub fn code_from_input(input: &str) -> Result<String> {
    let input = input.trim().trim_matches(['"', '\'']);
    if let Some(at) = input.find("oauth20_desktop.srf") {
        let start = input[..at].rfind("https://").unwrap_or(at);
        let address: String = input[start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>'))
            .collect();
        let address = if address.starts_with("https://") {
            address
        } else {
            format!("https://login.live.com/{address}")
        };
        if let Ok(url) = url::Url::parse(&address) {
            let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            if let Some((_, code)) = pairs.iter().find(|(k, v)| k == "code" && !v.is_empty()) {
                return Ok(code.clone());
            }
            if let Some((_, error)) = pairs.iter().find(|(k, _)| k == "error") {
                if error == "access_denied" {
                    bail!("The Microsoft sign-in was cancelled");
                }
                let why = pairs
                    .iter()
                    .find(|(k, _)| k == "error_description")
                    .map_or(error.as_str(), |(_, v)| v.as_str());
                bail!("Microsoft refused the sign-in: {why}");
            }
        }
        bail!("That address holds no sign-in code; sign in again and copy the address you land on");
    }
    let bare = !input.is_empty()
        && input.len() >= 8
        && !input.contains(['/', ' ', '\n', '\t', '?', '='])
        && input.is_ascii();
    if bare {
        return Ok(input.to_string());
    }
    bail!(
        "That isn't a Microsoft sign-in code. Copy the address of the page you land on after signing in (it starts with {REDIRECT_URL})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_verifiers_sha256() {
        // RFC 7636, appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let fresh = Pkce::new();
        assert_eq!(fresh.verifier.len(), 64);
        assert_ne!(fresh.verifier, Pkce::new().verifier);
        assert!(!format!("{fresh:?}").contains(&fresh.verifier));
    }

    #[test]
    fn finds_the_code_in_an_address_page_text_or_alone() {
        let url = "https://login.live.com/oauth20_desktop.srf?code=M.C1_BAY.2.U.abc-123&lc=1033";
        assert_eq!(code_from_input(url).unwrap(), "M.C1_BAY.2.U.abc-123");
        let text = format!("You are signed in.\n{url}\nClose this window.");
        assert_eq!(code_from_input(&text).unwrap(), "M.C1_BAY.2.U.abc-123");
        assert_eq!(
            code_from_input("  M.C1_BAY.2.U.abc-123 ").unwrap(),
            "M.C1_BAY.2.U.abc-123"
        );
        assert!(code_from_input("hello world").is_err());
        assert!(code_from_input("https://login.live.com/oauth20_desktop.srf?lc=1033").is_err());
        let cancelled = code_from_input(
            "https://login.live.com/oauth20_desktop.srf?error=access_denied&error_description=x",
        )
        .unwrap_err();
        assert!(format!("{cancelled}").contains("cancelled"));
    }

    #[test]
    fn the_capture_pattern_takes_the_code() {
        let re = regex::Regex::new(CAPTURE).unwrap();
        let page =
            "https://login.live.com/oauth20_desktop.srf?lc=1033&code=M.C1_BAY.2.U.x-y&state=z";
        assert_eq!(&re.captures(page).unwrap()[1], "M.C1_BAY.2.U.x-y");
        assert!(
            re.captures("https://login.live.com/oauth20_authorize.srf?code_challenge=a")
                .is_none()
        );
    }
}
