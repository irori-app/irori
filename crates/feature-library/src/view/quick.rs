//! The library header's quick controls: All / Installed / Not installed and
//! favourites only, one click each. They sit between the title and the
//! header's buttons and join, in that order, while the measured room allows
//! (`crate::quick`). They change the same criteria as the Filters popover,
//! which stays; `iroriUi.libraryQuickControls` turns them off. The Sort menu
//! shared with the toolbar lives here too.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Selectable as _, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, WeakEntity, Window, canvas, div,
};
use irori_services::LibrarySort;
use irori_ui::i18n::t;

use super::LibraryView;
use crate::filter::{InstallFilter, ViewFilter};
use crate::filters_panel::FilterModel;
use crate::quick::QuickControl;

/// The install choices, in the order the Filters popover lists them.
const INSTALLS: [(InstallFilter, &str, &str); 3] = [
    (
        InstallFilter::Any,
        "quick-install-all",
        "library.filters.install.all",
    ),
    (
        InstallFilter::Installed,
        "quick-install-installed",
        "library.filters.install.installed",
    ),
    (
        InstallFilter::NotInstalled,
        "quick-install-not-installed",
        "library.filters.install.not_installed",
    ),
];

/// The sort fields, then "Reverse order", for the toolbar's Sort menu.
pub(super) fn sort_menu(
    mut menu: PopupMenu,
    model: &Entity<FilterModel>,
    sort: LibrarySort,
    descending: bool,
) -> PopupMenu {
    for option in LibrarySort::ALL {
        let model = model.clone();
        menu = menu.item(
            PopupMenuItem::new(crate::i18n::sort_label(option))
                .checked(option == sort)
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.update(|f| f.sort = option, cx))
                }),
        );
    }
    let model = model.clone();
    menu.separator().item(
        PopupMenuItem::new(t("library.toolbar.reverse"))
            .checked(descending)
            .on_click(move |_, _, cx| reverse(&model, cx)),
    )
}

fn reverse(model: &Entity<FilterModel>, cx: &mut App) {
    model.update(cx, |m, cx| m.update(|f| f.descending = !f.descending, cx))
}

impl LibraryView {
    /// The quick controls that fit, in a strip that takes the header's free
    /// room; `None` while the setting is off.
    pub(super) fn render_quick_controls(
        &mut self,
        filter: &ViewFilter,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.ui_prefs.library_quick_controls {
            self.quick_shown = &[];
            return None;
        }
        self.quick.follow_language(&irori_ui::i18n::language());
        let rem = f32::from(window.rem_size());
        // `gap_2` between the controls, and `p_1` around them so the strip,
        // which clips, leaves the focus rings room.
        let gap = rem * 0.5;
        let inset = rem * 0.25;
        let shown = self.quick.shown(gap);
        self.quick_shown = shown;

        let view = cx.entity().downgrade();
        let room = {
            let view = view.clone();
            canvas(
                move |bounds, _, cx| {
                    let room = (f32::from(bounds.size.width) - 2.0 * inset).max(0.0);
                    view.update(cx, |this, cx| {
                        if this.quick.set_room(room) {
                            cx.notify();
                        }
                    })
                    .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let controls: Vec<AnyElement> = shown
            .iter()
            .map(|&control| {
                let element = match control {
                    QuickControl::Install => self.quick_install(filter, cx),
                    QuickControl::Favourites => self.quick_favourites(filter, cx),
                };
                measured(control, element, view.clone())
            })
            .collect();

        Some(
            div()
                .id("library-quick-controls")
                .relative()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(room)
                .child(
                    h_flex()
                        .w_full()
                        .p_1()
                        .gap_2()
                        .justify_end()
                        .children(controls),
                )
                .into_any_element(),
        )
    }

    fn quick_install(&self, filter: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let current = filter.install;
        ButtonGroup::new("library-quick-install")
            .small()
            .outline()
            .children(INSTALLS.iter().map(|(value, id, label)| {
                Button::new(*id).label(t(label)).selected(*value == current)
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                if let Some((value, _, _)) = clicked.first().and_then(|ix| INSTALLS.get(*ix)) {
                    let value = *value;
                    this.update_filter(|f| f.install = value, cx);
                }
            }))
            .into_any_element()
    }

    fn quick_favourites(&self, filter: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let on = filter.favourites_only;
        Button::new("library-quick-favourites")
            .small()
            .outline()
            .icon(if on {
                IconName::StarFill
            } else {
                IconName::Star
            })
            .label(t("library.filters.favourites"))
            .tooltip(t("library.quick.favourites_tooltip"))
            .selected(on)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.update_filter(|f| f.favourites_only = !on, cx)
            }))
            .into_any_element()
    }
}

/// `element` at its own width, reporting that width to the view.
fn measured(
    control: QuickControl,
    element: AnyElement,
    view: WeakEntity<LibraryView>,
) -> AnyElement {
    div()
        .relative()
        .flex_shrink_0()
        .child(element)
        .child(
            canvas(
                move |bounds, _, cx| {
                    let width = f32::from(bounds.size.width);
                    view.update(cx, |this, cx| {
                        if this.quick.set_width(control, width) {
                            cx.notify();
                        }
                    })
                    .ok();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}
