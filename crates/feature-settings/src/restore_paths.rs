//! Settings → Backup → Restore: the paths a chosen backup needs, as found on
//! this computer. A backup made on another computer, user or system names
//! its install folders, prefixes and Wine builds there; Irori moves its own
//! folders and the home folder by itself, and here the user finds the rest
//! ("Locate…" turns one found folder into a move of everything beside it).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, PathPromptOptions, SharedString,
    Styled as _, div,
};
use irori_services::{PathCheck, PathKind, Remap};
use irori_ui::i18n::{t, t_args};

use crate::backup::BackupPanel;
use crate::widgets::muted;

/// Missing paths listed before "N more".
const SHOWN_MISSING: usize = 6;

/// What the restore panel knows about the chosen backup's paths.
#[derive(Default)]
pub(crate) struct RestorePaths {
    /// Folders the user found, from "Locate…".
    pub remaps: Vec<Remap>,
    /// The paths checked; `None` until the check is done.
    pub checks: Option<Vec<PathCheck>>,
    /// Folders Irori moves by itself (Irori's own and the home folder).
    pub automatic: Vec<Remap>,
}

impl RestorePaths {
    pub fn render(&self, cx: &mut Context<BackupPanel>) -> Option<AnyElement> {
        let Some(checks) = &self.checks else {
            return Some(muted(t("settings.backup.paths_checking"), cx).into_any_element());
        };
        if checks.is_empty() && self.automatic.is_empty() {
            return None;
        }
        let found = checks.iter().filter(|c| c.found).count();
        let missing: Vec<&PathCheck> = checks.iter().filter(|c| !c.found).collect();
        let summary = if missing.is_empty() {
            t("settings.backup.paths_all_found")
        } else {
            t_args(
                "settings.backup.paths_found",
                &[
                    ("found", &found.to_string()),
                    ("total", &checks.len().to_string()),
                ],
            )
        };
        let mut moved: Vec<AnyElement> = Vec::new();
        for remap in &self.automatic {
            moved.push(moved_row(remap, None, cx));
        }
        for (ix, remap) in self.remaps.iter().enumerate() {
            moved.push(moved_row(remap, Some(ix), cx));
        }
        let more = missing.len().saturating_sub(SHOWN_MISSING);
        let rows: Vec<AnyElement> = missing
            .iter()
            .take(SHOWN_MISSING)
            .enumerate()
            .map(|(ix, check)| missing_row(ix, check, cx))
            .collect();
        Some(
            v_flex()
                .gap_2()
                .child(div().text_sm().font_medium().child(summary))
                .when(!moved.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .gap_1()
                            .child(muted(t("settings.backup.paths_moved"), cx))
                            .children(moved),
                    )
                })
                .children(rows)
                .when(more > 0, |this| {
                    this.child(muted(
                        t_args(
                            if more == 1 {
                                "settings.backup.paths_more_one"
                            } else {
                                "settings.backup.paths_more_other"
                            },
                            &[("count", &more.to_string())],
                        ),
                        cx,
                    ))
                })
                .into_any_element(),
        )
    }
}

/// One folder that moves: "old → new", with a button to forget the ones
/// the user found.
fn moved_row(remap: &Remap, user: Option<usize>, cx: &mut Context<BackupPanel>) -> AnyElement {
    h_flex()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{} → {}", remap.from, remap.to)),
        )
        .when_some(user, |this, ix| {
            this.child(
                Button::new(SharedString::from(format!("restore-forget-{ix}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .tooltip(t("settings.backup.forget_folder"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if ix < this.paths.remaps.len() {
                            this.paths.remaps.remove(ix);
                            this.plan_paths(cx);
                        }
                    })),
            )
        })
        .into_any_element()
}

/// A path that isn't here, with what fixes it: "Locate…" for a folder,
/// the Wine manager for a build.
fn missing_row(ix: usize, check: &PathCheck, cx: &mut Context<BackupPanel>) -> AnyElement {
    let label = path_label(check);
    let action = match check.kind {
        PathKind::Install | PathKind::Prefix => {
            let original = check.original.clone();
            Button::new(SharedString::from(format!("restore-locate-{ix}")))
                .outline()
                .small()
                .icon(IconName::FolderSearch)
                .label(t("settings.backup.locate"))
                .on_click(cx.listener(move |_, _, window, cx| {
                    let original = original.clone();
                    let rx = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some(t("settings.backup.locate_prompt")),
                    });
                    cx.spawn_in(window, async move |this, cx| {
                        if let Ok(Ok(Some(paths))) = rx.await
                            && let Some(found) = paths.into_iter().next()
                        {
                            let _ = this.update(cx, |this, cx| {
                                let remap = irori_services::remap_from_located(
                                    &original,
                                    &found.to_string_lossy(),
                                );
                                if !this.paths.remaps.contains(&remap) {
                                    this.paths.remaps.push(remap);
                                }
                                this.plan_paths(cx);
                            });
                        }
                    })
                    .detach();
                }))
        }
        PathKind::WineBuild => Button::new(SharedString::from(format!("restore-wine-{ix}")))
            .outline()
            .small()
            .icon(IconName::Wine)
            .label(t("settings.backup.open_wine_manager"))
            .on_click(|_, _, cx| irori_ui::navigate(irori_ui::nav::Route::WineManager, cx)),
    };
    h_flex()
        .gap_3()
        .min_w_0()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().truncate().child(label))
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(check.path.clone()),
                ),
        )
        .child(action)
        .into_any_element()
}

/// What a missing path is for: "Install folder of Hades".
fn path_label(check: &PathCheck) -> SharedString {
    let key = match (check.kind, check.game.is_some()) {
        (PathKind::Install, _) => "settings.backup.path_install",
        (PathKind::Prefix, true) => "settings.backup.path_prefix",
        (PathKind::Prefix, false) => "settings.backup.path_prefix_default",
        (PathKind::WineBuild, true) => "settings.backup.path_wine",
        (PathKind::WineBuild, false) => "settings.backup.path_wine_default",
    };
    t_args(key, &[("title", &check.title)])
}

/// What a restore brought back and what is still missing, below "Backup
/// restored"; `None` when there is nothing to add.
pub(crate) fn restored_details(summary: &irori_services::RestoreSummary) -> Option<SharedString> {
    let count = |n: usize, one: &str, other: &str| {
        t_args(
            if n == 1 { one } else { other },
            &[("count", &n.to_string())],
        )
    };
    let mut parts = Vec::new();
    if summary.installs_attached > 0 {
        parts.push(count(
            summary.installs_attached,
            "settings.backup.restored_installs_one",
            "settings.backup.restored_installs_other",
        ));
    }
    if !summary.missing.is_empty() {
        parts.push(count(
            summary.missing.len(),
            "settings.backup.restored_missing_one",
            "settings.backup.restored_missing_other",
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n").into())
}
