//! Settings → Controller: what each button does, drawn on the controller in
//! hand, with the buttons shown as a family, the button that confirms and
//! every job's button changeable in place.
//!
//! One tab per place a controller acts (Irori's screens, Big Picture, the
//! in-game overlay, with Guide held). The drawing lights the buttons the tab
//! uses and rings the one whose row the pointer or focus is on. A job is
//! moved to another button by swapping it with that button's job, so no
//! job is ever lost; the map applies everywhere at once
//! (`irori_bigpicture::gamepad::bindings`).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, Image, ImageFormat, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, img, rems,
};
use irori_bigpicture::gamepad::bindings::{self, ButtonMap, REBINDABLE};
use irori_bigpicture::gamepad::drawing::{self, Colours, Part, Shape};
use irori_bigpicture::gamepad::{self, ControllerKind, GamepadInput, PadButton};
use irori_core::controller_prefs::{ConfirmButton, ControllerPrefs, GlyphStyle};
use irori_ui::art::LruImageCache;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::appearance::hex;
use crate::widgets::{muted, panel};

/// Width of the drawing on the page, in rem.
const DRAWING_REM: f32 = 30.0;

/// A place a controller acts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Screens,
    BigPicture,
    Overlay,
    Guide,
}

impl Place {
    pub(crate) const ALL: [Place; 4] = [
        Place::Screens,
        Place::BigPicture,
        Place::Overlay,
        Place::Guide,
    ];

    fn label(self) -> SharedString {
        t(match self {
            Place::Screens => "settings.controller_map.place.screens",
            Place::BigPicture => "settings.controller_map.place.big_picture",
            Place::Overlay => "settings.controller_map.place.overlay",
            Place::Guide => "settings.controller_map.place.guide",
        })
    }
}

/// What a row names: a job, the D-pad and stick, or a job with Guide held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    Input(GamepadInput),
    Move,
    Chord(GamepadInput),
}

/// One row of a tab: the job and the translation key saying what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub job: Job,
    pub text: &'static str,
}

const fn entry(job: Job, text: &'static str) -> Entry {
    Entry { job, text }
}

/// The rows of a tab, in the order the page lists them.
pub(crate) fn entries(place: Place) -> &'static [Entry] {
    match place {
        Place::Screens => SCREENS,
        Place::BigPicture => BIG_PICTURE,
        Place::Overlay => OVERLAY,
        Place::Guide => GUIDE,
    }
}

use GamepadInput::{Alt, Back, Confirm, Down, Guide, Menu, NextPage, PrevPage, Up};
use Job::{Chord, Input, Move};

const SCREENS: &[Entry] = &[
    entry(Move, "settings.controller_map.screens.move"),
    entry(Input(Confirm), "settings.controller_map.screens.confirm"),
    entry(Input(Back), "settings.controller_map.screens.back"),
    entry(Input(Alt), "settings.controller_map.screens.alt"),
    entry(Input(Menu), "settings.controller_map.screens.menu"),
    entry(Input(PrevPage), "settings.controller_map.screens.prev"),
    entry(Input(NextPage), "settings.controller_map.screens.next"),
    entry(Input(Guide), "settings.controller_map.screens.guide"),
];
const BIG_PICTURE: &[Entry] = &[
    entry(Move, "settings.controller_map.big_picture.move"),
    entry(
        Input(Confirm),
        "settings.controller_map.big_picture.confirm",
    ),
    entry(Input(Back), "settings.controller_map.big_picture.back"),
    entry(Input(Alt), "settings.controller_map.big_picture.alt"),
    entry(Input(Menu), "settings.controller_map.big_picture.menu"),
    entry(Input(PrevPage), "settings.controller_map.big_picture.tabs"),
    entry(Input(NextPage), "settings.controller_map.big_picture.tabs"),
    entry(Input(Guide), "settings.controller_map.big_picture.guide"),
];
const OVERLAY: &[Entry] = &[
    entry(Input(Guide), "settings.controller_map.overlay.guide"),
    entry(Move, "settings.controller_map.overlay.move"),
    entry(Input(Confirm), "settings.controller_map.overlay.confirm"),
    entry(Input(Back), "settings.controller_map.overlay.back"),
    entry(Input(PrevPage), "settings.controller_map.overlay.tabs"),
    entry(Input(NextPage), "settings.controller_map.overlay.tabs"),
];
const GUIDE: &[Entry] = &[
    entry(Chord(Alt), "settings.controller_map.guide.clip"),
    entry(Chord(NextPage), "settings.controller_map.guide.screenshot"),
    entry(Chord(Up), "settings.controller_map.guide.volume_up"),
    entry(Chord(Down), "settings.controller_map.guide.volume_down"),
];

