//! Microsoft Store (GDK) MSIXVC packages, ported from Xodus's `msixvc` crate:
//! XVD headers and layout, AES-XTS page decryption, the hash tree, the NTFS
//! volume inside a package, XSP files and the streaming installer (HTTP range
//! reads with resume checkpoints, pause, bandwidth limits and updates that reuse
//! verified local pages).
//!
//! Knows nothing about Irori: no paths, settings or events.

#![forbid(unsafe_code)]
// The XTS and hash-tree code walks fixed-size blocks with `chunks_exact`, as
// Xodus does; `as_chunks` would change every loop for no behaviour change.
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod bandwidth;
pub mod crypt;
pub mod hash_tree;
pub mod layout;
pub mod math;
pub mod models;
pub mod parse;
pub mod streaming;
pub mod streaming_http;
pub mod streaming_ntfs;
pub mod xsp;
pub mod xvd;

/// The HTTP client package transfers need: no transparent gzip or brotli
/// decoding. A range of a package must arrive byte for byte, and a decoded
/// body would hide an encoded reply (its `Content-Encoding` is dropped) from
/// the checks in [`streaming_http`].
pub fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_gzip()
        .no_brotli()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
}
