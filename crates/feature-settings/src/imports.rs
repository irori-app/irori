//! Unified launcher imports panel for Settings -> Advanced -> Imports.
//! Pairs each detected external launcher's one-click Import button with its
//! optional automatic sync toggle so users never hunt across separated rows.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div,
};
use irori_core::settings::LauncherImports;
use irori_ui::components::EmptyState;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{group_header, group_panel};

/// External launchers that Irori can detect and import from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExternalLauncher {
    Heroic,
    Lutris,
    Bottles,
    Faugus,
    Playnite,
}

impl ExternalLauncher {
    fn meta(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Heroic => (
                "heroic",
                "Heroic",
                "settings.field.action_import_heroic.help",
            ),
            Self::Lutris => (
                "lutris",
                "Lutris",
                "settings.field.action_import_lutris.help",
            ),
            Self::Bottles => (
                "bottles",
                "Bottles",
                "settings.field.action_import_bottles.help",
            ),
            Self::Faugus => (
                "faugus",
                "Faugus Launcher",
                "settings.field.action_import_faugus.help",
            ),
            Self::Playnite => (
                "playnite",
                "Playnite",
                "settings.field.action_import_playnite.help",
            ),
        }
    }

    fn supports_sync(self) -> bool {
        matches!(self, Self::Lutris | Self::Bottles | Self::Faugus)
    }

    fn sync_enabled(self, imports: &LauncherImports) -> bool {
        match self {
            Self::Lutris => imports.lutris,
            Self::Bottles => imports.bottles,
            Self::Faugus => imports.faugus,
            Self::Heroic | Self::Playnite => false,
        }
    }

    fn set_sync(self, imports: &mut LauncherImports, on: bool) {
        match self {
            Self::Lutris => imports.lutris = on,
            Self::Bottles => imports.bottles = on,
            Self::Faugus => imports.faugus = on,
            Self::Heroic | Self::Playnite => {}
        }
    }
}

/// Which external launchers are installed on this computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetectedLaunchers {
    pub heroic: bool,
    pub lutris: bool,
    pub bottles: bool,
    pub faugus: bool,
    pub playnite: bool,
}

impl DetectedLaunchers {
    pub(crate) fn probe() -> Self {
        Self {
            heroic: irori_ui::hooks::heroic_detected(),
            lutris: irori_ui::hooks::lutris_detected(),
            bottles: irori_ui::hooks::bottles_detected(),
            faugus: irori_ui::hooks::faugus_detected(),
            playnite: irori_extras::playnite::is_installed(),
        }
    }

    pub(crate) fn list(self) -> Vec<ExternalLauncher> {
        [
            (ExternalLauncher::Heroic, self.heroic),
            (ExternalLauncher::Lutris, self.lutris),
            (ExternalLauncher::Bottles, self.bottles),
            (ExternalLauncher::Faugus, self.faugus),
            (ExternalLauncher::Playnite, self.playnite),
        ]
        .into_iter()
        .filter_map(|(launcher, present)| present.then_some(launcher))
        .collect()
    }
}

/// Count how many detected launchers currently have automatic sync enabled.
pub(crate) fn active_sync_count(
    launchers: &[ExternalLauncher],
    imports: &LauncherImports,
) -> usize {
    launchers
        .iter()
        .filter(|l| l.supports_sync() && l.sync_enabled(imports))
        .count()
}

