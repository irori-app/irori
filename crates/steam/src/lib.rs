//! Steam protocol client, built on Steam's Web API (the calls the Steam
//! client and steamcommunity.com make).
//!
//! * [`auth`]: sign-in through `IAuthenticationService` (QR code, or
//!   password and Steam Guard), refresh tokens, and [`TokenClaims`].
//! * [`library`]: owned games, the Steam Family library and store details.
//! * [`achievements`]: a game's achievements, the account's unlocks and
//!   their global unlock rates.
//! * [`friends`]: the account's friends and what they are playing.
//! * [`assets`]: store art (covers, heroes, logos) for many apps at once.
//! * [`search`]: the store's search by name, and app ids in pasted links.
//! * [`tags`]: the store's user tags of apps, with their votes.
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
mod proto;
pub mod search;
pub mod tags;

pub use achievements::{
    AchievementGroup, AchievementSchema, GameAchievement, UserAchievement, UserAchievementGroup,
    UserAchievements,
};
pub use api::{API_BASE, ASSETS_BASE, Client, Endpoints, STORE_BASE};
pub use assets::AppAssets;
pub use auth::{
    Confirmation, CredentialsSession, DeviceInfo, GuardKind, OS_TYPE, Poll, QrSession, Renewed,
    TokenClaims, Tokens,
};
pub use error::SteamError;
pub use friends::PlayerSummary;
pub use library::{
    AppDetails, Category, Genre, Metacritic, Movie, OwnedGame, Platforms, ReleaseDate,
    Requirements, Screenshot, SharedApp, StoreRating,
};
pub use search::{StoreHit, app_id_in};
pub use tags::TagVotes;
