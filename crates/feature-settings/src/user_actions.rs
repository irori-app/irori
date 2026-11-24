//! "Actions" panel of a game's Launch section (`iroriActions`, per game):
//! programs and links offered beside the game's Play button, in the game's
//! menu, the library's context menu and the command palette. Each can be
//! added, changed, moved up and removed here.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::{GameId, HostPlatform, UserAction};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::i18n::LanguageStamp;
use crate::widgets::{muted, panel};

/// Settings key of the list.
pub(crate) const KEY: &str = "iroriActions";

/// The list with the `ix`-th action moved one place up; unchanged for the
/// first or a place past the end.
pub(crate) fn moved_up(mut actions: Vec<UserAction>, ix: usize) -> Vec<UserAction> {
    if ix > 0 && ix < actions.len() {
        actions.swap(ix - 1, ix);
    }
    actions
}

/// The open editor: a new action (`index == None`) or one being changed.
struct Editor {
    index: Option<usize>,
    target: Entity<InputState>,
    args: Entity<InputState>,
    name: Entity<InputState>,
    in_prefix: bool,
    counts_as_playing: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) struct UserActionsPanel {
    id: GameId,
    actions: Vec<UserAction>,
    /// Programs can run in the game's Wine prefix (not Windows, not a native
    /// game).
    prefix_possible: bool,
    editor: Option<Editor>,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl UserActionsPanel {
    pub(crate) fn new(id: GameId, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            id,
            actions: Vec::new(),
            prefix_possible: false,
            editor: None,
            language: LanguageStamp::new(),
            _subscriptions: vec![cx.observe(&settings, |this, _, cx| {
                this.read(cx);
                cx.notify();
            })],
        };
        this.read(cx);
        this
    }

    fn read(&mut self, cx: &App) {
        let b = backend(cx);
        let host = HostPlatform::current();
        self.actions = b.settings.game(&self.id).actions;
        let wine = b
            .game(&self.id)
            .and_then(|g| g.installed_platform())
            .is_none_or(|p| host.needs_wine(p));
        self.prefix_possible = host != HostPlatform::Windows && wine;
    }

    fn save(&mut self, actions: Vec<UserAction>, cx: &mut Context<Self>) {
        let value = (!actions.is_empty())
            .then(|| serde_json::to_value(&actions).ok())
            .flatten();
        backend(cx).settings.set_game_value(&self.id, KEY, value);
        self.read(cx);
        cx.notify();
    }

    fn open_editor(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let action = index
            .and_then(|i| self.actions.get(i).cloned())
            .unwrap_or_default();
        let mut input = |text: String, placeholder: SharedString, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
            })
        };
        let target = input(
            action.target.clone(),
            t("settings.actions.target_placeholder"),
            cx,
        );
        let args = input(
            action.args.clone(),
            t("settings.programs.args_placeholder"),
            cx,
        );
        let name = input(
            action.name.clone(),
            t("settings.actions.name_placeholder"),
            cx,
        );
        let mut subscriptions = Vec::new();
        for state in [&target, &args, &name] {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.commit(window, cx),
                    InputEvent::Change => {
                        if let Some(editor) = &mut this.editor
                            && editor.error.take().is_some()
                        {
                            cx.notify();
                        }
                    }
                    _ => {}
                },
            ));
        }
        target.focus_handle(cx).focus(window, cx);
        self.editor = Some(Editor {
            index,
            target,
            args,
            name,
            in_prefix: action.in_prefix && self.prefix_possible,
            counts_as_playing: action.counts_as_playing,
            error: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let action = UserAction {
            name: editor.name.read(cx).value().trim().to_string(),
            target: editor.target.read(cx).value().trim().to_string(),
            args: editor.args.read(cx).value().trim().to_string(),
            in_prefix: editor.in_prefix && self.prefix_possible,
            counts_as_playing: editor.counts_as_playing,
        };
        if !action.is_set() {
            editor.error = Some(t("settings.actions.error.target"));
            cx.notify();
            return;
        }
        let mut actions = self.actions.clone();
        match editor.index.filter(|i| *i < actions.len()) {
            Some(i) => actions[i] = action,
            None => actions.push(action),
        }
        self.editor = None;
        self.save(actions, cx);
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.picker.select_file")),
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
            let Some(picked) = picked else { return };
            this.update_in(cx, |this, window, cx| {
                let prefix_possible = this.prefix_possible;
                let Some(editor) = &mut this.editor else {
                    return;
                };
                let text = picked.to_string_lossy().into_owned();
                if prefix_possible {
                    editor.in_prefix = irori_core::LaunchProgram {
                        path: text.clone(),
                        ..Default::default()
                    }
                    .looks_like_windows();
                }
                editor
                    .target
                    .update(cx, |s, cx| s.set_value(text, window, cx));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_row(&self, ix: usize, action: &UserAction, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let mono = theme.mono_font_family.clone();
        let label = action.label();
        let mut tags: Vec<SharedString> = Vec::new();
        if action.is_link() {
            tags.push(t("settings.actions.link_tag"));
        } else {
            if action.in_prefix && self.prefix_possible {
                tags.push(t("settings.programs.in_prefix_tag"));
            }
            if action.counts_as_playing {
                tags.push(t("settings.actions.counts_tag"));
            }
        }
        let command = if action.args.is_empty() {
            action.target.clone()
        } else {
            format!("{} {}", action.target, action.args)
        };
        let id = self.id.clone();
        h_flex()
            .id(ElementId::Name(format!("action-row-{ix}").into()))
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .flex_wrap()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_medium()
                                    .child(label.clone()),
                            )
                            .children(
                                tags.into_iter()
                                    .map(|tag| Tag::secondary().xsmall().outline().child(tag)),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted_fg)
                            .font_family(mono)
                            .child(command),
                    ),
            )
            .child(
                Button::new(ElementId::Name(format!("action-run-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(irori_ui::user_actions::icon(action))
                    .tooltip(t("settings.actions.run"))
                    .accessibility_label(t_args("settings.actions.run_named", &[("name", &label)]))
                    .on_click(move |_, window, cx| {
                        irori_ui::user_actions::run(id.clone(), ix, window, cx)
                    }),
            )
            .child(
                Button::new(ElementId::Name(format!("action-up-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowUp)
                    .disabled(ix == 0)
                    .tooltip(t("settings.actions.move_up"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let actions = moved_up(this.actions.clone(), ix);
                        this.editor = None;
                        this.save(actions, cx);
                    })),
            )
            .child(
                Button::new(ElementId::Name(format!("action-edit-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Pencil)
                    .tooltip(t("settings.common.edit"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_editor(Some(ix), window, cx)
                    })),
            )
            .child(
                Button::new(ElementId::Name(format!("action-remove-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .tooltip(t("settings.common.remove"))
                    .accessibility_label(t_args("settings.actions.remove", &[("name", &label)]))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut actions = this.actions.clone();
                        if ix < actions.len() {
                            actions.remove(ix);
                            this.editor = None;
                            this.save(actions, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    fn render_editor(&self, editor: &Editor, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let danger = theme.danger;
        let field = |label: SharedString, control: AnyElement| {
            v_flex()
                .gap_1()
                .min_w_0()
                .child(div().text_xs().font_medium().child(label))
                .child(control)
        };
        let is_link = UserAction {
            target: editor.target.read(cx).value().to_string(),
            ..Default::default()
        }
        .is_link();
        v_flex()
            .id("action-editor")
            .w_full()
            .gap_3()
            .p_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(field(
                t("settings.actions.target"),
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&editor.target)))
                    .child(
                        Button::new("action-pick")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.programs.choose"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_file(window, cx)),
                            ),
                    )
                    .into_any_element(),
            ))
            .when(!is_link, |col| {
                col.child(field(
                    t("settings.programs.args"),
                    Input::new(&editor.args).into_any_element(),
                ))
            })
            .child(div().max_w(rems(28.)).child(field(
                t("settings.programs.name"),
                Input::new(&editor.name).into_any_element(),
            )))
            .when(!is_link && self.prefix_possible, |col| {
                col.child(
                    Switch::new("action-in-prefix")
                        .checked(editor.in_prefix)
                        .label(t("settings.programs.in_prefix"))
                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(editor) = &mut this.editor {
                                editor.in_prefix = *on;
                                cx.notify();
                            }
                        })),
                )
            })
            .when(!is_link, |col| {
                col.child(
                    Switch::new("action-counts")
                        .checked(editor.counts_as_playing)
                        .label(t("settings.actions.counts_as_playing"))
                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(editor) = &mut this.editor {
                                editor.counts_as_playing = *on;
                                cx.notify();
                            }
                        })),
                )
                .child(muted(t("settings.actions.counts_as_playing_help"), cx).text_xs())
            })
            .when_some(editor.error.clone(), |col, e| {
                col.child(div().text_xs().text_color(danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("action-save")
                            .primary()
                            .icon(IconName::Check)
                            .label(if editor.index.is_some() {
                                t("settings.common.save")
                            } else {
                                t("settings.common.add")
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    )
                    .child(
                        Button::new("action-cancel")
                            .ghost()
                            .label(t("settings.common.cancel"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editor = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for UserActionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() && self.editor.is_some() {
            // The editor's placeholders were built in the old language.
            let index = self.editor.as_ref().and_then(|e| e.index);
            cx.defer_in(window, move |this, window, cx| {
                this.open_editor(index, window, cx)
            });
        }
        let muted_fg = cx.theme().muted_foreground;
        let actions = self.actions.clone();
        let list = panel(cx).when(actions.is_empty(), |p| {
            p.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("settings.actions.empty")),
            )
        });
        let list = actions
            .iter()
            .enumerate()
            .fold(list, |list, (ix, a)| list.child(self.render_row(ix, a, cx)));
        let editor = self.editor.as_ref().map(|e| self.render_editor(e, cx));
        v_flex()
            .id("user-actions")
            .w_full()
            .gap_2()
            .child(list)
            .children(editor)
            .when(self.editor.is_none(), |col| {
                col.child(
                    Button::new("action-add")
                        .outline()
                        .icon(IconName::Plus)
                        .label(t("settings.actions.add"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_up_swaps_with_the_one_before() {
        let named = |n: &str| UserAction {
            name: n.into(),
            target: "x".into(),
            ..Default::default()
        };
        let list = vec![named("a"), named("b"), named("c")];
        let names = |l: Vec<UserAction>| l.into_iter().map(|a| a.name).collect::<Vec<_>>();
        assert_eq!(names(moved_up(list.clone(), 2)), ["a", "c", "b"]);
        assert_eq!(names(moved_up(list.clone(), 0)), ["a", "b", "c"]);
        assert_eq!(names(moved_up(list, 9)), ["a", "b", "c"]);
    }
}
