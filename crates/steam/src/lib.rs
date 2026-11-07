//! Steam protocol client, built on Steam's Web API (the calls the Steam
//! client and steamcommunity.com make).
//!
//! * [`auth`]: sign-in through `IAuthenticationService` (QR code, or
//!   password and Steam Guard), refresh tokens, and [`TokenClaims`].
//! * [`library`]: owned games, the Steam Family library and store details.
//! * [`achievements`]: a game's achievements, the account's unlocks and
//!   their global unlock rates.
//! * [`assets`]: store art (covers, heroes, logos) for many apps at once.
//! * [`SteamError`]: the refusals callers act on.
//!
//! Nothing here knows about Irori: no paths, settings or events.

#![forbid(unsafe_code)]

pub mod achievements;
mod api;
pub mod assets;
pub mod auth;
mod error;
pub mod library;
mod proto;

pub use achievements::{
    AchievementGroup, AchievementSchema, GameAchievement, UserAchievement, UserAchievementGroup,
    UserAchievements,
};
pub use api::{API_BASE, ASSETS_BASE, Client, Endpoints, STORE_BASE};
pub use assets::AppAssets;
pub use auth::{
    Confirmation, CredentialsSession, DeviceInfo, GuardKind, OS_TYPE, Poll, QrSession, TokenClaims,
    Tokens,
};
pub use error::SteamError;
pub use library::{
    AppDetails, Genre, Metacritic, OwnedGame, Platforms, ReleaseDate, Requirements, Screenshot,
    SharedApp, StoreRating,
};
