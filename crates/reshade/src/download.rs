//! ReShade, its shaders, its compatibility list and `d3dcompiler_47.dll`,
//! from their official sources, cached under [`crate::cache_dir`].
//!
//! - **ReShade**: reshade.me's front page links the current setup program
//!   (`/downloads/ReShade_Setup_<version>.exe`); that link is where the
//!   version is read. The setup program is a Windows program with a zip of
//!   `ReShade32.dll` and `ReShade64.dll` appended, which its own installer
//!   extracts the same way. The build with full add-on support
//!   (`ReShade_Setup_<version>_Addon.exe`, the same layout) is meant for
//!   single-player games only; it is downloaded only for add-ons that need
//!   it (RenoDX), for the version the standard build's link names.
//! - **Shaders**: the `slim` branch of crosire/reshade-shaders, the
//!   "Standard effects" package ReShade's installer always installs.
//! - **Compatibility list**: see [`crate::compat`].
//! - **`d3dcompiler_47.dll`**: ReShade compiles its effects with it, and
//!   Wine's own is incomplete. Mozilla keeps Microsoft's redistributable
//!   DLL in its fxc2 repository, where Winetricks took it from; it is one
//!   fixed file, so it is checked against its known SHA-256.
//!
//! Lists are refreshed once a day and a stale copy stands in when the
//! network fails.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard::relative_entry_path;
use irori_core::install::ProgressSink;
use irori_core::json_store;
use irori_core::process::CancellationToken;
use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::compat::{COMPATIBILITY_URL, CompatList};
use crate::fetch;
use crate::pe::{self, Machine};

/// Page that links the current ReShade release.
pub const RESHADE_SITE: &str = "https://reshade.me";
/// The standard shaders.
pub const SHADERS_URL: &str =
    "https://github.com/crosire/reshade-shaders/archive/refs/heads/slim.zip";
/// Folder name of the shaders' archive.
const SHADERS_ROOT: &str = "reshade-shaders-slim";

/// Mozilla's copies of Microsoft's `d3dcompiler_47.dll`, with their SHA-256.
const D3DCOMPILER_64: (&str, &str) = (
    "https://raw.githubusercontent.com/mozilla/fxc2/master/dll/d3dcompiler_47.dll",
    "4432bbd1a390874f3f0a503d45cc48d346abc3a8c0213c289f4b615bf0ee84f3",
);
const D3DCOMPILER_32: (&str, &str) = (
    "https://raw.githubusercontent.com/mozilla/fxc2/master/dll/d3dcompiler_47_32.dll",
    "2ad0d4987fc4624566b190e747c9d95038443956ed816abfd1e2d389b5ec0851",
);

/// Limits on the shaders' archive, which is a few megabytes.
const MAX_SHADER_FILES: usize = 10_000;
const MAX_SHADER_BYTES: u64 = 256 << 20;

static SETUP_LINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"/downloads/ReShade_Setup_(\d+(?:\.\d+){1,3})\.exe").expect("valid regex")
});

static VERSION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\d+(?:\.\d+){1,3}$").expect("valid regex"));

