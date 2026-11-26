//! Size calculations, validation blockers, and install/modify submission for
//! [`InstallSheet`].

use std::collections::BTreeSet;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, Context, Window};
use irori_core::{InstallInfo, InstallRequest, Runner, UpdateRequest};
use irori_extras::anticheat::InstallGate;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use super::{InstallMode, InstallSheet, nothing_to_install};
use crate::hooks;
use crate::load::Load;
use crate::ops::notify_error;
use crate::sizes::{
    DiskSpace, DiskVerdict, InstallSize, install_button_label, install_size, modify_disk_verdict,
};
use crate::state::is_unsupported_third_party;

impl InstallSheet {
    /// Bytes this choice downloads and occupies.
    pub fn size(&self) -> InstallSize {
        self.info
            .ready()
            .map(|info| {
                install_size(
                    info,
                    self.language.as_deref(),
                    &self.selected_dlcs,
                    &self.dlc_sizes,
                )
            })
            .unwrap_or_default()
    }

    /// Disk verdict for the current mode. In Modify mode, accounts for bytes
    /// the installed game already occupies on disk so the base game is never
    /// double-counted against free space.
    pub(super) fn disk_verdict(&self) -> DiskVerdict {
        let space = self.disk.ready().copied();
        let size = self.size();
        if !self.is_modify() {
            return DiskVerdict::evaluate(space, size.disk);
        }
        let installed_disk = self
            .game
            .install
            .as_ref()
            .map_or(0, |i| i.install_size_bytes);
        let installed_dlcs: BTreeSet<&str> = self
            .game
            .install
            .as_ref()
            .map(|i| i.installed_dlcs.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let mut added_dlc_disk = 0u64;
        for dlc in &self.selected_dlcs {
            if !installed_dlcs.contains(dlc.as_str())
                && let Some(s) = self.dlc_sizes.get(dlc)
            {
                added_dlc_disk = added_dlc_disk.saturating_add(s.disk);
            }
        }
        let mut removed_dlc_disk = 0u64;
        for dlc in &installed_dlcs {
            if !self.selected_dlcs.contains(*dlc)
                && let Some(s) = self.dlc_sizes.get(*dlc)
            {
                removed_dlc_disk = removed_dlc_disk.saturating_add(s.disk);
            }
        }
        modify_disk_verdict(
            space,
            size.disk,
            installed_disk,
            added_dlc_disk,
            removed_dlc_disk,
        )
    }

    pub(super) fn third_party(&self) -> Option<String> {
        self.info
            .ready()
            .and_then(|i| i.third_party_managed_app.clone())
            .or_else(|| self.game.third_party_managed_app.clone())
    }

    pub(super) fn anticheat_gate(&self, cx: &App) -> InstallGate {
        hooks::install_gate(
            self.compat.as_ref().and_then(|c| c.anticheat.as_ref()),
            &backend(cx).settings.app(),
        )
    }

    pub(super) fn not_installable(&self) -> bool {
        self.info
            .ready()
            .is_some_and(|i| nothing_to_install(i, &self.game))
    }

    /// Why the primary button is disabled, if it is (a `game.install.blocker`
    /// key).
    pub(super) fn blocker(&self, cx: &App) -> Option<&'static str> {
        if self.submitting {
            return Some("game.install.blocker.working");
        }
        if !matches!(self.info, Load::Ready(_)) {
            return Some("game.install.blocker.loading");
        }
        if self.not_installable() {
            return Some("game.install.blocker.not_installable");
        }
        if is_unsupported_third_party(&self.game) {
            return Some("game.install.blocker.unsupported_launcher");
        }
        match self.anticheat_gate(cx) {
            InstallGate::Blocked => return Some("game.install.blocker.anticheat"),
            InstallGate::Confirm if !self.anticheat_ack => {
                return Some("game.install.blocker.anticheat");
            }
            _ => {}
        }
        if self.is_modify() {
            return (!self.has_changes()).then_some("game.install.blocker.no_changes");
        }
        if self.path.read(cx).value().trim().is_empty() {
            return Some("game.install.blocker.choose_folder");
        }
        if matches!(
            self.disk,
            Load::Ready(DiskSpace {
                writable: false,
                ..
            })
        ) {
            return Some("game.install.blocker.not_writable");
        }
        None
    }

