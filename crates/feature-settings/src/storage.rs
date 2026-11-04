//! The Storage section (`Route::Settings("storage")`): every disk holding
//! Irori's games, prefixes, Wine builds, emulators and caches, with a usage
//! bar and a treemap of its games, and the cleanups that free space safely.
//!
//! The report comes from `Backend::storage_report`, measured off the UI
//! thread and cached on disk; opening the section reuses the cache and
//! Rescan measures again. Each cleanup lists what it deletes with a box per
//! item, and asks before deleting, naming how much goes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, px, relative, rems,
};
use irori_core::util::format_bytes;
use irori_services::storage::{
    Cleanup, CleanupItem, CleanupKind, DiskUsage, GameSize, Holding, ItemNote, StorageReport,
};
use irori_ui::art::LruImageCache;
use irori_ui::components::SectionTitle;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{Route, backend, models, navigate};

use crate::treemap::{self, Rect};
use crate::widgets::muted;

/// Games drawn as their own treemap tile per disk; the rest share one.
const TREEMAP_TILES: usize = 30;
/// The treemap's width over its height, for the tiles' shapes.
const TREEMAP_ASPECT: f64 = 2.6;
/// Height of the treemap, in rem.
const TREEMAP_REM: f32 = 13.;
/// Tiles narrower or shorter than this share of the treemap carry no label.
const LABEL_MIN: (f32, f32) = (0.11, 0.2);
/// Width of the art thumbnails in treemap tiles, in pixels.
const TILE_ART_PX: u32 = 480;
/// Items of a cleanup listed before "and N more".
const LISTED_ITEMS: usize = 8;

/// Whether an item is chosen until the user says otherwise. A prefix can hold
/// saves a game without cloud saves keeps nowhere else, so each is picked by
/// hand; everything else is made again or downloaded again.
fn chosen_by_default(kind: CleanupKind) -> bool {
    kind != CleanupKind::OrphanPrefixes
}

/// Which cleanup items the user has chosen, kept across rescans.
#[derive(Debug, Default)]
pub(crate) struct Selection {
    chosen: HashMap<CleanupKind, HashSet<PathBuf>>,
    /// Items offered before, so a rescan doesn't choose again what the user
    /// left out.
    seen: HashSet<PathBuf>,
}

impl Selection {
    /// Follow a new report: forget items it no longer offers, and choose new
    /// ones as [`chosen_by_default`] says.
    pub(crate) fn sync(&mut self, report: &StorageReport) {
        for cleanup in &report.cleanups {
            let chosen = self.chosen.entry(cleanup.kind).or_default();
            chosen.retain(|path| cleanup.items.iter().any(|i| &i.path == path));
            for item in &cleanup.items {
                if self.seen.insert(item.path.clone()) && chosen_by_default(cleanup.kind) {
                    chosen.insert(item.path.clone());
                }
            }
        }
    }

    pub(crate) fn is_chosen(&self, kind: CleanupKind, path: &PathBuf) -> bool {
        self.chosen.get(&kind).is_some_and(|c| c.contains(path))
    }

    pub(crate) fn set(&mut self, kind: CleanupKind, path: PathBuf, chosen: bool) {
        let set = self.chosen.entry(kind).or_default();
        if chosen {
            set.insert(path);
        } else {
            set.remove(&path);
        }
    }

    pub(crate) fn set_all(&mut self, cleanup: &Cleanup, chosen: bool) {
        for item in &cleanup.items {
            self.set(cleanup.kind, item.path.clone(), chosen);
        }
    }

    /// The chosen items of `cleanup`, in its order.
    pub(crate) fn items<'a>(&self, cleanup: &'a Cleanup) -> Vec<&'a CleanupItem> {
        cleanup
            .items
            .iter()
            .filter(|i| self.is_chosen(cleanup.kind, &i.path))
            .collect()
    }

    pub(crate) fn bytes(&self, cleanup: &Cleanup) -> u64 {
        self.items(cleanup).iter().map(|i| i.bytes).sum()
    }
}

fn kind_id(kind: CleanupKind) -> &'static str {
    match kind {
        CleanupKind::DownloadLeftovers => "download_leftovers",
        CleanupKind::OrphanPrefixes => "orphan_prefixes",
        CleanupKind::UnusedWineBuilds => "unused_wine_builds",
        CleanupKind::ArtCache => "art_cache",
        CleanupKind::Logs => "logs",
    }
}

