//! An ffmpeg for Windows, downloaded when the user asks for one. Windows
//! has no package manager most people use for it, so the game page offers
//! this when a trailer can't play. The build is one of those ffmpeg.org's
//! download page points to (BtbN's FFmpeg-Builds on GitHub, LGPL, static):
//! its `latest` release keeps stable file names, so nothing is looked up
//! through GitHub's API, and it publishes `checksums.sha256`, which the
//! download must match before anything is unpacked. Only `ffmpeg.exe` is
//! taken out of the archive, into [`super::ffmpeg::managed_dir`].

use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::install::ignore_progress;
use irori_core::process::CancellationToken;
use irori_runtime::download::{self, DigestKind};

/// The release the builds are downloaded from.
pub const SOURCE: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest";
/// The project's page, named to the user before downloading.
pub const SOURCE_PAGE: &str = "https://github.com/BtbN/FFmpeg-Builds";

/// The archive for a Windows computer of `arch` (Rust's names), if the
/// project builds one.
pub fn archive_name(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("ffmpeg-master-latest-win64-lgpl.zip"),
        "aarch64" => Some("ffmpeg-master-latest-winarm64-lgpl.zip"),
        _ => None,
    }
}

/// Whether Irori offers to download ffmpeg on this computer.
pub fn offered() -> bool {
    cfg!(windows) && archive_name(std::env::consts::ARCH).is_some()
}

/// What the download would be, for the question asked before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub file: String,
    /// Bytes, when the server says.
    pub size: Option<u64>,
    pub source: &'static str,
}

/// The download for this computer from `base` ([`SOURCE`]), with its size.
pub async fn offer(base: &str) -> Result<Offer> {
    let file = archive_name(std::env::consts::ARCH)
        .context("There is no ffmpeg build for this kind of computer")?;
    let url = format!("{}/{file}", base.trim_end_matches('/'));
    let size = irori_core::http::client()
        .head(&url)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        .and_then(|r| r.content_length())
        .filter(|n| *n > 0);
    Ok(Offer {
        file: file.to_string(),
        size,
        source: SOURCE_PAGE,
    })
}

/// Download `file` from `base`, check it against the release's SHA-256 and
/// put its `ffmpeg.exe` at `dest/ffmpeg.exe`. Nothing is left behind when
/// any step fails.
pub async fn install(
    base: &str,
    file: &str,
    dest: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    let base = base.trim_end_matches('/');
    let sums = download::get_text(&format!("{base}/checksums.sha256"))
        .await
        .context("Couldn't read the ffmpeg build's checksums")?;
    let Some(expected) = download::parse_checksum(&sums, file, DigestKind::Sha256) else {
        bail!("The ffmpeg build's checksums don't list {file}");
    };
    let archive = dest.join(".download").join(file);
    let got = download::download_hashed(
        &format!("{base}/{file}"),
        &archive,
        &ignore_progress(),
        cancel,
        DigestKind::Sha256,
    )
    .await
    .context("Couldn't download ffmpeg")?;
    let program = dest.join("ffmpeg.exe");
    let result = if download::checksum_matches(&expected, &got.digest) {
        let (archive, program) = (archive.clone(), program.clone());
        tokio::task::spawn_blocking(move || extract_program(&archive, &program)).await?
    } else {
        Err(anyhow::anyhow!(
            "The downloaded ffmpeg doesn't match its checksum, so it wasn't used"
        ))
    };
    let _ = tokio::fs::remove_dir_all(dest.join(".download")).await;
    result.map(|()| program)
}

/// Take `bin/ffmpeg.exe` out of the zip at `archive` into `program`. The
/// entry's own path never names where it goes, so the archive can't write
/// anywhere else.
fn extract_program(archive: &Path, program: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(BufReader::new(file)).context("The ffmpeg download is damaged")?;
    let is_program = |name: &str| {
        let name = name.replace('\\', "/");
        name == "bin/ffmpeg.exe" || name.ends_with("/bin/ffmpeg.exe")
    };
    let index = (0..zip.len())
        .find(|i| {
            zip.by_index(*i)
                .is_ok_and(|e| e.is_file() && e.name().is_ok_and(|n| is_program(&n)))
        })
        .context("The ffmpeg download holds no ffmpeg.exe")?;
    let mut entry = zip
        .by_index(index)
        .context("The ffmpeg download is damaged")?;
    if let Some(dir) = program.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = program.with_extension("exe.part");
    let copied = (|| -> Result<()> {
        let mut out = std::fs::File::create(&partial)?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n])?;
        }
        out.sync_all()?;
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(e).context("Couldn't unpack ffmpeg");
    }
    std::fs::rename(&partial, program).context("Couldn't save ffmpeg")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use sha2::{Digest as _, Sha256};
    use std::io::Write as _;

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for (name, data) in entries {
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                zip.start_file(*name, options).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn names_the_build_for_the_computer() {
        assert_eq!(
            archive_name("x86_64"),
            Some("ffmpeg-master-latest-win64-lgpl.zip")
        );
        assert_eq!(
            archive_name("aarch64"),
            Some("ffmpeg-master-latest-winarm64-lgpl.zip")
        );
        assert_eq!(archive_name("x86"), None);
    }

    #[tokio::test]
    async fn installs_only_a_build_matching_its_checksum() {
        let file = "ffmpeg-master-latest-win64-lgpl.zip";
        let archive = zip_bytes(&[
            ("ffmpeg-master-latest-win64-lgpl/LICENSE.txt", b"LGPL"),
            (
                "ffmpeg-master-latest-win64-lgpl/bin/ffmpeg.exe",
                b"MZ ffmpeg",
            ),
            (
                "ffmpeg-master-latest-win64-lgpl/bin/ffprobe.exe",
                b"MZ ffprobe",
            ),
            ("../../evil.exe", b"never written"),
        ]);
        let good = hex::encode(Sha256::digest(&archive));
        let sums = |digest: &str| {
            format!("{}  other.zip\n{digest}  {file}\n", "0".repeat(64)).into_bytes()
        };
        let (right, wrong) = (sums(&good), sums(&"a".repeat(64)));
        let server = {
            let archive = archive.clone();
            TestServer::start(move |req| match req.path.as_str() {
                "/good/checksums.sha256" => Resp {
                    status: 200,
                    body: right.clone(),
                    ..Default::default()
                },
                "/bad/checksums.sha256" => Resp {
                    status: 200,
                    body: wrong.clone(),
                    ..Default::default()
                },
                p if p.ends_with(file) => Resp {
                    status: 200,
                    body: archive.clone(),
                    ..Default::default()
                },
                _ => Resp::status(404),
            })
        };
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();

        let err = install(&server.url("/bad"), file, dir.path(), &cancel)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"));
        assert!(!dir.path().join("ffmpeg.exe").exists());
        assert!(!dir.path().join(".download").exists());

        let program = install(&server.url("/good"), file, dir.path(), &cancel)
            .await
            .unwrap();
        assert_eq!(program, dir.path().join("ffmpeg.exe"));
        assert_eq!(std::fs::read(&program).unwrap(), b"MZ ffmpeg");
        // Only ffmpeg.exe comes out; the download is gone.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["ffmpeg.exe"]);
        assert!(!dir.path().parent().unwrap().join("evil.exe").exists());

        let missing = install(&server.url("/good"), "other.zip", dir.path(), &cancel).await;
        assert!(missing.is_err());
    }
}
