//! The receipt the itch app leaves in each install folder
//! (`.itch/receipt.json.gz`, written by its butler): which game, upload and
//! build the files came from. Importing such a folder reads it, so the game
//! keeps getting updates in Irori as it did in the itch app.

use std::io::Read as _;
use std::path::Path;

use flate2::read::GzDecoder;
use irori_itch_api::Upload;
use serde::Deserialize;

/// The receipt's place inside an install folder.
pub const PATH: &str = ".itch/receipt.json.gz";

/// More than any receipt holds (one line per installed file).
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// What a receipt says about the installed files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub game_id: u64,
    pub upload_id: u64,
    /// As [`Upload::revision`] writes it, to compare with the uploads now.
    pub revision: String,
    pub version: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    game: Option<RawGame>,
    upload: Option<RawUpload>,
    build: Option<RawBuild>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGame {
    id: u64,
}

// butler writes camelCase; the snake_case aliases take receipts written by
// tools that kept itch.io's API names.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawUpload {
    id: u64,
    #[serde(rename = "updatedAt", alias = "updated_at")]
    updated_at: String,
    #[serde(rename = "buildId", alias = "build_id")]
    build_id: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawBuild {
    id: u64,
    version: u64,
    #[serde(rename = "userVersion", alias = "user_version")]
    user_version: String,
}

/// The receipt in install folder `dir`, when the itch app left one that
/// can be read.
pub fn read(dir: &Path) -> Option<Receipt> {
    let file = std::fs::File::open(dir.join(PATH)).ok()?;
    let mut text = Vec::new();
    GzDecoder::new(file)
        .take(MAX_BYTES)
        .read_to_end(&mut text)
        .ok()?;
    parse(&text)
}

fn parse(json: &[u8]) -> Option<Receipt> {
    let raw: Raw = serde_json::from_slice(json).ok()?;
    let upload = raw.upload.filter(|u| u.id > 0)?;
    let build = raw.build.filter(|b| b.id > 0);
    let revision = match (&build, upload.build_id) {
        (Some(b), _) => format!("build:{}", b.id),
        (None, id) if id > 0 => format!("build:{id}"),
        (None, _) => format!("upload:{}:{}", upload.id, upload.updated_at),
    };
    let version = build
        .as_ref()
        .map(|b| {
            if b.user_version.trim().is_empty() {
                if b.version > 0 {
                    b.version.to_string()
                } else {
                    String::new()
                }
            } else {
                b.user_version.trim().to_string()
            }
        })
        .unwrap_or_default();
    Some(Receipt {
        game_id: raw.game.map_or(0, |g| g.id),
        upload_id: upload.id,
        revision,
        version,
    })
}

/// Whether two revisions name the same content. An upload without builds
/// is named by its last change, which the itch app writes in Go's RFC 3339
/// and itch.io's API with fractions or a space; they compare to the second.
pub fn same_revision(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (upload_time(a), upload_time(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// `(upload id, "YYYY-MM-DDTHH:MM:SS")` of an upload revision.
fn upload_time(revision: &str) -> Option<(&str, String)> {
    let rest = revision.strip_prefix("upload:")?;
    let (id, time) = rest.split_once(':')?;
    let time = time.trim().replacen(' ', "T", 1);
    let second = time.get(..19)?;
    let shape = second.len() == 19
        && second.bytes().enumerate().all(|(i, c)| match i {
            4 | 7 => c == b'-',
            10 => c == b'T',
            13 | 16 => c == b':',
            _ => c.is_ascii_digit(),
        });
    shape.then(|| (id, second.to_string()))
}

/// The upload the installed files came from, when the game still offers it:
/// updates keep to it, as the itch app does, instead of switching to
/// another upload of the game.
pub fn installed_upload(uploads: &[Upload], upload_id: u64) -> Option<&Upload> {
    (upload_id > 0)
        .then(|| uploads.iter().find(|u| u.id == upload_id))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_of_builds_and_plain_uploads() {
        let build = br#"{"game":{"id":42},"upload":{"id":7,"updatedAt":"2024-01-02T03:04:05Z","buildId":99},
            "build":{"id":99,"version":12,"userVersion":"1.2.0"},"files":["game.exe"],"installerName":"archive"}"#;
        assert_eq!(
            parse(build),
            Some(Receipt {
                game_id: 42,
                upload_id: 7,
                revision: "build:99".into(),
                version: "1.2.0".into(),
            })
        );
        let plain = br#"{"game":{"id":42},"upload":{"id":8,"updatedAt":"2024-01-02T03:04:05Z"},"build":null}"#;
        let r = parse(plain).unwrap();
        assert_eq!(r.revision, "upload:8:2024-01-02T03:04:05Z");
        assert_eq!(r.version, "");
        assert!(parse(br#"{"game":{"id":1}}"#).is_none(), "no upload");
        assert!(parse(b"not json").is_none());
    }

    #[test]
    fn a_receipt_is_read_from_its_gzip_file() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".itch")).unwrap();
        let file = std::fs::File::create(dir.path().join(PATH)).unwrap();
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        gz.write_all(br#"{"game":{"id":3},"upload":{"id":4,"buildId":5}}"#)
            .unwrap();
        gz.finish().unwrap();
        let r = read(dir.path()).unwrap();
        assert_eq!(
            (r.game_id, r.upload_id, r.revision.as_str()),
            (3, 4, "build:5")
        );
        assert!(read(&dir.path().join("elsewhere")).is_none());
    }

    #[test]
    fn upload_times_compare_to_the_second() {
        assert!(same_revision("build:5", "build:5"));
        assert!(!same_revision("build:5", "build:6"));
        assert!(same_revision(
            "upload:8:2024-01-02T03:04:05Z",
            "upload:8:2024-01-02 03:04:05.000000"
        ));
        assert!(!same_revision(
            "upload:8:2024-01-02T03:04:05Z",
            "upload:9:2024-01-02T03:04:05Z"
        ));
        assert!(!same_revision(
            "upload:8:2024-01-02T03:04:05Z",
            "upload:8:2024-01-03T03:04:05Z"
        ));
        assert!(
            !same_revision("upload:8:", "upload:8:x"),
            "nothing to compare"
        );
    }
}
