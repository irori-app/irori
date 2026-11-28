//! The Storage section (`Route::Settings("storage")`): every disk holding
//! Irori's games, prefixes, Wine builds, emulators and caches, with a usage
//! bar and a treemap of its games, and the cleanups that free space safely.
//!
//! The report comes from `Backend::storage_report`, measured off the UI
//! thread and cached on disk; opening the section reuses the cache and
//! Rescan measures again. Each cleanup lists what it deletes with a box per
//! item, and asks before deleting, naming how much goes and whether it goes
//! to the trash. The game list sorts installed games by size or by when they
//! were last played, and opens or uninstalls each.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, Hsla, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Subscription, Window, div, px, relative, rems, uniform_list,
};
use irori_core::GameId;
use irori_core::util::format_bytes;
use irori_services::storage::{
    CleanedUp, Cleanup, CleanupItem, CleanupKind, DiskUsage, GameSize, Holding, ItemNote,
    StorageReport,
};
use irori_ui::art::LruImageCache;
use irori_ui::components::SectionTitle;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, caption};
use irori_ui::{Route, backend, models, navigate};

use crate::treemap::{self, Rect};
use crate::widgets::{muted, panel};

/// Maximum width of the Storage content column in `SettingsView`.
pub(crate) const STORAGE_MAX_WIDTH_REM: f32 = 74.;
/// Games drawn as their own treemap tile per disk; the rest share one.
const TREEMAP_TILES: usize = 30;
/// The treemap's width over its height, for the tiles' shapes.
const TREEMAP_ASPECT: f64 = 2.6;
/// Height of the treemap, in rem.
const TREEMAP_REM: f32 = 13.;
/// Tiles narrower or shorter than this share of the treemap carry no label.
const LABEL_MIN: (f32, f32) = (0.09, 0.22);
/// Width of the art thumbnails in treemap tiles, in pixels.
const TILE_ART_PX: u32 = 480;
/// Items of a cleanup listed before "and N more".
const LISTED_ITEMS: usize = 8;
/// Rows of the game list shown before it scrolls.
const GAME_ROWS: usize = 8;
/// Height of a game list row, in rem.
const GAME_ROW_REM: f32 = 3.25;
/// A game not played for this many days is marked as one to consider.
const IDLE_DAYS: i64 = 182;

/// Active workspace tab in the Storage view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StorageTab {
    #[default]
    Games,
    Cleanup,
}

#[derive(Clone, Copy)]
struct Palette {
    bg: Hsla,
    secondary: Hsla,
    border: Hsla,
    muted_fg: Hsla,
    track: Hsla,
    primary: Hsla,
    success: Hsla,
    info: Hsla,
    warning: Hsla,
    radius: gpui_kit::Pixels,
    radius_lg: gpui_kit::Pixels,
}

impl Palette {
    fn of(cx: &App) -> Self {
        let th = cx.theme();
        Self {
            bg: th.background,
            secondary: th.secondary,
            border: th.border,
            muted_fg: th.muted_foreground,
            track: th.muted,
            primary: th.primary,
            success: th.success,
            info: th.info,
            warning: th.warning,
            radius: th.radius,
            radius_lg: th.radius_lg,
        }
    }

    fn tone(self, tone: PillTone) -> (Hsla, Hsla, Hsla) {
        match tone {
            PillTone::Success => (
                self.success,
                self.success.opacity(0.12),
                self.success.opacity(0.28),
            ),
            PillTone::Info => (self.info, self.info.opacity(0.12), self.info.opacity(0.28)),
            PillTone::Warning => (
                self.warning,
                self.warning.opacity(0.13),
                self.warning.opacity(0.32),
            ),
            PillTone::Muted => (self.muted_fg, self.track.opacity(0.55), self.border),
        }
    }
}

#[derive(Clone, Copy)]
enum PillTone {
    Success,
    Info,
    Warning,
    Muted,
}

fn pill_badge(label: SharedString, tone: PillTone, p: Palette) -> gpui_kit::Div {
    let (fg, bg, border) = p.tone(tone);
    h_flex()
        .flex_shrink_0()
        .items_center()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .font_medium()
        .text_color(fg)
        .child(label)
}

fn card_box(p: Palette) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .rounded(p.radius_lg)
        .bg(p.secondary)
        .border_1()
        .border_color(p.border)
        .overflow_hidden()
}

fn count_text(n: usize, one: &str, other: &str) -> SharedString {
    if n == 1 {
        t(one)
    } else {
        t_args(other, &[("count", &n.to_string())])
    }
}

/// Whether an item is chosen until the user says otherwise. A prefix or a
/// folder can hold saves or the user's own files, and a shader cache costs
/// stutter while it is compiled again, so those are picked by hand; the rest
/// is made again or downloaded again.
fn chosen_by_default(kind: CleanupKind) -> bool {
    matches!(
        kind,
        CleanupKind::DownloadLeftovers
            | CleanupKind::UnusedWineBuilds
            | CleanupKind::ArtCache
            | CleanupKind::Logs
    )
}

/// How the game list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GameSort {
    /// Largest first.
    #[default]
    Size,
    /// Played longest ago first, never played before all.
    LastPlayed,
}

