use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea, TextareaState};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div,
};
use irori_core::settings::UiPrefs;
use irori_ui::Route;
use irori_ui::i18n::t;
use serde_json::Value;

use crate::registry::{ActionKind, CustomKind, FieldKind, FieldSpec, PanelKind, TextKind};
use crate::widgets::{
    PairedRow, RowState, field_row, group_header, group_panel, muted, paired_field_row, panel,
    section_heading,
};

use super::SettingsForm;

impl SettingsForm {
    /// A field's help, followed by what the displays say about it.
    fn field_help(&self, spec: &FieldSpec) -> SharedString {
        let help = spec.help_text();
        let Some(hint) = self
            .displays
            .as_deref()
            .and_then(|d| crate::displays::hint_text(spec.path, d))
        else {
            return help;
        };
        if help.is_empty() {
            hint
        } else {
            format!("{help} {hint}").into()
        }
    }

    /// Use these displays instead of the computer's.
    #[cfg(test)]
    pub(crate) fn set_displays_for_test(
        &mut self,
        displays: irori_services::displays::DisplayReport,
        cx: &mut Context<Self>,
    ) {
        self.displays = Some(std::sync::Arc::new(displays));
        cx.notify();
    }

    /// The help shown under a field.
    #[cfg(test)]
    pub(crate) fn help_for_test(&self, path: &str) -> SharedString {
        let spec = crate::registry::find(path).expect("a registered field");
        self.field_help(spec)
    }

    // ---- rendering ----

