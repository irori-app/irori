//! The Wine manager's Prefixes view: every Wine prefix with the installed
//! games running in it, from [`Backend::wine_prefixes`]. A prefix a game
//! made per title can number in the hundreds, so the list is virtual.
//!
//! [`Backend::wine_prefixes`]: irori_services::Backend::wine_prefixes

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Size, StatefulInteractiveElement as _, Styled as _, Window, div, px, rems, size,
};
use irori_services::prefixes::PrefixEntry;
use irori_ui::Route;
use irori_ui::components::EmptyState;
use irori_ui::i18n::{t, t_args};

use super::WineManagerView;
use super::installed::shorten_home;
use super::ui::{
    PAGE_MAX_WIDTH_REM, PillTone, eid, icon_tile, muted_xs, pill, seg_button, seg_group,
};
use crate::common::{card, plain_tooltip};

/// Height of one row in rem: name + status pills, path, and clickable game chips.
const ROW_REMS: f32 = 5.75;
/// Maximum game chips displayed directly on one prefix row before collapsing overflow.
const MAX_INLINE_GAME_CHIPS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrefixFilter {
    All,
    InUse,
    Unused,
}

/// What the Prefixes view shows.
pub(super) enum PrefixState {
    Loading,
    Loaded {
        list: Rc<Vec<PrefixEntry>>,
        visible: Rc<Vec<usize>>,
        sizes: Rc<Vec<Size<Pixels>>>,
        row_height: Pixels,
    },
}

/// Indices into `list` matching `query` and `filter`.
pub(super) fn filter_prefixes(
    list: &[PrefixEntry],
    query: &str,
    filter: PrefixFilter,
) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    list.iter()
        .enumerate()
        .filter(|(_, entry)| match filter {
            PrefixFilter::All => true,
            PrefixFilter::InUse => !entry.unused(),
            PrefixFilter::Unused => entry.unused(),
        })
        .filter(|(_, entry)| {
            needle.is_empty()
                || entry.name().to_lowercase().contains(&needle)
                || entry
                    .path
                    .display()
                    .to_string()
                    .to_lowercase()
                    .contains(&needle)
                || entry
                    .games
                    .iter()
                    .any(|(_, title)| title.to_lowercase().contains(&needle))
                || entry
                    .former
                    .iter()
                    .any(|title| title.to_lowercase().contains(&needle))
        })
        .map(|(ix, _)| ix)
        .collect()
}

/// Who uses a prefix, in one line: the installed games, else what it was.
pub(super) fn used_by(entry: &PrefixEntry) -> SharedString {
    let names: Vec<&str> = entry.games.iter().map(|(_, t)| t.as_str()).collect();
    match names.as_slice() {
        [] if entry.is_launcher || entry.is_default => t("manage.wine.prefixes.no_games"),
        [] if !entry.former.is_empty() => t_args(
            "manage.wine.prefixes.former",
            &[("games", &entry.former.join(", "))],
        ),
        [] => t("manage.wine.prefixes.unused"),
        [one] => t_args("manage.wine.prefixes.used_by", &[("games", one)]),
        many => t_args(
            "manage.wine.prefixes.used_by_many",
            &[
                ("count", &many.len().to_string()),
                ("games", &many.join(", ")),
            ],
        ),
    }
}

impl WineManagerView {
    /// Total prefixes and how many are currently unused, once loaded.
    pub(super) fn prefix_counts(&self) -> Option<(usize, usize)> {
        match &self.prefixes {
            Some(PrefixState::Loaded { list, .. }) => {
                Some((list.len(), list.iter().filter(|e| e.unused()).count()))
            }
            _ => None,
        }
    }

