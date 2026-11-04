//! "Programs alongside" panel of the Launch section: programs started with
//! a game (trainers, overlays, voice chat, scripts), for the game defaults
//! or one game.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::{HostPlatform, LaunchProgram, Runner};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::form::FormTarget;
use crate::i18n::LanguageStamp;
use crate::widgets::{muted, panel};

/// Settings key of the list.
pub(crate) const KEY: &str = "iroriLaunchPrograms";

/// Why an edited program can't be saved, as a translation key.
pub(crate) fn validate(path: &str, delay: &str) -> Result<u32, &'static str> {
    if path.trim().is_empty() {
        return Err("settings.programs.error.path");
    }
    let delay = delay.trim();
    if delay.is_empty() {
        return Ok(0);
    }
    match delay.parse::<u32>() {
        Ok(s) if s <= LaunchProgram::MAX_DELAY => Ok(s),
        _ => Err("settings.programs.error.delay"),
    }
}

/// The open editor: a new program (`index == None`) or one being changed.
struct Editor {
    index: Option<usize>,
    path: Entity<InputState>,
    args: Entity<InputState>,
    name: Entity<InputState>,
    delay: Entity<InputState>,
    in_prefix: bool,
    stop_with_game: bool,
    /// The switch was set by hand, so picking a file leaves it alone.
    prefix_chosen: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) struct LaunchProgramsPanel {
    target: FormTarget,
    programs: Vec<LaunchProgram>,
    /// Game target: the list is the defaults' (not overridden).
    inherited: bool,
    /// Programs can run in a Wine prefix here (not Windows, not a native
    /// game).
    prefix_possible: bool,
    steam: bool,
    editor: Option<Editor>,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl LaunchProgramsPanel {
    pub(crate) fn new(target: FormTarget, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            target,
            programs: Vec::new(),
            inherited: false,
            prefix_possible: false,
            steam: false,
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
        match &self.target {
            FormTarget::Game(id) => {
                self.programs = b.settings.game(id).launch_programs;
                self.inherited = !b.settings.overrides(id).is_overridden(KEY);
                self.steam = id.runner == Runner::Steam;
                let wine = b
                    .game(id)
                    .and_then(|g| g.installed_platform())
                    .is_none_or(|p| host.needs_wine(p));
                self.prefix_possible = host != HostPlatform::Windows && (wine || self.steam);
            }
            _ => {
                self.programs = b.settings.app().game.launch_programs.clone();
                self.inherited = false;
                self.prefix_possible = host != HostPlatform::Windows;
            }
        }
    }

    fn save(&mut self, programs: Vec<LaunchProgram>, cx: &mut Context<Self>) {
        let b = backend(cx);
        match &self.target {
            FormTarget::Game(id) => {
                b.settings
                    .set_game_value(id, KEY, serde_json::to_value(&programs).ok());
            }
            _ => b.settings.update_app(|a| a.game.launch_programs = programs),
        }
        self.read(cx);
        cx.notify();
    }

    fn use_defaults(&mut self, cx: &mut Context<Self>) {
        if let FormTarget::Game(id) = &self.target {
            backend(cx).settings.set_game_value(id, KEY, None);
        }
        self.editor = None;
        self.read(cx);
        cx.notify();
    }

    fn open_editor(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let program = index
            .and_then(|i| self.programs.get(i).cloned())
            .unwrap_or_default();
        let mut input = |text: String, placeholder: SharedString, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
            })
        };
        let path = input(
            program.path.clone(),
            t("settings.programs.path_placeholder"),
            cx,
        );
        let args = input(
            program.args.clone(),
            t("settings.programs.args_placeholder"),
            cx,
        );
        let name = input(
            program.name.clone(),
            t("settings.programs.name_placeholder"),
            cx,
        );
        let delay = input(
            if program.delay_seconds == 0 {
                String::new()
            } else {
                program.delay_seconds.to_string()
            },
            SharedString::new_static("0"),
            cx,
        );
        let mut subscriptions = Vec::new();
        for state in [&path, &args, &name, &delay] {
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
        path.focus_handle(cx).focus(window, cx);
        self.editor = Some(Editor {
            index,
            path,
            args,
            name,
            delay,
            in_prefix: program.in_prefix && self.prefix_possible,
            stop_with_game: program.stop_with_game,
            prefix_chosen: index.is_some(),
            error: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let path = editor.path.read(cx).value().to_string();
        let delay = editor.delay.read(cx).value().to_string();
        let delay_seconds = match validate(&path, &delay) {
            Ok(d) => d,
            Err(key) => {
                editor.error = Some(t(key));
                cx.notify();
                return;
            }
        };
        let mut programs = self.programs.clone();
        let enabled = editor
            .index
            .and_then(|i| programs.get(i))
            .is_none_or(|p| p.enabled);
        let program = LaunchProgram {
            name: editor.name.read(cx).value().trim().to_string(),
            path: path.trim().to_string(),
            args: editor.args.read(cx).value().trim().to_string(),
            delay_seconds,
            in_prefix: editor.in_prefix && self.prefix_possible,
            stop_with_game: editor.stop_with_game,
            enabled,
        };
        match editor.index.filter(|i| *i < programs.len()) {
            Some(i) => programs[i] = program,
            None => programs.push(program),
        }
        self.editor = None;
        self.save(programs, cx);
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
                if !editor.prefix_chosen && prefix_possible {
                    editor.in_prefix = LaunchProgram {
                        path: text.clone(),
                        ..Default::default()
                    }
                    .looks_like_windows();
                }
                editor
                    .path
                    .update(cx, |s, cx| s.set_value(text, window, cx));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_row(&self, ix: usize, program: &LaunchProgram, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let mono = theme.mono_font_family.clone();
        let label = program.label();
        let mut tags: Vec<SharedString> = vec![if program.delay_seconds == 0 {
            t("settings.programs.with_game")
        } else {
            t_args(
                "settings.programs.after",
                &[("seconds", &program.delay_seconds.to_string())],
            )
        }];
        if program.in_prefix && self.prefix_possible {
            tags.push(t("settings.programs.in_prefix_tag"));
        }
        if !program.stop_with_game {
            tags.push(t("settings.programs.keeps_running"));
        }
        let command = if program.args.is_empty() {
            program.path.clone()
        } else {
            format!("{} {}", program.path, program.args)
        };
        let editing = self.editor.as_ref().is_some_and(|e| e.index == Some(ix));
        h_flex()
            .id(ElementId::Name(format!("program-row-{ix}").into()))
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .child(
                Switch::new(ElementId::Name(format!("program-enabled-{ix}").into()))
                    .small()
                    .checked(program.enabled)
                    .accessibility_label(t_args(
                        "settings.programs.start_with_game",
                        &[("name", &label)],
                    ))
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        let mut programs = this.programs.clone();
                        if let Some(p) = programs.get_mut(ix) {
                            p.enabled = *on;
                            this.save(programs, cx);
                        }
                    })),
            )
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
                                    .when(!program.enabled || editing, |d| d.text_color(muted_fg))
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
                Button::new(ElementId::Name(format!("program-edit-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Pencil)
                    .tooltip(t("settings.common.edit"))
                    .accessibility_label(t_args("settings.programs.edit", &[("name", &label)]))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_editor(Some(ix), window, cx)
                    })),
            )
            .child(
                Button::new(ElementId::Name(format!("program-remove-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .tooltip(t("settings.common.remove"))
                    .accessibility_label(t_args("settings.programs.remove", &[("name", &label)]))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut programs = this.programs.clone();
                        if ix < programs.len() {
                            programs.remove(ix);
                            this.editor = None;
                            this.save(programs, cx);
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
        v_flex()
            .id("program-editor")
            .w_full()
            .gap_3()
            .p_3()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(field(
                t("settings.programs.path"),
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&editor.path)))
                    .child(
                        Button::new("program-pick")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.programs.choose"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_file(window, cx)),
                            ),
                    )
                    .into_any_element(),
            ))
            .child(field(
                t("settings.programs.args"),
                Input::new(&editor.args).into_any_element(),
            ))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .items_start()
                    .child(div().flex_1().min_w(rems(12.)).child(field(
                        t("settings.programs.name"),
                        Input::new(&editor.name).into_any_element(),
                    )))
                    .child(div().w_48().child(field(
                        t("settings.programs.delay"),
                        Input::new(&editor.delay).into_any_element(),
                    ))),
            )
            .when(self.prefix_possible, |col| {
                col.child(
                    Switch::new("program-in-prefix")
                        .checked(editor.in_prefix)
                        .label(t("settings.programs.in_prefix"))
                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(editor) = &mut this.editor {
                                editor.in_prefix = *on;
                                editor.prefix_chosen = true;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    muted(
                        if self.steam {
                            t("settings.programs.in_prefix_steam_help")
                        } else {
                            t("settings.programs.in_prefix_help")
                        },
                        cx,
                    )
                    .text_xs(),
                )
            })
            .child(
                Switch::new("program-stop")
                    .checked(editor.stop_with_game)
                    .label(t("settings.programs.stop_with_game"))
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.stop_with_game = *on;
                            cx.notify();
                        }
                    })),
            )
            .when_some(editor.error.clone(), |col, e| {
                col.child(div().text_xs().text_color(danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("program-save")
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
                        Button::new("program-cancel")
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

impl Render for LaunchProgramsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() && self.editor.is_some() {
            // The editor's placeholders were built in the old language.
            let index = self.editor.as_ref().and_then(|e| e.index);
            cx.defer_in(window, move |this, window, cx| {
                this.open_editor(index, window, cx)
            });
        }
        let muted_fg = cx.theme().muted_foreground;
        let programs = self.programs.clone();
        let list = panel(cx).when(programs.is_empty(), |p| {
            p.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("settings.programs.empty")),
            )
        });
        let list = programs
            .iter()
            .enumerate()
            .fold(list, |list, (ix, p)| list.child(self.render_row(ix, p, cx)));
        let editor = self.editor.as_ref().map(|e| self.render_editor(e, cx));
        let overridden = matches!(self.target, FormTarget::Game(_)) && !self.inherited;
        v_flex()
            .id("launch-programs")
            .w_full()
            .gap_2()
            .when(self.inherited && !programs.is_empty(), |col| {
                col.child(muted(t("settings.programs.inherited"), cx).text_xs())
            })
            .child(list)
            .children(editor)
            .when(self.editor.is_none(), |col| {
                col.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("program-add")
                                .outline()
                                .icon(IconName::Plus)
                                .label(t("settings.programs.add"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_editor(None, window, cx)
                                })),
                        )
                        .when(overridden, |row| {
                            row.child(
                                Button::new("program-use-defaults")
                                    .ghost()
                                    .icon(IconName::Undo2)
                                    .label(t("settings.programs.use_defaults"))
                                    .on_click(cx.listener(|this, _, _, cx| this.use_defaults(cx))),
                            )
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_program_and_its_delay() {
        assert_eq!(validate(" ", ""), Err("settings.programs.error.path"));
        assert_eq!(validate("/bin/x", ""), Ok(0));
        assert_eq!(validate("/bin/x", " 15 "), Ok(15));
        assert_eq!(
            validate("/bin/x", "-1"),
            Err("settings.programs.error.delay")
        );
        assert_eq!(
            validate("/bin/x", "601"),
            Err("settings.programs.error.delay")
        );
        assert_eq!(
            validate("/bin/x", "soon"),
            Err("settings.programs.error.delay")
        );
    }
}
