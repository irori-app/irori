//! Free space against what the queue still downloads, per disk, on the
//! downloads page. The disks are read off the UI thread, and again only when
//! the queue's items change, never on a progress tick.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use irori_core::GameId;
use irori_core::util::format_bytes;
use irori_services::{DownloadQueueSnapshot, DownloadState, QueueDisk};
use irori_ui::i18n::t_args;

/// What the disk reading depends on: the queue's items and their states.
/// Progress isn't part of it, so a running download doesn't reread disks.
pub(crate) type SpaceKey = Vec<(GameId, DownloadState, u64)>;

pub(crate) fn key_of(queue: &DownloadQueueSnapshot) -> SpaceKey {
    queue
        .active
        .iter()
        .map(|i| (i.id.clone(), i.state, i.download_size))
        .collect()
}

/// One line per disk: what the queue still downloads there and what is
/// free, in the warning colour when it won't fit.
pub(crate) fn line(disk: &QueueDisk) -> SharedString {
    let folder = disk.folder.display().to_string();
    let needed = format_bytes(disk.needed);
    match disk.free {
        Some(free) if disk.is_short() => t_args(
            "manage.downloads.space.short",
            &[
                ("folder", &folder),
                ("needed", &needed),
                ("free", &format_bytes(free)),
            ],
        ),
        Some(free) => t_args(
            "manage.downloads.space.fits",
            &[
                ("folder", &folder),
                ("needed", &needed),
                ("free", &format_bytes(free)),
            ],
        ),
        None => t_args(
            "manage.downloads.space.unknown",
            &[("folder", &folder), ("needed", &needed)],
        ),
    }
}

/// The disks the queue downloads to, or nothing when no size is known.
pub(crate) fn render(disks: &[QueueDisk], cx: &App) -> Option<AnyElement> {
    if disks.is_empty() {
        return None;
    }
    let theme = cx.theme();
    Some(
        v_flex()
            .gap_1()
            .children(disks.iter().map(|disk| {
                let color = if disk.is_short() {
                    theme.danger
                } else {
                    theme.muted_foreground
                };
                h_flex()
                    .gap_2()
                    .px_3()
                    .text_xs()
                    .text_color(color)
                    .child(
                        Icon::new(if disk.is_short() {
                            IconName::TriangleAlert
                        } else {
                            IconName::HardDrive
                        })
                        .xsmall(),
                    )
                    .child(div().min_w_0().truncate().child(line(disk)))
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn lines_say_whether_the_queue_fits() {
        crate::i18n::register();
        let disk = |free| QueueDisk {
            folder: PathBuf::from("/games"),
            needed: 2048,
            free,
        };
        assert_eq!(
            line(&disk(Some(1024))),
            "/games: the queue still downloads 2.0 KiB, only 1.0 KiB is free"
        );
        assert_eq!(
            line(&disk(Some(4096))),
            "/games: 4.0 KiB free, the queue still downloads 2.0 KiB"
        );
        assert_eq!(
            line(&disk(None)),
            "/games: the queue still downloads 2.0 KiB"
        );
    }
}
