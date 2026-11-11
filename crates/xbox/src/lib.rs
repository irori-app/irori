//! Xbox network client: a Microsoft account's Xbox profile, the titles it
//! played with their minutes played, and their achievements.
//!
//! * Sign-in ([`auth`]): Microsoft's OAuth 2.0 authorization code flow on
//!   `login.live.com` for a public client (no secret) with PKCE, the desktop
//!   redirect `https://login.live.com/oauth20_desktop.srf` and the scopes
//!   `Xboxlive.signin Xboxlive.offline_access`, so the answer carries a
//!   refresh token.
//! * [`Client::session`]: the access token becomes an Xbox Live user token
//!   (`user.auth.xboxlive.com`, an RPS ticket `d=<access token>`), then an
//!   XSTS token for the relying party `http://xboxlive.com`, which signs
//!   every data request as `XBL3.0 x=<user hash>;<token>` and names the
//!   account (xuid and gamertag).
//! * Data: the title history (`titlehub.xboxlive.com`, with achievement
//!   counts, images, details and last played), minutes played
//!   (`userstats.xboxlive.com`, the `MinutesPlayed` statistic) and a
//!   title's achievements (`achievements.xboxlive.com`, version 2).
//! * Friends ([`friends`]): the people the account follows, their profiles
//!   and presence, read through the services Microsoft documents.
//!
//! Tokens, codes and the PKCE verifier never appear in `Debug` output, logs
//! or error messages; transport errors are stripped of their address.
//!
//! Nothing here knows about Irori: no paths, settings or events.

#![forbid(unsafe_code)]

mod api;
pub mod auth;
pub mod friends;
pub mod model;

pub use api::{Client, Endpoints, Session, Tokens, XboxError};
pub use auth::{CAPTURE, Pkce, REDIRECT_URL, SCOPES, code_from_input};
pub use friends::{OnlineState, Person, Presence};
pub use model::{Achievement, Title};
