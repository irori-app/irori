//! Media around a game beyond its store's own page: screenshots and
//! trailers from the Steam app a game of another store is linked to, poster
//! frames and lengths of the user's clips, and trailers played through the
//! computer's own ffmpeg.

pub mod ffmpeg;
pub mod ffmpeg_download;
pub mod posters;
pub(crate) mod steam;
pub mod trailer;

use anyhow::{Context as _, Result};
use irori_core::process::CancellationToken;

use crate::Backend;
pub use trailer::{Frame, Trailer, TrailerEvent, TrailerRequest, TrailerStop};

/// What this computer's ffmpeg allows, for the game page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FfmpegStatus {
    /// There is an ffmpeg to play trailers with.
    pub found: bool,
    /// It can play their sound.
    pub sound: bool,
    /// Irori can download one here (Windows).
    pub can_download: bool,
}

impl Backend {
    /// Whether trailers can play, looking for ffmpeg again with
    /// `look_again` (the user says they installed it).
    pub async fn ffmpeg_status(&self, look_again: bool) -> FfmpegStatus {
        let found = if look_again {
            ffmpeg::find_again().await
        } else {
            ffmpeg::find().await
        };
        FfmpegStatus {
            found: found.is_some(),
            sound: found.as_ref().is_some_and(|f| f.audio_output.is_some()),
            can_download: ffmpeg_download::offered(),
        }
    }

    /// The ffmpeg download Irori would make here, to ask about first.
    pub async fn ffmpeg_download_offer(&self) -> Result<ffmpeg_download::Offer> {
        ffmpeg_download::offer(ffmpeg_download::SOURCE).await
    }

    /// Download ffmpeg (Windows), after the user agreed to `offer`.
    pub async fn download_ffmpeg(&self, offer: &ffmpeg_download::Offer) -> Result<()> {
        ffmpeg_download::install(
            ffmpeg_download::SOURCE,
            &offer.file,
            &ffmpeg::managed_dir(),
            &CancellationToken::new(),
        )
        .await?;
        ffmpeg::find_again()
            .await
            .map(|_| ())
            .context("The downloaded ffmpeg can't be started")
    }

    /// Start playing a trailer. Fails when the computer has no ffmpeg.
    pub async fn play_trailer(&self, request: TrailerRequest) -> Result<Trailer> {
        let ffmpeg = ffmpeg::find()
            .await
            .context("Trailers need ffmpeg, which isn't installed")?;
        trailer::play(ffmpeg, request)
    }
}
