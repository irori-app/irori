//! The layout-aware Look popover in the library toolbar.
//!
//! Shows the essential controls for the active layout (Grid, Focus, or List)
//! in a compact layout so the full popover—including Background—fits comfortably
//! without clipping. Per-tab controls (size and cover shape) save automatically
//! on the active tab; global preferences stay synced with Settings → Appearance.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    div, rems, transparent_black,
};
use irori_core::LibraryTopSection;
use irori_core::settings::{
    CoverCaption, CoverCorners, CoverShape, FocusCoverSize, LibraryBackground, LibraryLayout,
    ListRowSize, UiPrefs,
};
use irori_ui::i18n::t;
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::{backend, models};

use super::{LibraryView, layout_button, persist_ui};

/// Persist `AppSettings::library_top_section` off the UI thread.
fn persist_top_section(cx: &App, section: LibraryTopSection) {
    let b = backend(cx);
    irori_ui::state::run_infallible(cx, async move {
        b.settings
            .update_app(move |s| s.library_top_section = section)
    })
    .detach();
}

/// Whether the Look popover offers `background`: a picture needs choosing
/// first, in Settings, unless it is already the one shown.
pub(super) fn offers_background(ui: &UiPrefs, background: LibraryBackground) -> bool {
    background != LibraryBackground::Picture
        || !ui.library_background_picture.trim().is_empty()
        || ui.library_background == background
}

/// Readable primary accent text colour on the popover surface.
pub(super) fn active_accent(cx: &App) -> Hsla {
    let theme = cx.theme();
    legible(theme.primary, &[theme.popover], TEXT_CONTRAST, cx)
}

fn card_heading(label: SharedString, hint: Option<SharedString>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(muted)
                .child(label),
        )
        .when_some(hint, |el, text| {
            el.child(div().text_xs().text_color(muted.opacity(0.8)).child(text))
        })
        .into_any_element()
}

pub(super) fn control_label(label: SharedString, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(label)
        .into_any_element()
}