/// The buttons a tab uses, lit on the drawing.
pub(crate) fn lit(place: Place, map: &ButtonMap, kind: ControllerKind) -> Vec<PadButton> {
    let mut out = Vec::new();
    for e in entries(place) {
        match e.job {
            Job::Move => out.extend([
                PadButton::DPadUp,
                PadButton::DPadDown,
                PadButton::DPadLeft,
                PadButton::DPadRight,
            ]),
            Job::Input(input) => out.extend(map.button_for(input, kind)),
            Job::Chord(input) => {
                out.push(PadButton::Guide);
                out.extend(map.button_for(input, kind));
            }
        }
    }
    let mut unique = Vec::with_capacity(out.len());
    for button in out {
        if !unique.contains(&button) {
            unique.push(button);
        }
    }
    unique
}

/// The name of a job, for its button menu.
fn job_label(input: GamepadInput) -> SharedString {
    t(match input {
        GamepadInput::Confirm => "settings.controller_map.job.confirm",
        GamepadInput::Back => "settings.controller_map.job.back",
        GamepadInput::Alt => "settings.controller_map.job.alt",
        GamepadInput::Menu => "settings.controller_map.job.menu",
        GamepadInput::PrevPage => "settings.controller_map.job.prev",
        GamepadInput::NextPage => "settings.controller_map.job.next",
        _ => "settings.controller_map.job.other",
    })
}

fn glyph_style_label(style: GlyphStyle) -> SharedString {
    match style {
        GlyphStyle::Auto => t("settings.controller_map.automatic"),
        GlyphStyle::Xbox => "Xbox".into(),
        GlyphStyle::PlayStation => "PlayStation".into(),
        GlyphStyle::Nintendo => "Nintendo".into(),
    }
}

fn confirm_label(choice: ConfirmButton) -> SharedString {
    t(match choice {
        ConfirmButton::Auto => "settings.controller_map.confirm.auto",
        ConfirmButton::South => "settings.controller_map.confirm.south",
        ConfirmButton::East => "settings.controller_map.confirm.east",
    })
}

/// The controller page's button map.
pub(crate) struct ButtonMapPanel {
    place: Place,
    /// The button whose row the pointer is over.
    ringed: Option<PadButton>,
    images: Entity<LruImageCache>,
    _subscriptions: Vec<Subscription>,
}

