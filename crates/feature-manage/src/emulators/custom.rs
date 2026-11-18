//! "Add emulator": an emulator of the user's own, for one Irori doesn't
//! know or a build it doesn't offer. The user names it, picks its program,
//! types the arguments it takes (`{rom}` for the game) and ticks the systems
//! it plays; it then shows on the Emulators page, in each system's default
//! picker and in "Play with…" (see [`irori_engines::custom`]).

use std::collections::BTreeSet;
use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    div, rems,
};
use irori_engines::custom::{CustomEngine, PLACEHOLDERS};
use irori_engines::systems;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::common::{notify_error, notify_success};

/// Open the dialog: a new emulator, or `existing` (its id and what it is) to
/// change.
pub fn open_custom_engine_dialog(
    existing: Option<(String, CustomEngine)>,
    window: &mut Window,
    cx: &mut App,
) {
    crate::i18n::register();
    let title = if existing.is_some() {
        t("manage.emulators.custom.edit_title")
    } else {
        t("manage.emulators.custom.add_title")
    };
    let dialog = cx.new(|cx| CustomEngineDialog::new(existing, window, cx));
    let width = window.rem_size() * 36.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(title.clone()).width(width).child(dialog.clone())
    });
}

/// Ask before forgetting the emulator `id` the user added.
pub fn confirm_remove_custom_engine(id: String, name: String, window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (id, name) = (id.clone(), name.clone());
        alert
            .confirm()
            .title(t_args(
                "manage.emulators.custom.remove_title",
                &[("engine", &name)],
            ))
            .description(t("manage.emulators.custom.remove_message"))
            .ok_text(t("manage.emulators.custom.remove_confirm"))
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                let (b, engine, name) = (backend(cx), id.clone(), name.clone());
                let task = irori_ui::run(cx, async move { b.remove_custom_engine(&engine).await });
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        let _ = cx.update(|window, cx| {
                            if let Err(e) = result {
                                notify_error(
                                    t_args(
                                        "manage.emulators.custom.remove_failed",
                                        &[("engine", &name)],
                                    ),
                                    &e,
                                    window,
                                    cx,
                                );
                            }
                        });
                    })
                    .detach();
                true
            })
    });
}

struct CustomEngineDialog {
    id: Option<String>,
    name: Entity<InputState>,
    program: Entity<InputState>,
    args: Entity<InputState>,
    systems: BTreeSet<String>,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl CustomEngineDialog {
    fn new(
        existing: Option<(String, CustomEngine)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (id, def) = match existing {
            Some((id, def)) => (Some(id), def),
            None => (
                None,
                CustomEngine {
                    args: "{rom}".into(),
                    ..Default::default()
                },
            ),
        };
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("manage.emulators.custom.name_placeholder"))
                .default_value(def.name.clone())
        });
        let program = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("manage.emulators.custom.program_placeholder"))
                .default_value(def.program.display().to_string())
        });
        let args = cx.new(|cx| InputState::new(window, cx).default_value(def.args.clone()));
        let repaint = |cx: &mut Context<Self>, input: &Entity<InputState>| {
            cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
        };
        let subscriptions = vec![repaint(cx, &name), repaint(cx, &program)];
        Self {
            id,
            name,
            program,
            args,
            systems: def.systems.into_iter().collect(),
            saving: false,
            _subscriptions: subscriptions,
        }
    }

    fn def(&self, cx: &App) -> CustomEngine {
        CustomEngine {
            name: self.name.read(cx).value().trim().to_string(),
            program: PathBuf::from(self.program.read(cx).value().trim()),
            args: self.args.read(cx).value().to_string(),
            systems: self.systems.iter().cloned().collect(),
        }
    }

    fn pick_program(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            // A macOS app is a folder.
            directories: cfg!(target_os = "macos"),
            multiple: false,
            prompt: Some(t("manage.emulators.custom.choose_program")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.program.update(cx, |s, cx| {
                    s.set_value(path.display().to_string(), window, cx)
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let def = self.def(cx);
        if let Err(e) = irori_engines::custom::parse_args(&def.args) {
            notify_error(t("manage.emulators.custom.save_failed"), &e, window, cx);
            return;
        }
        self.saving = true;
        cx.notify();
        let (b, id, name) = (backend(cx), self.id.clone(), def.name.clone());
        let task = irori_ui::run(cx, async move { b.save_custom_engine(id, def).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => {
                        window.close_dialog(cx);
                        notify_success(
                            t_args("manage.emulators.custom.saved", &[("engine", &name)]),
                            window,
                            cx,
                        );
                    }
                    Err(e) => {
                        notify_error(t("manage.emulators.custom.save_failed"), &e, window, cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn field(label: SharedString, body: impl IntoElement) -> AnyElement {
        v_flex()
            .gap_1()
            .child(div().text_sm().font_medium().child(label))
            .child(body)
            .into_any_element()
    }
}

impl Render for CustomEngineDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let def = self.def(cx);
        let ready = !def.name.is_empty()
            && !def.program.as_os_str().is_empty()
            && !def.systems.is_empty()
            && !self.saving;
        // `{rom}` is named on its own in the help.
        let placeholders = PLACEHOLDERS
            .iter()
            .filter(|p| **p != "rom")
            .map(|p| format!("{{{p}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        let boxes: Vec<AnyElement> = systems::all()
            .iter()
            .map(|s| {
                let id = s.id.clone();
                Checkbox::new(SharedString::from(format!("custom-system-{}", s.id)))
                    .label(s.name.clone())
                    .checked(self.systems.contains(&s.id))
                    .disabled(self.saving)
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.systems.insert(id.clone());
                        } else {
                            this.systems.remove(&id);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.emulators.custom.description")),
            )
            .child(Self::field(
                t("manage.emulators.custom.name"),
                Input::new(&self.name).disabled(self.saving),
            ))
            .child(Self::field(
                t("manage.emulators.custom.program"),
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.program).disabled(self.saving)),
                    )
                    .child(
                        Button::new("custom-choose-program")
                            .icon(IconName::FolderOpen)
                            .label(t("manage.emulators.custom.choose"))
                            .disabled(self.saving)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_program(window, cx)),
                            ),
                    ),
            ))
            .child(Self::field(
                t("manage.emulators.custom.args"),
                v_flex()
                    .gap_1()
                    .child(Input::new(&self.args).disabled(self.saving))
                    .child(div().text_xs().text_color(muted).child(t_args(
                        "manage.emulators.custom.args_help",
                        &[("rom", "{rom}"), ("placeholders", &placeholders)],
                    ))),
            ))
            .child(Self::field(
                t("manage.emulators.custom.systems"),
                // One scroll owner for the list, so the dialog fits small windows.
                div()
                    .id("custom-systems")
                    .max_h(rems(14.))
                    .overflow_y_scrollbar()
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_x_4()
                            .gap_y_2()
                            .children(boxes),
                    ),
            ))
            .child(
                DialogFooter::new()
                    .child(
                        Button::new("custom-cancel")
                            .ghost()
                            .label(t("manage.emulators.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("custom-save")
                            .primary()
                            .label(t("manage.emulators.custom.save"))
                            .loading(self.saving)
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
