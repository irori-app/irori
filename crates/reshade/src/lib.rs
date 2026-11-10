//! ReShade per game, below the UI and the backend, on a layer every
//! graphics mod shares ([`layer`]).
//!
//! - [`pe`] reads a Windows program's architecture and imported DLLs, and
//!   [`detect`] picks the program that draws the game (looking past a small
//!   launcher) and the render API, as ReShade's own installer does.
//! - [`compat`] parses ReShade's `Compatibility.ini`: games that ban
//!   ReShade, render APIs the imports don't tell, install folders.
//! - [`download`] fetches ReShade (the DLLs inside the official setup
//!   program from reshade.me), the standard shaders (crosire/reshade-shaders,
//!   `slim` branch) and, under Wine, `d3dcompiler_47.dll` (Mozilla's copy of
//!   Microsoft's DLL, as Winetricks used it). Nothing is bundled; versions are
//!   read from reshade.me's download page, and everything is cached under
//!   `<cache>/reshade`.
//! - [`journal`] writes the files into the game folder with a manifest of
//!   every file and its hash, kept under `<data>/reshade/<runner>/<app>.json`.
//!   Turning ReShade off renames its DLL; removing it deletes only the files
//!   Irori wrote that are still unchanged, so the user's `ReShade.ini` and
//!   presets stay. A failed or cancelled install undoes its writes.
//! - [`install`] puts these together.
//! - [`fetch`] holds the download helpers the mods share.

#![forbid(unsafe_code)]

pub mod compat;
pub mod detect;
pub mod download;
pub mod fetch;
pub mod install;
pub mod journal;
pub mod layer;
pub mod pe;

use std::path::PathBuf;

use irori_core::GameId;
use serde::{Deserialize, Serialize};

pub use compat::{CompatEntry, CompatList};
pub use detect::Detection;
pub use install::{InstallRequest, Step, install};
pub use journal::{FileRecord, Manifest, RemoveReport};
pub use layer::GraphicsMod;
pub use pe::{Machine, PeInfo};

/// A render API, as ReShade's installer tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    D3D8,
    D3D9,
    /// Direct3D 10, 11 and 12, all reached through `dxgi.dll`.
    Dxgi,
    #[serde(rename = "opengl")]
    OpenGl,
    Vulkan,
    #[serde(rename = "ddraw")]
    DDraw,
}

impl Api {
    /// The name shown to users; never translated.
    pub fn label(self) -> &'static str {
        match self {
            Api::D3D8 => "Direct3D 8",
            Api::D3D9 => "Direct3D 9",
            Api::Dxgi => "Direct3D 10/11/12",
            Api::OpenGl => "OpenGL",
            Api::Vulkan => "Vulkan",
            Api::DDraw => "DirectDraw",
        }
    }

    /// The DLL ReShade is installed as, for the APIs Irori installs.
    /// Direct3D 8 needs the separate d3d8to9 wrapper, DirectDraw a wrapper
    /// too, and Vulkan a layer registered in the system (or the prefix's)
    /// registry rather than a DLL beside the game.
    pub fn dll(self) -> Option<&'static str> {
        match self {
            Api::D3D9 => Some("d3d9.dll"),
            Api::Dxgi => Some("dxgi.dll"),
            Api::OpenGl => Some("opengl32.dll"),
            Api::D3D8 | Api::Vulkan | Api::DDraw => None,
        }
    }
}

/// Why ReShade can't be installed, for the UI to explain in the user's
/// language. Other failures are plain `anyhow` errors.
#[derive(Debug, thiserror::Error)]
pub enum ReshadeError {
    #[error("ReShade's compatibility list says {exe} blocks or bans ReShade")]
    Banned { exe: String },
    #[error("Could not tell which program of the game draws it, or with which graphics API")]
    NoApi,
    #[error("Irori can't install ReShade for games that use {}", .0.label())]
    Unsupported(Api),
    #[error("ReShade has no build for the game's processor architecture")]
    UnsupportedArch,
    #[error("{name} already exists in the game folder and isn't ReShade's")]
    DllTaken { name: String },
    #[error("The chosen program is not inside the game folder")]
    OutsideGame,
}

impl ReshadeError {
    /// The typed error anywhere in an `anyhow` chain.
    pub fn of(e: &anyhow::Error) -> Option<&ReshadeError> {
        e.chain().find_map(|c| c.downcast_ref::<ReshadeError>())
    }
}

/// Where a game's ReShade manifest lives.
pub fn journal_path(id: &GameId) -> PathBuf {
    GraphicsMod::Reshade.journal_path(id)
}

/// Where ReShade's downloads are cached.
pub fn cache_dir() -> PathBuf {
    GraphicsMod::Reshade.cache_dir()
}