fn kind_text(kind: CleanupKind, part: &str) -> SharedString {
    t(&format!(
        "settings.storage.cleanup.{}.{part}",
        kind_id(kind)
    ))
}

fn kind_icon(kind: CleanupKind) -> IconName {
    match kind {
        CleanupKind::DownloadLeftovers => IconName::HardDriveDownload,
        CleanupKind::OrphanPrefixes => IconName::FolderX,
        CleanupKind::UnusedWineBuilds => IconName::Wine,
        CleanupKind::ArtCache => IconName::Image,
        CleanupKind::Logs => IconName::ScrollText,
    }
}

fn holding_label(holding: Holding) -> SharedString {
    t(match holding {
        Holding::Game => "settings.storage.games",
        Holding::Prefix => "settings.storage.holding.prefixes",
        Holding::WineBuild => "settings.storage.holding.wine_builds",
        Holding::Emulators => "settings.storage.holding.emulators",
        Holding::IroriData => "settings.storage.holding.irori_data",
        Holding::Unfinished => "settings.storage.holding.unfinished",
    })
}

/// What an item is, for its row and the confirmation.
fn item_label(item: &CleanupItem) -> SharedString {
    match &item.note {
        ItemNote::None => item.name().into(),
        ItemNote::PartOf(title) => t_args(
            "settings.storage.item.part_of",
            &[("file", &item.name()), ("title", title)],
        ),
        ItemNote::PrefixOf(title) => t_args(
            "settings.storage.item.prefix_of",
            &[("name", &item.name()), ("title", title)],
        ),
        ItemNote::Unfinished => t_args(
            "settings.storage.item.unfinished",
            &[("name", &item.name())],
        ),
        ItemNote::LauncherInstallers => t("settings.storage.item.launcher_installers"),
        ItemNote::LogFiles(1) => t("settings.storage.item.log_files_one"),
        ItemNote::LogFiles(count) => t_args(
            "settings.storage.item.log_files_other",
            &[("count", &count.to_string())],
        ),
    }
}

fn items_text(count: usize, bytes: u64) -> SharedString {
    if count == 1 {
        t_args(
            "settings.storage.items_one",
            &[("size", &format_bytes(bytes))],
        )
    } else {
        t_args(
            "settings.storage.items_other",
            &[
                ("count", &count.to_string()),
                ("size", &format_bytes(bytes)),
            ],
        )
    }
}

