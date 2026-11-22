//! Irori core: the domain model and contracts shared by every other crate.
//! No UI dependency; everything here is usable from tests and headless tools.

pub mod achievement_cache;
pub mod achievement_sets;
pub mod age_rating;
pub mod archive_guard;
pub mod big_picture_prefs;
pub mod controller_prefs;
pub mod dll_override;
pub mod events;
pub mod fs_guard;
pub mod game;
pub mod game_mode;
pub mod host;
pub mod http;
pub mod install;
pub mod json_store;
pub mod launch;
pub mod launch_extras;
pub mod log_cap;
pub mod merged_lists;
pub mod paths;
pub mod platform;
pub use platform::HostPlatform;
pub mod process;
pub mod runner;
pub mod settings;
pub mod status;
pub mod store;
pub mod user_actions;
pub mod util;

pub use age_rating::{AgeRating, RatingSystem};
pub use dll_override::{DllMode, DllOverride};
pub use events::{Event, EventBus, Notice, NoticeLevel};
pub use game::*;
pub use game_mode::GameMode;
pub use install::*;
pub use launch::*;
pub use launch_extras::{LaunchPreset, LaunchProgram};
pub use paths::Paths;
pub use runner::{GameId, InstallPlatform, Runner};
pub use settings::*;
pub use status::{GameStatus, StatusKind};
pub use store::{
    AchievementSignal, ClientAction, DynStore, InstallInfoQuery, LoginApproval, LoginFlow,
    MAIN_ACCOUNT, OpContext, SessionExpired, SettingsFn, Store, StoreAccount,
};
pub use user_actions::UserAction;

pub use anyhow;
pub use async_trait::async_trait;
