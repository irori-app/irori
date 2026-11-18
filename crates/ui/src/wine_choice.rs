//! The Wine or Proton build and the prefix a Windows game plays with, chosen
//! where a game enters the library (an added app, an existing install).
//!
//! The choice starts at what the game already uses: its own build and prefix
//! when it has them, else "Default", which follows the game defaults. Only a
//! choice that differs from that is written, as the per-game `wineVersion`
//! and `winePrefix` overrides, so a game left on "Default" keeps following
//! the defaults when they change.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions,
    Render, SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::{GameId, GameInfo, WineInstallation};
use irori_services::Backend;

use crate::i18n::{t, t_args};
use crate::nav::{Route, navigate};
use crate::state::backend;

/// One build in the list; the value "" is "Default".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineItem {
    value: SharedString,
    title: SharedString,
}

impl SelectItem for WineItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SharedString {
        &self.value
    }
}

type WineSelect = SelectState<Vec<WineItem>>;

/// What the user chose, as plain data to apply once the game exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WineSetup {
    /// The build picked; `None` for "Default".
    pub wine: Option<WineInstallation>,
    /// The prefix as entered; empty for the default location.
    pub prefix: String,
}

/// A per-game setting to write (`Some`) or reset to the defaults (`None`).
pub type SettingChange = (&'static str, Option<serde_json::Value>);

impl WineSetup {
    /// The overrides that make `id` use this choice, given the build and
    /// prefix it is pinned to now (its own overrides, not the defaults).
    pub fn changes(
        &self,
        pinned_wine: Option<&WineInstallation>,
        pinned_prefix: Option<&str>,
        default_prefix: &Path,
    ) -> Vec<SettingChange> {
        let mut out = Vec::new();
        match (&self.wine, pinned_wine) {
            (None, Some(_)) => out.push(("wineVersion", None)),
            (Some(w), pinned) if pinned.is_none_or(|p| p.bin != w.bin) => {
                if let Ok(v) = serde_json::to_value(w) {
                    out.push(("wineVersion", Some(v)));
                }
            }
            _ => {}
        }
        let prefix = self.prefix.trim();
        if prefix.is_empty() {
            if pinned_prefix.is_some() {
                out.push(("winePrefix", None));
            }
        } else if pinned_prefix != Some(prefix)
            && !(pinned_prefix.is_none()
                && irori_core::paths::expand_home(prefix) == default_prefix)
        {
            out.push((
                "winePrefix",
                Some(serde_json::Value::String(prefix.to_string())),
            ));
        }
        out
    }

    /// Write the choice into `game`'s settings.
    pub fn apply(&self, backend: &Backend, id: &GameId, game: &GameInfo) {
        let overrides = backend.settings.overrides(id);
        let effective = backend.settings.game(id);
        let pinned_wine = overrides
            .is_overridden("wineVersion")
            .then_some(&effective.wine_version)
            .filter(|w| w.is_set());
        let pinned_prefix = overrides
            .is_overridden("winePrefix")
            .then_some(effective.wine_prefix.as_str())
            .filter(|p| !p.is_empty());
        let default = irori_runtime::default_prefix(&backend.settings.app(), game);
        for (key, value) in self.changes(pinned_wine, pinned_prefix, &default) {
            backend.settings.set_game_value(id, key, value);
        }
    }
}

/// The build list and prefix field.
pub struct WineChoice {
    /// Every build found; `None` while looking.
    wines: Option<Vec<WineInstallation>>,
    chosen: Option<WineInstallation>,
    select: Entity<WineSelect>,
    prefix: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl WineChoice {
    /// The choice for a game not in the library yet: "Default" and the
    /// default prefix, which `default_prefix` shows.
    pub fn new(default_prefix: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with(None, String::new(), default_prefix, window, cx)
    }

    /// The choice for `id`, starting at the build and prefix it is pinned to.
    pub fn for_game(
        id: &GameId,
        game: &GameInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let b = backend(cx);
        let overrides = b.settings.overrides(id);
        let effective = b.settings.game(id);
        let wine = (overrides.is_overridden("wineVersion") && effective.wine_version.is_set())
            .then(|| effective.wine_version.clone());
        let prefix = if overrides.is_overridden("winePrefix") {
            effective.wine_prefix.clone()
        } else {
            String::new()
        };
        let default = irori_runtime::default_prefix(&b.settings.app(), game);
        Self::with(wine, prefix, &default, window, cx)
    }

    fn with(
        wine: Option<WineInstallation>,
        prefix: String,
        default_prefix: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.new(|cx| SelectState::new(Vec::<WineItem>::new(), None, window, cx));
        let placeholder = SharedString::from(default_prefix.to_string_lossy().into_owned());
        let prefix = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(prefix)
        });
        let subscriptions = vec![cx.subscribe_in(
            &select,
            window,
            |this, _, e: &SelectEvent<Vec<WineItem>>, _, cx| {
                let SelectEvent::Confirm(value) = e;
                this.chosen = value.as_ref().filter(|v| !v.is_empty()).and_then(|v| {
                    this.wines
                        .as_ref()?
                        .iter()
                        .find(|w| w.bin == v.as_ref())
                        .cloned()
                });
                cx.notify();
            },
        )];
        let mut this = Self {
            wines: None,
            chosen: wine,
            select,
            prefix,
            _subscriptions: subscriptions,
        };
        this.look_for_builds(window, cx);
        this
    }

    fn look_for_builds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = backend(cx).settings.app();
        let default_name = app.game.wine_version.name.clone();
        let task = crate::state::run(cx, async move {
            Ok(irori_runtime::wine_installations(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let mut list = task.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                // A pinned build that is gone stays listed, so it isn't
                // dropped without the user choosing another.
                if let Some(current) = &this.chosen
                    && !list.iter().any(|w| w.bin == current.bin)
                {
                    list.insert(0, current.clone());
                }
                let default = if default_name.is_empty() {
                    t("ui.wine.default")
                } else {
                    t_args("ui.wine.default_named", &[("name", &default_name)])
                };
                let items: Vec<WineItem> = std::iter::once(WineItem {
                    value: SharedString::default(),
                    title: default,
                })
                .chain(list.iter().map(|w| WineItem {
                    value: w.bin.clone().into(),
                    title: w.name.clone().into(),
                }))
                .collect();
                let selected = this
                    .chosen
                    .as_ref()
                    .and_then(|c| list.iter().position(|w| w.bin == c.bin))
                    .map_or(0, |ix| ix + 1);
                this.select.update(cx, |state, cx| {
                    state.set_items(items, window, cx);
                    state.set_selected_index(Some(IndexPath::default().row(selected)), window, cx);
                });
                this.wines = Some(list);
                cx.notify();
            });
        })
        .detach();
    }

    /// Show `prefix` as the default location (an added app's follows its
    /// title).
    pub fn set_default_prefix(&mut self, prefix: &Path, window: &mut Window, cx: &mut App) {
        let text = SharedString::from(prefix.to_string_lossy().into_owned());
        self.prefix
            .update(cx, |s, cx| s.set_placeholder(text, window, cx));
    }

    /// Put `prefix` in the prefix field (an installer pinned it).
    pub fn set_prefix(&mut self, prefix: &Path, window: &mut Window, cx: &mut App) {
        let text = prefix.to_string_lossy().into_owned();
        self.prefix
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// The choice as it stands.
    pub fn setup(&self, cx: &App) -> WineSetup {
        WineSetup {
            wine: self.chosen.clone(),
            prefix: self.prefix.read(cx).value().trim().to_string(),
        }
    }

    fn browse_prefix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("ui.wine.select_prefix")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = rx
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths: Vec<PathBuf>| paths.into_iter().next())
            else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.set_prefix(&path, window, cx));
        })
        .detach();
    }
}