/// The newest ReShade version a reshade.me page links, add-on builds left out.
pub fn latest_version(html: &str) -> Option<String> {
    SETUP_LINK
        .captures_iter(html)
        .map(|c| c[1].to_string())
        .max_by_key(|v| {
            v.split('.')
                .map(|p| p.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
}

/// Where the setup program of `version` is downloaded from.
pub fn setup_url(version: &str) -> String {
    format!("{RESHADE_SITE}/downloads/ReShade_Setup_{version}.exe")
}

/// Where the setup program of `version` with full add-on support is
/// downloaded from.
pub fn addon_setup_url(version: &str) -> String {
    format!("{RESHADE_SITE}/downloads/ReShade_Setup_{version}_Addon.exe")
}

/// `ReShade32.dll` and `ReShade64.dll` from a setup program's appended zip,
/// each checked to be a Windows DLL for its architecture.
pub fn extract_dlls(setup: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(setup))
        .context("The ReShade setup program holds no ReShade files")?;
    let mut take = |name: &str, machine: Machine| -> Result<Vec<u8>> {
        let mut entry = zip
            .by_name(name)
            .with_context(|| format!("The ReShade setup program has no {name}"))?;
        let mut bytes = Vec::new();
        (&mut entry)
            .take(64 << 20)
            .read_to_end(&mut bytes)
            .with_context(|| format!("Could not unpack {name}"))?;
        let info = pe::read_bytes(&bytes).with_context(|| format!("{name} is damaged"))?;
        if info.machine != machine {
            bail!("{name} is not built for {machine:?}");
        }
        Ok(bytes)
    };
    Ok((
        take("ReShade32.dll", Machine::X86)?,
        take("ReShade64.dll", Machine::X64)?,
    ))
}

/// The files of the shaders' archive, as `reshade-shaders/Shaders/…` and
/// `reshade-shaders/Textures/…` paths relative to the install folder.
pub fn shader_files(archive: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .context("The ReShade shaders download is damaged")?;
    let mut out = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        let Some(rel) = entry.enclosed_name().and_then(|p| relative_entry_path(&p)) else {
            continue;
        };
        let mut parts = rel.components();
        if parts.next().map(|c| c.as_os_str()) != Some(SHADERS_ROOT.as_ref()) {
            continue;
        }
        let rest: PathBuf = parts.collect();
        let top = rest.components().next().map(|c| c.as_os_str().to_owned());
        if !matches!(
            top.as_deref().and_then(|t| t.to_str()),
            Some("Shaders" | "Textures")
        ) || rest.components().count() < 2
        {
            continue;
        }
        if out.len() >= MAX_SHADER_FILES {
            bail!("The ReShade shaders download has too many files");
        }
        total += entry.size();
        if total > MAX_SHADER_BYTES {
            bail!("The ReShade shaders download is too large");
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry)
            .take(MAX_SHADER_BYTES)
            .read_to_end(&mut bytes)?;
        out.push((Path::new("reshade-shaders").join(rest), bytes));
    }
    if out.is_empty() {
        bail!("The ReShade shaders download has no shaders");
    }
    Ok(out)
}

/// The `ReShade.ini` Irori writes when the folder has none: the shaders it
/// installed, and presets beside it.
pub fn default_ini() -> Vec<u8> {
    b"[GENERAL]\r\n\
EffectSearchPaths=.\\reshade-shaders\\Shaders\\**\r\n\
TextureSearchPaths=.\\reshade-shaders\\Textures\\**\r\n\
PresetPath=.\\ReShadePreset.ini\r\n"
        .to_vec()
}

/// A daily-refreshed ReShade list under [`crate::cache_dir`].
async fn cached<T>(
    path: PathBuf,
    fetch: impl AsyncFnOnce() -> Result<Vec<u8>>,
    parse: impl Fn(&[u8]) -> Result<T>,
) -> Result<T> {
    fetch::cached("ReShade", path, fetch, parse).await
}

async fn fetch_bytes(
    url: &str,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    fetch::fetch_bytes(url, &crate::cache_dir().join("downloads"), progress, cancel).await
}

/// The newest ReShade version, asked of reshade.me once a day.
pub async fn latest() -> Result<String> {
    cached(
        crate::cache_dir().join("latest.txt"),
        async || {
            let html = irori_runtime::download::get_text(RESHADE_SITE)
                .await
                .context("Could not read ReShade's download page")?;
            let version =
                latest_version(&html).context("ReShade's download page links no release")?;
            Ok(version.into_bytes())
        },
        |b| {
            let v = std::str::from_utf8(b)?.trim().to_string();
            // The version names a cache folder, so it must be plain digits.
            if !VERSION.is_match(&v) {
                bail!("not a ReShade version");
            }
            Ok(v)
        },
    )
    .await
}

/// ReShade's compatibility list.
pub async fn compatibility() -> Result<CompatList> {
    cached(
        crate::cache_dir().join("Compatibility.ini"),
        async || {
            Ok(irori_runtime::download::get_text(COMPATIBILITY_URL)
                .await
                .context("Could not read ReShade's list of games that block it")?
                .into_bytes())
        },
        |b| {
            let list = CompatList::parse(&String::from_utf8_lossy(b));
            if list.is_empty() {
                bail!("ReShade's compatibility list is empty");
            }
            Ok(list)
        },
    )
    .await
}

/// The shaders' files (see [`shader_files`]).
pub async fn shaders(
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let progress = progress.clone();
    let cancel = cancel.clone();
    cached(
        crate::cache_dir().join("reshade-shaders-slim.zip"),
        async move || fetch_bytes(SHADERS_URL, &progress, &cancel).await,
        shader_files,
    )
    .await
}

/// ReShade's DLL for `machine`, downloading the release once. `addon`
/// takes the build with full add-on support.
pub async fn reshade_dll(
    version: &str,
    machine: Machine,
    addon: bool,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    if !VERSION.is_match(version) {
        bail!("{version} is not a ReShade version");
    }
    let dir = crate::cache_dir().join(if addon {
        format!("{version}-addon")
    } else {
        version.to_string()
    });
    let name = match machine {
        Machine::X86 => "ReShade32.dll",
        Machine::X64 => "ReShade64.dll",
        _ => bail!(crate::ReshadeError::UnsupportedArch),
    };
    if let Ok(bytes) = tokio::fs::read(dir.join(name)).await
        && pe::read_bytes(&bytes).is_ok_and(|i| i.machine == machine)
    {
        return Ok(bytes);
    }
    let url = if addon {
        addon_setup_url(version)
    } else {
        setup_url(version)
    };
    let setup = fetch_bytes(&url, progress, cancel)
        .await
        .with_context(|| format!("Could not download ReShade {version}"))?;
    let (x86, x64) = tokio::task::spawn_blocking(move || extract_dlls(&setup)).await??;
    let save = dir.clone();
    let (a, b) = (x86.clone(), x64.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        json_store::write_bytes(&save.join("ReShade32.dll"), &a)?;
        json_store::write_bytes(&save.join("ReShade64.dll"), &b)
    })
    .await??;
    Ok(if machine == Machine::X86 { x86 } else { x64 })
}

