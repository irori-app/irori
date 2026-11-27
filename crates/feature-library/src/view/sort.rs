//! The modern "Sort & Group" popover in the library toolbar.
//!
//! Replaces the 23-item flat dropdown menu with a Look-style popover showing
//! the 6 primary sorts, 5 primary groupings, an Asc/Desc toggle, and a
//! collapsible "More options" drawer holding the remaining 4 sorts and 7
//! groupings (auto-expanding whenever a secondary sort or grouping is active).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div, rems,
};
use irori_services::LibrarySort;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::LibraryView;
use super::look::{
    Segment, active_accent, control_label, section_card, segmented_bar, toggle_pill,
};
use crate::filter::Grouping;

/// Primary sort orders always visible in the Sort & Group popover.
pub(crate) const PRIMARY_SORTS: [LibrarySort; 6] = [
    LibrarySort::LastPlayed,
    LibrarySort::Title,
    LibrarySort::Playtime,
    LibrarySort::DateAdded,
    LibrarySort::Score,
    LibrarySort::InstallSize,
];

/// Secondary sort orders housed inside "More options".
pub(crate) const SECONDARY_SORTS: [LibrarySort; 4] = [
    LibrarySort::TimesPlayed,
    LibrarySort::Rating,
    LibrarySort::ReleaseDate,
    LibrarySort::InstalledFirst,
];

/// Primary groupings always visible in the Sort & Group popover.
pub(crate) const PRIMARY_GROUPS: [Grouping; 5] = [
    Grouping::None,
    Grouping::Status,
    Grouping::Genre,
    Grouping::Store,
    Grouping::Category,
];

/// Secondary groupings housed inside "More options".
pub(crate) const SECONDARY_GROUPS: [Grouping; 7] = [
    Grouping::Mode,
    Grouping::Platform,
    Grouping::Developer,
    Grouping::Publisher,
    Grouping::Year,
    Grouping::Playtime,
    Grouping::Size,
];

/// Whether `sort` or `group` lives in the secondary "More options" drawer.
pub(crate) fn has_secondary_active(sort: LibrarySort, group: Grouping) -> bool {
    SECONDARY_SORTS.contains(&sort) || SECONDARY_GROUPS.contains(&group)
}

/// Whether `sort`'s natural order (`ViewFilter::descending == false`) runs
/// ascending (A→Z for Title, installed before uninstalled for Installed first)
/// rather than descending (newest/highest/most first for date and numeric sorts).
pub(crate) fn natural_is_ascending(sort: LibrarySort) -> bool {
    matches!(sort, LibrarySort::Title | LibrarySort::InstalledFirst)
}

/// Whether the library is currently ordered ascending for `(sort, descending)`.
pub(crate) fn sort_is_ascending(sort: LibrarySort, descending: bool) -> bool {
    if natural_is_ascending(sort) {
        !descending
    } else {
        descending
    }
}

/// The `ViewFilter::descending` flag that yields ascending (`want_ascending`)
/// or descending (`!want_ascending`) order for `sort`.
pub(crate) fn descending_for_direction(sort: LibrarySort, want_ascending: bool) -> bool {
    want_ascending != natural_is_ascending(sort)
}