impl Render for WineChoice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: SharedString| div().text_sm().font_semibold().child(text);
        let none_found = self.wines.as_ref().is_some_and(Vec::is_empty);
        v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(label(t("ui.wine.build")))
                    .map(|col| match &self.wines {
                        None => col.child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .text_color(muted)
                                .child(Spinner::new().small())
                                .child(div().min_w_0().child(t("ui.wine.looking"))),
                        ),
                        Some(_) => col.child(Select::new(&self.select)),
                    })
                    .when(none_found, |col| {
                        col.child(Alert::warning("wine-choice-none", t("ui.wine.none")))
                            .child(
                                h_flex().child(
                                    Button::new("wine-choice-manager")
                                        .small()
                                        .outline()
                                        .label(t("ui.wine.open_manager"))
                                        .on_click(|_, window, cx| {
                                            window.close_dialog(cx);
                                            window.close_sheet(cx);
                                            navigate(Route::WineManager, cx);
                                        }),
                                ),
                            )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label(t("ui.wine.prefix")))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.prefix)))
                            .child(
                                Button::new("wine-choice-browse-prefix")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label(t("ui.wine.browse"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse_prefix(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("ui.wine.prefix_help")),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wine(bin: &str) -> WineInstallation {
        WineInstallation {
            bin: bin.into(),
            name: bin.into(),
            ..Default::default()
        }
    }

    fn keys(changes: &[SettingChange]) -> Vec<(&str, bool)> {
        changes.iter().map(|(k, v)| (*k, v.is_some())).collect()
    }

    #[test]
    fn default_and_the_default_prefix_write_nothing() {
        let setup = WineSetup::default();
        assert!(setup.changes(None, None, Path::new("/p/Game")).is_empty());
        let same = WineSetup {
            wine: None,
            prefix: "/p/Game".into(),
        };
        assert!(same.changes(None, None, Path::new("/p/Game")).is_empty());
    }

    #[test]
    fn a_picked_build_and_prefix_are_written() {
        let setup = WineSetup {
            wine: Some(wine("/w/proton")),
            prefix: "/elsewhere".into(),
        };
        let changes = setup.changes(None, None, Path::new("/p/Game"));
        assert_eq!(
            keys(&changes),
            [("wineVersion", true), ("winePrefix", true)]
        );
        assert_eq!(
            changes[1].1,
            Some(serde_json::Value::String("/elsewhere".into()))
        );
        // The same pins again write nothing.
        assert!(
            setup
                .changes(
                    Some(&wine("/w/proton")),
                    Some("/elsewhere"),
                    Path::new("/p/Game")
                )
                .is_empty()
        );
    }

    #[test]
    fn going_back_to_default_resets_the_pins() {
        let setup = WineSetup::default();
        let changes = setup.changes(Some(&wine("/w/a")), Some("/x"), Path::new("/p/Game"));
        assert_eq!(
            keys(&changes),
            [("wineVersion", false), ("winePrefix", false)]
        );
    }

    #[test]
    fn another_build_replaces_the_pinned_one() {
        let setup = WineSetup {
            wine: Some(wine("/w/b")),
            prefix: String::new(),
        };
        let changes = setup.changes(Some(&wine("/w/a")), None, Path::new("/p/Game"));
        assert_eq!(keys(&changes), [("wineVersion", true)]);
    }
}
