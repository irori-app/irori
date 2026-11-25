//! "Manage genres and companies…": every genre, developer and publisher the
//! library's games carry, with how many games have it, and the rules that
//! rename one, merge it into another or remove it across the library
//! (`irori_services::field_rules`). Spellings that look alike are offered
//! for merging. The values are read off the UI thread when the dialog
//! opens; rules are written through `UserDataService::set_field_rule`,
//! which rebuilds the library.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Size, Styled as _, Subscription, Window, div, px, rems, size,
};
use irori_services::field_rules::{self, Field, FieldRules, FieldValue};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

const ROW_REMS: f32 = 2.5;
/// Look-alike spellings offered at once.
const SUGGESTIONS: usize = 4;

/// Open the manager.
pub fn open_fields_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let dialog = cx.new(|cx| FieldsDialog::new(window, cx));
    let width = window.rem_size() * 34.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("library.fields.title"))
            .width(width)
            .child(dialog.clone())
    });
}

/// `values` whose name contains `query` (case ignored), in their order.
fn matching<'a>(values: &'a [FieldValue], query: &str) -> Vec<&'a FieldValue> {
    let query = query.trim().to_lowercase();
    values
        .iter()
        .filter(|v| query.is_empty() || v.value.to_lowercase().contains(&query))
        .collect()
}

struct FieldsDialog {
    field: Field,
    /// Genres and companies, once read.
    genres: Option<Vec<FieldValue>>,
    companies: Option<Vec<FieldValue>>,
    search: Entity<InputState>,
    rename: Entity<InputState>,
    /// The value being edited, by name.
    selected: Option<String>,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
    _subscriptions: Vec<Subscription>,
}

