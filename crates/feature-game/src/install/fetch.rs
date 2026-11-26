//! Async metadata fetching, disk probing, folder browsing, and branch password
//! unlocking for [`InstallSheet`].

use std::collections::BTreeSet;
use std::path::PathBuf;

use futures::StreamExt as _;
use gpui_kit::{App, AppContext as _, Context, Window};
use irori_core::{InstallInfo, InstallInfoQuery};
use irori_ui::backend;
use irori_ui::i18n::t;

use super::{InstallMode, InstallSheet, InstallTarget};
use crate::choice::{Choice, set_choices};
use crate::hooks;
use crate::load::{Load, language_name, preferred_language};
use crate::ops::{notify_error, pick_path};
use crate::sizes::{InstallSize, detect_extra_drives, disk_space, free_space, same_dir};

impl Choice {
    fn value_str(&self) -> &str {
        use gpui_kit::component::select::SelectItem as _;
        self.value().as_ref()
    }
}

impl InstallSheet {
    pub(super) fn probe_library_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        let configured: Vec<PathBuf> = self
            .install_targets
            .iter()
            .map(|t| irori_core::paths::expand_home(t.base_path.trim()))
            .collect();
        let task = cx.background_spawn(async move {
            let spaces: Vec<(usize, u64)> = configured
                .iter()
                .enumerate()
                .filter_map(|(ix, path)| free_space(path).ok().map(|free| (ix, free)))
                .collect();
            let extras = detect_extra_drives(&configured);
            (spaces, extras)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (spaces, extras) = task.await;
            let _ = this.update(cx, |this, cx| {
                for (ix, free) in spaces {
                    if let Some(target) = this.install_targets.get_mut(ix) {
                        target.free = Some(free);
                    }
                }
                for drive in extras {
                    let already = this.install_targets.iter().any(|t| {
                        same_dir(
                            &irori_core::paths::expand_home(t.base_path.trim()),
                            &drive.mount,
                        )
                    });
                    if !already {
                        this.install_targets.push(InstallTarget {
                            label: drive.label.into(),
                            base_path: drive.suggested_base,
                            free: Some(drive.free),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn fetch_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info_rev += 1;
        let rev = self.info_rev;
        self.info = Load::Loading;
        self.dlc_sizes.clear();
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let platform = self.platform;
        let query = InstallInfoQuery {
            branch: self.branch.clone(),
            build: self.chosen_build(),
            language: None,
        };
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(id.runner)
                .ok_or_else(|| crate::i18n::store_unavailable(id.runner))?;
            store.install_info(&id.app_name, platform, &query).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.info_rev == rev {
                    this.apply_info(result, window, cx);
                }
            });
        })
        .detach();
    }

    fn apply_info(
        &mut self,
        result: anyhow::Result<InstallInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = match result {
            Ok(info) => info,
            Err(e) => {
                self.info = Load::Failed(format!("{e:#}").into());
                cx.notify();
                return;
            }
        };
        let ui_language = backend(cx).settings.app().ui_language.clone();
        self.language = preferred_language(&info.languages, self.language.as_deref(), &ui_language);
        let mut languages = info.languages.clone();
        languages.sort();
        let items = languages
            .iter()
            .map(|l| Choice::new(l.clone(), language_name(l)))
            .collect();
        set_choices(
            &self.language_select,
            items,
            self.language.as_deref(),
            window,
            cx,
        );

        let branches = std::iter::once(Choice::new("", t("game.install.branch_default")))
            .chain(
                info.branches
                    .iter()
                    .filter(|b| !b.is_empty())
                    .map(|b| Choice::new(b.clone(), b.clone())),
            )
            .collect();
        set_choices(
            &self.branch_select,
            branches,
            Some(self.branch.as_deref().unwrap_or("")),
            window,
            cx,
        );

        let builds: Vec<Choice> = info
            .builds
            .iter()
            .filter(|b| b.branch == self.branch)
            .map(|b| {
                let date = b.date.chars().take(10).collect::<String>();
                let label = if date.is_empty() {
                    b.version_name.clone()
                } else {
                    format!("{} · {date}", b.version_name)
                };
                Choice::new(b.build_id.clone(), label)
            })
            .collect();
        if self.build.is_none()
            || !builds
                .iter()
                .any(|c| Some(c.value_str()) == self.build.as_deref())
        {
            self.build = builds.first().map(|c| c.value_str().to_string());
        }
        set_choices(
            &self.build_select,
            builds,
            self.build.as_deref(),
            window,
            cx,
        );

        let owned: BTreeSet<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        if self.selection_made {
            // A refetch (branch, build, password) keeps the user's choices.
            let sdl_ids: BTreeSet<String> = info.sdl.iter().map(|o| o.id.clone()).collect();
            self.sdl.retain(|id| sdl_ids.contains(id));
            self.selected_dlcs.retain(|d| owned.contains(d));
        } else {
            self.selection_made = true;
            self.sdl = info
                .sdl
                .iter()
                .filter(|o| o.mandatory || o.default_selected)
                .map(|o| o.id.clone())
                .collect();
            self.selected_dlcs = self.default_dlcs(&owned);
        }
        let dlc_ids: Vec<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        self.info = Load::Ready(info);
        self.fetch_dlc_sizes(dlc_ids, window, cx);
        cx.notify();
    }

    /// Owned DLCs preselected for a new install; installed ones when modifying.
    fn default_dlcs(&self, owned: &BTreeSet<String>) -> BTreeSet<String> {
        match self.mode {
            InstallMode::Install => owned.clone(),
            InstallMode::Modify => self
                .game
                .install
                .as_ref()
                .map(|i| {
                    i.installed_dlcs
                        .iter()
                        .filter(|d| owned.contains(*d))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn fetch_dlc_sizes(&mut self, dlcs: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if dlcs.is_empty() {
            return;
        }
        let rev = self.info_rev;
        let b = backend(cx);
        let runner = self.id.runner;
        let platform = self.platform;
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(runner)
                .ok_or_else(|| crate::i18n::store_unavailable(runner))?;
            let sizes: Vec<(String, InstallSize)> = futures::stream::iter(dlcs)
                .map(|dlc| {
                    let store = store.clone();
                    async move {
                        let info = store
                            .install_info(&dlc, platform, &InstallInfoQuery::default())
                            .await
                            .ok()?;
                        Some((dlc, InstallSize::new(info.download_size, info.disk_size)))
                    }
                })
                .buffer_unordered(4)
                .filter_map(|x| async move { x })
                .collect()
                .await;
            Ok(sizes)
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(sizes) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.info_rev == rev {
                        this.dlc_sizes
                            .extend(sizes.into_iter().filter(|(_, s)| s.is_known()));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Replace a saved prefix that no longer exists with the one the
    /// defaults give, unless the user already typed another.
    pub(super) fn check_prefix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        let b = backend(cx);
        let app = b.settings.app();
        let settings = b.settings.game(&self.id);
        if settings.wine_prefix.is_empty() {
            return;
        }
        let shown = self.prefix.read(cx).value().to_string();
        let game = self.game.clone();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::install_prefix(&app, &settings, &game))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(suggested) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let suggested = suggested.to_string_lossy().into_owned();
                if this.prefix.read(cx).value() == shown && suggested != shown {
                    this.prefix
                        .update(cx, |s, cx| s.set_value(suggested, window, cx));
                }
            });
        })
        .detach();
    }

    pub(super) fn fetch_compat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = hooks::fetch_compatibility(self.game.clone(), cx) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.compat = result.ok();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn install_dir(&self, cx: &App) -> PathBuf {
        let raw = self.path.read(cx).value();
        irori_core::paths::expand_home(raw.trim())
    }

    fn disk_probe_dir(&self, cx: &App) -> Option<PathBuf> {
        if self.is_modify() {
            self.game.install_path()
        } else {
            let dir = self.install_dir(cx);
            (!dir.as_os_str().is_empty()).then_some(dir)
        }
    }

    pub(super) fn refresh_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.disk_rev += 1;
        let rev = self.disk_rev;
        let Some(dir) = self.disk_probe_dir(cx) else {
            self.disk = Load::Idle;
            cx.notify();
            return;
        };
        self.disk = Load::Loading;
        cx.notify();
        let task = cx.background_spawn(async move { disk_space(&dir) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.disk_rev == rev {
                    this.disk = Load::from_result(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn browse_install_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = pick_path(window, cx, true, t("game.common.select"));
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = pick.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    let value = path.to_string_lossy().into_owned();
                    this.path.update(cx, |s, cx| s.set_value(value, window, cx));
                    this.refresh_disk(window, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn browse_prefix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = pick_path(window, cx, true, t("game.common.select"));
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = pick.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    let value = path.to_string_lossy().into_owned();
                    this.prefix
                        .update(cx, |s, cx| s.set_value(value, window, cx));
                });
            }
        })
        .detach();
    }

    pub(super) fn apply_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.password.read(cx).value();
        self.password_busy = true;
        cx.notify();
        let task = hooks::set_branch_password(self.id.clone(), password, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.password_busy = false;
                match result {
                    Ok(()) => this.fetch_info(window, cx),
                    Err(e) => notify_error(window, cx, t("game.install.password_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
