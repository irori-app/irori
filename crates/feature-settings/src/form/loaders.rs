use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::{AnyElement, Context, IntoElement as _, SharedString, Window};
use irori_core::settings::WineInstallation;
use irori_runtime::graphics::HostGraphics;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::wine_choice::build_label;
use serde_json::Value;

use crate::registry::{CustomKind, FieldKind};
use crate::value::value_to_text;

use super::controls::hide_gpu_items;
use super::{ChoiceItem, FormTarget, SettingsForm};

impl SettingsForm {
    /// Load the Wine/Proton builds for the version picker (once).
    pub fn load_wine_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_tool_versions(window, cx);
        let Some(wine) = &mut self.wine else { return };
        if wine.loading || wine.builds.is_some() {
            return;
        }
        wine.loading = true;
        let app = backend(cx).settings.app();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations_with_sources(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                if let Some(wine) = &mut this.wine {
                    wine.loading = false;
                    wine.sources = found
                        .iter()
                        .filter_map(|(w, source)| Some((w.bin.clone(), (*source)?)))
                        .collect();
                    wine.builds = Some(found.into_iter().map(|(w, _)| w).collect());
                }
                this.sync_wine_select(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Look for the graphics cards, Smooth Motion and lsfg-vk.
    pub(super) fn load_graphics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let app = b.settings.app();
        let job = b.spawn_blocking(move || HostGraphics::probe(&app));
        let task = irori_ui::run(cx, async move { Ok(job.await?) });
        cx.spawn_in(window, async move |this, cx| {
            let graphics = task.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                this.graphics = Some(Arc::new(graphics));
                this.sync_gpu_select(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Look for the graphics cards a game could hide (Windows).
    pub(super) fn load_hideable_gpus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.hideable_gpus().await });
        cx.spawn_in(window, async move |this, cx| {
            let adapters = task.await.unwrap_or_else(|e| {
                tracing::warn!("graphics cards: {e:#}");
                Vec::new()
            });
            this.update_in(cx, |this, window, cx| {
                this.hideable_gpus = Some(Arc::new(adapters));
                this.sync_hide_gpu_select(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Look for the prefixes a game could share (its prefix menu).
    pub(super) fn load_prefix_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let FormTarget::Game(id) = &self.target else {
            return;
        };
        let (b, id) = (backend(cx), id.clone());
        let job = b.spawn_blocking({
            let b = b.clone();
            move || b.prefix_choices(&id)
        });
        let task = irori_ui::run(cx, async move { Ok(job.await?) });
        cx.spawn_in(window, async move |this, cx| {
            let choices = task.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.prefix_choices = Some(Arc::new(choices));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The menu beside a game's prefix field: the prefixes in the prefixes
    /// folder and those other games play in, to share one.
    pub(super) fn prefix_menu(&self, disabled: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let FormTarget::Game(_) = &self.target else {
            return None;
        };
        let choices = self.prefix_choices.clone();
        let view = cx.entity().downgrade();
        Some(
            Button::new("prefixes-winePrefix")
                .ghost()
                .xsmall()
                .icon(IconName::ChevronDown)
                .tooltip(t("settings.prefix_menu.tooltip"))
                .disabled(disabled)
                .dropdown_menu(move |menu, window, _| {
                    let menu = menu.scrollable(true).max_h(window.rem_size() * 24.);
                    let Some(list) = &choices else {
                        return menu.label(t("settings.prefix_menu.looking"));
                    };
                    if list.is_empty() {
                        return menu.label(t("settings.prefix_menu.none"));
                    }
                    list.iter().fold(menu, |menu, choice| {
                        let path = choice.path.to_string_lossy().into_owned();
                        let label: SharedString = match choice.games.as_slice() {
                            [] => path.clone().into(),
                            games => t_args(
                                "settings.prefix_menu.used_by",
                                &[("path", &path), ("games", &games.join(", "))],
                            ),
                        };
                        let view = view.clone();
                        menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            let path = path.clone();
                            let _ = view.update(cx, |this, cx| {
                                this.write("winePrefix", Value::String(path), window, cx)
                            });
                        }))
                    })
                })
                .into_any_element(),
        )
    }

    /// Read the displays for the HDR and gamescope fields' hints.
    pub(super) fn load_displays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.displays(false).await) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(displays) = task.await else { return };
            this.update(cx, |this, cx| {
                this.displays.get_or_insert(displays);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load the releases for the DXVK and VKD3D-Proton pickers (once).
    fn load_tool_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.tool_versions_requested, true) {
            return;
        }
        for spec in self.fields.clone() {
            let FieldKind::Custom(CustomKind::ToolVersion(tool)) = spec.kind else {
                continue;
            };
            let path = spec.path;
            let task = irori_ui::run(cx, irori_runtime::tools::tool_versions(tool));
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok(tags) => {
                            this.tool_versions.insert(path, tags);
                        }
                        Err(e) => {
                            this.errors.insert(path, format!("{e:#}").into());
                        }
                    }
                    let items = this.tool_version_items(path);
                    let current: SharedString = value_to_text(this.value(path)).into();
                    if let Some(state) = this.choices.get(path) {
                        state.update(cx, |s, cx| {
                            s.set_items(items, window, cx);
                            s.set_selected_value(&current, window, cx);
                        });
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// "Latest", the loaded releases, and the stored version even when it
    /// isn't among them (offline, or an older release than the list reaches).
    pub(super) fn tool_version_items(&self, path: &str) -> Vec<ChoiceItem> {
        let current = value_to_text(self.value(path));
        let tags = self.tool_versions.get(path).map_or(&[][..], Vec::as_slice);
        let mut items = vec![ChoiceItem::new("", t("settings.tool_version.latest"))];
        if !current.is_empty() && !tags.contains(&current) {
            items.push(ChoiceItem::new(current.clone(), current));
        }
        items.extend(
            tags.iter()
                .map(|tag| ChoiceItem::new(tag.clone(), tag.clone())),
        );
        items
    }

    /// "Let the system choose", every card found, and the chosen card even
    /// when it is gone (moved to another slot, or removed).
    fn gpu_items(&self) -> Vec<ChoiceItem> {
        let current = value_to_text(self.value("iroriGpu"));
        let current = current.trim();
        let mut items = vec![ChoiceItem::new("", t("settings.gpu.system_choice"))];
        let cards = self.graphics.as_ref().map_or(&[][..], |g| &g.adapters[..]);
        items.extend(cards.iter().map(|card| {
            let label = if card.boot_vga {
                t_args(
                    "settings.gpu.card_desktop",
                    &[("name", &card.name()), ("slot", &card.slot)],
                )
            } else {
                t_args(
                    "settings.gpu.card",
                    &[("name", &card.name()), ("slot", &card.slot)],
                )
            };
            ChoiceItem::new(card.slot.clone(), label)
        }));
        if !current.is_empty() && !cards.iter().any(|c| c.slot == current) {
            let label = if self.graphics.is_some() {
                t_args("settings.gpu.card_missing", &[("slot", current)])
            } else {
                current.to_string().into()
            };
            items.push(ChoiceItem::new(current.to_string(), label));
        }
        items
    }

    pub(super) fn sync_hide_gpu_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.choices.get("iroriHideGpu").cloned() else {
            return;
        };
        let current = value_to_text(self.value("iroriHideGpu")).trim().to_string();
        let items = hide_gpu_items(self.hideable_gpus.as_deref().map(Vec::as_slice), &current);
        let current: SharedString = current.into();
        state.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&current, window, cx);
        });
    }

    pub(super) fn sync_gpu_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.choices.get("iroriGpu").cloned() else {
            return;
        };
        let items = self.gpu_items();
        let current: SharedString = value_to_text(self.value("iroriGpu"))
            .trim()
            .to_string()
            .into();
        state.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&current, window, cx);
        });
    }

    pub(super) fn sync_wine_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current: WineInstallation = self
            .value("wineVersion")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let Some(wine) = &mut self.wine else { return };
        let mut items: Vec<ChoiceItem> = wine
            .builds
            .iter()
            .flatten()
            .map(|w| ChoiceItem {
                key: w.bin.clone().into(),
                label: build_label(w, wine.sources.get(&w.bin).copied()),
            })
            .collect();
        if current.is_set() && !items.iter().any(|i| i.key.as_ref() == current.bin) {
            let label = if wine.builds.is_some() {
                t_args("settings.wine.build_missing", &[("name", &current.name)])
            } else {
                current.name.clone().into()
            };
            items.insert(
                0,
                ChoiceItem {
                    key: current.bin.clone().into(),
                    label,
                },
            );
            if let Some(builds) = &mut wine.builds {
                builds.insert(0, current.clone());
            }
        }
        let key: SharedString = current.bin.into();
        wine.select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&key, window, cx);
        });
    }
}