/// Every disk's games in `sort` order.
pub(crate) fn sorted_games(report: &StorageReport, sort: GameSort) -> Vec<GameSize> {
    let mut games: Vec<GameSize> = report
        .disks
        .iter()
        .flat_map(|d| d.games.iter().cloned())
        .collect();
    let played = |g: &GameSize| {
        g.last_played
            .as_deref()
            .and_then(irori_core::util::parse_rfc3339)
    };
    match sort {
        GameSort::Size => games.sort_by_key(|g| std::cmp::Reverse(g.bytes)),
        GameSort::LastPlayed => {
            games.sort_by(|a, b| played(a).cmp(&played(b)).then(b.bytes.cmp(&a.bytes)))
        }
    }
    games
}

/// Whole days since `last_played` (RFC 3339), if it was ever played.
pub(crate) fn days_since(last_played: Option<&str>, now: i64) -> Option<i64> {
    let at = irori_core::util::parse_rfc3339(last_played?)?;
    Some((now - at).max(0) / 86_400)
}

/// "Played 3 days ago", "Played 7 months ago", "Never played".
fn played_label(days: Option<i64>) -> SharedString {
    let count = |n: i64, one: &str, other: &str| {
        t_args(
            if n == 1 { one } else { other },
            &[("count", &n.to_string())],
        )
    };
    match days {
        None => t("settings.storage.played_never"),
        Some(0) => t("settings.storage.played_today"),
        Some(d) if d < 60 => count(
            d,
            "settings.storage.played_days_one",
            "settings.storage.played_days_other",
        ),
        Some(d) => count(
            d / 30,
            "settings.storage.played_months_one",
            "settings.storage.played_months_other",
        ),
    }
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
        CleanupKind::SteamLeftovers => "steam_leftovers",
        CleanupKind::StrayFolders => "stray_folders",
        CleanupKind::UnusedWineBuilds => "unused_wine_builds",
        CleanupKind::ShaderCaches => "shader_caches",
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
        CleanupKind::SteamLeftovers => IconName::Gamepad2,
        CleanupKind::StrayFolders => IconName::FolderOpen,
        CleanupKind::UnusedWineBuilds => IconName::Wine,
        CleanupKind::ShaderCaches => IconName::Cpu,
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
        ItemNote::SteamPrefix(app, title) | ItemNote::SteamShaders(app, title) => {
            let prefix = matches!(item.note, ItemNote::SteamPrefix(..));
            let app = app.to_string();
            match (title, prefix) {
                (Some(title), true) => t_args(
                    "settings.storage.item.steam_prefix_of",
                    &[("title", title), ("app", &app)],
                ),
                (Some(title), false) => t_args(
                    "settings.storage.item.steam_shaders_of",
                    &[("title", title), ("app", &app)],
                ),
                (None, true) => t_args("settings.storage.item.steam_prefix", &[("app", &app)]),
                (None, false) => t_args("settings.storage.item.steam_shaders", &[("app", &app)]),
            }
        }
        ItemNote::ShadersOf(title) => t_args(
            "settings.storage.item.shaders_of",
            &[("file", &item.name()), ("title", title)],
        ),
        ItemNote::DriverShaders => t_args(
            "settings.storage.item.driver_shaders",
            &[("name", &item.path.display().to_string())],
        ),
        ItemNote::ArtThumbnails => t("settings.storage.item.art_thumbnails"),
        ItemNote::ArtOriginals => t("settings.storage.item.art_originals"),
        ItemNote::StoreCatalogs => t("settings.storage.item.store_catalogs"),
    }
}

fn kind_priority(kind: CleanupKind) -> u8 {
    match kind {
        CleanupKind::DownloadLeftovers => 0,
        CleanupKind::ArtCache => 1,
        CleanupKind::Logs => 2,
        CleanupKind::UnusedWineBuilds => 3,
        CleanupKind::ShaderCaches => 4,
        CleanupKind::OrphanPrefixes => 5,
        CleanupKind::SteamLeftovers => 6,
        CleanupKind::StrayFolders => 7,
    }
}

fn kind_safety_badge(kind: CleanupKind) -> (&'static str, PillTone) {
    match kind {
        CleanupKind::DownloadLeftovers
        | CleanupKind::ArtCache
        | CleanupKind::Logs
        | CleanupKind::UnusedWineBuilds => ("settings.storage.cleanup.tag_safe", PillTone::Success),
        CleanupKind::ShaderCaches => ("settings.storage.cleanup.tag_optional", PillTone::Info),
        CleanupKind::OrphanPrefixes | CleanupKind::SteamLeftovers | CleanupKind::StrayFolders => (
            "settings.storage.cleanup.tag_check_saves",
            PillTone::Warning,
        ),
    }
}

fn art_cache_limit_text(limit_mib: u32) -> SharedString {
    match limit_mib {
        0 => t("settings.option.art_cache_limit.0"),
        512 => t("settings.option.art_cache_limit.512"),
        1024 => t("settings.option.art_cache_limit.1024"),
        2048 => t("settings.option.art_cache_limit.2048"),
        5120 => t("settings.option.art_cache_limit.5120"),
        10240 => t("settings.option.art_cache_limit.10240"),
        other => format_bytes(u64::from(other) * 1024 * 1024).into(),
    }
}

