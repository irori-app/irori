//! RenoDX HDR mods for one game, in the game's Tools section below ReShade:
//! pick a mod from RenoDX's official list (the game's own preselected when
//! the list names it) or choose a mod file, install it (asking first when
//! the game has anticheat; ReShade's build with add-on support comes with
//! it), turn it on or off, update it when RenoDX rebuilt it, and remove it.

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_core::process::CancellationToken;
use irori_reshade::renodx::MODS_PAGE;
use irori_reshade::{GraphicsMod, Manifest, Step};
use irori_services::{RenodxChoice, RenodxStatus, supports_reshade};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::form::ChoiceItem;
use crate::graphics_mods::{
    self, GraphicsModsChanged, Report, progress_channel, removed_notification, step_text,
};
use crate::widgets::{RowState, field_row, group_panel, muted};

enum Load {
    Idle,
    Loading,
    Ready(Box<RenodxStatus>),
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Installing,
    Toggling,
    Removing,
}

fn error_text(e: &anyhow::Error) -> SharedString {
    graphics_mods::error_text(e, GraphicsMod::Renodx)
}

/// The mod preselected in the list: the one picked before when it is still
/// offered, else the best one the list names for the game.
pub(crate) fn preselect(status: &RenodxStatus, picked: Option<&str>) -> Option<String> {
    let catalog = status.catalog.as_ref()?;
    picked
        .filter(|id| catalog.get(id).is_some())
        .map(String::from)
        .or_else(|| status.suggested.first().cloned())
}

/// The help under the not-installed row: what RenoDX is, what installing
/// changes about ReShade, and how the list matched the game.
pub(crate) fn not_installed_help(status: &RenodxStatus) -> String {
    let mut lines = vec![t("settings.renodx.help").to_string()];
    if status.reshade.as_ref().is_some_and(|m| !m.addon) {
        lines.push(t("settings.renodx.switch_note").to_string());
    }
    match (&status.catalog, &status.catalog_error) {
        (Some(catalog), _) => match status.suggested.first().and_then(|id| catalog.get(id)) {
            Some(m) => {
                lines.push(t_args("settings.renodx.suggested", &[("mod", &m.label())]).to_string())
            }
            None => lines.push(t("settings.renodx.no_match").to_string()),
        },
        (None, Some(error)) => {
            lines.push(t_args("settings.renodx.catalog_failed", &[("error", error)]).to_string())
        }
        (None, None) => {}
    }
    if !cfg!(target_os = "windows") {
        lines.push(t("settings.renodx.wine_note").to_string());
    }
    lines.join("\n")
}

