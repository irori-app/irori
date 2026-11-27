//! Free space against what the queue still downloads, per disk, on the
//! downloads page, plus subtle free-space pills for configured install drives
//! when idle. The disks are read off the UI thread, and again only when the
//! queue's items or configured library folders change, never on a progress tick.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div, relative,
    rems,
};
use irori_core::GameId;
use irori_core::util::format_bytes;
use irori_services::{
    DownloadQueueSnapshot, DownloadState, QueueDisk, WebDownloadId, WebDownloadSnapshot,
    WebDownloadState,
};
use irori_ui::i18n::t_args;

/// What the queue disk reading depends on: active store and web items and their
/// states/destinations. Progress isn't part of it, so running transfers do not
/// reread disks on every tick.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SpaceKey {
    store: Vec<(GameId, DownloadState, u64)>,
    web: Vec<(WebDownloadId, WebDownloadState, PathBuf, u64)>,
}

impl SpaceKey {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

pub(crate) fn key_of(queue: &DownloadQueueSnapshot, web: &WebDownloadSnapshot) -> SpaceKey {
    SpaceKey {
        store: queue
            .active
            .iter()
            .map(|i| (i.id.clone(), i.state, i.download_size))
            .collect(),
        web: web
            .active
            .iter()
            .map(|w| (w.id.clone(), w.state, w.destination.clone(), w.bytes_total))
            .collect(),
    }
}

/// Proportional segments of a disk bar: `(already_used, queued_downloads)` in
/// `0.0..=1.0`, or `None` when total or free capacity is unknown.
pub(crate) fn bar_fractions(disk: &QueueDisk) -> Option<(f32, f32)> {
    let total = disk.total.filter(|&t| t > 0)?;
    let free = disk.free?;
    let total_f = total as f64;
    let used = total.saturating_sub(free) as f64 / total_f;
    let queued = disk.needed.min(free) as f64 / total_f;
    Some((used.clamp(0.0, 1.0) as f32, queued.clamp(0.0, 1.0) as f32))
}

/// Concise status text for one queued disk (its folder is displayed beside it).
pub(crate) fn line(disk: &QueueDisk) -> SharedString {
    let needed = format_bytes(disk.needed);
    match disk.free {
        Some(free) if disk.is_short() => t_args(
            "manage.downloads.space.short",
            &[("needed", &needed), ("free", &format_bytes(free))],
        ),
        Some(free) => t_args(
            "manage.downloads.space.fits",
            &[("needed", &needed), ("free", &format_bytes(free))],
        ),
        None => t_args("manage.downloads.space.unknown", &[("needed", &needed)]),
    }
}

/// Visual segmented disk space strip for the disks the queue downloads to,
/// or nothing when no size is known.
pub(crate) fn render(disks: &[QueueDisk], cx: &App) -> Option<AnyElement> {
    if disks.is_empty() {
        return None;
    }
    let theme = cx.theme();
    Some(
        v_flex()
            .gap_1p5()
            .children(disks.iter().map(|disk| {
                let short = disk.is_short();
                let color = if short {
                    theme.danger
                } else {
                    theme.muted_foreground
                };
                let queued_color = if short { theme.danger } else { theme.primary };
                let bar = bar_fractions(disk).map(|(used, queued)| {
                    h_flex()
                        .flex_1()
                        .min_w(rems(6.))
                        .h_1p5()
                        .rounded_full()
                        .overflow_hidden()
                        .bg(theme.muted)
                        .when(used > 0.0, |b| {
                            b.child(
                                div()
                                    .h_full()
                                    .w(relative(used))
                                    .bg(theme.muted_foreground.opacity(0.35)),
                            )
                        })
                        .when(queued > 0.0, |b| {
                            b.child(div().h_full().w(relative(queued)).bg(queued_color))
                        })
                });
                h_flex()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .items_center()
                    .rounded(theme.radius)
                    .bg(theme.group_box)
                    .border_1()
                    .border_color(if short {
                        theme.danger.opacity(0.45)
                    } else {
                        theme.border
                    })
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .min_w_0()
                            .max_w(rems(16.))
                            .text_color(color)
                            .child(
                                Icon::new(if short {
                                    IconName::TriangleAlert
                                } else {
                                    IconName::HardDrive
                                })
                                .xsmall(),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(disk.folder.display().to_string()),
                            ),
                    )
                    .children(bar)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(color)
                            .when(short, |d| d.font_medium())
                            .child(line(disk)),
                    )
            }))
            .into_any_element(),
    )
}

