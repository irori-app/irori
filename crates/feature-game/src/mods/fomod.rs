//! A FOMOD installer's steps as a dialog: each step's groups of options as
//! radio buttons or check boxes, with the focused option's picture and
//! description beside them, and Back, Next and Install.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, StyledImage as _,
    Window, div, img, rems,
};
use irori_mods::fomod::{Choices, GroupKind, ModuleConfig, PluginType};
use irori_services::FomodPrompt;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::ops::{notify_error, notify_success};

/// Ask the installer's questions, then install.
pub(crate) fn open_fomod_dialog(prompt: FomodPrompt, window: &mut Window, cx: &mut App) {
    let token = prompt.token;
    let title: SharedString = prompt.mod_name.clone().into();
    let wizard = cx.new(|_| Wizard::new(prompt));
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title(title.clone())
            .w(window.rem_size() * 50.)
            .on_close(move |_, _, cx| backend(cx).cancel_fomod(token))
            .child(wizard.clone())
    });
}

struct Wizard {
    prompt: FomodPrompt,
    choices: Choices,
    /// Index into the installer's steps.
    step: usize,
    /// Steps the user has seen; later ones get fresh defaults when earlier
    /// choices change their options.
    seen: Vec<bool>,
    /// The option whose picture and description show: (group, option).
    focused: Option<(usize, usize)>,
    installing: bool,
}

impl Wizard {
    fn new(prompt: FomodPrompt) -> Self {
        let p = prompt.clone();
        let choices = prompt.config.default_choices(&|f| p.file_state(f));
        let steps = prompt.config.steps.len();
        let mut this = Self {
            prompt,
            choices,
            step: 0,
            seen: vec![false; steps],
            focused: None,
            installing: false,
        };
        this.step = this.next_visible(0).unwrap_or(0);
        this.visit();
        this
    }

    fn config(&self) -> &ModuleConfig {
        &self.prompt.config
    }

    fn visible(&self, step: usize) -> bool {
        let p = &self.prompt;
        self.config()
            .step_visible(step, &self.choices, &|f| p.file_state(f))
    }

    fn next_visible(&self, from: usize) -> Option<usize> {
        (from..self.config().steps.len()).find(|s| self.visible(*s))
    }

    fn previous_visible(&self) -> Option<usize> {
        (0..self.step).rev().find(|s| self.visible(*s))
    }

    /// Entering a step: unseen steps take defaults that follow the choices
    /// made so far.
    fn visit(&mut self) {
        let step = self.step;
        if self.seen.get(step).copied().unwrap_or(true) {
            return;
        }
        let p = self.prompt.clone();
        let picks = self
            .config()
            .default_step(step, &self.choices, &|f| p.file_state(f));
        if let Some(slot) = self.choices.0.get_mut(step) {
            *slot = picks;
        }
        self.seen[step] = true;
        self.focused = None;
    }

    fn step_valid(&self) -> bool {
        self.config().steps.get(self.step).is_none_or(|s| {
            (0..s.groups.len()).all(|g| self.config().group_valid(self.step, g, &self.choices))
        })
    }

    fn toggle(&mut self, group: usize, plugin: usize, on: bool, cx: &mut Context<Self>) {
        let Some(kind) = self
            .config()
            .steps
            .get(self.step)
            .and_then(|s| s.groups.get(group))
            .map(|g| g.kind)
        else {
            return;
        };
        let step = self.step;
        let Some(picks) = self.choices.0.get_mut(step).and_then(|s| s.get_mut(group)) else {
            return;
        };
        match kind {
            GroupKind::All => {}
            k if k.single() => {
                for (i, p) in picks.iter_mut().enumerate() {
                    *p = i == plugin && (on || k == GroupKind::ExactlyOne);
                }
            }
            _ => {
                if let Some(p) = picks.get_mut(plugin) {
                    *p = on;
                }
            }
        }
        // Later steps follow the new flags.
        for seen in self.seen.iter_mut().skip(step + 1) {
            *seen = false;
        }
        self.focused = Some((group, plugin));
        cx.notify();
    }