/// The RenoDX rows of one game.
pub(crate) struct RenodxPanel {
    id: GameId,
    load: Load,
    busy: Option<Busy>,
    step: Option<Step>,
    percent: Option<f32>,
    cancel: Option<CancellationToken>,
    select: Entity<SelectState<Vec<ChoiceItem>>>,
    /// The mod picked in the list.
    picked: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GraphicsModsChanged> for RenodxPanel {}

impl RenodxPanel {
    pub(crate) fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let select = cx.new(|cx| {
            SelectState::new(Vec::<ChoiceItem>::new(), None, window, cx).searchable(true)
        });
        let subscription = cx.subscribe_in(
            &select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(key) = event;
                this.picked = key.as_ref().map(|k| k.to_string());
                cx.notify();
            },
        );
        Self {
            id,
            load: Load::Idle,
            busy: None,
            step: None,
            percent: None,
            cancel: None,
            select,
            picked: None,
            _subscriptions: vec![subscription],
        }
    }

    fn applies(&self, cx: &Context<Self>) -> bool {
        backend(cx)
            .game(&self.id)
            .is_some_and(|g| supports_reshade(&g))
    }

    fn title(&self, cx: &Context<Self>) -> String {
        backend(cx)
            .game(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string())
    }

    /// Read the install state the first time the panel is shown.
    pub(crate) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.load, Load::Idle) && self.applies(cx) {
            self.reload(window, cx);
        }
    }

    /// Read the install state again when it was read before: ReShade
    /// changed.
    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.load, Load::Idle) && self.busy.is_none() {
            self.reload(window, cx);
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load = Load::Loading;
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move { b.renodx_status(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(status) => {
                        this.fill_list(&status, window, cx);
                        this.load = Load::Ready(Box::new(status));
                    }
                    Err(e) => this.load = Load::Failed(error_text(&e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn fill_list(&mut self, status: &RenodxStatus, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<ChoiceItem> = status
            .catalog
            .iter()
            .flat_map(|c| c.mods.iter())
            .map(|m| ChoiceItem::new(m.id.clone(), m.label()))
            .collect();
        let picked = preselect(status, self.picked.as_deref());
        self.select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            if let Some(key) = &picked {
                s.set_selected_value(&SharedString::from(key.clone()), window, cx);
            }
        });
        self.picked = picked;
    }

    fn status(&self) -> Option<&RenodxStatus> {
        match &self.load {
            Load::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// Install, after asking when the game has anticheat.
    fn ask_install(&mut self, choice: RenodxChoice, window: &mut Window, cx: &mut Context<Self>) {
        let anticheat = self
            .status()
            .map(|s| s.anticheat.join(", "))
            .unwrap_or_default();
        let installed = self.status().is_some_and(|s| s.manifest.is_some());
        if anticheat.is_empty() || installed {
            self.install(choice, window, cx);
            return;
        }
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let choice = choice.clone();
            alert
                .title(t_args(
                    "settings.renodx.anticheat.title",
                    &[("title", &title)],
                ))
                .description(t_args(
                    "settings.renodx.anticheat.description",
                    &[("title", &title), ("anticheat", &anticheat)],
                ))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.renodx.anticheat.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.install(choice.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn install_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.picked.clone() {
            self.ask_install(RenodxChoice::Mod(id), window, cx);
        }
    }

    fn choose_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.renodx.choose_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(file) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    this.ask_install(RenodxChoice::File(file), window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn install(&mut self, choice: RenodxChoice, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Installing);
        self.step = None;
        self.percent = None;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (step, progress, mut rx) = progress_channel();
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move {
            b.install_renodx(&id, choice, step, progress, cancel).await
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Some(report) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match report {
                        Report::Step(s) => {
                            this.step = Some(s);
                            this.percent = None;
                        }
                        Report::Percent(p) => this.percent = p,
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let title = self.title(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.cancel = None;
                this.step = None;
                match result {
                    Ok(_) => window.push_notification(
                        Notification::success(t_args(
                            "settings.renodx.installed_notice",
                            &[("title", &title)],
                        )),
                        cx,
                    ),
                    Err(e) if irori_runtime::is_cancelled(&e) => window
                        .push_notification(Notification::info(t("settings.renodx.cancelled")), cx),
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e))
                            .title(t("settings.renodx.install_failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
                cx.emit(GraphicsModsChanged);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_enabled(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Toggling);
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move {
            b.set_graphics_mod_enabled(&id, GraphicsMod::Renodx, enabled)
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.renodx.failed")),
                        cx,
                    );
                }
                this.reload(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t_args("settings.renodx.remove.title", &[("title", &title)]))
                .description(t("settings.renodx.remove.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.renodx.remove.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.remove(window, cx)).ok();
                    true
                })
        });
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Removing);
        let b = backend(cx);
        let id = self.id.clone();
        let title = self.title(cx);
        let task = irori_ui::run(cx, async move {
            b.remove_graphics_mod(&id, GraphicsMod::Renodx).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(report) => {
                        let removed =
                            t_args("settings.renodx.removed_notice", &[("title", &title)]);
                        window.push_notification(removed_notification(removed, &report), cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.renodx.failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
                cx.emit(GraphicsModsChanged);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_installing(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut text = self
            .step
            .map(step_text)
            .unwrap_or_else(|| t("settings.reshade.step.checking"))
            .to_string();
        if let Some(p) = self.percent {
            text = format!("{text} {}%", p.round() as u32);
        }
        let row = RowState {
            label: t("settings.renodx.label"),
            help: text.into(),
            ..Default::default()
        };
        let cancel = Button::new("renodx-cancel")
            .outline()
            .label(t("settings.common.cancel"))
            .disabled(self.cancel.as_ref().is_none_or(|c| c.is_cancelled()))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(c) = &this.cancel {
                    c.cancel();
                }
                cx.notify();
            }));
        let control = h_flex()
            .gap_2()
            .child(Button::new("renodx-installing").ghost().loading(true))
            .child(cancel)
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }

    fn render_installed(
        &self,
        status: &RenodxStatus,
        manifest: &Manifest,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.busy.is_some();
        let incomplete = !manifest.complete;
        let name = manifest
            .source
            .as_ref()
            .map(|s| s.title.clone())
            .unwrap_or_else(|| manifest.dll.clone());
        let mut help = match &manifest.source {
            Some(_) => t_args(
                "settings.renodx.installed_help",
                &[("file", &manifest.dll), ("date", &manifest.version)],
            ),
            None => t_args(
                "settings.renodx.installed_help_file",
                &[("file", &manifest.dll)],
            ),
        }
        .to_string();
        if status.reshade.as_ref().is_some_and(|r| !r.enabled) {
            help.push('\n');
            help.push_str(&t("settings.renodx.reshade_off"));
        }
        if !cfg!(target_os = "windows") {
            help.push('\n');
            help.push_str(&t("settings.renodx.wine_note"));
        }
        let row = RowState {
            label: t_args("settings.renodx.installed_label", &[("mod", &name)]),
            help: help.into(),
            error: incomplete.then(|| t("settings.renodx.incomplete")),
            ..Default::default()
        };
        let toggle = Switch::new("renodx-enabled")
            .checked(manifest.enabled)
            .disabled(busy || incomplete)
            .accessibility_label(t("settings.renodx.enabled"))
            .on_change(
                cx.listener(|this, on: &bool, window, cx| this.set_enabled(*on, window, cx)),
            );
        let update = status
            .update()
            .is_some()
            .then(|| manifest.source.as_ref().map(|s| s.id.clone()))
            .flatten();
        let control = h_flex()
            .gap_2()
            .items_center()
            .when_some(update, |row, mod_id| {
                row.child(
                    Button::new("renodx-update")
                        .outline()
                        .small()
                        .label(t("settings.renodx.update"))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.install(RenodxChoice::Mod(mod_id.clone()), window, cx)
                        })),
                )
            })
            .child(
                Button::new("renodx-remove")
                    .ghost()
                    .small()
                    .label(t("settings.common.remove"))
                    .loading(self.busy == Some(Busy::Removing))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_remove(window, cx))),
            )
            .when(!incomplete, |row| row.child(toggle))
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }

    fn render_not_installed(&self, status: &RenodxStatus, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let has_list = status.catalog.is_some();
        let row = RowState {
            label: t("settings.renodx.label"),
            help: not_installed_help(status).into(),
            stacked: true,
            ..Default::default()
        };
        let control = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .when(has_list, |row| {
                row.child(
                    Select::new(&self.select)
                        .id("renodx-mod")
                        .accessibility_label(t("settings.renodx.mod_label"))
                        .placeholder(t("settings.renodx.select_placeholder"))
                        .search_placeholder(t("settings.renodx.search_placeholder"))
                        .disabled(busy)
                        .w_80(),
                )
                .child(
                    Button::new("renodx-install")
                        .outline()
                        .small()
                        .label(t("settings.renodx.install"))
                        .disabled(busy || self.picked.is_none())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.install_picked(window, cx)),
                        ),
                )
            })
            .child(
                Button::new("renodx-choose")
                    .ghost()
                    .small()
                    .label(t("settings.renodx.choose_file"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.choose_file(window, cx))),
            )
            .child(
                Button::new("renodx-list")
                    .ghost()
                    .small()
                    .label(t("settings.renodx.mod_list"))
                    .on_click(|_, _, cx| cx.open_url(MODS_PAGE)),
            )
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }
}

