//! The theme editor: a sheet at the right of Home that changes a draft of
//! the theme and layout while Home behind it shows the result. Up and Down
//! pick a row, Left and Right (or a click) change its value, Confirm runs
//! the row's command, and Back closes the sheet and keeps the draft: over a
//! theme of the user's own, or as a new theme when the draft started from
//! the gallery.
//!
//! Choosing a picture, importing and exporting go through the system's file
//! dialogs; deleting a theme asks first.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, App, AppContext as _, Context, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, rems,
};
use irori_ui::i18n::{t, t_args};

use super::home::SCROLL_SPRING;
use super::{BigPictureView, Colors, Overlay, Tab};
use crate::gamepad::GamepadInput;
use crate::layout::{HomeLayout, Metrics, list_scroll};
use crate::theme::store::{self, ThemeRef};
use crate::theme::{
    Background, MAX_NAME, MAX_PICTURE_BYTES, Picture, Rgb, ShelfKind, Theme, ThemeError, gallery,
};

/// Width of the editor sheet, in rem.
const SHEET_W: f32 = 30.0;
/// Height of a row, in rem.
const ROW_H: f32 = 3.0;
/// Height of the sheet's title, in rem.
const TITLE_H: f32 = 4.5;

mod draft;

pub(super) use draft::EditorRow;
use draft::{change_draft, step};

/// The editor's state: the draft, where it came from, and the focused row.
pub(super) struct Editor {
    pub draft: Theme,
    pub layout: HomeLayout,
    /// The theme the draft started from, and that theme as it was.
    pub source: ThemeRef,
    pub original: Theme,
    pub ix: usize,
    /// The last picture chosen, kept while another background is tried.
    pub picture: Option<Picture>,
    /// The name field, while it has the keyboard.
    pub naming: bool,
    pub name: gpui_kit::Entity<InputState>,
}

impl Editor {
    /// The rows, Delete only for the user's own themes.
    pub fn rows(&self) -> Vec<EditorRow> {
        let mut rows = vec![
            EditorRow::Theme,
            EditorRow::Layout,
            EditorRow::Accent,
            EditorRow::BackgroundColor,
            EditorRow::TextColor,
            EditorRow::Backdrop,
            EditorRow::ChoosePicture,
            EditorRow::Font,
            EditorRow::Shape,
            EditorRow::Size,
            EditorRow::Corners,
        ];
        rows.extend(ShelfKind::ALL.map(EditorRow::Shelf));
        rows.extend([EditorRow::Name, EditorRow::Import, EditorRow::Export]);
        if matches!(self.source, ThemeRef::User(_)) {
            rows.push(EditorRow::Delete);
        }
        rows
    }

    /// Start over from `theme`, `source` of it.
    fn load(&mut self, source: ThemeRef, theme: Theme) {
        if let Some(layout) = theme.layout {
            self.layout = layout;
        }
        self.picture = match &theme.background {
            Background::Picture(p) => Some(p.clone()),
            _ => None,
        };
        self.source = source;
        self.original = theme.clone();
        self.draft = theme;
    }

    /// Change the value of `row` one step. Returns whether it changed.
    pub fn change(&mut self, row: EditorRow, forward: bool) -> bool {
        change_draft(
            &mut self.draft,
            &mut self.layout,
            self.picture.as_ref(),
            row,
            forward,
        )
    }

    /// The text of `row`'s value.
    fn value(&self, row: EditorRow) -> Option<SharedString> {
        let theme = &self.draft;
        let color = |c: Option<Rgb>| match c {
            Some(c) => SharedString::from(c.hex()),
            None => t("bigpicture.editor.color_default"),
        };
        Some(match row {
            EditorRow::Theme => theme.name.clone().into(),
            EditorRow::Layout => self.layout.label(),
            EditorRow::Accent => color(theme.palette.accent),
            EditorRow::BackgroundColor => color(theme.palette.background),
            EditorRow::TextColor => match theme.palette.foreground {
                Some(c) => c.hex().into(),
                None => t("bigpicture.editor.color_automatic"),
            },
            EditorRow::Backdrop => theme.background.label(),
            EditorRow::Font => theme.font.label(),
            EditorRow::Shape => theme.covers.shape.label(),
            EditorRow::Size => theme.covers.size.label(),
            EditorRow::Corners => theme.covers.corners.label(),
            EditorRow::Shelf(kind) => {
                if theme.shelves.contains(&kind) {
                    t("bigpicture.toggle.on")
                } else {
                    t("bigpicture.toggle.off")
                }
            }
            EditorRow::Name => theme.name.clone().into(),
            _ => return None,
        })
    }

