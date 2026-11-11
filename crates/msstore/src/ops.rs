//! Account operations of the Microsoft Store and Xbox Live, ported from the
//! Nexus backend's commands: the account library (entitlements), Game Pass
//! subscription, content licences and the device-bound offline licence
//! cache, package resolution and lookup, the offline-PC designation, profiles,
//! achievements and play activity.

pub mod achievements;
pub mod game_activity;
pub mod library;
pub mod license;
pub mod license_cache;
pub mod offline_pc;
pub mod package;
pub mod package_plan;
pub mod profile;
pub mod session;
pub mod subscription;
