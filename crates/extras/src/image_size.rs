//! A picture's size in pixels from its first bytes, for art pickers that
//! show each picture's resolution where its source doesn't say it. Only the
//! header is read: a range request, cut off once the size is known.

use anyhow::{Context as _, Result, bail};

/// The most of a picture read to find its size. A JPEG's size can follow
/// its metadata, which is rarely longer.
pub const HEADER_BYTES: usize = 64 * 1024;
/// How long one picture may take: a size is a nicety, never worth a wait.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The width and height a PNG, JPEG or WebP starting with `bytes` says it
/// has; `None` while the header is incomplete or not a picture.
pub fn from_header(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
        .filter(|(w, h)| *w > 0 && *h > 0)
}

/// The size of the picture at `url` (`http`/`https` only), reading no more
/// than [`HEADER_BYTES`] of it. `None` when its start doesn't hold one.
pub async fn fetch(url: &str) -> Result<Option<(u32, u32)>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("not a web address");
    }
    let mut resp = irori_core::http::client()
        .get(url)
        .header(
            reqwest::header::RANGE,
            format!("bytes=0-{}", HEADER_BYTES - 1),
        )
        .timeout(READ_TIMEOUT)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("reading a picture: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("reading a picture: {}", e.without_url()))?;
    let mut head = Vec::new();
    // A server that ignores the range sends the whole picture; dropping the
    // response once enough arrived stops it.
    while head.len() < HEADER_BYTES {
        let Some(chunk) = resp.chunk().await.context("reading a picture")? else {
            break;
        };
        head.extend_from_slice(&chunk);
        if let Some(size) = from_header(&head) {
            return Ok(Some(size));
        }
    }
    Ok(from_header(&head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::new(w, h)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::new(w, h)
            .write_to(
                &mut std::io::Cursor::new(&mut out),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        out
    }

    #[test]
    fn reads_the_size_from_the_first_bytes() {
        let png = png(600, 900);
        assert_eq!(from_header(&png[..64]), Some((600, 900)));
        let jpeg = jpeg(920, 430);
        assert_eq!(from_header(&jpeg[..jpeg.len().min(1024)]), Some((920, 430)));
        assert_eq!(from_header(&png[..8]), None);
        assert_eq!(from_header(b"<html>not a picture</html>"), None);
    }

    #[tokio::test]
    async fn fetches_only_the_header() {
        let picture = png(512, 512);
        let body = picture.clone();
        let server = TestServer::start(move |req| {
            assert_eq!(req.header("range"), Some("bytes=0-65535"));
            Resp {
                status: 206,
                body: body.clone(),
                ..Default::default()
            }
        });
        let size = fetch(&server.url("/a.png")).await.unwrap();
        assert_eq!(size, Some((512, 512)));
        assert!(fetch("file:///etc/passwd").await.is_err());
        let missing = TestServer::start(|_| Resp::status(404));
        assert!(fetch(&missing.url("/b.png")).await.is_err());
    }
}