fn dialog_callout(icon: IconName, text: SharedString, tone: PillTone, p: Palette) -> AnyElement {
    let (fg, bg, border) = p.tone(tone);
    h_flex()
        .gap_2()
        .items_start()
        .px_3()
        .py_2()
        .rounded(p.radius)
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .child(
            Icon::new(icon)
                .small()
                .text_color(fg)
                .mt_0p5()
                .flex_shrink_0(),
        )
        .child(div().flex_1().min_w_0().child(text))
        .into_any_element()
}

struct CleanupPlan {
    title: SharedString,
    intro: SharedString,
    rows: Vec<(SharedString, u64)>,
    bytes: u64,
    trash: bool,
    reloads_library: bool,
    clears_art: bool,
    single_kind: Option<CleanupKind>,
    batch: Vec<(CleanupKind, Vec<PathBuf>)>,
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
    active_tab: StorageTab,
    sort: GameSort,
    selection: Selection,
    cleaning: bool,
    cleaning_kind: Option<CleanupKind>,
    expanded: HashSet<CleanupKind>,
    already_clean_open: bool,
    highlighted_cleanup: Option<CleanupKind>,
    images: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl StoragePanel {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let art = irori_ui::art::art_store(cx);
        let library = models(cx).library.clone();
        let settings = models(cx).settings.clone();
        Self {
            report: None,
            loading: false,
            error: None,
            active_tab: StorageTab::Games,
            sort: GameSort::default(),
            selection: Selection::default(),
            cleaning: false,
            cleaning_kind: None,
            expanded: HashSet::new(),
            already_clean_open: false,
            highlighted_cleanup: None,
            images: LruImageCache::new(TREEMAP_TILES * 2, cx),
            _subscriptions: vec![
                cx.observe(&art, |_, _, cx| cx.notify()),
                cx.observe(&library, |_, _, cx| cx.notify()),
                cx.observe(&settings, |_, _, cx| cx.notify()),
            ],
        }
    }

    pub(crate) fn loading(&self) -> bool {
        self.loading
    }

    pub(crate) fn cleaning(&self) -> bool {
        self.cleaning
    }

    pub(crate) fn selected_cleanup_bytes(&self) -> u64 {
        self.report
            .as_ref()
            .map(|r| r.cleanups.iter().map(|c| self.selection.bytes(c)).sum())
            .unwrap_or(0)
    }