/// The Storage section's content.
pub(crate) struct StoragePanel {
    report: Option<Arc<StorageReport>>,
    loading: bool,
    error: Option<SharedString>,
    selection: Selection,
    cleaning: Option<CleanupKind>,
    expanded: HashSet<CleanupKind>,
    images: gpui_kit::Entity<LruImageCache>,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl StoragePanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let art = irori_ui::art::art_store(cx);
        let library = models(cx).library.clone();
        Self {
            report: None,
            loading: false,
            error: None,
            selection: Selection::default(),
            cleaning: None,
            expanded: HashSet::new(),
            images: LruImageCache::new(TREEMAP_TILES * 2, cx),
            _subscriptions: vec![
                cx.observe(&art, |_, _, cx| cx.notify()),
                cx.observe(&library, |_, _, cx| cx.notify()),
            ],
        }
    }

    pub(crate) fn loading(&self) -> bool {
        self.loading
    }

    /// Measure on first show; later shows keep what is on screen.
    pub(crate) fn ensure_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.report.is_none() {
            self.load(false, window, cx);
        }
    }

    /// Build the report again; `rescan` measures every folder again.
    pub(crate) fn load(&mut self, rescan: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.storage_report(rescan).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(report) => {
                        this.selection.sync(&report);
                        this.report = Some(report);
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm(&mut self, kind: CleanupKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cleanup) = self.report.as_ref().and_then(|r| r.cleanup(kind)) else {
            return;
        };
        let items: Vec<CleanupItem> = self.selection.items(cleanup).into_iter().cloned().collect();
        if items.is_empty() {
            return;
        }
        let bytes: u64 = items.iter().map(|i| i.bytes).sum();
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let muted_fg = cx.theme().muted_foreground;
            let listed = items.iter().take(LISTED_ITEMS).map(|item| {
                h_flex()
                    .gap_3()
                    .text_sm()
                    .child(div().flex_1().min_w_0().truncate().child(item_label(item)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(muted_fg)
                            .child(format_bytes(item.bytes)),
                    )
            });
            let more = items.len().saturating_sub(LISTED_ITEMS);
            let description = v_flex()
                .gap_2()
                .child(div().child(kind_text(kind, "confirm_description")))
                .child(v_flex().gap_1().children(listed).when(more > 0, |v| {
                    v.child(div().text_xs().text_color(muted_fg).child(t_args(
                        "settings.storage.and_more",
                        &[("count", &more.to_string())],
                    )))
                }));
            let paths: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
            let weak = weak.clone();
            alert
                .title(kind_text(kind, "confirm_title"))
                .description(description)
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t_args(
                    "settings.storage.confirm_ok",
                    &[("size", &format_bytes(bytes))],
                ))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let paths = paths.clone();
                    weak.update(cx, |this, cx| this.clean(kind, paths, window, cx))
                        .ok();
                    true
                })
        });
    }

    fn clean(
        &mut self,
        kind: CleanupKind,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cleaning = Some(kind);
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.clean_up(kind, paths).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.cleaning = None;
                match result {
                    Ok(freed) => window.push_notification(
                        Notification::success(t_args(
                            "settings.storage.freed",
                            &[("size", &format_bytes(freed))],
                        )),
                        cx,
                    ),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.storage.clean_failed")),
                        cx,
                    ),
                }
                this.load(false, window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_summary(&self, report: &StorageReport, cx: &App) -> AnyElement {
        let games: u64 = report.disks.iter().map(DiskUsage::games_bytes).sum();
        let support: u64 = report.disks.iter().map(DiskUsage::support_bytes).sum();
        let freeable: u64 = report
            .cleanups
            .iter()
            .map(|c| self.selection.bytes(c))
            .sum();
        let theme = cx.theme();
        let stat = |label: SharedString, value: String| {
            v_flex()
                .flex_1()
                .min_w_32()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(label),
                )
                .child(div().text_xl().font_semibold().child(value))
        };
        h_flex()
            .flex_wrap()
            .gap_4()
            .child(stat(t("settings.storage.games"), format_bytes(games)))
            .child(stat(t("settings.storage.support"), format_bytes(support)))
            .child(stat(t("settings.storage.freeable"), format_bytes(freeable)))
            .into_any_element()
    }

    fn render_disk(&self, ix: usize, disk: &DiskUsage, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted_fg, track) = (theme.muted_foreground, theme.muted);
        let colors = [theme.chart_1, theme.chart_2, theme.chart_3];
        let drive = &disk.drive;
        let parts = [
            (t("settings.storage.games"), disk.games_bytes(), colors[0]),
            (
                t("settings.storage.support"),
                disk.support_bytes(),
                colors[1],
            ),
            (t("settings.storage.other"), disk.other_bytes(), colors[2]),
        ];
        let total = drive.total.max(1) as f32;
        let bar = h_flex()
            .w_full()
            .h(px(10.))
            .rounded(px(5.))
            .overflow_hidden()
            .bg(track)
            .children(parts.iter().map(|(_, bytes, color)| {
                div()
                    .h_full()
                    .w(relative((*bytes as f32 / total).clamp(0., 1.)))
                    .bg(*color)
            }));
        let legend = h_flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .text_xs()
            .text_color(muted_fg)
            .children(parts.iter().map(|(label, bytes, color)| {
                h_flex()
                    .gap_1p5()
                    .child(div().size_2p5().rounded_full().bg(*color))
                    .child(t_args(
                        "settings.storage.legend",
                        &[("what", label), ("size", &format_bytes(*bytes))],
                    ))
            }))
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .size_2p5()
                            .rounded_full()
                            .bg(track)
                            .border_1()
                            .border_color(theme.border),
                    )
                    .child(t_args(
                        "settings.storage.legend",
                        &[
                            ("what", &t("settings.storage.free")),
                            ("size", &format_bytes(drive.free)),
                        ],
                    )),
            );
        let support: Vec<AnyElement> = Holding::SUPPORT
            .into_iter()
            .filter_map(|h| Some((h, *disk.support.get(&h)?)))
            .filter(|(_, bytes)| *bytes > 0)
            .map(|(holding, bytes)| {
                h_flex()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded(theme.radius)
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .child(holding_label(holding))
                    .child(div().text_color(muted_fg).child(format_bytes(bytes)))
                    .into_any_element()
            })
            .collect();
        let treemap = (!disk.games.is_empty()).then(|| self.render_treemap(ix, &disk.games, cx));
        panel(cx)
            .id(ElementId::Name(format!("storage-disk-{ix}").into()))
            .gap_3()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_x_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(Icon::new(IconName::HardDrive).text_color(muted_fg))
                            .child(
                                div()
                                    .min_w_0()
                                    .font_semibold()
                                    .truncate()
                                    .child(drive.label()),
                            ),
                    )
                    .child(div().text_sm().text_color(muted_fg).child(t_args(
                        "settings.storage.disk_summary",
                        &[
                            ("free", &format_bytes(drive.free)),
                            ("total", &format_bytes(drive.total)),
                        ],
                    ))),
            )
            .child(bar)
            .child(legend)
            .when(!support.is_empty(), |p| {
                p.child(h_flex().flex_wrap().gap_2().children(support))
            })
            .children(treemap)
            .into_any_element()
    }

    fn render_treemap(
        &self,
        disk: usize,
        games: &[GameSize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut shown: Vec<&GameSize> = games.iter().take(TREEMAP_TILES).collect();
        shown.retain(|g| g.bytes > 0);
        let rest: Vec<&GameSize> = games.iter().skip(TREEMAP_TILES).collect();
        let rest_bytes: u64 = rest.iter().map(|g| g.bytes).sum();
        let mut values: Vec<f64> = shown.iter().map(|g| g.bytes as f64).collect();
        if rest_bytes > 0 {
            values.push(rest_bytes as f64);
        }
        let rects = treemap::squarify(&values, TREEMAP_ASPECT, 1.);
        let library = models(cx).library.read(cx).snapshot.clone();
        let theme = cx.theme();
        let charts = [
            theme.chart_1,
            theme.chart_2,
            theme.chart_3,
            theme.chart_4,
            theme.chart_5,
        ];
        let (radius, popover, popover_fg, muted_fg, rest_fill) = (
            theme.radius,
            theme.popover,
            theme.popover_foreground,
            theme.muted_foreground,
            theme.muted,
        );
        let mut tiles: Vec<AnyElement> = Vec::new();
        for (ix, (game, rect)) in shown.iter().zip(&rects).enumerate() {
            let art = library
                .get(&game.id)
                .and_then(|g| g.hero_art().map(str::to_owned))
                .and_then(|url| irori_ui::components::art_img(&url, TILE_ART_PX, false, cx));
            let id = game.id.clone();
            let label = (rect.w >= LABEL_MIN.0 && rect.h >= LABEL_MIN.1)
                .then(|| tile_label(&game.title, game.bytes, popover, popover_fg, muted_fg));
            let tile = tile_frame(radius, charts[ix % charts.len()])
                .id(ElementId::Name(
                    format!("storage-tile-{disk}-{}", game.id.key()).into(),
                ))
                .keyboard_focusable(cx)
                .cursor_pointer()
                .when_some(art, |d, img| {
                    d.child(
                        img.absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .children(label)
                .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx));
            tiles.push(wrap_tile(*rect, tile.into_any_element()));
        }
        if rest_bytes > 0
            && let Some(rect) = rects.last()
        {
            let title = if rest.len() == 1 {
                t("settings.storage.more_games_one")
            } else {
                t_args(
                    "settings.storage.more_games_other",
                    &[("count", &rest.len().to_string())],
                )
            };
            let label = (rect.w >= LABEL_MIN.0 && rect.h >= LABEL_MIN.1)
                .then(|| tile_label(&title, rest_bytes, popover, popover_fg, muted_fg));
            tiles.push(wrap_tile(
                *rect,
                tile_frame(radius, rest_fill)
                    .children(label)
                    .into_any_element(),
            ));
        }
        v_flex()
            .gap_2()
            .child(SectionTitle::new(t("settings.storage.games_by_size")))
            .child(
                div()
                    .image_cache(self.images.clone())
                    .relative()
                    .w_full()
                    .h(rems(TREEMAP_REM))
                    .children(tiles),
            )
            .into_any_element()
    }

    fn render_cleanup(&self, cleanup: &Cleanup, cx: &mut Context<Self>) -> AnyElement {
        let kind = cleanup.kind;
        let theme = cx.theme();
        let (muted_fg, border) = (theme.muted_foreground, theme.border);
        let chosen = self.selection.items(cleanup);
        let chosen_bytes: u64 = chosen.iter().map(|i| i.bytes).sum();
        let empty = cleanup.items.is_empty();
        let expanded = self.expanded.contains(&kind);
        let cleaning = self.cleaning == Some(kind);
        let summary: SharedString = if empty {
            t("settings.storage.nothing_to_free")
        } else {
            t_args(
                "settings.storage.chosen",
                &[
                    ("chosen", &items_text(chosen.len(), chosen_bytes)),
                    ("offered", &format_bytes(cleanup.bytes())),
                ],
            )
        };
        let header = h_flex()
            .gap_3()
            .items_start()
            .child(
                Icon::new(kind_icon(kind))
                    .text_color(muted_fg)
                    .mt_0p5()
                    .flex_shrink_0(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(kind_text(kind, "title")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(kind_text(kind, "description")),
                    )
                    .child(div().text_xs().text_color(muted_fg).child(summary)),
            )
            .when(!empty, |h| {
                h.child(
                    h_flex()
                        .gap_2()
                        .flex_shrink_0()
                        .child(
                            Button::new(ElementId::Name(
                                format!("storage-show-{}", kind_id(kind)).into(),
                            ))
                            .ghost()
                            .small()
                            .icon(if expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .label(if expanded {
                                t("settings.storage.hide_items")
                            } else {
                                t("settings.storage.show_items")
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.expanded.remove(&kind) {
                                        this.expanded.insert(kind);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Button::new(ElementId::Name(
                                format!("storage-clean-{}", kind_id(kind)).into(),
                            ))
                            .outline()
                            .small()
                            .icon(IconName::BrushCleaning)
                            .label(t("settings.storage.clean_up"))
                            .loading(cleaning)
                            .disabled(chosen.is_empty() || self.cleaning.is_some())
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.confirm(kind, window, cx),
                            )),
                        ),
                )
            });
        let list = (expanded && !empty).then(|| {
            let all = chosen.len() == cleanup.items.len();
            let items = cleanup
                .items
                .iter()
                .take(LISTED_ITEMS)
                .enumerate()
                .map(|(ix, item)| {
                    let path = item.path.clone();
                    h_flex()
                        .gap_3()
                        .child(
                            Checkbox::new(ElementId::Name(
                                format!("storage-item-{}-{ix}", kind_id(kind)).into(),
                            ))
                            .checked(self.selection.is_chosen(kind, &item.path))
                            .label(item_label(item))
                            .on_click(cx.listener(
                                move |this, checked: &bool, _, cx| {
                                    this.selection.set(kind, path.clone(), *checked);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_sm()
                                .text_color(muted_fg)
                                .child(format_bytes(item.bytes)),
                        )
                });
            let more = cleanup.items.len().saturating_sub(LISTED_ITEMS);
            v_flex()
                .gap_2()
                .pt_3()
                .border_t_1()
                .border_color(border)
                .when(cleanup.items.len() > 1, |v| {
                    v.child(
                        Checkbox::new(ElementId::Name(
                            format!("storage-all-{}", kind_id(kind)).into(),
                        ))
                        .checked(all)
                        .label(t("settings.storage.choose_all"))
                        .on_click(cx.listener(
                            move |this, checked: &bool, _, cx| {
                                if let Some(c) = this.report.as_ref().and_then(|r| r.cleanup(kind))
                                {
                                    this.selection.set_all(c, *checked);
                                }
                                cx.notify();
                            },
                        )),
                    )
                })
                .children(items)
                .when(more > 0, |v| {
                    v.child(div().text_xs().text_color(muted_fg).child(t_args(
                        "settings.storage.and_more",
                        &[("count", &more.to_string())],
                    )))
                })
        });
        let warning = (kind == CleanupKind::OrphanPrefixes && !empty).then(|| {
            Alert::warning(
                "storage-prefix-warning",
                t("settings.storage.cleanup.orphan_prefixes.warning"),
            )
        });
        v_flex()
            .gap_3()
            .px_4()
            .py_3()
            .child(header)
            .children(warning)
            .children(list)
            .into_any_element()
    }
}

/// Position `tile` at `rect` inside the treemap, a hairline from its
/// neighbours.
fn wrap_tile(rect: Rect, tile: AnyElement) -> AnyElement {
    div()
        .absolute()
        .left(relative(rect.x))
        .top(relative(rect.y))
        .w(relative(rect.w))
        .h(relative(rect.h))
        .p(px(1.5))
        .child(tile)
        .into_any_element()
}

fn tile_frame(radius: gpui_kit::Pixels, fill: Hsla) -> gpui_kit::Div {
    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .rounded(radius)
        .bg(fill)
}

/// A game's title and size on a chip at the bottom of its tile, in the
/// popover's colours so it reads over any art.
fn tile_label(title: &str, bytes: u64, bg: Hsla, fg: Hsla, muted_fg: Hsla) -> AnyElement {
    v_flex()
        .absolute()
        .left_1()
        .bottom_1()
        .max_w(relative(0.92))
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(bg)
        .text_xs()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(fg)
                .font_medium()
                .child(title.to_string()),
        )
        .child(div().text_color(muted_fg).child(format_bytes(bytes)))
        .into_any_element()
}

impl Render for StoragePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self
            .error
            .clone()
            .map(|e| Alert::error("storage-error", e).title(t("settings.storage.measure_failed")));
        let Some(report) = self.report.clone() else {
            return v_flex()
                .gap_4()
                .children(error)
                .when(self.loading, |v| {
                    v.child(muted(t("settings.storage.measuring"), cx))
                })
                .into_any_element();
        };
        let disks: Vec<AnyElement> = report
            .disks
            .iter()
            .enumerate()
            .map(|(ix, disk)| self.render_disk(ix, disk, cx))
            .collect();
        let border = cx.theme().border;
        let cleanups: Vec<AnyElement> = report
            .cleanups
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let row = self.render_cleanup(c, cx);
                div()
                    .when(ix > 0, |d| d.border_t_1().border_color(border))
                    .child(row)
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_4()
            .children(error)
            .child(self.render_summary(&report, cx))
            .when(disks.is_empty(), |v| {
                v.child(muted(t("settings.storage.no_disks"), cx))
            })
            .children(disks)
            .child(SectionTitle::new(t("settings.storage.free_up")))
            .child(
                v_flex()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(border)
                    .children(cleanups),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, bytes: u64) -> CleanupItem {
        CleanupItem {
            path: path.into(),
            bytes,
            note: ItemNote::None,
        }
    }

    fn report(prefixes: &[&str], logs: bool) -> StorageReport {
        StorageReport {
            disks: Vec::new(),
            cleanups: vec![
                Cleanup {
                    kind: CleanupKind::OrphanPrefixes,
                    items: prefixes.iter().map(|p| item(p, 100)).collect(),
                },
                Cleanup {
                    kind: CleanupKind::Logs,
                    items: if logs { vec![item("/logs", 7)] } else { vec![] },
                },
            ],
        }
    }

    #[test]
    fn prefixes_wait_to_be_chosen_while_the_rest_is_chosen() {
        let report = report(&["/p/A", "/p/B"], true);
        let mut selection = Selection::default();
        selection.sync(&report);
        let prefixes = &report.cleanups[0];
        let logs = &report.cleanups[1];
        assert!(selection.items(prefixes).is_empty());
        assert_eq!(selection.bytes(logs), 7);
        selection.set(CleanupKind::OrphanPrefixes, "/p/B".into(), true);
        assert_eq!(selection.items(prefixes), [&prefixes.items[1]]);
        selection.set_all(prefixes, true);
        assert_eq!(selection.bytes(prefixes), 200);
        selection.set_all(prefixes, false);
        assert_eq!(selection.bytes(prefixes), 0);
    }

    #[test]
    fn a_rescan_keeps_choices_and_forgets_what_is_gone() {
        let mut selection = Selection::default();
        selection.sync(&report(&["/p/A", "/p/B"], true));
        selection.set(CleanupKind::OrphanPrefixes, "/p/A".into(), true);
        selection.set(CleanupKind::Logs, "/logs".into(), false);

        // B went, C is new, logs are still offered.
        let next = report(&["/p/A", "/p/C"], true);
        selection.sync(&next);
        assert!(selection.is_chosen(CleanupKind::OrphanPrefixes, &"/p/A".into()));
        assert!(!selection.is_chosen(CleanupKind::OrphanPrefixes, &"/p/C".into()));
        // What the user left out stays out.
        assert!(!selection.is_chosen(CleanupKind::Logs, &"/logs".into()));

        // C gone again: forgotten.
        selection.sync(&report(&["/p/A"], false));
        assert!(!selection.is_chosen(CleanupKind::OrphanPrefixes, &"/p/C".into()));
        assert_eq!(selection.bytes(&next.cleanups[0]), 100);
    }
}
