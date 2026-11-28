//! Colour and icon pickers for collections and category look editors.
//!
//! Keeps [`super::collection`] focused on tag/manual game logic while sharing
//! the 14 curated colour swatches and Hue [`ColorWheel`] setup with
//! [`super::categories`].

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, rems,
    transparent_black,
};
use irori_services::smart_categories::COLOURS;
use irori_ui::categories::{QUICK_ICONS, filter_icons, is_custom_hex_colour, raw_colour_hex};
use irori_ui::components::color_wheel::hsla_to_hex6;
use irori_ui::components::{ColorWheel, ColorWheelSwatch};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};

use super::text;

/// Build the 14 theme-aware curated swatches displayed inside the Hue wheel popover.
pub(crate) fn collection_quick_swatches(cx: &App) -> Vec<ColorWheelSwatch> {
    let muted = cx.theme().muted_foreground;
    COLOURS
        .into_iter()
        .map(|c| {
            let color = irori_ui::categories::colour(Some(c), cx).unwrap_or(muted);
            ColorWheelSwatch::named(
                SharedString::from(format!("collection-wheel-swatch-{c}")),
                c,
                color,
                irori_ui::categories::colour_label(c),
            )
        })
        .collect()
}

/// Resolve an initial uppercase `RRGGBB` hex for seeding the Hue wheel.
pub(crate) fn initial_wheel_hex(colour: Option<&str>, cx: &App) -> String {
    raw_colour_hex(colour, cx).unwrap_or_else(|| hsla_to_hex6(cx.theme().primary))
}

/// Circular trigger button at the end of the colour row that opens the Hue wheel popover.
#[derive(IntoElement)]
struct CustomColourTrigger {
    custom_colour: Option<Hsla>,
    tooltip: SharedString,
    selected: bool,
}

impl CustomColourTrigger {
    fn new(custom_colour: Option<Hsla>, tooltip: SharedString) -> Self {
        Self {
            custom_colour,
            tooltip,
            selected: false,
        }
    }
}

impl Selectable for CustomColourTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for CustomColourTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = if self.selected || self.custom_colour.is_some() {
            theme.primary
        } else {
            theme.border
        };
        let muted = theme.muted_foreground;
        let tip = self.tooltip;

        div()
            .id("collection-colour-custom")
            .debug_selector(|| "collection-colour-custom".into())
            .size(rems(1.25))
            .rounded_full()
            .border_2()
            .border_color(border)
            .when_some(self.custom_colour, |d, fill| d.bg(fill))
            .when(self.custom_colour.is_none(), |d| {
                d.bg(theme.secondary.opacity(0.45))
                    .child(Icon::new(IconName::Palette).xsmall().text_color(muted))
            })
            .cursor_pointer()
            .keyboard_focusable(cx)
            .flex()
            .items_center()
            .justify_center()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
    }
}

/// Square tile trigger at the end of the quick icon row that opens the searchable icon popover.
#[derive(IntoElement)]
struct MoreIconsTrigger {
    tooltip: SharedString,
    selected: bool,
}

impl MoreIconsTrigger {
    fn new(tooltip: SharedString) -> Self {
        Self {
            tooltip,
            selected: false,
        }
    }
}

impl Selectable for MoreIconsTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for MoreIconsTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = if self.selected {
            theme.primary
        } else {
            theme.border
        };
        let idle_bg = theme.secondary.opacity(0.32);
        let hover_bg = theme.secondary.opacity(0.75);
        let radius = theme.radius;
        let tip = self.tooltip;

        div()
            .id("collection-icon-more")
            .debug_selector(|| "collection-icon-more".into())
            .size(rems(1.8))
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(if self.selected { hover_bg } else { idle_bg })
            .text_color(if self.selected { fg } else { muted })
            .hover(move |s| s.bg(hover_bg).text_color(fg))
            .cursor_pointer()
            .keyboard_focusable(cx)
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(IconName::Plus).small())
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
    }
}

type IconPickCallback = Rc<dyn Fn(&'static str, &mut Window, &mut App)>;

/// Searchable popover displaying all 64 curated collection icons.
pub(crate) struct IconPickerPopover {
    search: Entity<InputState>,
    selected_slug: String,
    tint_colour: Option<String>,
    on_pick: IconPickCallback,
    _subscriptions: Vec<Subscription>,
}

impl IconPickerPopover {
    pub(crate) fn new(
        selected_slug: Option<&str>,
        tint_colour: Option<String>,
        on_pick: impl Fn(&'static str, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("library.collection.icon_search_placeholder"))
        });
        let sub = cx.subscribe_in(&search, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            search,
            selected_slug: selected_slug.unwrap_or("sparkles").to_owned(),
            tint_colour,
            on_pick: Rc::new(on_pick),
            _subscriptions: vec![sub],
        }
    }

    pub(crate) fn sync(
        &mut self,
        selected_slug: Option<&str>,
        tint_colour: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.selected_slug = selected_slug.unwrap_or("sparkles").to_owned();
        self.tint_colour = tint_colour;
        cx.notify();
    }

    pub(crate) fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search.read(cx).value().is_empty() {
            self.search
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
        }
    }
}

