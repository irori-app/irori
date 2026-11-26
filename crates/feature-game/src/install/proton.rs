//! Wine discovery, inline GE-Proton downloads, and per-game Wine/prefix
//! override persistence for [`InstallSheet`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, alert::Alert, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement, SharedString, Styled, Window, div,
};
use irori_core::WineInstallation;
use irori_runtime::manager::{self, ReleaseKind};
use irori_ui::i18n::{t, t_args};
use irori_ui::wine_choice::build_label;
use irori_ui::wine_installs::{self, Outcome};
use irori_ui::{Route, backend, navigate};

use super::InstallSheet;
use crate::choice::{Choice, set_choices};
use crate::load::Load;

/// Repaint interval while GE-Proton downloads.
const PROTON_TICK: std::time::Duration = std::time::Duration::from_millis(300);

/// The GE-Proton download offered when no Wine or Proton build exists.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ProtonDownload {
    /// Offered, not asked for.
    Offered,
    /// Looking up the newest release.
    Finding,
    /// Installing through the Wine manager, by its install key.
    Running(String),
    Failed(SharedString),
}

impl InstallSheet {
    pub(super) fn fetch_wines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        self.wines = Load::Loading;
        let app = backend(cx).settings.app();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations_with_sources(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let mut found = result.unwrap_or_default();
                if let Some(current) = &this.wine
                    && !found.iter().any(|(w, _)| w.bin == current.bin)
                {
                    found.insert(0, (current.clone(), None));
                }
                if this.wine.is_none() {
                    this.wine = found.first().map(|(w, _)| w.clone());
                }
                let items = found
                    .iter()
                    .map(|(w, source)| Choice::new(w.bin.clone(), build_label(w, *source)))
                    .collect();
                let list: Vec<WineInstallation> = found.into_iter().map(|(w, _)| w).collect();
                let selected = this.wine.as_ref().map(|w| w.bin.clone());
                set_choices(&this.wine_select, items, selected.as_deref(), window, cx);
                this.wines = Load::Ready(list);
                this.sync_auto_expand();
                cx.notify();
            });
        })
        .detach();
    }

    /// Download the newest GE-Proton through the Wine manager, which makes it
    /// the default Wine once installed. Asked for by the user's click; the
    /// download goes on when the modal closes.
    fn download_proton(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.proton,
            ProtonDownload::Finding | ProtonDownload::Running(_)
        ) {
            return;
        }
        self.proton = ProtonDownload::Finding;
        self.sync_auto_expand();
        cx.notify();
        let task = irori_ui::run(cx, manager::newest_release(ReleaseKind::GeProton));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(release) => {
                        let key = wine_installs::start_install(release, true, cx);
                        this.proton = ProtonDownload::Running(key);
                        this.tick_proton(window, cx);
                    }
                    Err(e) => this.proton = ProtonDownload::Failed(format!("{e:#}").into()),
                }
                this.sync_auto_expand();
                cx.notify();
            });
        })
        .detach();
    }

    /// Follow the GE-Proton install: pick the build when it is there.
    pub(super) fn proton_progressed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ProtonDownload::Running(key) = &self.proton else {
            return;
        };
        let installs = wine_installs::wine_ops(cx);
        let outcome = installs.read(cx).finished.get(key).cloned();
        match outcome {
            None => {}
            Some(Outcome::Installed(wine)) => {
                self.proton = ProtonDownload::Offered;
                self.wine = Some(wine);
                self.fetch_wines(window, cx);
            }
            Some(Outcome::Failed(message)) => {
                self.proton = ProtonDownload::Failed(message.into());
                self.sync_auto_expand();
            }
            Some(Outcome::Cancelled) => self.proton = ProtonDownload::Offered,
        }
        cx.notify();
    }

    /// Repaint while GE-Proton downloads so its progress moves.
    fn tick_proton(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._proton_ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROTON_TICK).await;
                let keep = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.proton, ProtonDownload::Running(_))
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
    }

    pub(super) fn render_proton(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let open_manager = Button::new("open-wine-manager")
            .small()
            .outline()
            .label(t("game.install.open_wine_manager"))
            .on_click(|_, window, cx| {
                window.close_dialog(cx);
                navigate(Route::WineManager, cx);
            });
        // GE-Proton is offered only where the Wine manager has it (Linux).
        if !ReleaseKind::available().contains(&ReleaseKind::GeProton) {
            return v_flex()
                .gap_2()
                .child(Alert::warning("no-wine", t("game.install.no_wine")))
                .child(open_manager)
                .into_any_element();
        }
        let download = |label: SharedString, cx: &mut Context<Self>| {
            Button::new("download-ge-proton")
                .small()
                .primary()
                .icon(IconName::Download)
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| this.download_proton(window, cx)))
        };
        match &self.proton {
            ProtonDownload::Offered => v_flex()
                .gap_2()
                .child(Alert::warning("no-wine", t("game.install.ge_proton_offer")))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(download(t("game.install.download_ge_proton"), cx))
                        .child(open_manager),
                )
                .into_any_element(),
            ProtonDownload::Finding => h_flex()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child(t("game.install.ge_proton_finding"))
                .into_any_element(),
            ProtonDownload::Running(key) => {
                let installs = wine_installs::wine_ops(cx);
                let (version, live) = installs
                    .read(cx)
                    .running
                    .get(key)
                    .map(|op| (op.version.clone(), op.snapshot()))
                    .unwrap_or_default();
                let extracting = live.extracting();
                let progress = live.progress.unwrap_or_default();
                let text = if extracting {
                    t_args("game.install.ge_proton_unpacking", &[("version", &version)])
                } else if let Some(p) = progress.percent {
                    t_args(
                        "game.install.ge_proton_progress",
                        &[("version", &version), ("percent", &format!("{p:.0}"))],
                    )
                } else {
                    t_args("game.install.ge_proton_starting", &[("version", &version)])
                };
                let cancel_key = key.clone();
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().text_sm().truncate().child(text))
                                    .child(
                                        Progress::new("ge-proton-progress")
                                            .value(progress.percent.unwrap_or(0.0))
                                            .loading(extracting || progress.percent.is_none()),
                                    ),
                            )
                            .child(
                                Button::new("cancel-ge-proton")
                                    .small()
                                    .ghost()
                                    .label(t("game.common.cancel"))
                                    .disabled(extracting)
                                    .on_click(move |_, _, cx| {
                                        wine_installs::cancel_install(&cancel_key, cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("game.install.ge_proton_meanwhile")),
                    )
                    .into_any_element()
            }
            ProtonDownload::Failed(message) => v_flex()
                .gap_2()
                .child(Alert::error(
                    "ge-proton-failed",
                    t_args("game.install.ge_proton_failed", &[("error", message)]),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(download(t("game.install.ge_proton_retry"), cx))
                        .child(open_manager),
                )
                .into_any_element(),
        }
    }

    /// Store the chosen Wine build and prefix as per-game overrides when they
    /// differ from what the game would use anyway.
    pub(super) fn persist_wine(&self, cx: &App) {
        let b = backend(cx);
        let effective = b.settings.game(&self.id);
        if let Some(wine) = &self.wine
            && *wine != effective.wine_version
            && let Ok(v) = serde_json::to_value(wine)
        {
            b.settings.set_game_value(&self.id, "wineVersion", Some(v));
        }
        let prefix = self.prefix.read(cx).value().trim().to_string();
        let app = b.settings.app();
        let current = irori_runtime::prefix_path(&app, &effective, &self.game);
        let inherited = irori_runtime::prefix_path(
            &app,
            &irori_core::GameSettings {
                wine_prefix: app.game.wine_prefix.clone(),
                ..effective.clone()
            },
            &self.game,
        );
        if prefix.is_empty() {
            return;
        }
        let chosen = irori_core::paths::expand_home(&prefix);
        if chosen == inherited && chosen != current {
            // The saved prefix was gone and the defaults' one was taken.
            b.settings.set_game_value(&self.id, "winePrefix", None);
        } else if chosen != current {
            b.settings.set_game_value(
                &self.id,
                "winePrefix",
                Some(serde_json::Value::String(prefix)),
            );
        }
    }
}
