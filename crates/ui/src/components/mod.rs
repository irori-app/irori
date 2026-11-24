//! Domain-aware components shared by feature crates.

mod backdrop;
mod brand;
mod clip;
mod cover;
mod empty;
mod page;
mod status;
mod store_badge;
mod store_logo;

pub use backdrop::{
    ArtBackdrop, BackdropInk, library_backdrop, library_backdrop_url, strongest_legible_weight,
};
pub use brand::{BrandMark, BrandWordmark};
pub use clip::{clip_length, clip_mark, is_clip};
pub use cover::{
    ArtImage, GameCover, art_arrival, art_image, art_img, art_is_fresh, art_loading, art_missing,
    cover_art_for, cover_radius, missing_art_fill, physical_width, program_icon_cover,
    program_icon_img,
};
pub use empty::EmptyState;
pub use page::{PageHeader, SectionTitle};
pub use status::{ProgressSummary, progress_text};
pub use store_badge::{BadgeMark, BadgeRow, MAX_BADGES, StoreBadge, badge_row};
pub use store_logo::{StoreLogo, runner_icon};