/// Unified launcher import and auto-sync table shown on Settings -> Advanced -> Imports.
pub(crate) struct ImportsPanel {
    detected: DetectedLaunchers,
    busy: HashSet<ExternalLauncher>,
    heroic_summary: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl ImportsPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        Self {
            detected: DetectedLaunchers::probe(),
            busy: HashSet::new(),
            heroic_summary: None,
            _subscriptions: vec![cx.observe(&settings, |_, _, cx| cx.notify())],
        }
    }

    #[cfg(test)]
    pub(crate) fn set_detected_for_test(
        &mut self,
        detected: DetectedLaunchers,
        cx: &mut Context<Self>,
    ) {
        self.detected = detected;
        cx.notify();
    }

    /// Number of detected external launchers.
    pub(crate) fn detected_count(&self) -> usize {
        self.detected.list().len()
    }

    fn toggle_sync(&mut self, launcher: ExternalLauncher, on: bool, cx: &mut Context<Self>) {
        if !launcher.supports_sync() {
            return;
        }
        backend(cx)
            .settings
            .update_app(|s| launcher.set_sync(&mut s.launcher_imports, on));
        cx.notify();
    }

    fn run_import(
        &mut self,
        launcher: ExternalLauncher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.contains(&launcher) {
            return;
        }
        let (task, error_key) = match launcher {
            ExternalLauncher::Heroic => return self.import_heroic(window, cx),
            ExternalLauncher::Lutris => (
                irori_ui::hooks::import_from_lutris(cx),
                "settings.error.import_lutris",
            ),
            ExternalLauncher::Bottles => (
                irori_ui::hooks::import_from_bottles(cx),
                "settings.error.import_launcher",
            ),
            ExternalLauncher::Faugus => (
                irori_ui::hooks::import_from_faugus(cx),
                "settings.error.import_launcher",
            ),
            ExternalLauncher::Playnite => (
                irori_ui::hooks::import_from_playnite(cx),
                "settings.error.import_launcher",
            ),
        };
        self.import_summary_launcher(launcher, task, error_key, window, cx);
    }

    fn import_heroic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(ExternalLauncher::Heroic);
        self.heroic_summary = None;
        let task = irori_ui::hooks::import_from_heroic(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&ExternalLauncher::Heroic);
                match result {
                    Ok(s) => {
                        let stores = if s.stores.is_empty() {
                            t("settings.import.no_accounts").to_string()
                        } else {
                            s.stores.join(", ")
                        };
                        this.heroic_summary = Some(t_args(
                            "settings.import.summary",
                            &[
                                ("stores", &stores),
                                ("games", &s.games_with_settings.to_string()),
                                ("favourites", &s.favourites.to_string()),
                                ("categories", &s.categories.to_string()),
                            ],
                        ));
                        models::refresh_library(false, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.error.import_heroic")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_summary_launcher(
        &mut self,
        launcher: ExternalLauncher,
        task: Task<anyhow::Result<irori_ui::hooks::LutrisSummary>>,
        error_key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy.insert(launcher);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&launcher);
                match result {
                    Ok(summary) => {
                        models::refresh_library(false, cx);
                        irori_ui::lutris_summary::open_lutris_summary(summary, window, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t(error_key)),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn render_row(
        &self,
        ix: usize,
        launcher: ExternalLauncher,
        imports_cfg: &LauncherImports,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (id, name, help_key) = launcher.meta();
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let busy = self.busy.contains(&launcher);
        let help = (launcher == ExternalLauncher::Heroic)
            .then(|| self.heroic_summary.clone())
            .flatten()
            .unwrap_or_else(|| t(help_key));
        let sync_control = if launcher.supports_sync() {
            h_flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.imports.auto_sync")),
                )
                .child(
                    Switch::new(ElementId::Name(format!("import-sync-{id}").into()))
                        .checked(launcher.sync_enabled(imports_cfg))
                        .accessibility_label(format!("{} {name}", t("settings.imports.auto_sync")))
                        .on_change(cx.listener(move |this, on: &bool, _, cx| {
                            this.toggle_sync(launcher, *on, cx);
                        })),
                )
                .into_any_element()
        } else {
            Tag::secondary()
                .xsmall()
                .outline()
                .child(t("settings.imports.manual_only"))
                .into_any_element()
        };

        h_flex()
            .id(ElementId::Name(format!("import-row-{id}").into()))
            .debug_selector(move || format!("import-row-{id}"))
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().font_medium().truncate().child(name))
                    .child(div().text_xs().text_color(muted_fg).child(help)),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .flex_shrink_0()
                    .child(sync_control)
                    .child(
                        Button::new(ElementId::Name(format!("import-run-{id}").into()))
                            .outline()
                            .small()
                            .label(t("settings.action.import"))
                            .loading(busy)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_import(launcher, window, cx);
                            })),
                    ),
            )
    }
}

impl Render for ImportsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let launchers = self.detected.list();
        let theme = cx.theme();
        let (muted_fg, primary, success) = (theme.muted_foreground, theme.primary, theme.success);

        if launchers.is_empty() {
            return v_flex()
                .id("advanced-imports-empty")
                .w_full()
                .child(group_header(
                    t("settings.group.imports"),
                    Some(t("settings.group_desc.imports")),
                    true,
                    cx,
                ))
                .child(
                    group_panel(cx).p_6().child(
                        EmptyState::new(IconName::FolderInput, t("settings.imports.empty_title"))
                            .description(t("settings.imports.empty_description")),
                    ),
                );
        }

        let imports_cfg = models(cx).settings.read(cx).app.launcher_imports;
        let found_count = launchers.len();
        let syncing_count = active_sync_count(&launchers, &imports_cfg);
        let found_key = if found_count == 1 {
            "settings.imports.summary_found_one"
        } else {
            "settings.imports.summary_found_other"
        };
        let found_str = found_count.to_string();
        let syncing_str = syncing_count.to_string();
        let sync_dot = if syncing_count > 0 { success } else { muted_fg };

        let summary = crate::advanced::summary_strip("advanced-imports-summary", cx)
            .justify_start()
            .gap_x_4()
            .gap_y_1()
            .child(crate::advanced::dot_stat(
                primary,
                t_args(found_key, &[("count", &found_str)]),
                false,
                cx,
            ))
            .child(crate::advanced::dot_stat(
                sync_dot,
                t_args(
                    "settings.imports.summary_syncing",
                    &[("count", &syncing_str)],
                ),
                true,
                cx,
            ));

        let rows = launchers
            .into_iter()
            .enumerate()
            .fold(group_panel(cx), |panel, (ix, launcher)| {
                panel.child(self.render_row(ix, launcher, &imports_cfg, cx))
            });

        v_flex()
            .id("advanced-imports-panel")
            .w_full()
            .gap_2()
            .child(summary)
            .child(group_header(
                t("settings.group.imports"),
                Some(t("settings.group_desc.imports")),
                true,
                cx,
            ))
            .child(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_detected_launchers_and_counts_active_sync() {
        let detected = DetectedLaunchers {
            heroic: true,
            lutris: true,
            bottles: false,
            faugus: true,
            playnite: false,
        };
        let list = detected.list();
        assert_eq!(
            list,
            vec![
                ExternalLauncher::Heroic,
                ExternalLauncher::Lutris,
                ExternalLauncher::Faugus,
            ]
        );

        let mut cfg = LauncherImports::default();
        assert_eq!(active_sync_count(&list, &cfg), 0);

        cfg.lutris = true;
        cfg.bottles = true; // Not detected, so excluded from active count.
        assert_eq!(active_sync_count(&list, &cfg), 1);

        cfg.faugus = true;
        assert_eq!(active_sync_count(&list, &cfg), 2);
    }
}
