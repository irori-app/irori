//! The "Save view" dialog and the view manager (rename, reorder, delete with
//! an undo).

use std::cell::Cell;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div,
};
use irori_core::settings::LibraryLayout;
use irori_ui::i18n::{t, t_args};

use super::{dialog_width, text};
use crate::filters_panel::active_filters;
use crate::views::{self, ViewsModel};

/// Delete saved view `id` and push an undo notification that restores it at
/// its previous position.
pub(crate) fn delete_view_with_undo(id: u64, window: &mut Window, cx: &mut App) {
    let model = views::saved_views(cx);
    let Some((at, removed)) = model.update(cx, |m, cx| m.update(|v| v.remove(id), cx)) else {
        return;
    };
    let name = removed.label();
    let model = model.downgrade();
    window.push_notification(
        Notification::info(t_args("library.views.deleted", &[("name", &name)]))
            .action(move |_, _, cx| {
                let note = cx.entity().downgrade();
                let (model, removed) = (model.clone(), removed.clone());
                Button::new("undo-delete-view")
                    .label(t("library.bulk.undo"))
                    .small()
                    .on_click(move |_, window, cx| {
                        let removed = removed.clone();
                        let _ = model.update(cx, |m, cx| m.update(|v| v.restore(at, removed), cx));
                        let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                    })
            })
            .autohide(true),
        cx,
    );
}

fn layout_label(layout: LibraryLayout) -> SharedString {
    match layout {
        LibraryLayout::Grid => t("library.toolbar.grid"),
        LibraryLayout::List => t("library.toolbar.list"),
        LibraryLayout::Focus => t("library.toolbar.focus"),
    }
}

/// "Save view": name the library's current filters, grouping, sort and layout.
pub struct SaveViewDialog {
    name: Entity<InputState>,
    layout: LibraryLayout,
    save_layout: bool,
    _subscriptions: Vec<Subscription>,
}

impl SaveViewDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("library.views.name_placeholder")));
        let layout = crate::view::remove::active_library(cx)
            .map(|v| v.read(cx).current_layout())
            .unwrap_or_else(|| crate::library_layout(cx));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.save(window, cx),
                _ => {}
            }),
        ];
        Self {
            name,
            layout,
            save_layout: true,
            _subscriptions: subscriptions,
        }
    }

    fn result(&self, cx: &mut App) -> Result<String, views::NameError> {
        let raw = text(&self.name, cx);
        views::saved_views(cx)
            .read(cx)
            .views()
            .validate_name(&raw, None)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(name) = self.result(cx) else { return };
        let filter = active_filters(cx).map(|f| f.read(cx).filter().clone());
        let Some(filter) = filter else { return };
        let pinned_layout = self.save_layout.then_some(self.layout);
        let model = views::saved_views(cx);
        let saved = model.update(cx, |m, cx| {
            m.update(|v| v.add(&name, &filter, pinned_layout), cx)
        });
        window.close_dialog(cx);
        if saved.is_ok() {
            window.push_notification(
                Notification::success(t_args("library.views.saved", &[("name", &name)])),
                cx,
            );
        }
    }
}

impl Render for SaveViewDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let result = self.result(cx);
        let error = match &result {
            Err(views::NameError::Exists) => Some(views::NameError::Exists.message()),
            _ => None,
        };
        let mut summary = active_filters(cx)
            .map(|f| {
                f.read(cx)
                    .filter()
                    .describe(&irori_ui::models(cx).library.read(cx).user)
            })
            .unwrap_or_default();
        let layout_name = layout_label(self.layout);
        if self.save_layout {
            summary.push(
                t_args("library.views.summary_layout", &[("layout", &layout_name)]).to_string(),
            );
        }
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.views.save_description")),
            )
            .child(Input::new(&self.name))
            .when_some(error, |d, e| {
                d.child(div().text_xs().text_color(cx.theme().danger).child(e))
            })
            .child(
                Checkbox::new("save-view-layout")
                    .label(t_args(
                        "library.views.save_layout",
                        &[("layout", &layout_name)],
                    ))
                    .checked(self.save_layout)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.save_layout = *checked;
                        cx.notify();
                    })),
            )
            .when(!summary.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(summary.join(&t("library.filter.list_separator"))),
                )
            })
            .child(
                DialogFooter::new()
                    .child(
                        Button::new("save-view-cancel")
                            .ghost()
                            .label(t("library.common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("save-view-confirm")
                            .primary()
                            .label(t("library.views.save_confirm"))
                            .disabled(result.is_err())
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// Open "Save view" for the library's current criteria.
pub fn open_save_view_dialog(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| SaveViewDialog::new(window, cx));
    let width = dialog_width(window, 26.);
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let input = view.read(cx).name.clone();
            input.focus_handle(cx).focus(window, cx);
        }
        dialog
            .title(t("library.views.save_title"))
            .w(width)
            .child(view.clone())
    });
}