    /// Measure on first show; later shows keep what is on screen.
    pub(crate) fn ensure_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.report.is_none() {
            self.load(false, window, cx);
        }
    }

    /// Jump directly to Free Up Space, optionally expanding and highlighting a
    /// category (used by Settings -> Advanced -> Artwork & metadata).
    pub(crate) fn open_cleanup_focus(
        &mut self,
        focus: Option<CleanupKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_tab = StorageTab::Cleanup;
        if let Some(kind) = focus {
            self.expanded.insert(kind);
            self.highlighted_cleanup = Some(kind);
        }
        self.ensure_loaded(window, cx);
        cx.notify();
    }

    pub(crate) fn confirm_selected_from_header(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_cleanup_bytes() > 0 {
            self.confirm_all_selected(window, cx);
        } else {
            self.active_tab = StorageTab::Cleanup;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_report_for_test(&mut self, report: StorageReport, cx: &mut Context<Self>) {
        self.selection.sync(&report);
        self.report = Some(Arc::new(report));
        self.loading = false;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn active_tab(&self) -> StorageTab {
        self.active_tab
    }

    #[cfg(test)]
    pub(crate) fn is_cleanup_expanded(&self, kind: CleanupKind) -> bool {
        self.expanded.contains(&kind)
    }

    #[cfg(test)]
    pub(crate) fn set_tab_for_test(&mut self, tab: StorageTab, cx: &mut Context<Self>) {
        self.active_tab = tab;
        cx.notify();
    }

    fn cleanup_totals(&self, report: &StorageReport) -> (u64, usize, u64) {
        report
            .cleanups
            .iter()
            .fold((0u64, 0usize, 0u64), |(cb, cc, to), c| {
                let items = self.selection.items(c);
                (
                    cb + items.iter().map(|i| i.bytes).sum::<u64>(),
                    cc + items.len(),
                    to + c.bytes(),
                )
            })
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
        let plan = CleanupPlan {
            title: kind_text(kind, "confirm_title"),
            intro: kind_text(kind, "confirm_description"),
            bytes: items.iter().map(|i| i.bytes).sum(),
            trash: items.iter().any(|i| i.goes_to_trash(kind)),
            reloads_library: items.iter().any(|i| i.reloads_library(kind)),
            clears_art: kind == CleanupKind::ArtCache,
            single_kind: Some(kind),
            rows: items.iter().map(|i| (item_label(i), i.bytes)).collect(),
            batch: vec![(kind, items.into_iter().map(|i| i.path).collect())],
        };
        self.open_cleanup_dialog(plan, window, cx);
    }

    fn confirm_all_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let (mut batch, mut rows, mut bytes) = (Vec::new(), Vec::new(), 0u64);
        let (mut trash, mut reloads_library, mut clears_art) = (false, false, false);
        for cleanup in &report.cleanups {
            let chosen = self.selection.items(cleanup);
            if chosen.is_empty() {
                continue;
            }
            let cat_bytes: u64 = chosen.iter().map(|i| i.bytes).sum();
            bytes += cat_bytes;
            trash |= chosen.iter().any(|i| i.goes_to_trash(cleanup.kind));
            reloads_library |= chosen.iter().any(|i| i.reloads_library(cleanup.kind));
            clears_art |= cleanup.kind == CleanupKind::ArtCache;
            rows.push((
                format!(
                    "{} ({})",
                    kind_text(cleanup.kind, "title"),
                    items_text(chosen.len(), cat_bytes)
                )
                .into(),
                cat_bytes,
            ));
            batch.push((
                cleanup.kind,
                chosen.into_iter().map(|i| i.path.clone()).collect(),
            ));
        }
        if batch.is_empty() {
            return;
        }
        self.open_cleanup_dialog(
            CleanupPlan {
                title: t_args(
                    "settings.storage.cleanup.confirm_all_title",
                    &[("size", &format_bytes(bytes))],
                ),
                intro: t("settings.storage.cleanup.confirm_all_desc"),
                rows,
                bytes,
                trash,
                reloads_library,
                clears_art,
                single_kind: None,
                batch,
            },
            window,
            cx,
        );
    }

    fn open_cleanup_dialog(
        &mut self,
        plan: CleanupPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let p = Palette::of(cx);
            let listed = plan.rows.iter().take(LISTED_ITEMS).map(|(label, size)| {
                h_flex()
                    .gap_3()
                    .text_sm()
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(p.muted_fg)
                            .child(format_bytes(*size)),
                    )
            });
            let more = plan.rows.len().saturating_sub(LISTED_ITEMS);
            let description = v_flex()
                .gap_2()
                .child(div().child(plan.intro.clone()))
                .child(v_flex().gap_1().children(listed).when(more > 0, |v| {
                    v.child(div().text_xs().text_color(p.muted_fg).child(t_args(
                        "settings.storage.and_more",
                        &[("count", &more.to_string())],
                    )))
                }))
                .when(plan.trash, |v| {
                    v.child(
                        div()
                            .text_xs()
                            .text_color(p.muted_fg)
                            .child(t("settings.storage.trash_note")),
                    )
                })
                .when(!plan.trash, |v| {
                    v.child(dialog_callout(
                        IconName::ShieldCheck,
                        t("settings.storage.cleanup.kept_intact"),
                        PillTone::Success,
                        p,
                    ))
                })
                .when(plan.reloads_library, |v| {
                    v.child(dialog_callout(
                        IconName::RefreshCw,
                        t("settings.storage.cleanup.reload_library_note"),
                        PillTone::Info,
                        p,
                    ))
                });
            let ok_key = if plan.trash {
                "settings.storage.confirm_trash"
            } else if plan.reloads_library {
                "settings.storage.cleanup.confirm_reload_ok"
            } else {
                "settings.storage.confirm_ok"
            };
            let (batch, weak) = (plan.batch.clone(), weak.clone());
            let (single_kind, clears_art, reloads_library) =
                (plan.single_kind, plan.clears_art, plan.reloads_library);
            alert
                .title(plan.title.clone())
                .description(description)
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t_args(ok_key, &[("size", &format_bytes(plan.bytes))]))
                .ok_variant(if plan.trash {
                    ButtonVariant::Danger
                } else {
                    ButtonVariant::Primary
                })
                .on_ok(move |_, window, cx| {
                    let batch = batch.clone();
                    weak.update(cx, |this, cx| {
                        this.clean_batch(
                            single_kind,
                            batch,
                            clears_art,
                            reloads_library,
                            window,
                            cx,
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    fn clean_batch(
        &mut self,
        single_kind: Option<CleanupKind>,
        batch: Vec<(CleanupKind, Vec<PathBuf>)>,
        clears_art: bool,
        reloads_library: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cleaning = true;
        self.cleaning_kind = single_kind;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.clean_up_many(batch).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.cleaning = false;
                this.cleaning_kind = None;
                this.highlighted_cleanup = None;
                match result {
                    Ok(done) => {
                        if clears_art {
                            irori_ui::art::art_store(cx)
                                .update(cx, |store, cx| store.clear_memory_cache(cx));
                        }
                        if reloads_library {
                            models::refresh_library(true, cx);
                        }
                        window.push_notification(
                            Notification::success(cleaned_text(done, reloads_library)),
                            cx,
                        );
                    }
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
            .debug_selector(move || format!("storage-disk-{ix}"))
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

    fn render_tabs(&self, report: &StorageReport, cx: &mut Context<Self>) -> AnyElement {
        let game_count: usize = report.disks.iter().map(|d| d.games.len()).sum();
        let chosen_bytes = self.selected_cleanup_bytes();
        let games_label: SharedString =
            format!("{} ({game_count})", t("settings.storage.tab.games")).into();
        let cleanup_label: SharedString = if chosen_bytes > 0 {
            format!(
                "{} ({})",
                t("settings.storage.tab.cleanup"),
                format_bytes(chosen_bytes)
            )
            .into()
        } else {
            t("settings.storage.tab.cleanup")
        };
        div()
            .w_full()
            .child(
                TabBar::new("storage-tabs")
                    .underline()
                    .large()
                    .selected_index(usize::from(self.active_tab == StorageTab::Cleanup))
                    .child(Tab::new().label(games_label))
                    .child(Tab::new().label(cleanup_label))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.active_tab = if *ix == 1 {
                            StorageTab::Cleanup
                        } else {
                            StorageTab::Games
                        };
                        cx.notify();
                    })),
            )
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

    /// Every installed game with its size and when it was last played, in
    /// the chosen order, each with Open folder and Uninstall.
    fn render_games(&self, report: &StorageReport, cx: &mut Context<Self>) -> AnyElement {
        let games = sorted_games(report, self.sort);
        let library = models(cx).library.read(cx).snapshot.clone();
        let now = irori_core::util::unix_now() as i64;
        let rows: Rc<Vec<GameRow>> = Rc::new(
            games
                .into_iter()
                .map(|g| {
                    let game = library.get(&g.id);
                    GameRow {
                        folder: game
                            .and_then(|g| g.install.as_ref())
                            .map(|i| PathBuf::from(&i.install_path))
                            .filter(|p| !p.as_os_str().is_empty()),
                        can_uninstall: g.id.runner.manages_installs(),
                        days: days_since(g.last_played.as_deref(), now),
                        game: g,
                    }
                })
                .collect(),
        );
        let count = rows.len();
        let sort_button = |id: &'static str, sort: GameSort, label: &str| {
            Button::new(id)
                .small()
                .when(self.sort == sort, |b| b.primary())
                .when(self.sort != sort, |b| b.ghost())
                .label(t(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort = sort;
                    cx.notify();
                }))
        };
        let list = uniform_list("storage-games", count, move |range, _, cx| {
            range
                .filter_map(|ix| rows.get(ix).map(|row| game_row(ix, row, cx)))
                .collect()
        })
        .h(rems(GAME_ROW_REM * count.clamp(1, GAME_ROWS) as f32));
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(SectionTitle::new(t("settings.storage.game_list")))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(sort_button(
                                "storage-sort-size",
                                GameSort::Size,
                                "settings.storage.sort_size",
                            ))
                            .child(sort_button(
                                "storage-sort-played",
                                GameSort::LastPlayed,
                                "settings.storage.sort_last_played",
                            )),
                    ),
            )
            .child(
                div()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .py_1()
                    .child(list),
            )
            .into_any_element()
    }

    fn render_cleanup_tab(&self, report: &StorageReport, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let mut active: Vec<&Cleanup> = report
            .cleanups
            .iter()
            .filter(|c| !c.items.is_empty())
            .collect();
        active.sort_by_key(|c| kind_priority(c.kind));
        let empty: Vec<&Cleanup> = report
            .cleanups
            .iter()
            .filter(|c| c.items.is_empty())
            .collect();
        let (chosen_bytes, chosen_count, total_offered) = self.cleanup_totals(report);
        let optional_bytes = total_offered.saturating_sub(chosen_bytes);
        let all_clear = active.is_empty();
        let (summary_title, summary_desc) = if all_clear {
            (
                t("settings.storage.cleanup.all_clear_title"),
                t("settings.storage.cleanup.all_clear_desc"),
            )
        } else if chosen_bytes > 0 {
            let chosen_str = items_text(chosen_count, chosen_bytes);
            let desc = if optional_bytes > 0 {
                t_args(
                    "settings.storage.cleanup.ready_desc_with_optional",
                    &[
                        ("chosen", &chosen_str),
                        ("optional", &format_bytes(optional_bytes)),
                    ],
                )
            } else {
                t_args(
                    "settings.storage.cleanup.ready_desc",
                    &[("chosen", &chosen_str)],
                )
            };
            (
                t_args(
                    "settings.storage.cleanup.ready_title",
                    &[("size", &format_bytes(chosen_bytes))],
                ),
                desc,
            )
        } else {
            (
                t_args(
                    "settings.storage.cleanup.optional_title",
                    &[("size", &format_bytes(total_offered))],
                ),
                t("settings.storage.cleanup.optional_desc"),
            )
        };
        let summary_card = h_flex()
            .w_full()
            .p_4()
            .gap_4()
            .justify_between()
            .items_center()
            .flex_wrap()
            .rounded(p.radius_lg)
            .bg(if all_clear {
                p.success.opacity(0.08)
            } else if chosen_bytes > 0 {
                p.primary.opacity(0.08)
            } else {
                p.secondary
            })
            .border_1()
            .border_color(if all_clear {
                p.success.opacity(0.32)
            } else if chosen_bytes > 0 {
                p.primary.opacity(0.35)
            } else {
                p.border
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(16.))
                    .gap_0p5()
                    .child(div().text_base().font_semibold().child(summary_title))
                    .child(div().text_xs().text_color(p.muted_fg).child(summary_desc)),
            )
            .child(
                Button::new("storage-clean-all-selected")
                    .primary()
                    .icon(IconName::BrushCleaning)
                    .label(if chosen_bytes > 0 {
                        t_args(
                            "settings.storage.cleanup.clean_selected",
                            &[("size", &format_bytes(chosen_bytes))],
                        )
                    } else {
                        t("settings.storage.cleanup.nothing_selected")
                    })
                    .loading(self.cleaning && self.cleaning_kind.is_none())
                    .disabled(chosen_bytes == 0 || self.cleaning)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.confirm_all_selected(window, cx)),
                    ),
            );
        let active_rows: Vec<AnyElement> = active
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let row = self.render_cleanup(c, cx);
                div()
                    .when(ix > 0, |d| d.border_t_1().border_color(p.border))
                    .child(row)
                    .into_any_element()
            })
            .collect();
        let already_clean = (!empty.is_empty()).then(|| self.render_already_clean(&empty, cx));
        v_flex()
            .id("storage-tab-cleanup")
            .debug_selector(|| "storage-tab-cleanup".into())
            .w_full()
            .gap_3()
            .child(summary_card)
            .when(!active_rows.is_empty(), |v| {
                v.child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(caption(
                            t_args(
                                "settings.storage.cleanup.section_ready",
                                &[("count", &active.len().to_string())],
                            ),
                            cx,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_fg)
                                .child(format_bytes(total_offered)),
                        ),
                )
                .child(card_box(p).children(active_rows))
            })
            .children(already_clean)
            .into_any_element()
    }

    fn render_cleanup(&self, cleanup: &Cleanup, cx: &mut Context<Self>) -> AnyElement {
        let kind = cleanup.kind;
        let p = Palette::of(cx);
        let chosen = self.selection.items(cleanup);
        let chosen_bytes: u64 = chosen.iter().map(|i| i.bytes).sum();
        let expanded = self.expanded.contains(&kind);
        let cleaning = self.cleaning && self.cleaning_kind == Some(kind);
        let focused = self.highlighted_cleanup == Some(kind);
        let (safety_key, safety_tone) = kind_safety_badge(kind);
        let summary = t_args(
            "settings.storage.chosen",
            &[
                ("chosen", &items_text(chosen.len(), chosen_bytes)),
                ("offered", &format_bytes(cleanup.bytes())),
            ],
        );
        let art_limit_row = (kind == CleanupKind::ArtCache).then(|| {
            let limit = models(cx).settings.read(cx).app.art_cache_limit;
            h_flex()
                .mt_1p5()
                .px_2p5()
                .py_1p5()
                .gap_3()
                .justify_between()
                .items_center()
                .flex_wrap()
                .rounded(p.radius)
                .bg(p.bg.opacity(0.65))
                .border_1()
                .border_color(p.border)
                .child(div().text_xs().text_color(p.muted_fg).child(t_args(
                    "settings.storage.cleanup.art_limit_label",
                    &[("limit", &art_cache_limit_text(limit))],
                )))
                .child(
                    Button::new("storage-open-art-limit")
                        .ghost()
                        .xsmall()
                        .label(t("settings.storage.cleanup.change_in_advanced"))
                        .on_click(|_, _, cx| navigate(Route::Settings("advanced".into()), cx)),
                )
        });
        let header = h_flex()
            .w_full()
            .gap_3()
            .items_start()
            .child(
                h_flex()
                    .w_8()
                    .h_8()
                    .mt_0p5()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(p.radius)
                    .bg(if chosen_bytes > 0 {
                        p.primary.opacity(0.13)
                    } else {
                        p.bg
                    })
                    .border_1()
                    .border_color(if chosen_bytes > 0 {
                        p.primary.opacity(0.32)
                    } else {
                        p.border
                    })
                    .child(
                        Icon::new(kind_icon(kind))
                            .small()
                            .text_color(if chosen_bytes > 0 {
                                p.primary
                            } else {
                                p.muted_fg
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(kind_text(kind, "title")),
                            )
                            .child(pill_badge(t(safety_key), safety_tone, p))
                            .when(kind == CleanupKind::ArtCache, |h| {
                                h.child(pill_badge(
                                    t("settings.storage.cleanup.tag_reloads_library"),
                                    PillTone::Muted,
                                    p,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_fg)
                            .child(kind_text(kind, "description")),
                    )
                    .child(div().text_xs().text_color(p.muted_fg).child(summary))
                    .children(art_limit_row),
            )
            .child(
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
                        .label(t(if expanded {
                            "settings.storage.hide_items"
                        } else {
                            "settings.storage.show_items"
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&kind) {
                                this.expanded.insert(kind);
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        Button::new(ElementId::Name(
                            format!("storage-clean-{}", kind_id(kind)).into(),
                        ))
                        .when(chosen_bytes > 0, |b| b.primary())
                        .when(chosen_bytes == 0, |b| b.outline())
                        .small()
                        .icon(IconName::BrushCleaning)
                        .label(t("settings.storage.clean_up"))
                        .loading(cleaning)
                        .disabled(chosen.is_empty() || self.cleaning)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.confirm(kind, window, cx)),
                        ),
                    ),
            );
        let list = expanded.then(|| {
            let all = chosen.len() == cleanup.items.len();
            let items = cleanup
                .items
                .iter()
                .take(LISTED_ITEMS)
                .enumerate()
                .map(|(ix, item)| {
                    let path = item.path.clone();
                    let reloads = item.reloads_library(kind);
                    h_flex()
                        .w_full()
                        .gap_3()
                        .px_2p5()
                        .py_1p5()
                        .rounded(p.radius)
                        .bg(p.secondary)
                        .border_1()
                        .border_color(p.border)
                        .items_center()
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
                        .when(reloads, |h| {
                            h.child(pill_badge(
                                t("settings.storage.cleanup.tag_reloads_library"),
                                PillTone::Info,
                                p,
                            ))
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .font_medium()
                                .text_color(p.muted_fg)
                                .child(format_bytes(item.bytes)),
                        )
                });
            let more = cleanup.items.len().saturating_sub(LISTED_ITEMS);
            v_flex()
                .gap_2()
                .mt_2()
                .p_3()
                .rounded(p.radius)
                .bg(p.bg.opacity(0.55))
                .border_1()
                .border_color(p.border)
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
                    v.child(div().text_xs().text_color(p.muted_fg).child(t_args(
                        "settings.storage.and_more",
                        &[("count", &more.to_string())],
                    )))
                })
        });
        let warning = matches!(
            kind,
            CleanupKind::OrphanPrefixes | CleanupKind::StrayFolders | CleanupKind::SteamLeftovers
        )
        .then(|| {
            Alert::warning(
                ElementId::Name(format!("storage-warning-{}", kind_id(kind)).into()),
                kind_text(kind, "warning"),
            )
        });
        v_flex()
            .id(ElementId::Name(
                format!("storage-row-{}", kind_id(kind)).into(),
            ))
            .debug_selector(move || format!("storage-row-{}", kind_id(kind)))
            .w_full()
            .gap_2()
            .px_4()
            .py_3p5()
            .when(focused, |v| {
                v.bg(p.primary.opacity(0.06))
                    .border_l_2()
                    .border_color(p.primary)
            })
            .child(header)
            .children(warning)
            .children(list)
            .into_any_element()
    }

    fn render_already_clean(&self, empty: &[&Cleanup], cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let label = count_text(
            empty.len(),
            "settings.storage.cleanup.already_clean_one",
            "settings.storage.cleanup.already_clean_other",
        );
        let header = h_flex()
            .id("storage-already-clean-toggle")
            .w_full()
            .px_4()
            .py_3()
            .justify_between()
            .items_center()
            .gap_2()
            .keyboard_focusable(cx)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.already_clean_open = !this.already_clean_open;
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .small()
                            .text_color(p.success),
                    )
                    .child(div().text_sm().text_color(p.muted_fg).child(label)),
            )
            .child(
                Icon::new(if self.already_clean_open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .small()
                .text_color(p.muted_fg),
            );
        let body = self.already_clean_open.then(|| {
            h_flex()
                .w_full()
                .flex_wrap()
                .gap_2()
                .p_3()
                .border_t_1()
                .border_color(p.border)
                .bg(p.bg.opacity(0.35))
                .children(empty.iter().map(|c| {
                    h_flex()
                        .flex_1()
                        .min_w(rems(18.))
                        .gap_2()
                        .items_start()
                        .p_2p5()
                        .rounded(p.radius)
                        .bg(p.secondary)
                        .border_1()
                        .border_color(p.border)
                        .child(
                            Icon::new(kind_icon(c.kind))
                                .small()
                                .text_color(p.muted_fg)
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
                                        .text_xs()
                                        .font_medium()
                                        .child(kind_text(c.kind, "title")),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(p.muted_fg)
                                        .child(kind_text(c.kind, "description")),
                                ),
                        )
                }))
        });
        card_box(p).child(header).children(body).into_any_element()
    }
}