    fn go_next(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.next_visible(self.step + 1) {
            self.step = next;
            self.visit();
            cx.notify();
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.previous_visible() {
            self.step = prev;
            self.focused = None;
            cx.notify();
        }
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Steps never reached keep the defaults they would have shown.
        for step in 0..self.config().steps.len() {
            if !self.seen[step] && self.visible(step) {
                let saved = self.step;
                self.step = step;
                self.visit();
                self.step = saved;
            }
        }
        self.installing = true;
        cx.notify();
        let b = backend(cx);
        let (token, choices) = (self.prompt.token, self.choices.clone());
        let task = irori_ui::run(cx, async move { b.finish_fomod(token, choices).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.installing = false;
                match result {
                    Ok(name) => {
                        window.close_dialog(cx);
                        notify_success(
                            window,
                            cx,
                            t_args("game.mods.installed", &[("name", &name)]),
                        );
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.install_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let step = self.config().steps.get(self.step);
        let (group, plugin) = self
            .focused
            .or_else(|| {
                // Show the first chosen option until the user points at one.
                let s = step?;
                s.groups.iter().enumerate().find_map(|(g, group)| {
                    (0..group.plugins.len())
                        .find(|p| self.choices.is_chosen(self.step, g, *p))
                        .map(|p| (g, p))
                })
            })
            .unwrap_or((0, 0));
        let plugin = step
            .and_then(|s| s.groups.get(group))
            .and_then(|g| g.plugins.get(plugin));
        let image = plugin
            .and_then(|p| p.image.as_deref())
            .or(self.config().image.as_deref())
            .and_then(|path| self.prompt.image(path))
            .map(|p| p.to_path_buf());
        v_flex()
            .w(rems(18.))
            .flex_shrink_0()
            .gap_3()
            .child(match image {
                Some(path) => img(path)
                    .w_full()
                    .h(rems(11.))
                    .rounded(theme.radius_lg)
                    .object_fit(gpui_kit::ObjectFit::Cover)
                    .into_any_element(),
                None => div()
                    .w_full()
                    .h(rems(11.))
                    .rounded(theme.radius_lg)
                    .bg(theme.secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Image)
                            .large()
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element(),
            })
            .when_some(plugin, |col, p| {
                col.child(div().font_semibold().child(p.name.clone()))
                    .child(
                        div()
                            .id("fomod-description")
                            .max_h(rems(12.))
                            .overflow_y_scrollbar()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(if p.description.is_empty() {
                                t("game.mods.fomod_no_description").to_string()
                            } else {
                                p.description.clone()
                            }),
                    )
            })
            .into_any_element()
    }

    fn render_groups(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let p = self.prompt.clone();
        let flags = self
            .config()
            .flags_before(self.step, &self.choices, &|f| p.file_state(f));
        let Some(step) = self.config().steps.get(self.step).cloned() else {
            return div().into_any_element();
        };
        let mut col = v_flex().gap_4();
        for (g, group) in step.groups.iter().enumerate() {
            let rule = match group.kind {
                GroupKind::ExactlyOne => t("game.mods.fomod_pick_one"),
                GroupKind::AtMostOne => t("game.mods.fomod_pick_one_or_none"),
                GroupKind::AtLeastOne => t("game.mods.fomod_pick_some"),
                GroupKind::All => t("game.mods.fomod_all"),
                GroupKind::Any => t("game.mods.fomod_pick_any"),
            };
            let mut options = v_flex().gap_1();
            for (ix, plugin) in group.plugins.iter().enumerate() {
                let kind = ModuleConfig::plugin_type(plugin, &flags, &|f| p.file_state(f));
                let chosen = self.choices.is_chosen(self.step, g, ix);
                let locked = group.kind == GroupKind::All
                    || kind == PluginType::Required
                    || kind == PluginType::NotUsable;
                let label: SharedString = plugin.name.clone().into();
                let id = ElementId::Name(format!("fomod-{}-{g}-{ix}", self.step).into());
                let control: AnyElement = if group.kind.single() {
                    Radio::new(id)
                        .label(label)
                        .checked(chosen)
                        .disabled(locked)
                        .on_click(
                            cx.listener(move |this, on: &bool, _, cx| this.toggle(g, ix, *on, cx)),
                        )
                        .into_any_element()
                } else {
                    Checkbox::new(id)
                        .label(label)
                        .checked(chosen)
                        .disabled(locked)
                        .on_click(
                            cx.listener(move |this, on: &bool, _, cx| this.toggle(g, ix, *on, cx)),
                        )
                        .into_any_element()
                };
                let badge = match kind {
                    PluginType::Recommended => {
                        Some(Tag::info().xsmall().child(t("game.mods.fomod_recommended")))
                    }
                    PluginType::Required => Some(
                        Tag::secondary()
                            .xsmall()
                            .child(t("game.mods.fomod_required")),
                    ),
                    PluginType::NotUsable => Some(
                        Tag::warning()
                            .xsmall()
                            .child(t("game.mods.fomod_not_usable")),
                    ),
                    _ => None,
                };
                options = options.child(
                    h_flex()
                        .id(("fomod-option", g * 1000 + ix))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .when(self.focused == Some((g, ix)), |r| r.bg(theme.muted))
                        .hover(|s| s.bg(theme.muted))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.focused != Some((g, ix)) {
                                this.focused = Some((g, ix));
                                cx.notify();
                            }
                        }))
                        .child(div().flex_1().min_w_0().child(control))
                        .children(badge),
                );
            }
            let valid = self.config().group_valid(self.step, g, &self.choices);
            col = col.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().font_semibold().text_sm().child(group.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if valid {
                                        theme.muted_foreground
                                    } else {
                                        theme.danger
                                    })
                                    .child(rule),
                            ),
                    )
                    .child(options),
            );
        }
        col.into_any_element()
    }
}

