//! Irori's per-game mod manager, below the UI and the backend.
//!
//! Mods never live in the game folder. Each game has a folder of its own
//! (`<data>/mods/<runner>/<app>/`, see [`ModLibrary`]) holding `mods.json`
//! (the mods in load order, with name, version, source and whether each is
//! enabled) and one folder per mod whose files already mirror the game
//! folder. Applying them ([`deploy`]) happens only while the game runs:
//!
//! - On Linux with `fuse-overlayfs`, an overlay of the enabled mods is
//!   mounted over the game folder ([`overlay`]); the folder itself is never
//!   written.
//! - Elsewhere, files are linked in (hard link, symbolic link, else a copy)
//!   and every step is recorded in a deployment manifest first, so
//!   [`deploy::undeploy`] puts the folder back exactly, even after a crash.
//!   A game file a mod replaces is renamed beside itself, never deleted.
//!
//! Mods come from archives ([`archive`], zip, 7z and rar), FOMOD installers
//! ([`fomod`]), Thunderstore ([`thunderstore`]) and Nexus Mods ([`nexus`]).
//! [`conflicts`] tells which files several mods provide and which one wins.

pub mod archive;
pub mod conflicts;
pub mod deploy;
pub mod fomod;
mod fsutil;
pub mod layout;
mod library;
mod model;
mod net;
pub mod nexus;
pub mod overlay;
pub mod thunderstore;

pub use conflicts::{Conflicts, FileConflict, ModConflicts};
pub use deploy::{Applied, Deployment, Layer};
pub use fsutil::align_case;
pub use library::{ModLibrary, Staging, WRITTEN_LAYER};
pub use model::{DeployMethod, ModEntry, ModList, ModSource, is_valid_id};
pub use net::{DownloadProgress, MAX_MOD_BYTES, download_to};

#[cfg(test)]
mod tests;