impl LibraryView {
    /// The Sort & Group toolbar button and Look-style popover.
    pub(super) fn render_sort_popover(
        &mut self,
        sort: LibrarySort,
        descending: bool,
        group: Grouping,
        searching: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let order = crate::i18n::sort_label(sort);
        let sort_label = if searching {
            t("library.toolbar.sort_best_match")
        } else if descending {
            t_args("library.toolbar.sort_reversed", &[("order", &order)])
        } else {
            t_args("library.toolbar.sort", &[("order", &order)])
        };

        let secondary_active = has_secondary_active(sort, group);
        if secondary_active && self.sort_more_open.is_none() {
            self.sort_more_open = Some(true);
        }
        let more_expanded = self.sort_more_open.unwrap_or(false);
        let model = self.filters.clone();
        let view: WeakEntity<LibraryView> = cx.entity().downgrade();
        let open = self.sort_open;

        Popover::new("library-sort-popover")
            .anchor(Anchor::TopLeft)
            .open(open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                if this.sort_open != *open {
                    this.sort_open = *open;
                    if !*open {
                        this.sort_more_open = None;
                    }
                    cx.notify();
                }
            }))
            .trigger(
                Button::new("library-sort")
                    .debug_selector(|| "library-sort-trigger".into())
                    .small()
                    .outline()
                    .icon(IconName::ArrowDownUp)
                    .label(sort_label)
                    .dropdown_caret(true),
            )
            .content(move |_, window, cx| {
                let theme = cx.theme();
                let primary = theme.primary;
                let active_fg = active_accent(cx);
                let fg = theme.foreground;
                let muted = theme.muted_foreground;
                let border = theme.border;
                let secondary = theme.secondary;
                let radius = theme.radius;
                let visible = irori_ui::style::visible_bounds(window);
                let rem = window.rem_size();
                let max_h = (visible.size.height - rem * 12.).max(rem * 12.);

                let is_ascending = sort_is_ascending(sort, descending);
                let asc_model = model.clone();
                let desc_model = model.clone();
                let direction = div().w(rems(8.5)).flex_none().child(segmented_bar(
                    [
                        Segment::new(
                            "sort-dir-asc",
                            t("library.sort.asc"),
                            is_ascending,
                            move |cx| {
                                asc_model.update(cx, |m, cx| {
                                    m.update(
                                        |f| f.descending = descending_for_direction(f.sort, true),
                                        cx,
                                    )
                                });
                            },
                        ),
                        Segment::new(
                            "sort-dir-desc",
                            t("library.sort.desc"),
                            !is_ascending,
                            move |cx| {
                                desc_model.update(cx, |m, cx| {
                                    m.update(
                                        |f| f.descending = descending_for_direction(f.sort, false),
                                        cx,
                                    )
                                });
                            },
                        ),
                    ],
                    cx,
                ));

                let sort_pills = |items: &[LibrarySort]| -> AnyElement {
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap_1p5()
                        .children(items.iter().copied().map(|option| {
                            let model = model.clone();
                            toggle_pill(
                                SharedString::from(format!("sort-opt-{option:?}")),
                                crate::i18n::sort_label(option).into(),
                                option == sort,
                                move |cx| {
                                    model.update(cx, |m, cx| {
                                        m.update(
                                            |f| {
                                                if f.sort != option {
                                                    f.sort = option;
                                                    f.descending = false;
                                                }
                                            },
                                            cx,
                                        )
                                    });
                                },
                                cx,
                            )
                        }))
                        .into_any_element()
                };

                let group_pills = |items: &[Grouping]| -> AnyElement {
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap_1p5()
                        .children(items.iter().copied().map(|option| {
                            let model = model.clone();
                            toggle_pill(
                                SharedString::from(format!("group-opt-{option:?}")),
                                option.label().into(),
                                option == group,
                                move |cx| {
                                    model.update(cx, |m, cx| m.update(|f| f.group = option, cx));
                                },
                                cx,
                            )
                        }))
                        .into_any_element()
                };

                let more_body = more_expanded.then(|| {
                    v_flex()
                        .debug_selector(|| "library-sort-more-body".into())
                        .w_full()
                        .gap_2p5()
                        .p_2p5()
                        .rounded(radius)
                        .bg(secondary.opacity(0.28))
                        .border_1()
                        .border_color(border.opacity(0.65))
                        .child(
                            v_flex()
                                .gap_1p5()
                                .child(control_label(t("library.sort.more_sorts"), cx))
                                .child(sort_pills(&SECONDARY_SORTS)),
                        )
                        .child(
                            v_flex()
                                .gap_1p5()
                                .child(control_label(t("library.sort.more_groups"), cx))
                                .child(group_pills(&SECONDARY_GROUPS)),
                        )
                });

                let toggle_view = view.clone();
                v_flex()
                    .id("library-sort-panel")
                    .debug_selector(|| "library-sort-panel".into())
                    .w(rems(21.))
                    .gap_2p5()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_bold()
                                    .min_w_0()
                                    .truncate()
                                    .child(t("library.sort.title_popover")),
                            )
                            .child(direction),
                    )
                    .child(
                        v_flex().w_full().max_h(max_h).child(
                            v_flex().flex_1().overflow_hidden().child(
                                div().flex_1().overflow_hidden().child(
                                    v_flex()
                                        .id("library-sort-body")
                                        .size_full()
                                        .overflow_y_scrollbar()
                                        .pr_1()
                                        .gap_2p5()
                                        .child(section_card(
                                            t("library.sort.section.sort_by"),
                                            [sort_pills(&PRIMARY_SORTS)],
                                            cx,
                                        ))
                                        .child(section_card(
                                            t("library.toolbar.group_by"),
                                            [group_pills(&PRIMARY_GROUPS)],
                                            cx,
                                        ))
                                        .child(
                                            v_flex()
                                                .w_full()
                                                .gap_2()
                                                .child(
                                                    h_flex()
                                                        .id("sort-more-toggle")
                                                        .debug_selector(|| {
                                                            "library-sort-more-toggle".into()
                                                        })
                                                        .w_full()
                                                        .items_center()
                                                        .justify_between()
                                                        .px_2p5()
                                                        .py_1p5()
                                                        .rounded(radius)
                                                        .border_1()
                                                        .cursor_pointer()
                                                        .keyboard_focusable(cx)
                                                        .when(secondary_active, |el| {
                                                            el.bg(primary.opacity(0.12))
                                                                .border_color(primary.opacity(0.5))
                                                                .text_color(active_fg)
                                                                .font_semibold()
                                                        })
                                                        .when(!secondary_active, |el| {
                                                            el.bg(secondary.opacity(0.35))
                                                                .border_color(border)
                                                                .text_color(muted)
                                                                .font_medium()
                                                                .hover(move |s| {
                                                                    s.bg(secondary.opacity(0.75))
                                                                        .text_color(fg)
                                                                })
                                                        })
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .min_w_0()
                                                                .truncate()
                                                                .child(t(
                                                                    "library.sort.more_options",
                                                                )),
                                                        )
                                                        .child(
                                                            Icon::new(if more_expanded {
                                                                IconName::ChevronDown
                                                            } else {
                                                                IconName::ChevronRight
                                                            })
                                                            .xsmall()
                                                            .flex_shrink_0(),
                                                        )
                                                        .on_click(move |_, _, cx| {
                                                            toggle_view
                                                                .update(cx, |this, cx| {
                                                                    this.sort_more_open =
                                                                        Some(!more_expanded);
                                                                    cx.notify();
                                                                })
                                                                .ok();
                                                        }),
                                                )
                                                .children(more_body),
                                        ),
                                ),
                            ),
                        ),
                    )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_and_secondary_partition_all_sorts_and_groups() {
        let mut sorts: Vec<LibrarySort> =
            PRIMARY_SORTS.into_iter().chain(SECONDARY_SORTS).collect();
        assert_eq!(sorts.len(), LibrarySort::ALL.len());
        for s in LibrarySort::ALL {
            assert!(sorts.contains(&s), "missing sort {s:?}");
            assert!(
                !(PRIMARY_SORTS.contains(&s) && SECONDARY_SORTS.contains(&s)),
                "duplicate sort {s:?}"
            );
        }
        sorts.clear();

        let groups: Vec<Grouping> = PRIMARY_GROUPS.into_iter().chain(SECONDARY_GROUPS).collect();
        assert_eq!(groups.len(), Grouping::ALL.len());
        for g in Grouping::ALL {
            assert!(groups.contains(&g), "missing grouping {g:?}");
            assert!(
                !(PRIMARY_GROUPS.contains(&g) && SECONDARY_GROUPS.contains(&g)),
                "duplicate grouping {g:?}"
            );
        }
        assert!(!has_secondary_active(LibrarySort::Title, Grouping::None));
        assert!(has_secondary_active(LibrarySort::Rating, Grouping::None));
        assert!(has_secondary_active(
            LibrarySort::Title,
            Grouping::Developer
        ));
    }

    #[test]
    fn direction_mapping_matches_each_sort_orders_natural_direction() {
        // Title and InstalledFirst are naturally ascending (descending == false).
        for asc_natural in [LibrarySort::Title, LibrarySort::InstalledFirst] {
            assert!(sort_is_ascending(asc_natural, false));
            assert!(!sort_is_ascending(asc_natural, true));
            assert!(!descending_for_direction(asc_natural, true));
            assert!(descending_for_direction(asc_natural, false));
        }
        // Date, numeric, and score sorts are naturally descending (highest/newest first
        // when descending == false; lowest/oldest first when descending == true).
        for desc_natural in [
            LibrarySort::LastPlayed,
            LibrarySort::Playtime,
            LibrarySort::InstallSize,
            LibrarySort::ReleaseDate,
            LibrarySort::DateAdded,
            LibrarySort::Score,
            LibrarySort::TimesPlayed,
            LibrarySort::Rating,
        ] {
            assert!(!sort_is_ascending(desc_natural, false));
            assert!(sort_is_ascending(desc_natural, true));
            assert!(descending_for_direction(desc_natural, true));
            assert!(!descending_for_direction(desc_natural, false));
        }
        // Round-trip every sort order with both directions.
        for sort in LibrarySort::ALL {
            for want_asc in [true, false] {
                let flag = descending_for_direction(sort, want_asc);
                assert_eq!(sort_is_ascending(sort, flag), want_asc, "{sort:?}");
            }
        }
    }
}