    fn render_field(
        &self,
        spec: &'static FieldSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled_reason = self.disabled_reason(spec);
        let disabled = disabled_reason.is_some();
        let id = |part: &str| ElementId::Name(format!("{part}-{}", spec.path).into());
        let overridden = self.is_overridden(spec);
        let path = spec.path;
        let reset = overridden.then(|| {
            Button::new(id("reset"))
                .ghost()
                .xsmall()
                .icon(IconName::RotateCcw)
                .tooltip(t("settings.field.use_default"))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.reset_field(path, window, cx)),
                )
                .into_any_element()
        });
        let label = spec.label_text();
        let tooltip = spec.tooltip_text();
        let mut row = RowState {
            label,
            help: self.field_help(spec),
            tooltip: (!tooltip.is_empty()).then(|| (SharedString::new_static(path), tooltip)),
            tag: spec.tag.map(|tag| tag.text()),
            reset,
            disabled_reason,
            error: self.errors.get(spec.path).cloned(),
            stacked: false,
        };
        let control = self.render_control(spec, disabled, false, Some(&mut row), window, cx);
        field_row(row, control, cx)
            .id(id("row"))
            .debug_selector(move || format!("row-{path}"))
            .into_any_element()
    }

    fn render_paired_fields(
        &self,
        first: &'static FieldSpec,
        second: &'static FieldSpec,
        pair: &'static crate::layout::FieldPair,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled_a = self.disabled_reason(first).is_some();
        let disabled_b = self.disabled_reason(second).is_some();
        let ctrl_a = self.render_control(first, disabled_a, true, None, window, cx);
        let ctrl_b = self.render_control(second, disabled_b, true, None, window, cx);
        let mut help = if pair.help_key.is_empty() {
            String::new()
        } else {
            t(pair.help_key).to_string()
        };
        if let Some(hint) = self.displays.as_deref().and_then(|d| {
            crate::displays::hint_text(first.path, d)
                .or_else(|| crate::displays::hint_text(second.path, d))
        }) {
            if help.is_empty() {
                help = hint.to_string();
            } else {
                help = format!("{help} {hint}");
            }
        }
        let error = self
            .errors
            .get(first.path)
            .or_else(|| self.errors.get(second.path))
            .cloned();
        let pair_id = pair.first;
        let tooltip = (!pair.tooltip_key.is_empty())
            .then(|| (SharedString::new_static(pair_id), t(pair.tooltip_key)));
        paired_field_row(
            PairedRow {
                label: t(pair.label_key),
                help: help.into(),
                tooltip,
                first_sublabel: t(pair.first_sublabel_key),
                first_ctrl: ctrl_a,
                separator: pair.separator,
                second_sublabel: t(pair.second_sublabel_key),
                second_ctrl: ctrl_b,
                error,
            },
            cx,
        )
        .id(ElementId::Name(format!("pair-{pair_id}").into()))
        .debug_selector(move || format!("pair-{pair_id}"))
        .into_any_element()
    }

    fn render_control(
        &self,
        spec: &'static FieldSpec,
        disabled: bool,
        compact_pair: bool,
        row: Option<&mut RowState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = |part: &str| ElementId::Name(format!("{part}-{}", spec.path).into());
        let path = spec.path;
        let label = spec.label_text();
        match spec.kind {
            FieldKind::Toggle => {
                let inverted = spec.inverted;
                Switch::new(id("field"))
                    .checked(self.bool_value(spec.path) ^ inverted)
                    .disabled(disabled)
                    .accessibility_label(label)
                    .on_change(cx.listener(move |this, on: &bool, window, cx| {
                        this.write(path, Value::Bool(*on ^ inverted), window, cx)
                    }))
                    .into_any_element()
            }
            FieldKind::Text(kind) => {
                let Some(state) = self.texts.get(spec.path) else {
                    return div().into_any_element();
                };
                let picker = matches!(kind, TextKind::Directory | TextKind::File).then(|| {
                    Button::new(id("pick"))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .tooltip(if kind == TextKind::Directory {
                            t("settings.picker.choose_folder")
                        } else {
                            t("settings.picker.choose_file")
                        })
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_path(path, kind == TextKind::Directory, window, cx)
                        }))
                });
                let prefixes = (path == "winePrefix")
                    .then(|| self.prefix_menu(disabled, cx))
                    .flatten();
                let picker = match (picker, prefixes) {
                    (Some(picker), Some(menu)) => {
                        Some(h_flex().child(menu).child(picker).into_any_element())
                    }
                    (picker, _) => picker.map(IntoElement::into_any_element),
                };
                let wide = matches!(
                    kind,
                    TextKind::Plain | TextKind::Directory | TextKind::File | TextKind::Secret
                );
                Input::new(state)
                    .id(id("field"))
                    .aria_label(label)
                    .disabled(disabled)
                    .when(wide && !compact_pair, |i| i.w_80())
                    .when(wide && compact_pair, |i| i.w_48())
                    .when(!wide && !compact_pair, |i| i.w_24())
                    .when(!wide && compact_pair, |i| i.w_20())
                    .when_some(picker, |i, p| i.suffix(p))
                    .into_any_element()
            }
            FieldKind::Choice(_) => match self.choices.get(spec.path) {
                Some(state) => Select::new(state)
                    .id(id("field"))
                    .accessibility_label(label)
                    .disabled(disabled)
                    .when(compact_pair, |s| s.w_40())
                    .when(!compact_pair, |s| s.w_64())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            FieldKind::Custom(kind) => {
                let mut fallback = RowState::default();
                let row = row.unwrap_or(&mut fallback);
                if matches!(
                    kind,
                    CustomKind::EnvVars
                        | CustomKind::DllOverrides
                        | CustomKind::Wrappers
                        | CustomKind::GogSaves
                ) {
                    row.stacked = true;
                }
                self.render_custom(spec, kind, row, window, cx)
            }
            FieldKind::Panel(_) => {
                if let Some(row) = row {
                    row.stacked = true;
                }
                match self.panels.get(spec.path) {
                    Some(view) => view.clone().into_any_element(),
                    None => div().into_any_element(),
                }
            }
            FieldKind::Action(action) => {
                if action == ActionKind::ImportHeroic
                    && let Some(summary) = &self.import_summary
                    && let Some(row) = row
                {
                    row.help = summary.clone();
                }
                let (key, danger) = match action {
                    ActionKind::ClearCaches => ("settings.action.clear_caches", false),
                    ActionKind::ResetSettings => ("settings.action.reset_settings", true),
                    ActionKind::OpenLogsFolder => ("settings.action.open_folder", false),
                    ActionKind::ImportHeroic
                    | ActionKind::ImportLutris
                    | ActionKind::ImportBottles
                    | ActionKind::ImportFaugus
                    | ActionKind::ImportPlaynite => ("settings.action.import", false),
                    ActionKind::OpenWindowsFiles => ("settings.action.use_irori", false),
                };
                Button::new(id("action"))
                    .label(t(key))
                    .when(danger, |b| b.danger())
                    .loading(self.busy.contains(&action))
                    .disabled(self.busy.contains(&action))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.run_action(action, window, cx)),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_custom(
        &self,
        spec: &'static FieldSpec,
        kind: CustomKind,
        row: &mut RowState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = spec.path;
        match kind {
            CustomKind::WineVersion => {
                let Some(wine) = &self.wine else {
                    return div().into_any_element();
                };
                let empty = wine.builds.as_ref().is_some_and(|b| b.is_empty())
                    && self
                        .value(path)
                        .and_then(|v| v.get("bin"))
                        .and_then(Value::as_str)
                        .is_none_or(|b| b.is_empty());
                if wine.loading {
                    row.help = t("settings.wine.searching");
                }
                if empty {
                    row.help = t("settings.wine.none_found");
                    return Button::new("open-wine-manager")
                        .label(t("settings.wine.open_manager"))
                        .on_click(|_, _, cx| irori_ui::navigate(Route::WineManager, cx))
                        .into_any_element();
                }
                if let Some(bin) = self
                    .value(path)
                    .and_then(|v| v.get("bin"))
                    .and_then(Value::as_str)
                    && !bin.is_empty()
                    && row.help.is_empty()
                {
                    row.help = bin.to_string().into();
                }
                Select::new(&wine.select)
                    .id("field-wineVersion")
                    .accessibility_label(spec.label_text())
                    .placeholder(if wine.loading {
                        t("settings.common.loading")
                    } else {
                        t("settings.wine.choose_build")
                    })
                    .w_64()
                    .into_any_element()
            }
            CustomKind::ToolVersion(_) => match self.choices.get(path) {
                Some(state) => Select::new(state)
                    .id(path)
                    .accessibility_label(spec.label_text())
                    .w_64()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            CustomKind::GpuCard => match self.choices.get(path) {
                Some(state) => Select::new(state)
                    .id("field-iroriGpu")
                    .accessibility_label(spec.label_text())
                    .placeholder(t("settings.gpu.system_choice"))
                    .disabled(row.disabled_reason.is_some())
                    .w_80()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            CustomKind::HiddenGpu => match self.choices.get(path) {
                Some(state) => Select::new(state)
                    .id("field-iroriHideGpu")
                    .accessibility_label(spec.label_text())
                    .placeholder(t("settings.hide_gpu.none"))
                    .disabled(row.disabled_reason.is_some())
                    .w_80()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            CustomKind::ThemeName => match self.choices.get(path) {
                Some(state) => Select::new(state)
                    .id("field-themeName")
                    .accessibility_label(spec.label_text())
                    .w_64()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            CustomKind::AccentColor | CustomKind::FontFamily => match self.panels.get(path) {
                Some(view) => view.clone().into_any_element(),
                None => div().into_any_element(),
            },
            CustomKind::FontSize | CustomKind::TileSize | CustomKind::Percent => {
                let Some(state) = self.sliders.get(path) else {
                    return div().into_any_element();
                };
                let v = state.read(cx).value().start();
                let label = match kind {
                    CustomKind::FontSize => format!("{:.0}%", v / 16. * 100.),
                    CustomKind::TileSize => format!("{v:.1} rem"),
                    _ => format!("{v:.0}%"),
                };
                let defaults = UiPrefs::default();
                let reset_value = match path {
                    "iroriUi.fontSize" => defaults.font_size,
                    "iroriUi.libraryBackgroundStrength" => {
                        defaults.library_background_strength as f32
                    }
                    "iroriUi.libraryBackgroundDim" => defaults.library_background_dim as f32,
                    _ => defaults.tile_size,
                };
                h_flex()
                    .gap_3()
                    .w_80()
                    .child(
                        Slider::new(state)
                            .accessibility_label(spec.label_text())
                            .flex_1(),
                    )
                    .child(
                        div()
                            .w_16()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_right()
                            .child(label),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("default-{path}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .tooltip(t("settings.field.default_size"))
                            .disabled((v - reset_value).abs() < f32::EPSILON)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(s) = this.sliders.get(path) {
                                    s.update(cx, |s, cx| s.set_value(reset_value, window, cx));
                                }
                                this.slider_changed(path, reset_value, true, window, cx);
                            })),
                    )
                    .into_any_element()
            }
            CustomKind::EnvVars
            | CustomKind::DllOverrides
            | CustomKind::Wrappers
            | CustomKind::GogSaves => self.render_pairs(spec, cx).into_any_element(),
        }
    }

    fn render_pairs(
        &self,
        spec: &'static FieldSpec,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let path = spec.path;
        let Some(editor) = self.pairs.get(path) else {
            return v_flex();
        };
        if let Some(text) = &editor.text {
            return self.render_env_text(path, text, editor.error.clone(), cx);
        }
        let rows = self.rows(path);
        let sep = match editor.kind {
            CustomKind::EnvVars | CustomKind::DllOverrides => "=",
            _ => " ",
        };
        let theme = cx.theme();
        let (border, muted_fg, danger) = (theme.border, theme.muted_foreground, theme.danger);
        let list = panel(cx).when(rows.is_empty(), |p| {
            p.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("settings.pairs.empty")),
            )
        });
        let list = rows
            .into_iter()
            .enumerate()
            .fold(list, |list, (ix, (a, b))| {
                let editing = editor.editing == Some(ix);
                list.child(
                    h_flex()
                        .id(ElementId::Name(format!("{path}-row-{ix}").into()))
                        .gap_2()
                        .px_3()
                        .py_1()
                        .when(ix > 0, |r| r.border_t_1().border_color(border))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_family(cx.theme().mono_font_family.clone())
                                .when(editing, |d| d.text_color(muted_fg))
                                .child(format!("{a}{sep}{b}")),
                        )
                        .child(
                            Button::new(ElementId::Name(format!("{path}-edit-{ix}").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Pencil)
                                .tooltip(t("settings.common.edit"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_row(path, ix, window, cx)
                                })),
                        )
                        .child(
                            Button::new(ElementId::Name(format!("{path}-remove-{ix}").into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Trash)
                                .tooltip(t("settings.common.remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_row(path, ix, window, cx)
                                })),
                        ),
                )
            });
        let editing = editor.editing.is_some();
        let add = h_flex()
            .gap_2()
            .child(
                Input::new(&editor.first)
                    .id(ElementId::Name(format!("{path}-first").into()))
                    .flex_1(),
            )
            .child(
                Input::new(&editor.second)
                    .id(ElementId::Name(format!("{path}-second").into()))
                    .flex_1(),
            )
            .child(
                Button::new(ElementId::Name(format!("{path}-add").into()))
                    .icon(if editing {
                        IconName::Check
                    } else {
                        IconName::Plus
                    })
                    .label(if editing {
                        t("settings.common.save")
                    } else {
                        t("settings.common.add")
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.commit_pair(path, window, cx)),
                    ),
            )
            .when(editing, |r| {
                r.child(
                    Button::new(ElementId::Name(format!("{path}-cancel").into()))
                        .ghost()
                        .label(t("settings.common.cancel"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.cancel_edit(path, window, cx)
                        })),
                )
            })
            .when(editor.kind == CustomKind::EnvVars && !editing, |r| {
                r.child(
                    Button::new(ElementId::Name(format!("{path}-as-text").into()))
                        .ghost()
                        .icon(IconName::Pencil)
                        .label(t("settings.env_text.open"))
                        .tooltip(t("settings.env_text.open_tooltip"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_env_text(path, window, cx)
                        })),
                )
            });
        let hint = (editor.kind == CustomKind::Wrappers)
            .then(|| muted(t("settings.pairs.wrapper_hint"), cx).text_xs());
        v_flex()
            .gap_2()
            .w_full()
            .child(list)
            .child(add)
            .children(hint)
            .when_some(editor.error.clone(), |v, e| {
                v.child(div().text_xs().text_color(danger).child(e))
            })
    }

    /// The environment variables as text, with Apply and Cancel.
    fn render_env_text(
        &self,
        path: &'static str,
        text: &Entity<TextareaState>,
        error: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let danger = cx.theme().danger;
        v_flex()
            .gap_2()
            .w_full()
            .child(
                Textarea::new(text)
                    .aria_label(t("settings.field.environment_options.label"))
                    .font_family(cx.theme().mono_font_family.clone()),
            )
            .child(muted(t("settings.env_text.hint"), cx).text_xs())
            .when_some(error, |v, e| {
                v.child(div().text_xs().text_color(danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(ElementId::Name(format!("{path}-text-apply").into()))
                            .primary()
                            .icon(IconName::Check)
                            .label(t("settings.env_text.apply"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_env_text(path, window, cx)
                            })),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("{path}-text-cancel").into()))
                            .ghost()
                            .label(t("settings.common.cancel"))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.close_env_text(path, cx)),
                            ),
                    ),
            )
    }

    pub(super) fn render_field_list(
        &self,
        fields: &[&'static FieldSpec],
        multi_section: bool,
        compact_rows: bool,
        first_tight: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let border = cx.theme().border;
        let mut body = v_flex().w_full();
        // Rows of the current group, gathered into one panel.
        let mut rows: Option<gpui_kit::Div> = None;
        let mut section = None;
        let mut group = "";
        let mut first_group = true;
        let mut i = 0;
        while i < fields.len() {
            let spec = fields[i];
            if section != Some(spec.section) {
                body = body.children(rows.take());
                if multi_section {
                    body = body.child(section_heading(spec.section.label()));
                }
                section = Some(spec.section);
                group = "";
            }
            if matches!(
                spec.kind,
                FieldKind::Panel(PanelKind::PerformanceHud | PanelKind::Optiscaler)
            ) {
                body = body.children(rows.take());
                body = body.child(section_heading(spec.group_text()));
                if let Some(view) = self.panels.get(spec.path) {
                    body = body.child(
                        div()
                            .id(ElementId::Name(format!("row-{}", spec.path).into()))
                            .child(view.clone()),
                    );
                }
                group = spec.group;
                first_group = false;
                i += 1;
                continue;
            }
            if group != spec.group {
                body = body.children(rows.take());
                let desc = crate::layout::group_description_key(spec.group).map(t);
                body = body.child(group_header(
                    spec.group_text(),
                    desc,
                    first_group && first_tight,
                    cx,
                ));
                group = spec.group;
                first_group = false;
            }
            let first = rows.is_none();
            let row_content = if compact_rows
                && i + 1 < fields.len()
                && fields[i + 1].section == spec.section
                && fields[i + 1].group == spec.group
                && let Some(pair) = crate::layout::match_pair(spec.path, fields[i + 1].path)
            {
                let next = fields[i + 1];
                i += 2;
                self.render_paired_fields(spec, next, pair, window, cx)
            } else {
                i += 1;
                self.render_field(spec, window, cx)
            };
            let row = div()
                .when(!first, |d| d.border_t_1().border_color(border))
                .child(row_content);
            rows = Some(rows.unwrap_or_else(|| group_panel(cx)).child(row));
        }
        body.children(rows)
    }
}
