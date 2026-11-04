//! The selected mod's details: where it came from, its size, what it needs,
//! and every file it shares with another mod, with the one the game gets.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled,
    Window, div, px, rems, size,
};
use irori_mods::{FileConflict, ModEntry};
use irori_services::ModsView;
use irori_ui::i18n::{t, t_args};

use super::{ModsPanel, source_label};

/// Height of one conflict row, in rems.
const FILE_ROW_REMS: f32 = 2.25;
const VISIBLE_FILES: usize = 8;

impl ModsPanel {
    pub(super) fn render_details(
        &mut self,
        view: &Arc<ModsView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let m = view.list.get(self.selected.as_deref()?)?.clone();
        let name_of = |id: &str| -> String {
            view.list
                .get(id)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| id.to_string())
        };
        let files: Vec<FileConflict> = view
            .conflicts
            .files
            .iter()
            .filter(|f| f.mods.contains(&m.id))
            .cloned()
            .collect();
        let standing = self.render_standing(view, &m, &name_of, cx);
        let file_list =
            (!files.is_empty()).then(|| self.render_conflict_files(view, files, window, cx));
        let theme = cx.theme();
        let mut facts: Vec<(SharedString, String)> = vec![
            (
                t("game.mods.detail_source"),
                source_label(&m.source).to_string(),
            ),
            (
                t("game.mods.detail_files"),
                t_args(
                    "game.mods.detail_files_value",
                    &[
                        ("count", &m.files.to_string()),
                        ("size", &irori_core::util::format_bytes(m.size)),
                    ],
                )
                .to_string(),
            ),
        ];
        if let Some(v) = &m.version {
            facts.push((t("game.mods.detail_version"), v.clone()));
        }
        if let Some(a) = &m.author {
            facts.push((t("game.mods.detail_author"), a.clone()));
        }
        if let Some(date) = m.installed_at.get(..10) {
            facts.push((t("game.mods.detail_installed"), date.to_string()));
        }
        if !m.dependencies.is_empty() {
            facts.push((t("game.mods.detail_needs"), m.dependencies.join(", ")));
        }
        let page = m.source.page_url().and_then(|u| irori_ui::web_url(&u));
        Some(
            v_flex()
                .id("mod-details")
                .gap_3()
                .p_4()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
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
                                .child(m.name.clone()),
                        )
                        .when_some(page, |row, url| {
                            row.child(
                                Button::new("mod-page")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label(t("game.mods.open_page"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                )
                .when_some(m.summary.clone(), |col, s| {
                    col.child(div().text_sm().text_color(theme.muted_foreground).child(s))
                })
                .child(
                    v_flex()
                        .gap_1()
                        .children(facts.into_iter().map(|(label, value)| {
                            h_flex()
                                .gap_3()
                                .text_sm()
                                .child(
                                    div()
                                        .w(rems(7.))
                                        .flex_shrink_0()
                                        .text_color(theme.muted_foreground)
                                        .child(label),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(value))
                        })),
                )
                .child(standing)
                .children(file_list)
                .into_any_element(),
        )
    }

    /// One line per mod this one beats or loses to.
    fn render_standing(
        &self,
        view: &ModsView,
        m: &ModEntry,
        name_of: &dyn Fn(&str) -> String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let line = |icon: IconName, text: SharedString, color| {
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .child(Icon::new(icon).small().text_color(color))
                .child(div().flex_1().min_w_0().child(text))
        };
        let mut col = v_flex().gap_1();
        if !m.enabled {
            return col
                .child(line(
                    IconName::Info,
                    t("game.mods.disabled_note"),
                    theme.muted_foreground,
                ))
                .into_any_element();
        }
        let Some(c) = view.conflicts.of(&m.id) else {
            return col
                .child(line(
                    IconName::CircleCheck,
                    t("game.mods.no_conflicts"),
                    theme.success,
                ))
                .into_any_element();
        };
        if c.hidden {
            col = col.child(line(
                IconName::TriangleAlert,
                t("game.mods.hidden_note"),
                theme.warning,
            ));
        }
        for (other, n) in &c.wins_over {
            col = col.child(line(
                IconName::ArrowUp,
                super::counted(
                    "game.mods.wins_over_one",
                    "game.mods.wins_over_other",
                    *n,
                    &[("name", &name_of(other))],
                ),
                theme.success,
            ));
        }
        for (other, n) in &c.loses_to {
            col = col.child(line(
                IconName::ArrowDown,
                super::counted(
                    "game.mods.loses_to_one",
                    "game.mods.loses_to_other",
                    *n,
                    &[("name", &name_of(other))],
                ),
                theme.warning,
            ));
        }
        if c.game_files > 0 {
            col = col.child(line(
                IconName::Replace,
                super::counted(
                    "game.mods.replaces_game_files_one",
                    "game.mods.replaces_game_files_other",
                    c.game_files,
                    &[],
                ),
                theme.muted_foreground,
            ));
        }
        if c.wins_over.is_empty() && c.loses_to.is_empty() && !c.hidden {
            col = col.child(line(
                IconName::CircleCheck,
                t("game.mods.no_conflicts"),
                theme.success,
            ));
        }
        col.into_any_element()
    }

    fn render_conflict_files(
        &mut self,
        view: &Arc<ModsView>,
        files: Vec<FileConflict>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_h = window.rem_size() * FILE_ROW_REMS;
        let count = files.len();
        let sizes = Rc::new(vec![size(px(0.), row_h); count]);
        let names: Vec<(String, String)> = files
            .iter()
            .map(|f| {
                let winner = view
                    .list
                    .get(f.winner())
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                (f.path.clone(), winner)
            })
            .collect();
        let names = Rc::new(names);
        let theme = cx.theme();
        let list = v_virtual_list(cx.entity(), "mod-conflict-files", sizes, {
            let names = names.clone();
            move |_, range: Range<usize>, _, cx| {
                let theme = cx.theme();
                range
                    .filter_map(|ix| {
                        let (path, winner) = names.get(ix)?;
                        Some(
                            h_flex()
                                .h(rems(FILE_ROW_REMS))
                                .px_3()
                                .gap_3()
                                .items_center()
                                .text_xs()
                                .when(ix > 0, |r| r.border_t_1().border_color(theme.border))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
                                        .child(path.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w(rems(14.))
                                        .truncate()
                                        .text_color(theme.muted_foreground)
                                        .child(t_args("game.mods.file_from", &[("name", winner)])),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect()
            }
        })
        .size_full();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(t_args(
                        "game.mods.shared_files",
                        &[("count", &count.to_string())],
                    )),
            )
            .child(
                div()
                    .w_full()
                    .h(rems(FILE_ROW_REMS * count.min(VISIBLE_FILES) as f32))
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(list),
            )
            .into_any_element()
    }
}