impl FieldsDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("library.fields.search")));
        let rename =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("library.fields.rename_to")));
        let library = models(cx).library.clone();
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&rename, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.apply_rename(window, cx);
                }
            }),
            // The library is rebuilt after each rule: redraw what it says.
            cx.observe(&library, |_, _, cx| cx.notify()),
        ];
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            let reader = b.clone();
            b.spawn_blocking(move || {
                (
                    reader.field_values(Field::Genre),
                    reader.field_values(Field::Company),
                )
            })
            .await
            .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let (genres, companies) = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.genres = Some(genres);
                this.companies = Some(companies);
                cx.notify();
            });
        })
        .detach();
        Self {
            field: Field::Genre,
            genres: None,
            companies: None,
            search,
            rename,
            selected: None,
            sizes: Rc::new(Vec::new()),
            sized_for: (0, px(0.)),
            _subscriptions: subscriptions,
        }
    }

    fn values(&self) -> Option<&[FieldValue]> {
        match self.field {
            Field::Genre => self.genres.as_deref(),
            Field::Company => self.companies.as_deref(),
        }
    }

    fn rules(cx: &App) -> FieldRules {
        models(cx).library.read(cx).user.field_rules.clone()
    }

    fn shown(&self, cx: &App) -> Vec<FieldValue> {
        let query = self.search.read(cx).value().to_string();
        self.values()
            .map(|v| matching(v, &query).into_iter().cloned().collect())
            .unwrap_or_default()
    }

    fn select(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let to = Self::rules(cx)
            .rule(self.field, &value)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| value.clone());
        self.rename.update(cx, |s, cx| s.set_value(to, window, cx));
        self.selected = Some(value);
        cx.notify();
    }

    fn set_field(&mut self, field: Field, cx: &mut Context<Self>) {
        self.field = field;
        self.selected = None;
        cx.notify();
    }

    /// Write one rule off the UI thread.
    fn write(
        &mut self,
        value: String,
        to: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (b, field) = (backend(cx), self.field);
        let task = irori_ui::run(cx, async move {
            let writer = b.clone();
            b.spawn_blocking(move || writer.userdata.set_field_rule(field, &value, to.as_deref()))
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("library.fields.failed")),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self.selected.clone() else {
            return;
        };
        let to = self.rename.read(cx).value().trim().to_string();
        if to.is_empty() {
            return;
        }
        self.write(value, Some(to), window, cx);
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let shown = self.shown(cx);
        let rules = Self::rules(cx);
        let muted = cx.theme().muted_foreground;
        range
            .filter_map(|ix| {
                let v = shown.get(ix)?.clone();
                let selected = self.selected.as_deref() == Some(v.value.as_str());
                let rule: Option<SharedString> = rules.rule(self.field, &v.value).map(|to| {
                    if to.is_empty() {
                        t("library.fields.removed")
                    } else {
                        t_args("library.fields.shown_as", &[("value", to)])
                    }
                });
                let count_key = if v.games == 1 {
                    "library.fields.games_one"
                } else {
                    "library.fields.games_other"
                };
                let count = t_args(count_key, &[("count", &v.games.to_string())]);
                let value = v.value.clone();
                Some(
                    Button::new(("field-value", ix))
                        .ghost()
                        .small()
                        .w_full()
                        .h(rems(ROW_REMS))
                        .selected(selected)
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(div().flex_1().min_w_0().truncate().child(v.value.clone()))
                                .children(rule.map(|r| {
                                    div()
                                        .flex_shrink_0()
                                        .max_w(rems(10.))
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(r)
                                }))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(count),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select(value.clone(), window, cx)
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for FieldsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rules = Self::rules(cx);
        let tab = |id: &'static str, label: SharedString, field: Field, current: Field| {
            Button::new(id)
                .small()
                .ghost()
                .selected(field == current)
                .label(label)
        };
        let tabs = h_flex()
            .gap_1()
            .child(
                tab(
                    "fields-genres",
                    t("library.fields.genres"),
                    Field::Genre,
                    self.field,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_field(Field::Genre, cx))),
            )
            .child(
                tab(
                    "fields-companies",
                    t("library.fields.companies"),
                    Field::Company,
                    self.field,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_field(Field::Company, cx))),
            );
        let Some(values) = self.values().map(<[FieldValue]>::to_vec) else {
            return v_flex().gap_3().child(tabs).child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.fields.loading")),
            );
        };
        let suggestion_rows = field_rules::duplicates(&values, self.field, &rules)
            .into_iter()
            .take(SUGGESTIONS)
            .enumerate()
            .map(|(i, (from, to))| {
                let label = t_args(
                    "library.fields.merge_suggestion",
                    &[("from", &from), ("to", &to)],
                );
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .child(
                        Button::new(("field-merge", i))
                            .xsmall()
                            .outline()
                            .label(t("library.fields.merge"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.write(from.clone(), Some(to.clone()), window, cx)
                            })),
                    )
            });
        let count = self.shown(cx).len();
        let row_h = window.rem_size() * ROW_REMS;
        if self.sized_for != (count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (count, row_h);
        }
        let list = v_virtual_list(
            cx.entity(),
            "field-values",
            self.sizes.clone(),
            |this, range, _, cx| this.render_rows(range, cx),
        )
        .h(rems((count.clamp(1, 8) as f32) * ROW_REMS))
        .w_full();
        let editor =
            self.selected.clone().map(|value| {
                let has_rule = rules.rule(self.field, &value).is_some();
                let (remove_value, restore_value) = (value.clone(), value.clone());
                v_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .child(t_args("library.fields.editing", &[("value", &value)])),
                    )
                    .child(Input::new(&self.rename).small())
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .when(has_rule, |row| {
                                row.child(
                                    Button::new("field-restore")
                                        .ghost()
                                        .small()
                                        .label(t("library.fields.restore"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.write(restore_value.clone(), None, window, cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("field-remove")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Trash)
                                    .label(t("library.fields.remove"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.write(
                                            remove_value.clone(),
                                            Some(String::new()),
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("field-rename")
                                    .primary()
                                    .small()
                                    .label(t("library.fields.apply"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_rename(window, cx)
                                    })),
                            ),
                    )
            });
        v_flex()
            .gap_3()
            .child(tabs)
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.fields.help")),
            )
            .when(!values.is_empty(), |c| {
                c.child(v_flex().gap_1().children(suggestion_rows))
            })
            .child(Input::new(&self.search).small())
            .child(if count == 0 {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.fields.none"))
                    .into_any_element()
            } else {
                list.into_any_element()
            })
            .children(editor)
            .child(
                h_flex().justify_end().child(
                    Button::new("fields-done")
                        .small()
                        .label(t("library.fields.done"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_finds_values_by_any_part() {
        let values = vec![
            FieldValue {
                value: "Role-playing".into(),
                games: 3,
            },
            FieldValue {
                value: "Action".into(),
                games: 2,
            },
        ];
        let names = |q: &str| -> Vec<String> {
            matching(&values, q)
                .iter()
                .map(|v| v.value.clone())
                .collect()
        };
        assert_eq!(names(""), ["Role-playing", "Action"]);
        assert_eq!(names("PLAY"), ["Role-playing"]);
        assert!(names("strategy").is_empty());
    }
}
