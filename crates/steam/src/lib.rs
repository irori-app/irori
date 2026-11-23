//! Steam Web API client (the public Web API and web session endpoints).
//!
//! * [`auth`]: browser-session cookie parsing, web JWT renewal
//!   (`/jwt/refresh`), [`SteamAuth`] (`WebSession` or `ApiKey`), and
//!   [`TokenClaims`].
//! * [`library`]: owned games, the Steam Family library and store details.
//! * [`achievements`]: a game's achievements, the account's unlocks and
//!   their global unlock rates.
//! * [`friends`]: the account's friends and what they are playing.
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
pub mod friends;
pub mod library;
pub mod tags;

pub use achievements::{
    AchievementGroup, AchievementSchema, GameAchievement, UserAchievement, UserAchievementGroup,
    UserAchievements,
};
pub use api::{API_BASE, ASSETS_BASE, Client, Endpoints, LOGIN_BASE, STORE_BASE};
pub use assets::AppAssets;
pub use auth::{Renewed, SteamAuth, TokenClaims, parse_steam_cookie};
pub use error::SteamError;
pub use friends::PlayerSummary;
pub use library::{
    AppDetails, Category, Genre, Metacritic, OwnedGame, Platforms, ReleaseDate, Requirements,
    Screenshot, SharedApp, StoreRating,
};