    /// The colour a row shows as a swatch.
    fn swatch(&self, row: EditorRow) -> Option<Rgb> {
        let p = &self.draft.palette;
        match row {
            EditorRow::Accent => p.accent,
            EditorRow::BackgroundColor => p.background,
            EditorRow::TextColor => p.foreground,
            _ => None,
        }
    }
}

/// Run blocking file work on the backend's blocking pool.
fn blocking<T: Send + 'static>(
    cx: &App,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Task<Result<T>> {
    let handle = irori_ui::backend(cx).spawn_blocking(f);
    cx.background_executor().spawn(async move {
        handle
            .await
            .map_err(|e| anyhow::anyhow!("task failed: {e}"))?
    })
}

fn notify_error(window: &mut Window, title: SharedString, e: &anyhow::Error, cx: &mut App) {
    let text = match e.downcast_ref::<ThemeError>() {
        Some(theme) => theme.message().to_string(),
        None => format!("{e:#}"),
    };
    window.push_notification(Notification::error(text).title(title), cx);
}

impl BigPictureView {
    /// Read the user's themes from disk, then use the one the settings name.
    pub(super) fn load_user_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = blocking(cx, || Ok(store::load(&store::themes_dir())));
        cx.spawn_in(window, async move |this, cx| {
            let themes = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.look.user = themes;
                this.look.loaded = true;
                this.look.resolve();
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the editor over Home, starting from the theme in use.
    pub(super) fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.look.theme.clone();
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("bigpicture.editor.name")));
        let subscription = cx.subscribe_in(
            &name,
            window,
            |this, input, event: &gpui_kit::component::input::InputEvent, window, cx| {
                use gpui_kit::component::input::InputEvent;
                match event {
                    InputEvent::Change => {
                        let text: String = input
                            .read(cx)
                            .value()
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(MAX_NAME)
                            .collect();
                        if let Some(editor) = &mut this.look.editor
                            && !text.trim().is_empty()
                        {
                            editor.draft.name = text.trim().to_owned();
                        }
                    }
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.stop_naming(window, cx)
                    }
                    _ => {}
                }
                cx.notify();
            },
        );
        self._editor_subscription = Some(subscription);
        self.look.editor = Some(Editor {
            layout: self.look.layout,
            source: self.look.active.clone(),
            picture: match &theme.background {
                Background::Picture(p) => Some(p.clone()),
                _ => None,
            },
            original: theme.clone(),
            draft: theme,
            ix: 0,
            naming: false,
            name,
        });
        self.overlay = None;
        self.page = None;
        self.set_tab(Tab::Home, cx);
        cx.notify();
    }

    fn stop_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.look.editor
            && editor.naming
        {
            editor.naming = false;
            self.focus.focus(window, cx);
        }
    }

    pub(super) fn editor_input(
        &mut self,
        input: GamepadInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.look.editor else {
            return;
        };
        let rows = editor.rows();
        let last = rows.len().saturating_sub(1);
        let ix = editor.ix.min(last);
        match input {
            GamepadInput::Up => editor.ix = ix.saturating_sub(1),
            GamepadInput::Down => editor.ix = (ix + 1).min(last),
            GamepadInput::Left | GamepadInput::Right => {
                let forward = input == GamepadInput::Right;
                self.change_editor_row(rows[ix], forward, cx);
            }
            GamepadInput::Confirm | GamepadInput::Alt => {
                self.activate_editor_row(rows[ix], window, cx)
            }
            GamepadInput::Back | GamepadInput::Menu => self.close_editor(cx),
            _ => {}
        }
        cx.notify();
    }

    fn change_editor_row(&mut self, row: EditorRow, forward: bool, cx: &mut Context<Self>) {
        if row == EditorRow::Theme {
            let choices = self.look.choices();
            let Some(editor) = &self.look.editor else {
                return;
            };
            let next = step(
                &(0..choices.len()).collect::<Vec<_>>(),
                choices
                    .iter()
                    .position(|c| *c == editor.source)
                    .unwrap_or(0),
                forward,
            );
            let source = choices[next].clone();
            if let Some(theme) = self.look.find(&source)
                && let Some(editor) = &mut self.look.editor
            {
                editor.load(source, theme);
            }
        } else if let Some(editor) = &mut self.look.editor {
            editor.change(row, forward);
        }
        // Shelves may have changed.
        self.refresh(cx);
    }

    fn activate_editor_row(&mut self, row: EditorRow, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            EditorRow::ChoosePicture => self.choose_picture(window, cx),
            EditorRow::Name => {
                if let Some(editor) = &mut self.look.editor {
                    editor.naming = true;
                    let name = editor.draft.name.clone();
                    editor.name.update(cx, |input, cx| {
                        input.set_value(name, window, cx);
                        input.focus(window, cx);
                    });
                }
            }
            EditorRow::Import => self.import_theme(window, cx),
            EditorRow::Export => self.export_theme(window, cx),
            EditorRow::Delete => {
                if let Some(Editor {
                    source: ThemeRef::User(id),
                    ..
                }) = &self.look.editor
                {
                    self.overlay = Some(Overlay::DeleteTheme {
                        id: id.clone(),
                        choice: 0,
                    });
                }
            }
            row => self.change_editor_row(row, true, cx),
        }
        cx.notify();
    }

    /// Close the editor and keep what changed (see the module docs).
    pub(super) fn close_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.look.editor.take() else {
            return;
        };
        self._editor_subscription = None;
        let mut draft = editor.draft;
        let theme_changed = draft != editor.original;
        let mut active = editor.source.clone();
        if theme_changed {
            draft.layout = Some(editor.layout);
            let id = match &editor.source {
                ThemeRef::User(id) => id.clone(),
                ThemeRef::BuiltIn(_) => {
                    if draft.name == editor.original.name {
                        let name = t_args(
                            "bigpicture.theme.copy_name",
                            &[("name", draft.name.as_str())],
                        );
                        draft.name = name.chars().take(MAX_NAME).collect();
                    }
                    store::new_id(&draft.name, self.look.user.iter().map(|u| u.id.as_str()))
                }
            };
            match self.look.user.iter_mut().find(|u| u.id == id) {
                Some(user) => user.theme = draft.clone(),
                None => self.look.user.push(store::UserTheme {
                    id: id.clone(),
                    theme: draft.clone(),
                }),
            }
            let (save_id, save_theme) = (id.clone(), draft.clone());
            let task = blocking(cx, move || {
                store::save(&store::themes_dir(), &save_id, &save_theme)
            });
            cx.spawn(async move |_, cx| {
                if let Err(e) = task.await {
                    cx.update(|cx| {
                        if let Some(window) = cx.active_window() {
                            let _ = window.update(cx, |_, window, cx| {
                                notify_error(window, t("bigpicture.theme.error.save"), &e, cx)
                            });
                        }
                    });
                }
            })
            .detach();
            active = ThemeRef::User(id);
        }
        self.look.active = active.clone();
        self.look.layout = editor.layout;
        self.look.resolve();
        let layout = editor.layout.id().to_owned();
        let setting = active.setting();
        let b = irori_ui::backend(cx);
        irori_ui::state::run_infallible(cx, async move {
            b.settings.update_app(|app| {
                app.ui.big_picture_layout = layout;
                app.ui.big_picture_theme = setting;
            })
        })
        .detach();
        self.refresh(cx);
        cx.notify();
    }

    /// The quick menu's layout entry: the next layout, saved at once.
    pub(super) fn next_layout(&mut self, cx: &mut Context<Self>) {
        let next = step(&HomeLayout::ALL, self.look.layout, true);
        self.look.layout = next;
        let b = irori_ui::backend(cx);
        irori_ui::state::run_infallible(cx, async move {
            b.settings
                .update_app(|app| app.ui.big_picture_layout = next.id().to_owned())
        })
        .detach();
        cx.notify();
    }

    fn choose_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("bigpicture.editor.choose_picture")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let task = cx.update(|_, cx| blocking(cx, move || read_picture(path)));
            let Ok(task) = task else { return };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(picture) => {
                        if let Some(editor) = &mut this.look.editor {
                            editor.picture = Some(picture.clone());
                            editor.draft.background = Background::Picture(picture);
                        }
                    }
                    Err(e) => notify_error(window, t("bigpicture.editor.picture_failed"), &e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("bigpicture.editor.import")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let task =
                cx.update(|_, cx| blocking(cx, move || store::import(&store::themes_dir(), &path)));
            let Ok(task) = task else { return };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(user) => {
                        let name = user.theme.name.clone();
                        let source = ThemeRef::User(user.id.clone());
                        this.look.user.retain(|u| u.id != user.id);
                        this.look.user.push(user.clone());
                        if let Some(editor) = &mut this.look.editor {
                            editor.load(source, user.theme);
                            editor.ix = 0;
                        }
                        window.push_notification(
                            Notification::success(t_args(
                                "bigpicture.editor.imported",
                                &[("name", name.as_str())],
                            )),
                            cx,
                        );
                        this.refresh(cx);
                    }
                    Err(e) => notify_error(window, t("bigpicture.editor.import_failed"), &e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.look.editor else {
            return;
        };
        let mut theme = editor.draft.clone();
        theme.layout = Some(editor.layout);
        let dir = dirs_home();
        let rx = cx.prompt_for_new_path(&dir, Some(&store::export_name(&theme)));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let task = cx.update(|_, cx| blocking(cx, move || store::export(&theme, &path)));
            let Ok(task) = task else { return };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(path) => {
                    let file = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    window.push_notification(
                        Notification::success(t_args(
                            "bigpicture.editor.exported",
                            &[("file", file.as_str())],
                        )),
                        cx,
                    );
                }
                Err(e) => notify_error(window, t("bigpicture.editor.export_failed"), &e, cx),
            });
        })
        .detach();
    }

    /// Delete the user's theme `id` (asked first, see `Overlay::DeleteTheme`)
    /// and go back to Irori's own.
    pub(super) fn delete_theme(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.look.user.retain(|u| u.id != id);
        let default = ThemeRef::BuiltIn(gallery::DEFAULT_ID.into());
        if let Some(editor) = &mut self.look.editor {
            editor.load(default.clone(), gallery::all()[0].theme.clone());
            editor.ix = 0;
        }
        if self.look.active == ThemeRef::User(id.clone()) {
            self.look.active = default.clone();
            self.look.resolve();
            let b = irori_ui::backend(cx);
            irori_ui::state::run_infallible(cx, async move {
                b.settings
                    .update_app(|app| app.ui.big_picture_theme = default.setting())
            })
            .detach();
        }
        let task = blocking(cx, move || store::delete(&store::themes_dir(), &id));
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    notify_error(window, t("bigpicture.theme.error.delete"), &e, cx)
                });
            }
        })
        .detach();
        self.refresh(cx);
        cx.notify();
    }

    pub(super) fn render_editor(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.look.editor.as_ref()?;
        let rows = editor.rows();
        let ix = editor.ix.min(rows.len().saturating_sub(1));
        let width = SHEET_W.min(m.width * 0.55);
        let list_h = m.height - m.top_bar - TITLE_H - 1.5;
        let scroll = list_scroll(ix, None, ROW_H, 0.0, list_h);
        let focused_overlay = self.overlay.is_none();
        let items: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| self.editor_row(editor, *row, i, i == ix && focused_overlay, c, cx))
            .collect();
        let naming = editor.naming.then(|| editor.name.clone());
        Some(
            v_flex()
                .id("bp-editor")
                .role(Role::Dialog)
                .aria_label(t("bigpicture.editor.title"))
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(rems(width))
                .px_5()
                .pt(rems(m.top_bar))
                .bg(c.popover.opacity(0.94))
                .border_l_1()
                .border_color(c.border)
                .shadow_lg()
                .child(
                    v_flex()
                        .h(rems(TITLE_H))
                        .justify_center()
                        .gap_1()
                        .child(
                            div()
                                .text_2xl()
                                .font_extrabold()
                                .child(t("bigpicture.editor.title")),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(c.muted_foreground)
                                .child(t("bigpicture.editor.subtitle")),
                        ),
                )
                .child(
                    div()
                        .id("bp-editor-rows")
                        .role(Role::Menu)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .relative()
                        .overflow_hidden()
                        .child(
                            v_flex()
                                .absolute()
                                .left_0()
                                .right_0()
                                .children(items)
                                .with_spring(
                                    "bp-editor-scroll",
                                    SpringAnimation::new(SCROLL_SPRING).to(scroll),
                                    |el, v: f32| el.top(rems(-v)),
                                ),
                        ),
                )
                .when_some(naming, |d, input| {
                    d.child(
                        div()
                            .key_context(crate::SEARCH_CONTEXT)
                            .absolute()
                            .left_5()
                            .right_5()
                            .top(rems(m.top_bar + TITLE_H))
                            .on_action(cx.listener(|this, _: &crate::CloseSearch, window, cx| {
                                this.stop_naming(window, cx);
                                cx.notify();
                            }))
                            .child(Input::new(&input).large()),
                    )
                })
                .with_animation(
                    "bp-editor-in",
                    gpui_kit::Animation::new(irori_ui::motion::STANDARD)
                        .with_easing(gpui_kit::ease_out_quint()),
                    move |el, t| el.right(rems(-width * (1.0 - t))),
                )
                .into_any_element(),
        )
    }

    fn editor_row(
        &self,
        editor: &Editor,
        row: EditorRow,
        i: usize,
        focused: bool,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = row.label();
        let value = editor.value(row);
        let swatch = editor.swatch(row);
        let danger = row == EditorRow::Delete;
        let (fg, muted) = if focused {
            (c.background, c.background.opacity(0.8))
        } else {
            (
                if danger { c.danger } else { c.foreground },
                c.muted_foreground,
            )
        };
        let aria = match &value {
            Some(v) => SharedString::from(format!("{label}: {v}")),
            None => label.clone(),
        };
        let arrow = |id: &'static str, icon: IconName, forward: bool| {
            div()
                .id(SharedString::from(format!("{}-{id}", row.id())))
                .flex_shrink_0()
                .cursor_pointer()
                .child(Icon::new(icon).small())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(editor) = &mut this.look.editor {
                        editor.ix = i;
                    }
                    this.change_editor_row(row, forward, cx);
                    this.focus.focus(window, cx);
                    cx.notify();
                }))
        };
        h_flex()
            .id(row.id())
            .role(Role::MenuItem)
            .aria_label(aria)
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .w_full()
            .h(rems(ROW_H))
            .px_3()
            .gap_3()
            .items_center()
            .rounded(c.radius_lg)
            .cursor_pointer()
            .text_base()
            .text_color(fg)
            .when(focused, |d| {
                d.bg(if danger { c.danger } else { c.foreground })
            })
            .when_some(row.icon(), |d, icon| d.child(Icon::new(icon)))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when_some(value, |d, value| {
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .max_w(rems(14.))
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .text_color(muted)
                        .when(row.has_value(), |d| {
                            d.child(arrow("prev", IconName::ChevronLeft, false))
                        })
                        .when_some(swatch, |d, rgb| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .size(rems(1.1))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(fg.opacity(0.5))
                                    .bg(rgb.hsla()),
                            )
                        })
                        .child(div().min_w_0().truncate().child(value))
                        .when(row.has_value(), |d| {
                            d.child(arrow("next", IconName::ChevronRight, true))
                        }),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(editor) = &mut this.look.editor {
                    editor.ix = i;
                }
                this.activate_editor_row(row, window, cx);
                if !this.look.editor.as_ref().is_some_and(|e| e.naming) {
                    this.focus.focus(window, cx);
                }
            }))
            .into_any_element()
    }
}

/// Where the export dialog starts.
fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Read a picture the user chose, refusing a file too large before reading
/// it.
fn read_picture(path: PathBuf) -> Result<Picture> {
    let size = std::fs::metadata(&path)
        .with_context(|| t("bigpicture.theme.error.read"))?
        .len();
    if size > MAX_PICTURE_BYTES as u64 {
        return Err(ThemeError::PictureTooLarge.into());
    }
    let bytes = std::fs::read(&path).with_context(|| t("bigpicture.theme.error.read"))?;
    Ok(Picture::from_bytes(bytes)?)
}