/// One game of the game list, ready to draw.
struct GameRow {
    game: GameSize,
    folder: Option<PathBuf>,
    can_uninstall: bool,
    days: Option<i64>,
}

fn game_row(ix: usize, row: &GameRow, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (muted_fg, warning, radius, border) = (
        theme.muted_foreground,
        theme.warning,
        theme.radius,
        theme.border,
    );
    let id: GameId = row.game.id.clone();
    let idle = row.days.is_some_and(|d| d >= IDLE_DAYS);
    let folder = row.folder.clone();
    h_flex()
        .id(ElementId::Name(format!("storage-game-{ix}").into()))
        .h(rems(GAME_ROW_REM))
        .px_4()
        .gap_3()
        .when(ix > 0, |d| d.border_t_1().border_color(border))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .truncate()
                        .child(row.game.title.clone()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(div().truncate().child(played_label(row.days)))
                        .when(idle, |h| {
                            h.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .rounded(radius)
                                    .border_1()
                                    .border_color(warning)
                                    .text_color(warning)
                                    .child(t("settings.storage.idle")),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .text_color(muted_fg)
                .child(format_bytes(row.game.bytes)),
        )
        .child(
            Button::new(ElementId::Name(format!("storage-open-{ix}").into()))
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .tooltip(t("settings.storage.open_folder"))
                .disabled(folder.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(folder) = &folder {
                        cx.open_with_system(folder);
                    }
                }),
        )
        .when(row.can_uninstall, |h| {
            let id = id.clone();
            h.child(
                Button::new(ElementId::Name(format!("storage-uninstall-{ix}").into()))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(t("settings.storage.uninstall"))
                    .on_click(move |_, window, cx| {
                        irori_ui::hooks::uninstall(id.clone(), window, cx)
                    }),
            )
        })
        .child(
            Button::new(ElementId::Name(format!("storage-page-{ix}").into()))
                .ghost()
                .small()
                .icon(IconName::ChevronRight)
                .tooltip(t("settings.storage.open_game"))
                .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx)),
        )
        .into_any_element()
}