/// Subtle free-space pills for configured install drives, shown beneath the
/// idle empty state.
pub(crate) fn render_idle_pills(disks: &[QueueDisk], cx: &App) -> Option<AnyElement> {
    idle_pills(disks, false, cx)
}

/// Compact right-aligned drive pills for the post-download idle header card.
pub(crate) fn render_compact_idle_pills(disks: &[QueueDisk], cx: &App) -> Option<AnyElement> {
    idle_pills(disks, true, cx)
}

fn idle_pills(disks: &[QueueDisk], compact: bool, cx: &App) -> Option<AnyElement> {
    let readable: Vec<&QueueDisk> = disks.iter().filter(|d| d.free.is_some()).collect();
    if readable.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let row = if compact {
        h_flex().flex_wrap().justify_end().gap_2()
    } else {
        h_flex().flex_wrap().justify_center().gap_2().pt_2()
    };
    Some(
        row.children(readable.into_iter().map(|disk| {
            let free = disk.free.unwrap_or(0);
            let used_bar = bar_fractions(disk).map(|(used, _)| {
                div()
                    .w_12()
                    .h_1p5()
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme.muted)
                    .when(used > 0.0, |b| {
                        b.child(
                            div()
                                .h_full()
                                .w(relative(used))
                                .bg(theme.muted_foreground.opacity(0.45)),
                        )
                    })
            });
            h_flex()
                .gap_2()
                .items_center()
                .px_3()
                .py_1p5()
                .rounded_full()
                .bg(theme.group_box)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .child(
                    Icon::new(IconName::HardDrive)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .max_w(rems(14.))
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(disk.folder.display().to_string()),
                )
                .children(used_bar)
                .child(
                    div()
                        .flex_shrink_0()
                        .font_medium()
                        .text_color(theme.foreground)
                        .child(t_args(
                            "manage.downloads.space.idle_free",
                            &[("free", &format_bytes(free))],
                        )),
                )
        }))
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn disk(needed: u64, free: Option<u64>, total: Option<u64>) -> QueueDisk {
        QueueDisk {
            folder: PathBuf::from("/games"),
            needed,
            free,
            total,
        }
    }

    #[test]
    fn lines_say_whether_the_queue_fits() {
        crate::i18n::register();
        assert_eq!(
            line(&disk(2048, Some(1024), Some(8192))),
            "Needs 2.0 KiB · only 1.0 KiB free"
        );
        assert_eq!(
            line(&disk(2048, Some(4096), Some(8192))),
            "2.0 KiB queued · 4.0 KiB free"
        );
        assert_eq!(line(&disk(2048, None, None)), "2.0 KiB queued");
    }

    #[test]
    fn bar_fractions_measure_used_and_queued_shares() {
        assert_eq!(
            bar_fractions(&disk(400, Some(1000), Some(4000))),
            Some((0.75, 0.1))
        );
        // Queued share caps at remaining free space when short.
        assert_eq!(
            bar_fractions(&disk(9000, Some(1000), Some(4000))),
            Some((0.75, 0.25))
        );
        // Zero or unknown total omits the bar.
        assert_eq!(bar_fractions(&disk(400, Some(1000), Some(0))), None);
        assert_eq!(bar_fractions(&disk(400, Some(1000), None)), None);
        assert_eq!(bar_fractions(&disk(400, None, Some(4000))), None);
    }

    #[test]
    fn space_key_tracks_active_web_destinations() {
        let empty = key_of(
            &DownloadQueueSnapshot::default(),
            &WebDownloadSnapshot::default(),
        );
        let with_web = key_of(
            &DownloadQueueSnapshot::default(),
            &WebDownloadSnapshot {
                active: vec![irori_services::WebDownload {
                    id: "w1".into(),
                    destination: PathBuf::from("/downloads"),
                    bytes_total: 2048,
                    state: WebDownloadState::Running,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_ne!(empty, with_web);
    }
}