/// `d3dcompiler_47.dll` for `machine`, checked against its known hash.
pub async fn d3dcompiler(
    machine: Machine,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    let (url, sha) = match machine {
        Machine::X86 => D3DCOMPILER_32,
        Machine::X64 => D3DCOMPILER_64,
        _ => bail!(crate::ReshadeError::UnsupportedArch),
    };
    let path = crate::cache_dir()
        .join("d3dcompiler_47")
        .join(format!("{machine:?}.dll").to_lowercase());
    let matches = |b: &[u8]| hex::encode(Sha256::digest(b)) == sha;
    if let Ok(bytes) = tokio::fs::read(&path).await
        && matches(&bytes)
    {
        return Ok(bytes);
    }
    let bytes = fetch_bytes(url, progress, cancel)
        .await
        .context("Could not download d3dcompiler_47.dll")?;
    if !matches(&bytes) {
        bail!("The downloaded d3dcompiler_47.dll is not the expected file");
    }
    let save = bytes.clone();
    tokio::task::spawn_blocking(move || json_store::write_bytes(&path, &save)).await??;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::pe::tests::build_pe;

    fn zip_of(prefix: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in files {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        let archive = writer.finish().unwrap().into_inner();
        let mut out = prefix.to_vec();
        out.extend_from_slice(&archive);
        out
    }

    #[test]
    fn reads_the_version_from_the_download_page() {
        let html = r#"<a href="/downloads/ReShade_Setup_6.7.3.exe">old</a>
            <a class="btn" href="/downloads/ReShade_Setup_6.8.0.exe">Download</a>
            <a href="/downloads/ReShade_Setup_6.10.0_Addon.exe">add-on</a>
            <a href="/downloads/ReShade_Setup_6.9.1.exe">x</a>"#;
        assert_eq!(latest_version(html).as_deref(), Some("6.9.1"));
        assert_eq!(latest_version("<p>Something went wrong.</p>"), None);
        assert_eq!(
            setup_url("6.8.0"),
            "https://reshade.me/downloads/ReShade_Setup_6.8.0.exe"
        );
        assert_eq!(
            addon_setup_url("6.8.0"),
            "https://reshade.me/downloads/ReShade_Setup_6.8.0_Addon.exe"
        );
    }

    #[test]
    fn extracts_the_dlls_appended_to_the_setup_program() {
        let stub = build_pe(0x8664, &["kernel32.dll"], &[]);
        let x86 = build_pe(0x014c, &["kernel32.dll"], &[]);
        let x64 = build_pe(0x8664, &["kernel32.dll"], &[]);
        let setup = zip_of(
            &stub,
            &[
                ("ReShade32.dll", &x86),
                ("ReShade64.dll", &x64),
                ("ReShade64.json", b"{}"),
            ],
        );
        let (a, b) = extract_dlls(&setup).unwrap();
        assert_eq!(a, x86);
        assert_eq!(b, x64);

        let swapped = zip_of(&stub, &[("ReShade32.dll", &x64), ("ReShade64.dll", &x64)]);
        assert!(extract_dlls(&swapped).is_err());
        assert!(extract_dlls(&stub).is_err());
    }

    #[test]
    fn takes_shaders_and_textures_only() {
        let archive = zip_of(
            b"",
            &[
                ("reshade-shaders-slim/", b""),
                ("reshade-shaders-slim/README.md", b"readme"),
                ("reshade-shaders-slim/Shaders/", b""),
                ("reshade-shaders-slim/Shaders/Deband.fx", b"fx"),
                ("reshade-shaders-slim/Shaders/ReShade.fxh", b"fxh"),
                ("reshade-shaders-slim/Textures/UIMask.png", b"png"),
                ("reshade-shaders-slim/../escape.fx", b"no"),
                ("other/Shaders/x.fx", b"no"),
            ],
        );
        let mut files = shader_files(&archive).unwrap();
        files.sort();
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(
            paths,
            [
                Path::new("reshade-shaders/Shaders/Deband.fx").to_path_buf(),
                Path::new("reshade-shaders/Shaders/ReShade.fxh").to_path_buf(),
                Path::new("reshade-shaders/Textures/UIMask.png").to_path_buf(),
            ]
        );
        assert_eq!(files[0].1, b"fx");
    }

    #[test]
    fn the_default_ini_points_at_the_installed_shaders() {
        let ini = String::from_utf8(default_ini()).unwrap();
        assert!(ini.starts_with("[GENERAL]\r\n"));
        assert!(ini.contains("EffectSearchPaths=.\\reshade-shaders\\Shaders\\**\r\n"));
        assert!(ini.contains("TextureSearchPaths=.\\reshade-shaders\\Textures\\**\r\n"));
    }
}
