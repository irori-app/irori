//! Microsoft Store and Xbox Live protocol, ported from Xodus (the Nexus
//! backend): device registration (`deviceaddcredential.srf`), RST2 token
//! exchanges with XML-DSig signing and response decryption, the CLEP and
//! SPLicense device secrets, content licences and their keys, SISU and XSTS
//! tokens, the display catalog and package lookups.
//!
//! Knows nothing about Irori: callers pass HTTP clients and a [`tokens`]
//! storage backend, and keep the files.

#![forbid(unsafe_code)]

pub mod api;
pub mod auth;
pub mod catalog;
pub mod clep;
pub mod hardware;
pub mod http;
pub mod licensing;
pub mod models;
pub mod ops;
pub mod sisu;
pub mod tokens;

pub const XBOX_LIVE_PACKAGES_PC: &str = "https://packagespc.xboxlive.com";