impl Render for IconPickerPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = text(&self.search, cx);
        let matches = filter_icons(&query);
        let theme = cx.theme();
        let primary = theme.primary;
        let tint = irori_ui::categories::colour(self.tint_colour.as_deref(), cx).unwrap_or(primary);
        let active_fg = legible(tint, &[theme.background], TEXT_CONTRAST, cx);
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let border = theme.border;
        let idle_bg = theme.secondary.opacity(0.32);
        let hover_bg = theme.secondary.opacity(0.75);
        let radius = theme.radius;

        v_flex()
            .id("collection-icon-popover-body")
            .debug_selector(|| "collection-icon-popover-body".into())
            .w(rems(19.5))
            .gap_2()
            .child(
                Input::new(&self.search)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).xsmall()),
            )
            .child(if matches.is_empty() {
                div()
                    .w_full()
                    .py_4()
                    .text_xs()
                    .text_color(muted)
                    .flex()
                    .justify_center()
                    .child(t_args(
                        "library.collection.no_icons_match",
                        &[("query", query.as_str())],
                    ))
                    .into_any_element()
            } else {
                // Keep `max_h` outside `overflow_y_scrollbar()`: `Scrollable`
                // retains `max_size` on its inner content element, which caps
                // content height to the viewport and disables vertical scroll.
                v_flex()
                    .debug_selector(|| "collection-icon-popover-viewport".into())
                    .w_full()
                    .max_h(rems(13.5))
                    .child(
                        v_flex().flex_1().w_full().overflow_hidden().child(
                            div().flex_1().w_full().overflow_hidden().child(
                                v_flex()
                                    .id("collection-icon-popover-grid")
                                    .debug_selector(|| "collection-icon-popover-grid".into())
                                    .size_full()
                                    .overflow_y_scrollbar()
                                    .pr_3()
                                    .child(h_flex().w_full().flex_wrap().gap_1().children(
                                        matches.into_iter().map(|entry| {
                                            let slug = entry.slug;
                                            let selected = self.selected_slug == slug;
                                            let on_pick = self.on_pick.clone();
                                            div()
                                                .id(SharedString::from(format!(
                                                    "collection-more-icon-{slug}"
                                                )))
                                                .debug_selector(move || {
                                                    format!("collection-more-icon-{slug}")
                                                })
                                                .size(rems(1.8))
                                                .rounded(radius)
                                                .border_1()
                                                .cursor_pointer()
                                                .keyboard_focusable(cx)
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(selected, |el| {
                                                    el.bg(tint.opacity(0.2))
                                                        .border_color(tint)
                                                        .text_color(active_fg)
                                                })
                                                .when(!selected, |el| {
                                                    el.bg(idle_bg)
                                                        .border_color(border)
                                                        .text_color(muted)
                                                        .hover(move |s| {
                                                            s.bg(hover_bg).text_color(fg)
                                                        })
                                                })
                                                .child(Icon::new(entry.icon).small())
                                                .on_click(move |_, window, cx| {
                                                    on_pick(slug, window, cx);
                                                })
                                        }),
                                    )),
                            ),
                        ),
                    )
                    .into_any_element()
            })
    }
}