    /// Recompute visible indices and virtual row sizes for the current search
    /// query and status filter.
    pub(super) fn refresh_prefix_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.prefix_search.read(cx).value().to_string();
        let filter = self.prefix_filter;
        if let Some(PrefixState::Loaded {
            list,
            visible,
            sizes,
            row_height,
        }) = &mut self.prefixes
        {
            let indices = filter_prefixes(list, &query, filter);
            *sizes = Rc::new(vec![size(px(0.), *row_height); indices.len()]);
            *visible = Rc::new(indices);
            cx.notify();
        }
    }

    /// Read the prefixes again (on open, Refresh, and library or settings changes).
    pub(super) fn load_prefixes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prefixes.is_none() {
            self.prefixes = Some(PrefixState::Loading);
            cx.notify();
        }
        let backend = irori_ui::backend(cx);
        let task =
            irori_ui::state::run_infallible(cx, async move { backend.wine_prefixes().await });
        cx.spawn_in(window, async move |this, cx| {
            let list = task.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let row_height = window.rem_size() * ROW_REMS;
                let query = this.prefix_search.read(cx).value().to_string();
                let indices = filter_prefixes(&list, &query, this.prefix_filter);
                let sizes = Rc::new(vec![size(px(0.), row_height); indices.len()]);
                this.prefixes = Some(PrefixState::Loaded {
                    list: Rc::new(list),
                    visible: Rc::new(indices),
                    sizes,
                    row_height,
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn render_prefix_rows(
        &mut self,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(PrefixState::Loaded { list, visible, .. }) = &self.prefixes else {
            return Vec::new();
        };
        let (list, visible) = (list.clone(), visible.clone());
        range
            .filter_map(|row| Some(prefix_row(row, list.get(*visible.get(row)?)?, cx)))
            .collect()
    }

    /// The Prefixes view body, which owns its scrolling.
    pub(super) fn render_prefixes(&self, cx: &mut Context<Self>) -> AnyElement {
        let (muted, border, radius_lg, group_box) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.border,
                theme.radius_lg,
                theme.group_box,
            )
        };
        let body: AnyElement = match &self.prefixes {
            None | Some(PrefixState::Loading) => h_flex()
                .py_4()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.prefixes.loading"))
                .into_any_element(),
            Some(PrefixState::Loaded { list, .. }) if list.is_empty() => div()
                .py_4()
                .child(
                    EmptyState::new(IconName::Wine, t("manage.wine.prefixes.empty.title"))
                        .description(t("manage.wine.prefixes.empty.description")),
                )
                .into_any_element(),
            Some(PrefixState::Loaded {
                list,
                visible,
                sizes,
                ..
            }) => {
                let total = list.len();
                let unused = list.iter().filter(|e| e.unused()).count();
                let in_use = total.saturating_sub(unused);
                let filter_tab = |id: &'static str,
                                  label: SharedString,
                                  count: usize,
                                  value: PrefixFilter,
                                  cx: &mut Context<Self>| {
                    seg_button(
                        id,
                        self.prefix_filter == value,
                        cx,
                        cx.listener(move |this, _, _, cx| {
                            if this.prefix_filter != value {
                                this.prefix_filter = value;
                                this.refresh_prefix_filter(cx);
                            }
                        }),
                    )
                    .child(format!("{label} ({count})"))
                };
                let hint = if unused == 0 {
                    t("manage.wine.prefixes.hint")
                } else {
                    t_args(
                        "manage.wine.prefixes.hint_unused",
                        &[("count", &unused.to_string())],
                    )
                };
                let banner = card(cx).py_3().child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_3()
                                .items_center()
                                .when(unused > 0, |r| {
                                    r.child(icon_tile(
                                        IconName::HardDrive,
                                        Some(PillTone::Warning),
                                        cx,
                                    ))
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(hint),
                                ),
                        )
                        .when(unused > 0, |r| {
                            r.child(
                                h_flex()
                                    .gap_2()
                                    .flex_shrink_0()
                                    .items_center()
                                    .when(self.prefix_filter != PrefixFilter::Unused, |a| {
                                        a.child(
                                            Button::new("prefixes-view-unused")
                                                .outline()
                                                .small()
                                                .label(t_args(
                                                    "manage.wine.prefixes.view_unused",
                                                    &[("count", &unused.to_string())],
                                                ))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.prefix_filter = PrefixFilter::Unused;
                                                    this.refresh_prefix_filter(cx);
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("prefixes-storage")
                                            .primary()
                                            .small()
                                            .icon(IconName::HardDrive)
                                            .label(t("manage.wine.prefixes.open_storage"))
                                            .on_click(|_, _, cx| {
                                                irori_ui::navigate(
                                                    Route::Settings("storage".into()),
                                                    cx,
                                                )
                                            }),
                                    ),
                            )
                        }),
                );

                let filters = seg_group(cx)
                    .child(filter_tab(
                        "prefixes-filter-all",
                        t("manage.wine.prefixes.filter.all"),
                        total,
                        PrefixFilter::All,
                        cx,
                    ))
                    .child(filter_tab(
                        "prefixes-filter-in-use",
                        t("manage.wine.prefixes.filter.in_use"),
                        in_use,
                        PrefixFilter::InUse,
                        cx,
                    ))
                    .when(
                        unused > 0 || self.prefix_filter == PrefixFilter::Unused,
                        |r| {
                            r.child(filter_tab(
                                "prefixes-filter-unused",
                                t("manage.wine.prefixes.filter.unused"),
                                unused,
                                PrefixFilter::Unused,
                                cx,
                            ))
                        },
                    );

                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(banner)
                    .child(
                        h_flex()
                            .gap_3()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .child(filters)
                            .child(
                                div()
                                    .w(rems(18.))
                                    .flex_shrink_0()
                                    .child(Input::new(&self.prefix_search).small()),
                            ),
                    )
                    .child(if visible.is_empty() {
                        card(cx)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(t("manage.wine.prefixes.no_match")),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .rounded(radius_lg)
                            .border_1()
                            .border_color(border)
                            .bg(group_box)
                            .overflow_hidden()
                            .child(
                                v_virtual_list(
                                    cx.entity(),
                                    "wine-prefixes",
                                    sizes.clone(),
                                    |this, range, window, cx| {
                                        this.render_prefix_rows(range, window, cx)
                                    },
                                )
                                .size_full(),
                            )
                            .into_any_element()
                    })
                    .into_any_element()
            }
        };

        v_flex()
            .w_full()
            .max_w(rems(PAGE_MAX_WIDTH_REM))
            .mx_auto()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_4()
            .child(body)
            .into_any_element()
    }
}

fn prefix_row(ix: usize, entry: &PrefixEntry, cx: &App) -> AnyElement {
    let (border, primary) = (cx.theme().border, cx.theme().primary);
    let unused = entry.unused();
    let path = entry.path.clone();
    let full_path: SharedString = entry.path.display().to_string().into();

    let usage_line: AnyElement = if !entry.games.is_empty() {
        let shown = entry.games.iter().take(MAX_INLINE_GAME_CHIPS);
        let overflow: Vec<_> = entry
            .games
            .iter()
            .skip(MAX_INLINE_GAME_CHIPS)
            .cloned()
            .collect();
        h_flex()
            .gap_1p5()
            .items_center()
            .overflow_hidden()
            .children(shown.enumerate().map(|(gix, (game_id, title))| {
                let game_id = game_id.clone();
                Button::new(eid("prefix-game", format!("{ix}-{gix}")))
                    .outline()
                    .xsmall()
                    .icon(Icon::new(IconName::Gamepad2).xsmall().text_color(primary))
                    .label(SharedString::from(title.clone()))
                    .on_click(move |_, _, cx| irori_ui::navigate(Route::Game(game_id.clone()), cx))
            }))
            .when(!overflow.is_empty(), |row| {
                let extra = overflow.len();
                row.child(
                    Button::new(eid("prefix-games-more", ix))
                        .outline()
                        .xsmall()
                        .label(format!("+{extra}"))
                        .dropdown_menu(move |mut menu, _, _| {
                            for (game_id, title) in &overflow {
                                let game_id = game_id.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(title.clone())
                                        .icon(IconName::Gamepad2)
                                        .on_click(move |_, _, cx| {
                                            irori_ui::navigate(Route::Game(game_id.clone()), cx)
                                        }),
                                );
                            }
                            menu
                        }),
                )
            })
            .into_any_element()
    } else {
        muted_xs(used_by(entry), cx).into_any_element()
    };

    let hover_full_path = full_path.clone();
    h_flex()
        .w_full()
        .h(rems(ROW_REMS))
        .gap_3()
        .px_4()
        .items_center()
        .when(ix > 0, |r| r.border_t_1().border_color(border))
        .child(icon_tile(IconName::Folder, None, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .items_center()
                        .child(
                            div()
                                .min_w_0()
                                .text_sm()
                                .font_semibold()
                                .truncate()
                                .child(entry.name()),
                        )
                        .when(entry.is_default, |r| {
                            r.child(pill(
                                t("manage.wine.prefixes.default"),
                                PillTone::Accent,
                                false,
                                cx,
                            ))
                        })
                        .when(entry.is_launcher, |r| {
                            r.child(pill(
                                t("manage.wine.prefixes.launcher"),
                                PillTone::Info,
                                false,
                                cx,
                            ))
                        })
                        .when(unused, |r| {
                            r.child(pill(
                                t("manage.wine.prefixes.tag.unused"),
                                PillTone::Warning,
                                true,
                                cx,
                            ))
                        }),
                )
                .child(
                    muted_xs(shorten_home(&full_path), cx)
                        .id(eid("prefix-path", ix))
                        .tooltip(move |window, cx| {
                            plain_tooltip(hover_full_path.clone(), window, cx)
                        }),
                )
                .child(usage_line),
        )
        .child(
            Button::new(eid("prefix-open", ix))
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .label(t("manage.wine.prefixes.open_folder"))
                .on_click(move |_, _, cx| cx.open_with_system(&path)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{GameId, Runner};
    use std::path::PathBuf;

    fn entry(games: &[&str], former: &[&str]) -> PrefixEntry {
        PrefixEntry {
            path: PathBuf::from("/p/x"),
            games: games
                .iter()
                .map(|g| (GameId::new(Runner::Gog, *g), g.to_string()))
                .collect(),
            former: former.iter().map(|s| s.to_string()).collect(),
            is_default: false,
            is_launcher: false,
        }
    }

    #[test]
    fn says_who_uses_a_prefix() {
        crate::i18n::register();
        assert_eq!(
            used_by(&entry(&["Hades"], &[])),
            t_args("manage.wine.prefixes.used_by", &[("games", "Hades")])
        );
        assert_eq!(
            used_by(&entry(&["A", "B"], &["C"])),
            t_args(
                "manage.wine.prefixes.used_by_many",
                &[("count", "2"), ("games", "A, B")]
            )
        );
        assert_eq!(
            used_by(&entry(&[], &["Old"])),
            t_args("manage.wine.prefixes.former", &[("games", "Old")])
        );
        assert_eq!(used_by(&entry(&[], &[])), t("manage.wine.prefixes.unused"));
        let mut launcher = entry(&[], &["Old"]);
        launcher.is_launcher = true;
        assert_eq!(used_by(&launcher), t("manage.wine.prefixes.no_games"));
    }

    #[test]
    fn filter_prefixes_matches_status_and_search() {
        let used = PrefixEntry {
            path: PathBuf::from("/prefixes/hades"),
            games: vec![(GameId::new(Runner::Gog, "h"), "Hades II".into())],
            former: Vec::new(),
            is_default: false,
            is_launcher: false,
        };
        let unused = PrefixEntry {
            path: PathBuf::from("/prefixes/old"),
            games: Vec::new(),
            former: vec!["Cyberpunk 2077".into()],
            is_default: false,
            is_launcher: false,
        };
        let list = vec![used, unused];
        assert_eq!(filter_prefixes(&list, "", PrefixFilter::All), vec![0, 1]);
        assert_eq!(filter_prefixes(&list, "", PrefixFilter::InUse), vec![0]);
        assert_eq!(filter_prefixes(&list, "", PrefixFilter::Unused), vec![1]);
        assert_eq!(filter_prefixes(&list, "hades", PrefixFilter::All), vec![0]);
        assert_eq!(
            filter_prefixes(&list, "cyberpunk", PrefixFilter::All),
            vec![1]
        );
    }
}