pub(super) fn section_card(
    title: SharedString,
    children: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> AnyElement {
    section_card_with_hint(title, None, children, cx)
}

fn section_card_with_hint(
    title: SharedString,
    hint: Option<SharedString>,
    children: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .p_2()
        .gap_2()
        .rounded(theme.radius)
        .bg(theme.secondary.opacity(0.32))
        .border_1()
        .border_color(theme.border.opacity(0.65))
        .child(card_heading(title, hint, cx))
        .children(children)
        .into_any_element()
}

pub(super) fn separator(cx: &App) -> AnyElement {
    div()
        .w_full()
        .h(gpui_kit::px(1.))
        .bg(cx.theme().border)
        .into_any_element()
}

/// One segment inside [`segmented_bar`].
pub(super) struct Segment {
    id: &'static str,
    label: SharedString,
    selected: bool,
    on_click: Box<dyn Fn(&mut App)>,
}

impl Segment {
    pub(super) fn new(
        id: &'static str,
        label: SharedString,
        selected: bool,
        on_click: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self {
            id,
            label,
            selected,
            on_click: Box::new(on_click),
        }
    }
}

/// Single-line segmented control with unmistakable accent styling on the
/// active option.
pub(super) fn segmented_bar(segments: impl IntoIterator<Item = Segment>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;
    let active_fg = active_accent(cx);
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let hover_bg = theme.secondary.opacity(0.8);
    let rail_bg = theme.secondary.opacity(0.45);
    let border = theme.border;
    let radius = theme.radius;

    h_flex()
        .w_full()
        .p_0p5()
        .gap_1()
        .rounded(radius)
        .bg(rail_bg)
        .border_1()
        .border_color(border)
        .children(segments.into_iter().map(move |seg| {
            let on_click = seg.on_click;
            let debug_id = seg.id;
            h_flex()
                .id(seg.id)
                .debug_selector(move || debug_id.into())
                .flex_auto()
                .min_w_0()
                .justify_center()
                .items_center()
                .px_1p5()
                .py_1()
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .keyboard_focusable(cx)
                .text_xs()
                .when(seg.selected, |el| {
                    el.bg(primary.opacity(0.18))
                        .border_color(primary)
                        .text_color(active_fg)
                        .font_semibold()
                })
                .when(!seg.selected, |el| {
                    el.border_color(transparent_black())
                        .text_color(muted)
                        .hover(move |s| s.bg(hover_bg).text_color(fg))
                })
                .child(div().min_w_0().truncate().child(seg.label))
                .on_click(move |_, _, cx| on_click(cx))
        }))
        .into_any_element()
}

/// Multi-select accent-highlighted pill (for cover badges, list columns, and Sort/Group).
pub(super) fn toggle_pill(
    id: impl Into<SharedString>,
    label: SharedString,
    selected: bool,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;
    let active_fg = active_accent(cx);
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let border = theme.border;
    let idle_bg = theme.secondary.opacity(0.45);
    let hover_bg = theme.secondary.opacity(0.85);
    let id: SharedString = id.into();
    let debug_id = id.to_string();

    h_flex()
        .id(id)
        .debug_selector(move || debug_id.clone())
        .max_w_full()
        .min_w_0()
        .items_center()
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .keyboard_focusable(cx)
        .text_xs()
        .when(selected, |el| {
            el.bg(primary.opacity(0.18))
                .border_color(primary)
                .text_color(active_fg)
                .font_semibold()
        })
        .when(!selected, |el| {
            el.bg(idle_bg)
                .border_color(border)
                .text_color(muted)
                .hover(move |s| s.bg(hover_bg).text_color(fg))
        })
        .child(div().min_w_0().truncate().child(label))
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

fn background_control(ui: &UiPrefs, cx: &App) -> AnyElement {
    let current = ui.library_background;
    let items: [(LibraryBackground, &'static str, &'static str); 3] = [
        (
            LibraryBackground::None,
            "look-bg-none",
            "library.look.background_none",
        ),
        (
            LibraryBackground::GameArt,
            "look-bg-art",
            "library.look.background_art",
        ),
        (
            LibraryBackground::Picture,
            "look-bg-picture",
            "library.look.background_picture",
        ),
    ];
    let segments = items
        .into_iter()
        .filter(|&(value, _, _)| offers_background(ui, value))
        .map(|(value, id, key)| {
            Segment::new(id, t(key), current == value, move |cx| {
                persist_ui(cx, move |u| u.library_background = value);
            })
        });
    v_flex()
        .debug_selector(|| "look-section-background".into())
        .gap_1()
        .child(control_label(t("library.look.background"), cx))
        .child(segmented_bar(segments, cx))
        .into_any_element()
}

fn top_section_control(current: LibraryTopSection, cx: &App) -> AnyElement {
    let items: [(LibraryTopSection, &'static str, &'static str); 4] = [
        (
            LibraryTopSection::RecentlyPlayed,
            "look-top-recent",
            "library.look.top_recent",
        ),
        (
            LibraryTopSection::RecentlyPlayedInstalled,
            "look-top-installed",
            "library.look.top_installed",
        ),
        (
            LibraryTopSection::Favourites,
            "look-top-favourites",
            "library.look.top_favourites",
        ),
        (
            LibraryTopSection::Disabled,
            "look-top-none",
            "library.look.top_none",
        ),
    ];
    v_flex()
        .gap_1()
        .child(control_label(t("library.look.top_of_page"), cx))
        .child(segmented_bar(
            items.into_iter().map(|(value, id, key)| {
                Segment::new(id, t(key), current == value, move |cx| {
                    persist_top_section(cx, value);
                })
            }),
            cx,
        ))
        .into_any_element()
}

fn caption_control(current: CoverCaption, cx: &App) -> AnyElement {
    let items: [(CoverCaption, &'static str, &'static str); 3] = [
        (
            CoverCaption::Full,
            "look-cap-full",
            "library.look.caption_full",
        ),
        (
            CoverCaption::Title,
            "look-cap-title",
            "library.look.caption_title",
        ),
        (
            CoverCaption::None,
            "look-cap-none",
            "library.look.caption_none",
        ),
    ];
    v_flex()
        .gap_1()
        .child(control_label(t("library.look.under_covers"), cx))
        .child(segmented_bar(
            items.into_iter().map(|(value, id, key)| {
                Segment::new(id, t(key), current == value, move |cx| {
                    persist_ui(cx, move |u| u.cover_caption = value);
                })
            }),
            cx,
        ))
        .into_any_element()
}

fn corners_control(ui: &UiPrefs, cx: &App) -> AnyElement {
    let corners = ui.cover_corners;
    let corner_items: [(CoverCorners, &'static str, &'static str); 3] = [
        (
            CoverCorners::Square,
            "look-corners-square",
            "library.look.corners_square",
        ),
        (
            CoverCorners::Rounded,
            "look-corners-rounded",
            "library.look.corners_rounded",
        ),
        (
            CoverCorners::Round,
            "look-corners-round",
            "library.look.corners_round",
        ),
    ];
    v_flex()
        .gap_1()
        .child(control_label(t("library.look.corners"), cx))
        .child(segmented_bar(
            corner_items.into_iter().map(|(value, id, key)| {
                Segment::new(id, t(key), corners == value, move |cx| {
                    persist_ui(cx, move |u| u.cover_corners = value);
                })
            }),
            cx,
        ))
        .into_any_element()
}

fn on_covers_pills(ui: &UiPrefs, include_grid_only: bool, cx: &App) -> AnyElement {
    let mut pills = vec![
        toggle_pill(
            "look-star",
            t("library.look.star"),
            ui.cover_favourite,
            |cx| persist_ui(cx, |u| u.cover_favourite = !u.cover_favourite),
            cx,
        ),
        toggle_pill(
            "look-badges",
            t("library.look.badges"),
            ui.show_store_badges,
            |cx| persist_ui(cx, |u| u.show_store_badges = !u.show_store_badges),
            cx,
        ),
    ];
    if ui.show_store_badges {
        pills.push(toggle_pill(
            "look-badges-always",
            t("library.look.badges_always_short"),
            ui.store_badges_always,
            |cx| persist_ui(cx, |u| u.store_badges_always = !u.store_badges_always),
            cx,
        ));
    }
    if include_grid_only {
        pills.push(toggle_pill(
            "look-compat-badges",
            t("library.look.compat_badges"),
            ui.cover_compat_badges,
            |cx| persist_ui(cx, |u| u.cover_compat_badges = !u.cover_compat_badges),
            cx,
        ));
    }
    h_flex()
        .w_full()
        .flex_wrap()
        .gap_1p5()
        .children(pills)
        .into_any_element()
}

struct TabLookState {
    cover_shape: CoverShape,
    list_row_size: ListRowSize,
    focus_cover_size: FocusCoverSize,
    per_tab: bool,
}

fn build_look_body(
    layout: LibraryLayout,
    tab_state: &TabLookState,
    ui: &UiPrefs,
    top_section: LibraryTopSection,
    tile_slider: &Entity<SliderState>,
    view: &WeakEntity<LibraryView>,
    cx: &App,
) -> Vec<AnyElement> {
    let muted = cx.theme().muted_foreground;
    let tab_hint = tab_state.per_tab.then(|| t("library.look.per_tab_hint"));
    match layout {
        LibraryLayout::Grid => {
            let shape = tab_state.cover_shape;
            let shapes: [(CoverShape, &'static str, &'static str); 3] = [
                (
                    CoverShape::Portrait,
                    "look-shape-portrait",
                    "library.look.shape_portrait",
                ),
                (
                    CoverShape::Square,
                    "look-shape-square",
                    "library.look.shape_square",
                ),
                (
                    CoverShape::Banner,
                    "look-shape-banner",
                    "library.look.shape_banner",
                ),
            ];
            let covers = vec![
                v_flex()
                    .debug_selector(|| "look-section-tile-size".into())
                    .gap_1()
                    .child(control_label(t("library.toolbar.tile_size"), cx))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_1()
                            .child(Icon::new(IconName::Grid3x3).xsmall().text_color(muted))
                            .child(
                                div()
                                    .debug_selector(|| "look-tile-slider-wrap".into())
                                    .flex_1()
                                    .min_w_0()
                                    .px_3()
                                    .py_0p5()
                                    .child(
                                        Slider::new(tile_slider)
                                            .accessibility_label(t("library.toolbar.tile_size"))
                                            .w_full(),
                                    ),
                            )
                            .child(Icon::new(IconName::Grid2x2).xsmall().text_color(muted)),
                    )
                    .into_any_element(),
                v_flex()
                    .gap_1()
                    .child(control_label(t("library.look.shape"), cx))
                    .child(segmented_bar(
                        shapes.into_iter().map(|(value, id, key)| {
                            let view = view.clone();
                            Segment::new(id, t(key), shape == value, move |cx| {
                                view.update(cx, |this, cx| this.set_cover_shape(value, cx))
                                    .ok();
                            })
                        }),
                        cx,
                    ))
                    .into_any_element(),
                corners_control(ui, cx),
            ];

            let details = vec![
                caption_control(ui.cover_caption, cx),
                on_covers_pills(ui, true, cx),
            ];
            let page = vec![
                top_section_control(top_section, cx),
                background_control(ui, cx),
            ];

            vec![
                section_card_with_hint(t("library.look.section.covers"), tab_hint, covers, cx),
                section_card(t("library.look.on_covers"), details, cx),
                section_card(t("library.look.section.page"), page, cx),
            ]
        }
        LibraryLayout::Focus => {
            let focus_size = tab_state.focus_cover_size;
            let sizes: [(FocusCoverSize, &'static str, &'static str); 3] = [
                (
                    FocusCoverSize::Small,
                    "look-focus-small",
                    "library.look.focus_small",
                ),
                (
                    FocusCoverSize::Medium,
                    "look-focus-medium",
                    "library.look.focus_medium",
                ),
                (
                    FocusCoverSize::Large,
                    "look-focus-large",
                    "library.look.focus_large",
                ),
            ];
            let covers = vec![
                v_flex()
                    .debug_selector(|| "look-section-focus-size".into())
                    .gap_1()
                    .child(control_label(t("library.look.focus_size"), cx))
                    .child(segmented_bar(
                        sizes.into_iter().map(|(value, id, key)| {
                            let view = view.clone();
                            Segment::new(id, t(key), focus_size == value, move |cx| {
                                view.update(cx, |this, cx| this.set_focus_cover_size(value, cx))
                                    .ok();
                            })
                        }),
                        cx,
                    ))
                    .into_any_element(),
                corners_control(ui, cx),
            ];
            let details = vec![
                caption_control(ui.cover_caption, cx),
                on_covers_pills(ui, false, cx),
            ];

            vec![
                section_card_with_hint(t("library.look.section.covers"), tab_hint, covers, cx),
                section_card(t("library.look.on_covers"), details, cx),
                section_card(
                    t("library.look.section.page"),
                    [background_control(ui, cx)],
                    cx,
                ),
            ]
        }
        LibraryLayout::List => {
            let row_size = tab_state.list_row_size;
            let row_sizes: [(ListRowSize, &'static str, &'static str); 3] = [
                (
                    ListRowSize::Compact,
                    "look-row-compact",
                    "library.look.row_size_compact",
                ),
                (
                    ListRowSize::Medium,
                    "look-row-medium",
                    "library.look.row_size_medium",
                ),
                (
                    ListRowSize::Large,
                    "look-row-large",
                    "library.look.row_size_large",
                ),
            ];
            let row_size_control = v_flex()
                .debug_selector(|| "look-section-row-size".into())
                .gap_1()
                .child(control_label(t("library.look.row_size"), cx))
                .child(segmented_bar(
                    row_sizes.into_iter().map(|(value, id, key)| {
                        let view = view.clone();
                        Segment::new(id, t(key), row_size == value, move |cx| {
                            view.update(cx, |this, cx| this.set_list_row_size(value, cx))
                                .ok();
                        })
                    }),
                    cx,
                ))
                .into_any_element();

            let cols = ui.library_list_columns;
            let columns = h_flex()
                .debug_selector(|| "look-section-columns".into())
                .w_full()
                .flex_wrap()
                .gap_1p5()
                .child(toggle_pill(
                    "look-col-store",
                    t("library.list.store"),
                    cols.store,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.store = !u.library_list_columns.store
                        })
                    },
                    cx,
                ))
                .child(toggle_pill(
                    "look-col-size",
                    t("library.list.size"),
                    cols.size,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.size = !u.library_list_columns.size
                        })
                    },
                    cx,
                ))
                .child(toggle_pill(
                    "look-col-playtime",
                    t("library.list.playtime"),
                    cols.playtime,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.playtime = !u.library_list_columns.playtime
                        })
                    },
                    cx,
                ))
                .child(toggle_pill(
                    "look-col-last-played",
                    t("library.list.last_played"),
                    cols.last_played,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.last_played = !u.library_list_columns.last_played
                        })
                    },
                    cx,
                ))
                .child(toggle_pill(
                    "look-col-status",
                    t("library.list.status"),
                    cols.status,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.status = !u.library_list_columns.status
                        })
                    },
                    cx,
                ))
                .child(toggle_pill(
                    "look-col-compat",
                    t("library.list.compat"),
                    cols.compat,
                    |cx| {
                        persist_ui(cx, |u| {
                            u.library_list_columns.compat = !u.library_list_columns.compat
                        })
                    },
                    cx,
                ))
                .into_any_element();

            vec![
                section_card_with_hint(
                    t("library.look.section.list"),
                    tab_hint,
                    [
                        row_size_control,
                        v_flex()
                            .gap_1()
                            .child(control_label(t("library.look.columns"), cx))
                            .child(columns)
                            .into_any_element(),
                    ],
                    cx,
                ),
                section_card(
                    t("library.look.section.page"),
                    [
                        top_section_control(top_section, cx),
                        background_control(ui, cx),
                    ],
                    cx,
                ),
            ]
        }
    }
}