/// Render the collection colour picker row: "No colour", 14 curated swatches,
/// and the Hue wheel popover trigger for custom `#rrggbb` colours.
#[allow(clippy::too_many_arguments)] // Renders a stateless view fragment from caller-owned state.
pub(crate) fn render_colour_picker<V: 'static>(
    colour: Option<&str>,
    popover_open: bool,
    color_wheel: Entity<ColorWheel>,
    on_clear: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    on_named: impl Fn(&mut V, &'static str, &mut Window, &mut Context<V>) + Copy + 'static,
    on_popover_toggle: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let theme = cx.theme();
    let border = theme.border;
    let primary = theme.primary;
    let muted = theme.muted_foreground;
    let none_selected = colour.is_none();
    let custom_colour = is_custom_hex_colour(colour)
        .then(|| irori_ui::categories::colour(colour, cx))
        .flatten();
    let custom_tip = if let Some(raw) = colour.filter(|c| is_custom_hex_colour(Some(c))) {
        irori_ui::categories::colour_label(raw)
    } else {
        t("library.collection.custom_colour")
    };
    let none_tip = t("ui.category.colour.none");

    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(muted)
                .child(t("library.categories.colour")),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .id("collection-colour-none")
                        .debug_selector(|| "collection-colour-none".into())
                        .size(rems(1.25))
                        .rounded_full()
                        .border_2()
                        .border_color(if none_selected { primary } else { border })
                        .cursor_pointer()
                        .keyboard_focusable(cx)
                        .flex()
                        .items_center()
                        .justify_center()
                        .tooltip(move |window, cx| Tooltip::new(none_tip.clone()).build(window, cx))
                        .child(Icon::new(IconName::X).xsmall().text_color(muted))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            on_clear(this, window, cx);
                        })),
                )
                .children(COLOURS.into_iter().map(|c| {
                    let selected = colour == Some(c);
                    let swatch = irori_ui::categories::colour(Some(c), cx).unwrap_or(muted);
                    let tip = irori_ui::categories::colour_label(c);
                    div()
                        .id(SharedString::from(format!("collection-colour-{c}")))
                        .debug_selector(move || format!("collection-colour-{c}"))
                        .size(rems(1.25))
                        .rounded_full()
                        .bg(swatch)
                        .border_2()
                        .border_color(if selected {
                            primary
                        } else {
                            transparent_black()
                        })
                        .cursor_pointer()
                        .keyboard_focusable(cx)
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            on_named(this, c, window, cx);
                        }))
                }))
                .child(
                    Popover::new("collection-colour-wheel-popover")
                        .anchor(Anchor::TopLeft)
                        .open(popover_open)
                        .trigger(CustomColourTrigger::new(custom_colour, custom_tip))
                        .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                            on_popover_toggle(this, *open, window, cx);
                        }))
                        .content(move |_, _, _| color_wheel.clone()),
                ),
        )
        .into_any_element()
}

/// Render the quick icon row (18 curated icons, plus the active extended icon
/// if chosen outside the quick set) and the searchable "More icons…" popover.
#[allow(clippy::too_many_arguments)] // Renders a stateless view fragment from caller-owned state.
pub(crate) fn render_icon_picker<V: 'static>(
    colour: Option<&str>,
    icon: Option<&str>,
    popover_open: bool,
    icon_popover: Entity<IconPickerPopover>,
    on_pick: impl Fn(&mut V, &str, &mut Window, &mut Context<V>) + Copy + 'static,
    on_popover_toggle: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;
    let tint = irori_ui::categories::colour(colour, cx).unwrap_or(primary);
    let active_fg = legible(tint, &[theme.background], TEXT_CONTRAST, cx);
    let muted = theme.muted_foreground;
    let fg = theme.foreground;
    let border = theme.border;
    let idle_bg = theme.secondary.opacity(0.32);
    let hover_bg = theme.secondary.opacity(0.75);
    let radius = theme.radius;
    let active_slug = icon.unwrap_or("sparkles");

    // Keep the quick bar on a single tidy row; if the active icon came from the
    // extended catalog, surface it right after `sparkles` so selection stays visible.
    let mut visible_slugs: Vec<String> = QUICK_ICONS.into_iter().map(str::to_owned).collect();
    if !QUICK_ICONS.contains(&active_slug) {
        visible_slugs.insert(1, active_slug.to_owned());
    }

    v_flex()
        .w_full()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(muted)
                .child(t("library.collection.icon")),
        )
        .child(
            h_flex()
                .id("collection-icon-picker")
                .debug_selector(|| "collection-icon-picker".into())
                .w_full()
                .flex_wrap()
                .gap_1()
                .children(visible_slugs.into_iter().map(|slug| {
                    let selected = active_slug == slug;
                    let glyph =
                        irori_ui::categories::icon_by_name(Some(slug.as_str()), IconName::Sparkles);
                    let id_slug = slug.clone();
                    let dbg_slug = slug.clone();
                    div()
                        .id(SharedString::from(format!("collection-icon-{id_slug}")))
                        .debug_selector(move || format!("collection-icon-{dbg_slug}"))
                        .size(rems(1.8))
                        .rounded(radius)
                        .border_1()
                        .cursor_pointer()
                        .keyboard_focusable(cx)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |el| {
                            el.bg(tint.opacity(0.2))
                                .border_color(tint)
                                .text_color(active_fg)
                        })
                        .when(!selected, |el| {
                            el.bg(idle_bg)
                                .border_color(border)
                                .text_color(muted)
                                .hover(move |s| s.bg(hover_bg).text_color(fg))
                        })
                        .child(Icon::new(glyph).small())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            on_pick(this, &slug, window, cx);
                        }))
                }))
                .child(
                    Popover::new("collection-icon-popover")
                        .anchor(Anchor::TopRight)
                        .open(popover_open)
                        .trigger(MoreIconsTrigger::new(t("library.collection.more_icons")))
                        .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                            on_popover_toggle(this, *open, window, cx);
                        }))
                        .content(move |_, _, _| icon_popover.clone()),
                ),
        )
        .into_any_element()
}