/// What a finished cleanup says.
fn cleaned_text(done: CleanedUp, reloaded_library: bool) -> SharedString {
    let (freed, trashed) = (format_bytes(done.freed), format_bytes(done.trashed));
    if reloaded_library && done.trashed == 0 {
        return t_args(
            "settings.storage.cleanup.freed_and_reloaded",
            &[("size", &freed)],
        );
    }
    match (done.freed > 0, done.trashed > 0) {
        (_, false) => t_args("settings.storage.freed", &[("size", &freed)]),
        (false, true) => t_args("settings.storage.trashed", &[("size", &trashed)]),
        (true, true) => t_args(
            "settings.storage.freed_and_trashed",
            &[("freed", &freed), ("trashed", &trashed)],
        ),
    }
}

/// Position `tile` at `rect` inside the treemap, a hairline from its neighbours.
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
        let refreshing = models(cx).library.read(cx).refreshing;
        let (info, radius) = (cx.theme().info, cx.theme().radius);
        let tab_body: AnyElement = match self.active_tab {
            StorageTab::Games => {
                let disks: Vec<AnyElement> = report
                    .disks
                    .iter()
                    .enumerate()
                    .map(|(ix, disk)| self.render_disk(ix, disk, cx))
                    .collect();
                v_flex()
                    .id("storage-tab-games")
                    .debug_selector(|| "storage-tab-games".into())
                    .w_full()
                    .gap_3()
                    .when(disks.is_empty(), |v| {
                        v.child(muted(t("settings.storage.no_disks"), cx))
                    })
                    .children(disks)
                    .when(report.disks.iter().any(|d| !d.games.is_empty()), |v| {
                        v.child(self.render_games(&report, cx))
                    })
                    .into_any_element()
            }
            StorageTab::Cleanup => self.render_cleanup_tab(&report, cx),
        };
        v_flex()
            .w_full()
            .gap_3()
            .children(error)
            .when(refreshing, |v| {
                v.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded(radius)
                        .bg(info.opacity(0.11))
                        .border_1()
                        .border_color(info.opacity(0.3))
                        .text_xs()
                        .text_color(info)
                        .child(Icon::new(IconName::RefreshCw).small())
                        .child(t("settings.storage.cleanup.refreshing_library")),
                )
            })
            .child(self.render_summary(&report, cx))
            .child(self.render_tabs(&report, cx))
            .child(tab_body)
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

    fn sized(app: &str, bytes: u64, played: Option<&str>) -> GameSize {
        GameSize {
            id: GameId::new(irori_core::Runner::Gog, app),
            title: app.into(),
            bytes,
            last_played: played.map(str::to_owned),
        }
    }

    #[test]
    fn games_sort_by_size_or_by_how_long_ago_they_were_played() {
        let disk = |games| DiskUsage {
            drive: irori_services::storage::Drive {
                name: String::new(),
                mount: "/".into(),
                total: 1,
                free: 0,
            },
            games,
            support: Default::default(),
        };
        let report = StorageReport {
            disks: vec![
                disk(vec![
                    sized("big", 900, Some("2026-11-01T10:00:00Z")),
                    sized("old", 100, Some("2025-01-01T10:00:00Z")),
                ]),
                disk(vec![sized("never", 50, None)]),
            ],
            cleanups: Vec::new(),
        };
        let order = |sort| -> Vec<String> {
            sorted_games(&report, sort)
                .into_iter()
                .map(|g| g.title)
                .collect()
        };
        assert_eq!(order(GameSort::Size), ["big", "old", "never"]);
        assert_eq!(order(GameSort::LastPlayed), ["never", "old", "big"]);
    }

    #[test]
    fn days_since_counts_whole_days() {
        let now = irori_core::util::parse_rfc3339("2026-11-10T12:00:00Z").unwrap();
        assert_eq!(days_since(Some("2026-11-10T08:00:00Z"), now), Some(0));
        assert_eq!(days_since(Some("2026-05-01T12:00:00Z"), now), Some(193));
        assert_eq!(days_since(None, now), None);
        assert_eq!(days_since(Some("garbage"), now), None);
        // A clock behind the record never counts backwards.
        assert_eq!(days_since(Some("2026-12-01T00:00:00Z"), now), Some(0));
    }

    #[test]
    fn what_may_hold_saves_waits_to_be_chosen() {
        for kind in [
            CleanupKind::OrphanPrefixes,
            CleanupKind::SteamLeftovers,
            CleanupKind::StrayFolders,
            CleanupKind::ShaderCaches,
        ] {
            assert!(!chosen_by_default(kind), "{kind:?}");
        }
        assert!(chosen_by_default(CleanupKind::Logs));
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