impl ButtonMapPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut subscriptions = vec![cx.observe(&settings, |_, _, cx| cx.notify())];
        if let Some(model) = gamepad::model(cx) {
            subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
        }
        Self {
            place: Place::Screens,
            ringed: None,
            // A drawing per tab and ringed button; a few are enough.
            images: LruImageCache::new(4, cx),
            _subscriptions: subscriptions,
        }
    }

    fn prefs(cx: &App) -> ControllerPrefs {
        models(cx).settings.read(cx).app.controller.clone()
    }

    /// The family of the controller in hand, Xbox's without one.
    fn kind(cx: &App) -> ControllerKind {
        gamepad::model(cx)
            .and_then(|m| m.read(cx).active_kind())
            .unwrap_or(ControllerKind::Xbox)
    }

    fn save(prefs: ControllerPrefs, cx: &mut App) {
        backend(cx).settings.update_app(|s| s.controller = prefs);
    }

    fn assign(
        &mut self,
        job: GamepadInput,
        button: PadButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefs = Self::prefs(cx);
        match ButtonMap::assigning(&prefs, job, button, Self::kind(cx)) {
            Some(next) => Self::save(next, cx),
            None => window.push_notification(
                Notification::error(t("settings.controller_map.stranded"))
                    .title(t("settings.controller_map.not_changed")),
                cx,
            ),
        }
        cx.notify();
    }

    fn render_drawing(
        &self,
        map: &ButtonMap,
        actual: ControllerKind,
        shown: ControllerKind,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let shape = Shape::of(shown);
        let colours = Colours {
            body: hex(theme.secondary),
            outline: hex(theme.border),
            part: hex(theme.muted),
            lit: hex(theme.primary),
            ring: hex(theme.ring),
        };
        let lit = lit(self.place, map, actual);
        let svg = drawing::svg(shape, &colours, &lit, self.ringed);
        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        let width = DRAWING_REM;
        let height = DRAWING_REM * drawing::HEIGHT / drawing::WIDTH;
        let labels = REBINDABLE.into_iter().map(|button| {
            let (x, y) = drawing::spot(shape, Part::Button(button));
            let on = lit.contains(&button);
            div()
                .absolute()
                .left(rems(width * x - 2.0))
                .top(rems(height * y - 0.75))
                .w(rems(4.0))
                .h(rems(1.5))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_bold()
                .text_color(if on {
                    theme.primary_foreground
                } else {
                    theme.muted_foreground
                })
                .child(shown.button_label(button))
        });
        div()
            .image_cache(self.images.clone())
            .relative()
            .flex_shrink_0()
            .w(rems(width))
            .h(rems(height))
            .child(img(image).size_full())
            .children(labels)
            .into_any_element()
    }

    fn render_row(
        &self,
        ix: usize,
        e: Entry,
        map: &ButtonMap,
        actual: ControllerKind,
        shown: ControllerKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let (button, buttons_text): (Option<PadButton>, SharedString) = match e.job {
            Job::Move => (
                Some(PadButton::DPadUp),
                t("settings.controller_map.dpad_and_stick"),
            ),
            Job::Input(input) => {
                let button = map.button_for(input, actual);
                let text = button.map_or_else(
                    || t("settings.controller_map.no_button"),
                    |b| shown.button_label(b).into(),
                );
                (button, text)
            }
            Job::Chord(input) => {
                let button = map.button_for(input, actual);
                let label = match input {
                    GamepadInput::Up | GamepadInput::Down => shown.glyph(input).to_owned(),
                    _ => button.map_or("?", |b| shown.button_label(b)).to_owned(),
                };
                (
                    button,
                    t_args("settings.controller_map.with_guide", &[("button", &label)]),
                )
            }
        };
        let rebindable = matches!(e.job, Job::Input(input) if bindings::BINDABLE.contains(&input))
            && self.place != Place::Guide;
        let change = match e.job {
            Job::Input(job) if rebindable => {
                let view = cx.entity().downgrade();
                Some(
                    Button::new(("controller-change", ix))
                        .ghost()
                        .xsmall()
                        .label(t("settings.controller_map.change"))
                        .tooltip(t_args(
                            "settings.controller_map.change_tooltip",
                            &[("job", &job_label(job))],
                        ))
                        .dropdown_menu(move |mut menu, _, _| {
                            for b in REBINDABLE {
                                let view = view.clone();
                                let label = shown.button_label(b);
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .checked(button == Some(b))
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.assign(job, b, window, cx)
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
                )
            }
            _ => None,
        };
        h_flex()
            .id(ElementId::Name(format!("controller-row-{ix}").into()))
            .gap_3()
            .px_3()
            .py_2()
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered { button } else { None };
                if this.ringed != next {
                    this.ringed = next;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .min_w(rems(6.0))
                    .text_sm()
                    .font_semibold()
                    .child(buttons_text),
            )
            .child(div().flex_1().min_w_0().text_sm().child(t(e.text)))
            .children(change)
            .into_any_element()
    }
}

impl Render for ButtonMapPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = Self::prefs(cx);
        let map = bindings::current();
        let actual = Self::kind(cx);
        let shown = map.display_kind(actual);

        let glyphs = Button::new("controller-glyphs")
            .outline()
            .small()
            .label(glyph_style_label(prefs.glyphs))
            .dropdown_menu({
                let current = prefs.glyphs;
                move |mut menu, _, _| {
                    for style in [
                        GlyphStyle::Auto,
                        GlyphStyle::Xbox,
                        GlyphStyle::PlayStation,
                        GlyphStyle::Nintendo,
                    ] {
                        menu = menu.item(
                            PopupMenuItem::new(glyph_style_label(style))
                                .checked(style == current)
                                .on_click(move |_, _, cx| {
                                    let mut next = ButtonMapPanel::prefs(cx);
                                    next.glyphs = style;
                                    ButtonMapPanel::save(next, cx);
                                }),
                        );
                    }
                    menu
                }
            });
        let confirm = Button::new("controller-confirm")
            .outline()
            .small()
            .label(confirm_label(prefs.confirm_button))
            .dropdown_menu({
                let current = prefs.confirm_button;
                move |mut menu, _, _| {
                    for choice in [
                        ConfirmButton::Auto,
                        ConfirmButton::South,
                        ConfirmButton::East,
                    ] {
                        menu = menu.item(
                            PopupMenuItem::new(confirm_label(choice))
                                .checked(choice == current)
                                .on_click(move |_, _, cx| {
                                    let mut next = ButtonMapPanel::prefs(cx);
                                    next.confirm_button = choice;
                                    ButtonMapPanel::save(next, cx);
                                }),
                        );
                    }
                    menu
                }
            });
        let choice = |label: SharedString, control: AnyElement| {
            h_flex()
                .gap_2()
                .min_w_0()
                .child(div().text_sm().min_w_0().truncate().child(label))
                .child(control)
        };
        let tabs = Place::ALL.into_iter().enumerate().fold(
            h_flex().gap_1().flex_wrap(),
            |row, (ix, place)| {
                row.child(
                    Button::new(("controller-place", ix))
                        .small()
                        .ghost()
                        .selected(self.place == place)
                        .label(place.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.place = place;
                            this.ringed = None;
                            cx.notify();
                        })),
                )
            },
        );
        let rows = entries(self.place)
            .iter()
            .enumerate()
            .map(|(ix, e)| self.render_row(ix, *e, &map, actual, shown, cx))
            .collect::<Vec<_>>();
        let rebound = !prefs.buttons.is_empty();
        let footer = h_flex()
            .gap_2()
            .justify_between()
            .child(
                muted(t("settings.controller_map.everywhere"), cx)
                    .flex_1()
                    .min_w_0(),
            )
            .when(rebound, |row| {
                row.child(
                    Button::new("controller-reset")
                        .ghost()
                        .small()
                        .label(t("settings.controller_map.reset"))
                        .on_click(|_, _, cx| {
                            let next = ButtonMap::reset(&ButtonMapPanel::prefs(cx));
                            ButtonMapPanel::save(next, cx);
                        }),
                )
            });
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .child(choice(
                        t("settings.controller_map.shown_as"),
                        glyphs.into_any_element(),
                    ))
                    .child(choice(
                        t("settings.controller_map.confirm_with"),
                        confirm.into_any_element(),
                    )),
            )
            .child(tabs)
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .items_start()
                    .child(self.render_drawing(&map, actual, shown, cx))
                    .child(panel(cx).flex_1().min_w(rems(18.0)).children(rows)),
            )
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_lights_its_buttons() {
        let map = ButtonMap::default();
        let xbox = ControllerKind::Xbox;
        let screens = lit(Place::Screens, &map, xbox);
        assert!(screens.contains(&PadButton::South) && screens.contains(&PadButton::Guide));
        let guide = lit(Place::Guide, &map, xbox);
        assert!(guide.contains(&PadButton::Guide));
        assert!(guide.contains(&PadButton::West), "Guide + X saves a clip");
        assert!(!guide.contains(&PadButton::South));
        for place in Place::ALL {
            assert!(!entries(place).is_empty());
        }
    }

    #[test]
    fn every_text_is_translated() {
        crate::i18n::register();
        for place in Place::ALL {
            assert!(!place.label().is_empty());
            for e in entries(place) {
                assert_ne!(t(e.text).as_ref(), e.text, "{}", e.text);
            }
        }
    }
}