/// The view manager.
pub struct ManageViewsDialog {
    model: Entity<ViewsModel>,
    editing: Option<(u64, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
    _editing_subscription: Option<Subscription>,
}

impl ManageViewsDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let model = views::saved_views(cx);
        Self {
            _subscriptions: vec![cx.observe(&model, |_, _, cx| cx.notify())],
            model,
            editing: None,
            _editing_subscription: None,
        }
    }

    fn start_rename(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .model
            .read(cx)
            .views()
            .get(id)
            .map(|v| v.label())
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |s, cx| s.focus(window, cx));
        self._editing_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.commit_rename(window, cx),
                    _ => {}
                }),
            );
        self.editing = Some((id, input));
        cx.notify();
    }

    fn commit_rename(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some((id, input)) = self.editing.clone() else {
            return;
        };
        let name = text(&input, cx);
        let renamed = self
            .model
            .update(cx, |m, cx| m.update(|v| v.rename(id, &name), cx));
        if renamed.is_ok() {
            self.editing = None;
            self._editing_subscription = None;
        }
        cx.notify();
    }

    fn shift(&mut self, id: u64, by: isize, cx: &mut Context<Self>) {
        self.model
            .update(cx, |m, cx| m.update(|v| v.shift(id, by), cx));
    }

    /// Delete a view, with an undo in the notification instead of a
    /// confirmation.
    fn delete(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        delete_view_with_undo(id, window, cx);
    }

    /// Open the smart category editor with the rules of view `id`, and say
    /// which of its criteria no rule can hold.
    fn make_smart(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.model.read(cx).views().get(id).cloned() else {
            return;
        };
        let (draft, rest) = crate::categories::smart_from_view(&view.label(), &view.filter);
        let left = rest.describe(&irori_ui::models(cx).library.read(cx).user);
        window.close_dialog(cx);
        crate::dialogs::smart::open_smart_category_editor(None, Some(draft), window, cx);
        if !left.is_empty() {
            window.push_notification(
                Notification::info(t_args(
                    "library.views.smart_left_out",
                    &[("criteria", &left.join(" · "))],
                )),
                cx,
            );
        }
    }

    fn render_row(
        &self,
        ix: usize,
        count: usize,
        id: u64,
        name: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let base = h_flex()
            .id(SharedString::from(format!("view-{id}")))
            .w_full()
            .gap_2()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border);
        if let Some((_, input)) = self.editing.as_ref().filter(|(e, _)| *e == id) {
            let result = self
                .model
                .read(cx)
                .views()
                .validate_name(&text(input, cx), Some(id));
            let error = result.as_ref().err().map(|e| e.message());
            return base.child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                            .child(
                                Button::new("rename-view-save")
                                    .small()
                                    .primary()
                                    .label(t("library.views.rename"))
                                    .disabled(result.is_err())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.commit_rename(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("rename-view-cancel")
                                    .small()
                                    .ghost()
                                    .label(t("library.common.cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editing = None;
                                        this._editing_subscription = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(error, |d, e| {
                        d.child(div().text_xs().text_color(cx.theme().danger).child(e))
                    }),
            );
        }
        base.justify_between()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_medium()
                    .truncate()
                    .child(name.clone()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        Button::new("view-up")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronUp)
                            .tooltip(t_args("library.views.move_up", &[("name", &name)]))
                            .disabled(ix == 0)
                            .on_click(cx.listener(move |this, _, _, cx| this.shift(id, -1, cx))),
                    )
                    .child(
                        Button::new("view-down")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .tooltip(t_args("library.views.move_down", &[("name", &name)]))
                            .disabled(ix + 1 == count)
                            .on_click(cx.listener(move |this, _, _, cx| this.shift(id, 1, cx))),
                    )
                    .child(
                        Button::new("view-smart")
                            .small()
                            .ghost()
                            .icon(IconName::Sparkles)
                            .tooltip(t_args("library.views.make_smart", &[("name", &name)]))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.make_smart(id, window, cx)
                            })),
                    )
                    .child(
                        Button::new("view-rename")
                            .small()
                            .ghost()
                            .icon(IconName::Pencil)
                            .tooltip(t("library.views.rename"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_rename(id, window, cx)
                            })),
                    )
                    .child(
                        Button::new("view-delete")
                            .small()
                            .ghost()
                            .icon(IconName::Trash)
                            .tooltip(t("library.views.delete"))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for ManageViewsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<(u64, SharedString)> = self
            .model
            .read(cx)
            .views()
            .views
            .iter()
            .map(|v| (v.id, v.label()))
            .collect();
        let count = rows.len();
        let rendered: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, (id, name))| self.render_row(ix, count, id, name, cx).into_any_element())
            .collect();
        v_flex()
            .id("view-list")
            .max_h_96()
            .overflow_y_scroll()
            .when(count == 0, |d| {
                d.child(
                    div()
                        .py_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("library.views.empty")),
                )
            })
            .children(rendered)
    }
}

/// Open the view manager.
pub fn open_manage_views_dialog(window: &mut Window, cx: &mut App) {
    let view = cx.new(ManageViewsDialog::new);
    let width = dialog_width(window, 30.);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(t("library.views.manage_title"))
            .w(width)
            .child(view.clone())
    });
}