impl Render for Wizard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let visible: Vec<usize> = (0..self.config().steps.len())
            .filter(|s| self.visible(*s))
            .collect();
        let position = visible.iter().position(|s| *s == self.step).unwrap_or(0);
        let last = self.next_visible(self.step + 1).is_none();
        let first = self.previous_visible().is_none();
        let step_name = self
            .config()
            .steps
            .get(self.step)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let valid = self.step_valid();
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .child(step_name),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t_args(
                                "game.mods.fomod_step",
                                &[
                                    ("step", &(position + 1).to_string()),
                                    ("total", &visible.len().max(1).to_string()),
                                ],
                            )),
                    ),
            )
            .child(
                h_flex()
                    .gap_5()
                    .items_start()
                    .child(
                        div()
                            .id("fomod-groups")
                            .flex_1()
                            .min_w_0()
                            .max_h(rems(26.))
                            .overflow_y_scrollbar()
                            .child(self.render_groups(cx)),
                    )
                    .child(self.render_preview(cx)),
            )
            .child(
                DialogFooter::new()
                    .child(
                        Button::new("fomod-back")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .label(t("game.mods.fomod_back"))
                            .disabled(first || self.installing)
                            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
                    )
                    .child(if last {
                        Button::new("fomod-install")
                            .primary()
                            .icon(IconName::Download)
                            .label(t("game.mods.install"))
                            .loading(self.installing)
                            .disabled(!valid || self.installing)
                            .on_click(cx.listener(|this, _, window, cx| this.install(window, cx)))
                    } else {
                        Button::new("fomod-next")
                            .primary()
                            .label(t("game.mods.fomod_next"))
                            .icon(IconName::ArrowRight)
                            .disabled(!valid)
                            .on_click(cx.listener(|this, _, _, cx| this.go_next(cx)))
                    }),
            )
    }
}
