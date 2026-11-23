//! A set-up launcher's own Wine settings (Linux and macOS): the Wine build
//! it runs with, which can change after setup, and environment variables
//! for it and every game started through it. They are kept with the
//! launcher's setup (`irori_launchers::LaunchersStore::set_wine_settings`)
//! and laid over the game defaults, as a game's own settings are.

use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div, rems,
};
use irori_core::{EnvVar, WineInstallation};
use irori_launchers::LauncherKind as Launcher;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::common::{notify_error, notify_success};
use crate::launchers::{WineChoice, launchers_store};

/// Open the Wine settings of `launcher`.
pub fn open_launcher_wine_dialog(launcher: Launcher, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(store) = launchers_store(cx) else {
        return;
    };
    let Some((wine, variables)) = store.wine_settings(launcher) else {
        return;
    };
    let view = cx.new(|cx| LauncherWine::new(launcher, wine, &variables, window, cx));
    let title = t_args(
        "manage.launchers.wine.title",
        &[("launcher", launcher.display_name())],
    );
    let width = window.rem_size() * 32.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(title.clone()).w(width).child(view.clone())
    });
}

struct LauncherWine {
    launcher: Launcher,
    current: WineInstallation,
    wines: Vec<WineInstallation>,
    select: Entity<SelectState<Vec<WineChoice>>>,
    variables: Entity<InputState>,
    saving: bool,
}

impl LauncherWine {
    fn new(
        launcher: Launcher,
        current: WineInstallation,
        variables: &[EnvVar],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
        let text = format_variables(variables);
        let variables = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("DXVK_ASYNC=1 WINE_FULLSCREEN_FSR=1")
                .default_value(text)
        });
        let this = Self {
            launcher,
            current,
            wines: Vec::new(),
            select,
            variables,
            saving: false,
        };
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations_with_sources(&b.settings.app()).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(found) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let items: Vec<WineChoice> = found
                    .iter()
                    .map(|(w, source)| WineChoice {
                        value: w.bin.clone().into(),
                        title: irori_ui::wine_choice::build_label(w, *source),
                    })
                    .collect();
                this.wines = found.into_iter().map(|(w, _)| w).collect();
                let ix = this
                    .wines
                    .iter()
                    .position(|w| w.bin == this.current.bin)
                    .map(|row| IndexPath::default().row(row));
                this.select.update(cx, |s, cx| {
                    s.set_items(items, window, cx);
                    s.set_selected_index(ix, window, cx);
                });
                cx.notify();
            });
        })
        .detach();
        this
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = launchers_store(cx) else {
            return;
        };
        let bin = self.select.read(cx).selected_value().map(|v| v.to_string());
        let wine = bin
            .and_then(|bin| self.wines.iter().find(|w| w.bin == bin).cloned())
            .unwrap_or_else(|| self.current.clone());
        let variables = parse_variables(&self.variables.read(cx).value());
        let launcher = self.launcher;
        self.saving = true;
        cx.notify();
        let task = irori_ui::run(cx, async move {
            store.set_wine_settings(launcher, wine, variables).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                cx.notify();
                let name = launcher.display_name();
                match result {
                    Ok(()) => {
                        notify_success(
                            t_args("manage.launchers.wine.saved", &[("launcher", name)]),
                            window,
                            cx,
                        );
                        window.close_dialog(cx);
                    }
                    Err(e) => notify_error(
                        t_args("manage.launchers.wine.failed", &[("launcher", name)]),
                        &e,
                        window,
                        cx,
                    ),
                }
            });
        })
        .detach();
    }
}

/// `KEY=value` words of the variables field; other words are left out.
pub(crate) fn parse_variables(text: &str) -> Vec<EnvVar> {
    irori_core::process::split_args(text)
        .into_iter()
        .filter_map(|w| {
            let (key, value) = w.split_once('=')?;
            let valid = !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !key.starts_with(|c: char| c.is_ascii_digit());
            valid.then(|| EnvVar {
                key: key.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

/// The variables as the field shows them, quoted where needed.
pub(crate) fn format_variables(variables: &[EnvVar]) -> String {
    variables
        .iter()
        .map(|v| {
            format!(
                "{}={}",
                v.key,
                if v.value.is_empty() || v.value.contains(char::is_whitespace) {
                    irori_core::process::shell_quote(&v.value)
                } else {
                    v.value.clone()
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Render for LauncherWine {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child(t("manage.launchers.wine.build")))
                    .child(Select::new(&self.select).small()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child(t("manage.launchers.wine.variables")))
                    .child(Input::new(&self.variables).small())
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.launchers.wine.variables_help")),
                    ),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("launcher-wine-save")
                        .primary()
                        .small()
                        .w(rems(8.))
                        .loading(self.saving)
                        .label(t("manage.launchers.wine.save"))
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_read_back_as_written() {
        let vars = parse_variables("DXVK_ASYNC=1 FOO='a b' 1BAD=x junk EMPTY=");
        assert_eq!(
            vars,
            [
                EnvVar {
                    key: "DXVK_ASYNC".into(),
                    value: "1".into()
                },
                EnvVar {
                    key: "FOO".into(),
                    value: "a b".into()
                },
                EnvVar {
                    key: "EMPTY".into(),
                    value: String::new()
                },
            ]
        );
        assert_eq!(parse_variables(&format_variables(&vars)), vars);
    }
}