    pub(super) fn has_changes(&self) -> bool {
        let Some(install) = self.game.install.as_ref() else {
            return false;
        };
        let installed: BTreeSet<String> = install.installed_dlcs.iter().cloned().collect();
        installed != self.selected_dlcs
            || (self.language.is_some() && install.language != self.language)
            || install.branch != self.branch
            || self.pin_build != install.pinned_version
            || (self.pin_build && install.build_id != self.build)
    }

    /// Primary button label: `Install · 12.3 GiB` or `Save changes`.
    pub fn primary_label(&self) -> String {
        match self.mode {
            InstallMode::Install => {
                let resumable = self.info.ready().is_some_and(|i| i.resumable);
                let verb = if resumable {
                    t("game.install.continue")
                } else {
                    t("game.install.install")
                };
                install_button_label(&verb, self.size().download)
            }
            InstallMode::Modify => t("game.install.apply").to_string(),
        }
    }

    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocker(cx).is_some() {
            return;
        }
        match self.mode {
            InstallMode::Install => self.submit_install(window, cx),
            InstallMode::Modify => self.submit_modify(window, cx),
        }
    }

    fn selected_sdl_tags(&self, info: &InstallInfo) -> Vec<String> {
        info.sdl
            .iter()
            .filter(|o| o.mandatory || self.sdl.contains(&o.id))
            .flat_map(|o| o.tags.iter().cloned())
            .collect()
    }

    fn submit_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.info.ready().cloned() else {
            return;
        };
        let b = backend(cx);
        let id = self.id.clone();
        if self.platform.needs_wine() {
            self.persist_wine(cx);
        }
        let size = self.size();
        let request = InstallRequest {
            path: self.install_dir(cx),
            platform: self.platform,
            dlcs: info
                .dlcs
                .iter()
                .filter(|d| self.selected_dlcs.contains(&d.app_name))
                .map(|d| d.app_name.clone())
                .collect(),
            sdl: self.selected_sdl_tags(&info),
            language: self.language.clone(),
            branch: self.branch.clone(),
            build: self.chosen_build(),
            dependencies: info.dependencies.clone(),
            installer: None,
            account: self.account.clone(),
        };
        let title = self.game.title.clone();
        self.submitting = true;
        cx.notify();
        let dir = request.path.clone();
        let task = irori_ui::run(cx, async move {
            tokio::fs::create_dir_all(&dir).await.map_err(|e| {
                anyhow::anyhow!(t_args(
                    "game.install.create_dir_failed",
                    &[
                        ("path", &dir.display().to_string()),
                        ("error", &e.to_string())
                    ]
                ))
            })?;
            b.enqueue_install(id, title, request, size.download);
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.submitting = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => notify_error(window, cx, t("game.install.start_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_modify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = self.game.install.clone() else {
            return;
        };
        let b = backend(cx);
        let installed: BTreeSet<String> = install.installed_dlcs.iter().cloned().collect();
        // Epic DLCs are separate installs; queue each change on its own.
        if self.id.runner == Runner::Legendary
            && let Some(info) = self.info.ready()
        {
            for dlc in &info.dlcs {
                let want = self.selected_dlcs.contains(&dlc.app_name);
                if want != installed.contains(&dlc.app_name) {
                    crate::dlc::toggle_dlc(&self.game, dlc, want, window, cx);
                }
            }
        }
        let dlcs_changed = installed != self.selected_dlcs && self.id.runner != Runner::Legendary;
        let request = UpdateRequest {
            dlcs: dlcs_changed.then(|| self.selected_dlcs.iter().cloned().collect()),
            language: self
                .language
                .clone()
                .filter(|l| install.language.as_ref() != Some(l)),
            branch: (install.branch != self.branch)
                .then(|| self.branch.clone().unwrap_or_default()),
            build: (self.pin_build && install.build_id != self.build)
                .then(|| self.build.clone())
                .flatten(),
            dependencies: None,
        };
        let needs_update = request.dlcs.is_some()
            || request.language.is_some()
            || request.branch.is_some()
            || request.build.is_some();
        if needs_update {
            b.enqueue_update(self.id.clone(), self.game.title.clone(), request);
        }
        if self.pin_build != install.pinned_version {
            let id = self.id.clone();
            let pinned = self.pin_build;
            crate::ops::run_reporting(
                window,
                cx,
                t("game.install.pin_failed"),
                async move {
                    let store = b
                        .store(id.runner)
                        .ok_or_else(|| crate::i18n::store_unavailable(id.runner))?;
                    store.set_version_pinned(&id.app_name, pinned).await
                },
                |_, _, _| {},
            )
            .detach();
        }
        window.close_dialog(cx);
    }
}