impl LibraryView {
    /// The layout-aware Look popover button.
    pub(super) fn render_look_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let view: WeakEntity<LibraryView> = cx.entity().downgrade();
        let tile_slider = self.tile_slider.clone();
        let layout = self.layout;
        let selecting = self.selection.is_active();
        let open = self.look_open;

        Popover::new("library-look-popover")
            .anchor(Anchor::TopRight)
            .open(open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                if this.look_open != *open {
                    this.look_open = *open;
                    cx.notify();
                }
            }))
            .trigger(
                Button::new("library-look")
                    .debug_selector(|| "library-look-trigger".into())
                    .small()
                    .outline()
                    .icon(IconName::Palette)
                    .label(t("library.look.button"))
                    .tooltip(t("library.look.tooltip")),
            )
            .content(move |_, window, cx| {
                let app = models(cx).settings.read(cx).app.clone();
                let ui = &app.ui;
                let top_section = app.library_top_section;
                let tab_state = view
                    .upgrade()
                    .map(|v| {
                        let lib = v.read(cx);
                        TabLookState {
                            cover_shape: lib.cover_shape,
                            list_row_size: lib.list_row_size,
                            focus_cover_size: lib.focus_cover_size,
                            per_tab: lib.active_tab.is_some(),
                        }
                    })
                    .unwrap_or(TabLookState {
                        cover_shape: ui.cover_shape,
                        list_row_size: ui.library_list_row_size,
                        focus_cover_size: ui.library_focus_cover_size,
                        per_tab: false,
                    });
                let (_, layout_icon, layout_name) = layout_button(layout);
                let theme = cx.theme();
                let primary = theme.primary;
                let active_fg = active_accent(cx);
                let visible = irori_ui::style::visible_bounds(window);
                let rem = window.rem_size();
                let max_h = (visible.size.height - rem * 11.).max(rem * 12.);
                let select_view = view.clone();
                let more_view = view.clone();
                let body_items =
                    build_look_body(layout, &tab_state, ui, top_section, &tile_slider, &view, cx);

                v_flex()
                    .id("library-look-panel")
                    .debug_selector(|| "library-look-panel".into())
                    .w(rems(22.))
                    .gap_2()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(div().text_sm().font_bold().child(t("library.look.button")))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(primary.opacity(0.18))
                                    .border_1()
                                    .border_color(primary.opacity(0.45))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(active_fg)
                                    .child(Icon::new(layout_icon).xsmall())
                                    .child(layout_name),
                            ),
                    )
                    .child(
                        v_flex().w_full().max_h(max_h).child(
                            v_flex().flex_1().overflow_hidden().child(
                                div().flex_1().overflow_hidden().child(
                                    v_flex()
                                        .id("library-look-body")
                                        .debug_selector(|| "library-look-body".into())
                                        .size_full()
                                        .overflow_y_scrollbar()
                                        .pr_2()
                                        .gap_2()
                                        .children(body_items),
                                ),
                            ),
                        ),
                    )
                    .child(separator(cx))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child({
                                let btn = Button::new("library-look-select")
                                    .xsmall()
                                    .icon(IconName::ListChecks)
                                    .label(if selecting {
                                        t("library.bulk.done")
                                    } else {
                                        t("library.look.select")
                                    })
                                    .on_click(move |_, _, cx| {
                                        select_view
                                            .update(cx, |this, cx| {
                                                this.look_open = false;
                                                this.toggle_selection_mode(cx);
                                            })
                                            .ok();
                                    });
                                if selecting {
                                    btn.primary()
                                } else {
                                    btn.outline()
                                }
                            })
                            .child(
                                Button::new("library-look-more")
                                    .xsmall()
                                    .ghost()
                                    .label(t("library.look.more"))
                                    .on_click(move |_, _, cx| {
                                        more_view
                                            .update(cx, |this, cx| {
                                                this.look_open = false;
                                                cx.notify();
                                            })
                                            .ok();
                                        irori_ui::navigate(
                                            irori_ui::Route::Settings("appearance".into()),
                                            cx,
                                        );
                                    }),
                            ),
                    )
            })
            .into_any_element()
    }
}
