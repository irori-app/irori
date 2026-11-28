use std::time::Duration;

use gpui_kit::component::input::TextareaState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::{AppContext as _, Context, Focusable as _, PathPromptOptions, Task, Window};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};
use serde_json::Value;

use crate::registry::{self, CustomKind, FieldKind, TextKind};
use crate::value::{
    env_text, get_path, override_key, parse_env_text, plan_game_write, remove_row,
    split_assignment, text_to_value, upsert_row, validate_discord_client_id, validate_dll_override,
    validate_env_key, validate_launcher_args, value_to_text, with_path,
};

use super::controls::slider_json;
use super::{FormTarget, PairEditor, SettingsForm};

impl SettingsForm {
    /// Reset every override of the game target.
    pub fn reset_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let FormTarget::Game(id) = &self.target else {
            return;
        };
        // Replacing the whole map also drops stale entries equal to the defaults.
        backend(cx)
            .settings
            .replace_overrides(id, Default::default());
        self.reload(window, cx);
    }

    // ---- writes ----

    /// Persist a value at `path` for this form's target.
    pub(super) fn write(
        &mut self,
        path: &'static str,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        if path == "steamGridDbApiKey"
            && matches!(self.target, FormTarget::App | FormTarget::Defaults)
        {
            let key = value
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let task = irori_ui::run(cx, async move { b.set_steamgrid_api_key(key).await });
            cx.spawn_in(window, async move |this, cx| {
                let res = task.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Err(e) = res {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.error.save_setting")),
                            cx,
                        );
                    }
                    this.reload(window, cx);
                });
            })
            .detach();
            return;
        }
        let result = match &self.target {
            FormTarget::App | FormTarget::Defaults => {
                let mut result = Ok(());
                b.settings.update_app(|s| match with_path(s, path, value) {
                    Ok(next) => *s = next,
                    Err(e) => result = Err(e),
                });
                if result.is_ok() && path.starts_with("iroriUi.") {
                    irori_ui::theme::apply(&b.settings.app().ui, Some(window), cx);
                }
                result
            }
            FormTarget::Game(id) => plan_game_write(&self.defaults, &self.effective, path, value)
                .map(|plan| b.settings.set_game_value(id, override_key(path), plan)),
        };
        if let Err(e) = result {
            window.push_notification(
                Notification::error(format!("{e:#}")).title(t("settings.error.save_setting")),
                cx,
            );
        }
        self.reload(window, cx);
    }

    #[cfg(test)]
    pub(crate) fn write_for_test(
        &mut self,
        path: &'static str,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write(path, value, window, cx);
    }

    #[cfg(test)]
    pub(crate) fn reset_for_test(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset_field(path, window, cx);
    }

    /// Return a field to the global default (game target).
    pub(super) fn reset_field(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FormTarget::Game(id) = &self.target else {
            return;
        };
        let key = override_key(path);
        if key == path {
            backend(cx).settings.set_game_value(id, key, None);
            self.errors.remove(path);
            self.reload(window, cx);
        } else if let Some(default) = get_path(&self.defaults_json, path).cloned() {
            self.write(path, default, window, cx);
        }
    }

    pub(super) fn commit_text(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(spec), Some(input)) = (registry::find(path), self.texts.get(path)) else {
            return;
        };
        let FieldKind::Text(kind) = spec.kind else {
            return;
        };
        let text = input.read(cx).value().to_string();
        if kind == TextKind::Secret && text.trim().is_empty() {
            return;
        }
        let checked = text_to_value(&text, kind).and_then(|v| {
            let s = v.as_str().unwrap_or_default();
            if path == "launcherArgs" {
                validate_launcher_args(s)?;
            }
            if path == "discordClientId" {
                validate_discord_client_id(s)?;
            }
            Ok(v)
        });
        match checked {
            Err(e) => {
                self.errors.insert(path, e.into());
                cx.notify();
            }
            Ok(v) => {
                self.errors.remove(path);
                if kind == TextKind::Secret {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
                if self.value(path) != Some(&v) {
                    self.write(path, v, window, cx);
                } else {
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn pick_path(
        &mut self,
        path: &'static str,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !directory,
            directories: directory,
            multiple: false,
            prompt: Some(if directory {
                t("settings.picker.select_folder")
            } else {
                t("settings.picker.select_file")
            }),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.error.file_picker")),
                            cx,
                        );
                    })
                    .ok();
                    None
                }
                _ => None,
            };
            if let Some(p) = picked {
                this.update_in(cx, |this, window, cx| {
                    if let Some(input) = this.texts.get(path) {
                        input.update(cx, |s, cx| {
                            s.set_value(p.to_string_lossy().to_string(), window, cx)
                        });
                    }
                    this.commit_text(path, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// A slider moved (`released` once the drag ends, or for a reset).
    pub(super) fn slider_changed(
        &mut self,
        path: &'static str,
        v: f32,
        released: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !released && self.slider_seen.get(path) == Some(&v) {
            return;
        }
        self.slider_seen.insert(path, v);
        if path == "iroriUi.fontSize" {
            // The interface size rescales this page, slider included: applied
            // mid-drag, the track would move under the pointer and fight it.
            // The percentage follows the drag; the size applies on release.
            if released {
                self.slider_writes.remove(path);
                self.write(path, slider_json(path, v), window, cx);
            } else {
                // Keeps `reload` from snapping the slider back meanwhile.
                self.slider_writes.insert(path, Task::ready(()));
                cx.notify();
            }
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            this.update_in(cx, |this, window, cx| {
                this.slider_writes.remove(path);
                this.write(path, slider_json(path, v), window, cx);
            })
            .ok();
        });
        self.slider_writes.insert(path, task);
        cx.notify();
    }

    /// Use the theme called `name`. A theme for the other colour mode than
    /// the one showing switches the mode too, so picking it always shows it.
    /// Going back to the desktop's theme lets go of the mode, which it decides.
    pub(super) fn pick_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if name.is_empty() && irori_ui::theme::followed_desktop(cx).is_some() {
            self.write("iroriUi.theme", Value::from("system"), window, cx);
        } else if let Some(dark) = irori_ui::theme::is_dark_theme(name, cx)
            && dark != cx.theme().mode.is_dark()
        {
            let mode = if dark { "dark" } else { "light" };
            self.write("iroriUi.theme", Value::from(mode), window, cx);
        }
        self.write(
            "iroriUi.themeName",
            Value::String(name.to_string()),
            window,
            cx,
        );
    }

    pub(super) fn rows(&self, path: &str) -> Vec<(String, String)> {
        let kind = self
            .pairs
            .get(path)
            .map(|p| p.kind)
            .unwrap_or(CustomKind::EnvVars);
        let (k1, k2) = PairEditor::keys(kind);
        self.value(path)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|r| (value_to_text(r.get(k1)), value_to_text(r.get(k2))))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn commit_pair(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.rows(path);
        let Some(editor) = self.pairs.get_mut(path) else {
            return;
        };
        let (first, second) = split_assignment(
            &editor.first.read(cx).value(),
            &editor.second.read(cx).value(),
        );
        let existing: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        let checked = match editor.kind {
            CustomKind::EnvVars => {
                validate_env_key(&first, &existing, editor.editing).map(|first| (first, second))
            }
            CustomKind::DllOverrides => {
                validate_dll_override(&first, &second, &existing, editor.editing)
            }
            CustomKind::Wrappers if first.is_empty() => {
                Err(t("settings.validate.wrapper_empty").to_string())
            }
            CustomKind::GogSaves if first.is_empty() || second.is_empty() => {
                Err(t("settings.validate.save_location_empty").to_string())
            }
            _ => Ok((first, second)),
        };
        match checked {
            Err(e) => {
                editor.error = Some(e.into());
                cx.notify();
            }
            Ok((first, second)) => {
                let (k1, k2) = PairEditor::keys(editor.kind);
                let row = serde_json::json!({ k1: first, k2: second });
                let current = self
                    .value(path)
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new()));
                let editor = self.pairs.get_mut(path).expect("editor exists");
                let next = upsert_row(&current, editor.editing, row);
                editor.editing = None;
                editor.error = None;
                editor.first.update(cx, |s, cx| s.set_value("", window, cx));
                editor
                    .second
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.write(path, next, window, cx);
            }
        }
    }

    pub(super) fn edit_row(
        &mut self,
        path: &'static str,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.rows(path);
        let Some((a, b)) = rows.get(ix).cloned() else {
            return;
        };
        let Some(editor) = self.pairs.get_mut(path) else {
            return;
        };
        editor.editing = Some(ix);
        editor.error = None;
        editor.first.update(cx, |s, cx| s.set_value(a, window, cx));
        editor.second.update(cx, |s, cx| s.set_value(b, window, cx));
        editor.first.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn cancel_edit(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.pairs.get_mut(path) else {
            return;
        };
        editor.editing = None;
        editor.error = None;
        editor.first.update(cx, |s, cx| s.set_value("", window, cx));
        editor
            .second
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn remove_row(
        &mut self,
        path: &'static str,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .value(path)
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        if let Some(editor) = self.pairs.get_mut(path)
            && editor.editing == Some(ix)
        {
            editor.editing = None;
        }
        self.write(path, remove_row(&current, ix), window, cx);
    }

    /// Show the environment variables as text to edit or paste into.
    pub(super) fn open_env_text(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = env_text(&self.rows(path));
        let Some(editor) = self.pairs.get_mut(path) else {
            return;
        };
        let state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 14)
                .placeholder(t("settings.env_text.placeholder"))
                .default_value(text)
        });
        state.read(cx).focus_handle(cx).focus(window, cx);
        editor.editing = None;
        editor.error = None;
        editor.text = Some(state);
        cx.notify();
    }

    pub(super) fn close_env_text(&mut self, path: &'static str, cx: &mut Context<Self>) {
        if let Some(editor) = self.pairs.get_mut(path) {
            editor.text = None;
            editor.error = None;
            cx.notify();
        }
    }

    /// Replace the variables with the text's, and take what a launch line
    /// says around `%command%` into the settings it belongs to.
    pub(super) fn apply_env_text(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.pairs.get_mut(path) else {
            return;
        };
        let Some(state) = editor.text.clone() else {
            return;
        };
        let parsed = match parse_env_text(&state.read(cx).value()) {
            Ok(parsed) => parsed,
            Err(e) => {
                editor.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        editor.text = None;
        editor.error = None;
        let rows: Vec<Value> = parsed
            .vars
            .iter()
            .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
            .collect();
        self.write(path, Value::Array(rows), window, cx);
        let mut also = Vec::new();
        if parsed.game_mode {
            self.write("useGameMode", Value::Bool(true), window, cx);
            also.push("useGameMode");
        }
        if parsed.mangohud {
            self.write("showMangohud", Value::Bool(true), window, cx);
            also.push("showMangohud");
        }
        if let Some((exe, args)) = parsed.wrapper {
            let current = self
                .value("wrapperOptions")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let row = serde_json::json!({ "exe": exe, "args": args });
            let next = upsert_row(&current, None, row);
            self.write("wrapperOptions", next, window, cx);
            also.push("wrapperOptions");
        }
        if !parsed.args.is_empty() {
            let current = value_to_text(self.value("launcherArgs"));
            let added = crate::value::join_launch_args(&parsed.args);
            let next = [current.trim(), added.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            self.write("launcherArgs", Value::String(next), window, cx);
            also.push("launcherArgs");
        }
        if !also.is_empty() {
            let names = also
                .iter()
                .filter_map(|p| registry::find(p))
                .map(|f| f.label_text().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            window.push_notification(
                Notification::success(t_args(
                    "settings.env_text.also_set",
                    &[("settings", &names)],
                )),
                cx,
            );
        }
    }
}