impl Render for RenodxPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.applies(cx) {
            return div()
                .py_3()
                .child(muted(t("settings.renodx.unsupported"), cx))
                .into_any_element();
        }
        let body: AnyElement = if self.busy == Some(Busy::Installing) {
            self.render_installing(cx)
        } else {
            match &self.load {
                Load::Idle | Load::Loading => div()
                    .px_4()
                    .py_3()
                    .child(muted(t("settings.renodx.checking"), cx))
                    .into_any_element(),
                Load::Failed(e) => {
                    let row = RowState {
                        label: t("settings.renodx.label"),
                        error: Some(e.clone()),
                        ..Default::default()
                    };
                    let retry = Button::new("renodx-retry")
                        .outline()
                        .small()
                        .label(t("settings.common.try_again"))
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx)))
                        .into_any_element();
                    field_row(row, retry, cx).into_any_element()
                }
                Load::Ready(status) => match &status.manifest {
                    Some(m) => self.render_installed(status, m, cx),
                    None => self.render_not_installed(status, cx),
                },
            }
        };
        v_flex()
            .pt_2()
            .child(group_panel(cx).child(body))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use irori_reshade::renodx::{Catalog, DOWNLOADS_URL};
    use irori_reshade::{Api, Machine};

    use super::*;

    fn catalog() -> Arc<Catalog> {
        let index = serde_json::json!({"games": [{"title": "Hades II", "mods": [
            {"id": "hades2", "title": "Hades II",
             "artifacts": [{"name": "renodx-hades2.addon64", "arch": "x64"}]},
            {"id": "other", "title": "Other",
             "artifacts": [{"name": "renodx-other.addon64", "arch": "x64"}]}
        ]}]});
        let asset = |name: &str| {
            serde_json::json!({
                "name": name,
                "digest": format!("sha256:{}", "a".repeat(64)),
                "updated_at": "2026-11-10T00:00:00Z",
                "browser_download_url": format!("{DOWNLOADS_URL}{name}"),
            })
        };
        let release = serde_json::json!({"assets": [
            asset("renodx-hades2.addon64"),
            asset("renodx-other.addon64"),
        ]});
        Arc::new(
            Catalog::parse(
                index.to_string().as_bytes(),
                release.to_string().as_bytes(),
                DOWNLOADS_URL,
            )
            .unwrap(),
        )
    }

    fn reshade(addon: bool) -> Manifest {
        Manifest {
            kind: GraphicsMod::Reshade,
            version: "6.8.0".into(),
            exe: PathBuf::from("/g/Game.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: "dxgi.dll".into(),
            dir: PathBuf::from("/g"),
            enabled: true,
            complete: true,
            overrides: Vec::new(),
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon,
            source: None,
        }
    }

    #[test]
    fn preselects_the_pick_then_the_games_mod() {
        let mut status = RenodxStatus {
            catalog: Some(catalog()),
            suggested: vec!["hades2".into()],
            ..Default::default()
        };
        assert_eq!(preselect(&status, None).as_deref(), Some("hades2"));
        assert_eq!(preselect(&status, Some("other")).as_deref(), Some("other"));
        assert_eq!(preselect(&status, Some("gone")).as_deref(), Some("hades2"));
        status.suggested.clear();
        assert_eq!(preselect(&status, None), None);
        status.catalog = None;
        assert_eq!(preselect(&status, Some("other")), None);
    }

    #[test]
    fn the_help_says_what_installing_changes() {
        crate::i18n::register();
        let status = RenodxStatus {
            catalog: Some(catalog()),
            suggested: vec!["hades2".into()],
            reshade: Some(reshade(false)),
            ..Default::default()
        };
        let help = not_installed_help(&status);
        assert!(help.contains(&*t("settings.renodx.switch_note")), "{help}");
        assert!(help.contains("Hades II"), "{help}");

        let status = RenodxStatus {
            catalog: Some(catalog()),
            reshade: Some(reshade(true)),
            ..Default::default()
        };
        let help = not_installed_help(&status);
        assert!(!help.contains(&*t("settings.renodx.switch_note")), "{help}");
        assert!(help.contains(&*t("settings.renodx.no_match")), "{help}");

        let status = RenodxStatus {
            catalog_error: Some("offline".into()),
            ..Default::default()
        };
        assert!(not_installed_help(&status).contains("offline"));
    }
}
