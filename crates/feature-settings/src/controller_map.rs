//! Settings -> Controller: what each button does, drawn on an interactive
//! controller beside a grouped, scannable binding list.
//!
//! One tab per place a controller acts (Irori's screens, Big Picture, the
//! in-game overlay, with Guide held). The drawing (`stage`) softly lights the
//! parts the tab uses and highlights + rings whichever part or row is hovered
//! or being reassigned. Reassigning swaps with the displaced button so no job
//! is ever lost, and the single map applies everywhere at once
//! (`irori_bigpicture::gamepad::bindings`).

mod rows;
mod stage;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, rems,
};
use irori_bigpicture::gamepad::bindings::{self, ButtonMap};
use irori_bigpicture::gamepad::drawing::Part;
use irori_bigpicture::gamepad::{self, ControllerKind, GamepadInput, PadButton};
use irori_core::controller_prefs::{ConfirmButton, ControllerPrefs, GlyphStyle};
use irori_ui::art::LruImageCache;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::controllers::kind_text;
use crate::widgets::{muted, panel};

use self::rows::RowContext;

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

/// What a row names: a job, the D-pad and left stick, or a job with Guide held.
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
    entry(Input(Confirm), "settings.controller_map.screens.confirm"),
    entry(Input(Back), "settings.controller_map.screens.back"),
    entry(Input(Alt), "settings.controller_map.screens.alt"),
    entry(Input(Menu), "settings.controller_map.screens.menu"),
    entry(Input(PrevPage), "settings.controller_map.screens.prev"),
    entry(Input(NextPage), "settings.controller_map.screens.next"),
    entry(Move, "settings.controller_map.screens.move"),
    entry(Input(Guide), "settings.controller_map.screens.guide"),
];
const BIG_PICTURE: &[Entry] = &[
    entry(
        Input(Confirm),
        "settings.controller_map.big_picture.confirm",
    ),
    entry(Input(Back), "settings.controller_map.big_picture.back"),
    entry(Input(Alt), "settings.controller_map.big_picture.alt"),
    entry(Input(Menu), "settings.controller_map.big_picture.menu"),
    entry(Input(PrevPage), "settings.controller_map.big_picture.tabs"),
    entry(Input(NextPage), "settings.controller_map.big_picture.tabs"),
    entry(Move, "settings.controller_map.big_picture.move"),
    entry(Input(Guide), "settings.controller_map.big_picture.guide"),
];
const OVERLAY: &[Entry] = &[
    entry(Input(Confirm), "settings.controller_map.overlay.confirm"),
    entry(Input(Back), "settings.controller_map.overlay.back"),
    entry(Input(PrevPage), "settings.controller_map.overlay.tabs"),
    entry(Input(NextPage), "settings.controller_map.overlay.tabs"),
    entry(Move, "settings.controller_map.overlay.move"),
    entry(Input(Guide), "settings.controller_map.overlay.guide"),
];
const GUIDE: &[Entry] = &[
    entry(Chord(Alt), "settings.controller_map.guide.clip"),
    entry(Chord(NextPage), "settings.controller_map.guide.screenshot"),
    entry(Chord(Up), "settings.controller_map.guide.volume_up"),
    entry(Chord(Down), "settings.controller_map.guide.volume_down"),
];

pub(super) fn is_rebindable(place: Place, job: Job) -> bool {
    place != Place::Guide && matches!(job, Job::Input(i) if bindings::BINDABLE.contains(&i))
}

/// The parts of the controller a single job uses.
pub(crate) fn parts_for_job(job: Job, map: &ButtonMap, kind: ControllerKind) -> Vec<Part> {
    match job {
        Job::Move => vec![
            Part::Button(PadButton::DPadUp),
            Part::Button(PadButton::DPadDown),
            Part::Button(PadButton::DPadLeft),
            Part::Button(PadButton::DPadRight),
            Part::LeftStick,
        ],
        Job::Input(input) | Job::Chord(input) => map
            .button_for(input, kind)
            .map(Part::Button)
            .into_iter()
            .collect(),
    }
}

/// All parts a tab uses, softly lit on the drawing at rest.
pub(crate) fn lit_parts(place: Place, map: &ButtonMap, kind: ControllerKind) -> Vec<Part> {
    let mut out = Vec::new();
    for e in entries(place) {
        if matches!(e.job, Job::Chord(_)) {
            out.push(Part::Button(PadButton::Guide));
        }
        out.extend(parts_for_job(e.job, map, kind));
    }
    let mut unique = Vec::with_capacity(out.len());
    for part in out {
        if !unique.contains(&part) {
            unique.push(part);
        }
    }
    unique
}

pub(super) fn entry_for_part(
    place: Place,
    part: Part,
    map: &ButtonMap,
    kind: ControllerKind,
) -> Option<Entry> {
    entries(place)
        .iter()
        .copied()
        .find(|e| parts_for_job(e.job, map, kind).contains(&part))
}

pub(super) fn job_title(job: Job) -> SharedString {
    match job {
        Job::Move => t("settings.controller_map.job.move"),
        Job::Input(GamepadInput::Guide) => t("settings.controller_map.job.guide"),
        Job::Input(input) | Job::Chord(input) => job_label(input),
    }
}

pub(super) fn job_label(input: GamepadInput) -> SharedString {
    t(match input {
        GamepadInput::Confirm => "settings.controller_map.job.confirm",
        GamepadInput::Back => "settings.controller_map.job.back",
        GamepadInput::Alt => "settings.controller_map.job.alt",
        GamepadInput::Menu => "settings.controller_map.job.menu",
        GamepadInput::PrevPage => "settings.controller_map.job.prev",
        GamepadInput::NextPage => "settings.controller_map.job.next",
        GamepadInput::Up => "settings.controller_map.job.volume_up",
        GamepadInput::Down => "settings.controller_map.job.volume_down",
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

pub(super) fn guide_label(kind: ControllerKind) -> &'static str {
    match kind {
        ControllerKind::PlayStation4 | ControllerKind::PlayStation5 => "PS",
        ControllerKind::Nintendo => "Home",
        _ => "Guide",
    }
}

pub(super) fn is_round_button(button: PadButton) -> bool {
    matches!(
        button,
        PadButton::South | PadButton::East | PadButton::West | PadButton::North
    )
}

pub(super) fn render_keycap(
    label: SharedString,
    round: bool,
    active: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.theme();
    let (bg, fg, border) = if active {
        (t.primary, t.primary_foreground, t.primary)
    } else {
        (t.background, t.foreground, t.border)
    };
    let cap = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_semibold()
        .child(label);
    if round {
        cap.size_6().rounded_full().into_any_element()
    } else {
        cap.h_6()
            .min_w(rems(2.0))
            .px_1p5()
            .rounded(t.radius)
            .into_any_element()
    }
}

/// The controller page's interactive button map.
pub(crate) struct ButtonMapPanel {
    pub(super) place: Place,
    pub(super) hovered_job: Option<Job>,
    pub(super) hovered_part: Option<Part>,
    pub(super) editing_job: Option<GamepadInput>,
    pub(super) images: Entity<LruImageCache>,
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
            hovered_job: None,
            hovered_part: None,
            editing_job: None,
            // Bounded LRU covering tab switches and hover states smoothly.
            images: LruImageCache::new(16, cx),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn prefs(cx: &App) -> ControllerPrefs {
        models(cx).settings.read(cx).app.controller.clone()
    }

    fn nav_disabled(cx: &App) -> bool {
        models(cx).settings.read(cx).app.disable_controller
    }

    /// Physical family of the connected controller in hand, defaulting to Xbox.
    pub(super) fn actual_kind(cx: &App) -> ControllerKind {
        gamepad::model(cx)
            .and_then(|m| m.read(cx).controllers().first().map(|c| c.kind))
            .unwrap_or(ControllerKind::Xbox)
    }

    fn save(prefs: ControllerPrefs, cx: &mut App) {
        backend(cx).settings.update_app(|s| s.controller = prefs);
    }

    pub(super) fn assign(
        &mut self,
        job: GamepadInput,
        button: PadButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefs = Self::prefs(cx);
        match ButtonMap::assigning(&prefs, job, button, Self::actual_kind(cx)) {
            Some(next) => {
                Self::save(next, cx);
                self.editing_job = None;
            }
            None => window.push_notification(
                Notification::error(t("settings.controller_map.stranded"))
                    .title(t("settings.controller_map.not_changed")),
                cx,
            ),
        }
        cx.notify();
    }

    pub(super) fn active_parts(&self, map: &ButtonMap, actual: ControllerKind) -> Vec<Part> {
        if let Some(job) = self.editing_job {
            return parts_for_job(Job::Input(job), map, actual);
        }
        if let Some(job) = self.hovered_job {
            return parts_for_job(job, map, actual);
        }
        if let Some(part) = self.hovered_part {
            if let Some(entry) = entry_for_part(self.place, part, map, actual) {
                return parts_for_job(entry.job, map, actual);
            }
            return vec![part];
        }
        Vec::new()
    }

    pub(super) fn is_entry_highlighted(
        &self,
        entry: Entry,
        map: &ButtonMap,
        actual: ControllerKind,
    ) -> bool {
        self.editing_job.is_some_and(|e| entry.job == Job::Input(e))
            || self.hovered_job == Some(entry.job)
            || self
                .hovered_part
                .is_some_and(|p| parts_for_job(entry.job, map, actual).contains(&p))
    }
}

impl Render for ButtonMapPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted_fg, bg, radius) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.radius,
            )
        };
        let prefs = Self::prefs(cx);
        let map = ButtonMap::from_prefs(&prefs);
        let actual = Self::actual_kind(cx);
        let shown = map.display_kind(actual);
        let disabled = Self::nav_disabled(cx);

        let paused_banner = disabled.then(|| {
            h_flex()
                .w_full()
                .px_3()
                .py_2()
                .gap_3()
                .items_center()
                .justify_between()
                .rounded(radius)
                .bg(bg.opacity(0.6))
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Info).small().text_color(muted_fg))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(t("settings.controller_map.nav_paused")),
                        ),
                )
                .child(
                    Button::new("controller-enable-nav")
                        .outline()
                        .xsmall()
                        .label(t("settings.controller_map.turn_on"))
                        .on_click(|_, _, cx| {
                            backend(cx)
                                .settings
                                .update_app(|s| s.disable_controller = false);
                        }),
                )
        });

        let glyphs = Button::new("controller-glyphs")
            .outline()
            .small()
            .dropdown_caret(true)
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
            .dropdown_caret(true)
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

        let glyph_auto = (prefs.glyphs == GlyphStyle::Auto).then(|| {
            t_args(
                "settings.controller_map.resolved_using",
                &[("value", kind_text(shown).as_ref())],
            )
        });
        let confirm_auto = (prefs.confirm_button == ConfirmButton::Auto).then(|| {
            let btn = map
                .button_for(GamepadInput::Confirm, actual)
                .map_or("?", |b| shown.button_label(b));
            t_args("settings.controller_map.resolved_using", &[("value", btn)])
        });

        let labeled_choice =
            |label: SharedString, resolved: Option<SharedString>, control: AnyElement| {
                v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(muted_fg)
                                    .child(label),
                            )
                            .when_some(resolved, |row, text| {
                                row.child(Tag::secondary().xsmall().outline().child(text))
                            }),
                    )
                    .child(control)
            };

        let rebound = !prefs.buttons.is_empty();
        let prefs_bar = h_flex()
            .w_full()
            .gap_4()
            .flex_wrap()
            .items_end()
            .justify_between()
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .items_end()
                    .child(labeled_choice(
                        t("settings.controller_map.shown_as"),
                        glyph_auto,
                        glyphs.into_any_element(),
                    ))
                    .child(labeled_choice(
                        t("settings.controller_map.confirm_with"),
                        confirm_auto,
                        confirm.into_any_element(),
                    )),
            )
            .when(rebound, |row| {
                row.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Tag::primary()
                                .xsmall()
                                .outline()
                                .child(t("settings.controller_map.custom")),
                        )
                        .child(
                            Button::new("controller-reset")
                                .outline()
                                .small()
                                .icon(IconName::RotateCcw)
                                .label(t("settings.controller_map.reset"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let next = ButtonMap::reset(&ButtonMapPanel::prefs(cx));
                                    ButtonMapPanel::save(next, cx);
                                    this.editing_job = None;
                                    cx.notify();
                                })),
                        ),
                )
            });

        let tabs = Place::ALL.into_iter().enumerate().fold(
            h_flex()
                .gap_1()
                .flex_wrap()
                .p_1()
                .rounded(radius)
                .bg(bg.opacity(0.5))
                .border_1()
                .border_color(border),
            |row, (ix, place)| {
                row.child(
                    Button::new(("controller-place", ix))
                        .xsmall()
                        .ghost()
                        .selected(self.place == place)
                        .label(place.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.place = place;
                            this.hovered_job = None;
                            this.hovered_part = None;
                            this.editing_job = None;
                            cx.notify();
                        })),
                )
            },
        );

        let scope_hint = muted(
            t(if self.place == Place::Guide {
                "settings.controller_map.scope_guide"
            } else {
                "settings.controller_map.scope_global"
            }),
            cx,
        )
        .text_xs();

        let tabs_bar = h_flex()
            .w_full()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(tabs)
            .child(scope_hint);

        let row_ctx = RowContext {
            map: &map,
            actual,
            shown,
        };
        let all_entries = entries(self.place);
        let mut custom_rows = Vec::new();
        let mut fixed_rows = Vec::new();
        for (ix, &e) in all_entries.iter().enumerate() {
            if is_rebindable(self.place, e.job) {
                let first = custom_rows.is_empty();
                custom_rows.push(self.render_row(ix, e, &row_ctx, first, cx));
            } else {
                let first = fixed_rows.is_empty();
                fixed_rows.push(self.render_row(ix, e, &row_ctx, first, cx));
            }
        }

        let group_box =
            |title: SharedString, trailing: AnyElement, rows: Vec<AnyElement>, cx: &App| {
                (!rows.is_empty()).then(|| {
                    v_flex()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .px_1()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(muted_fg)
                                        .child(title),
                                )
                                .child(trailing),
                        )
                        .child(panel(cx).overflow_hidden().children(rows))
                })
            };

        let custom_panel = group_box(
            t("settings.controller_map.group_customizable"),
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(t("settings.controller_map.rebind_hint"))
                .into_any_element(),
            custom_rows,
            cx,
        );

        let fixed_title = t(if self.place == Place::Guide {
            "settings.controller_map.group_chords"
        } else {
            "settings.controller_map.group_fixed"
        });
        let fixed_panel = group_box(
            fixed_title,
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(muted_fg)
                .child(Icon::new(IconName::Lock).xsmall())
                .child(t("settings.controller_map.fixed"))
                .into_any_element(),
            fixed_rows,
            cx,
        );

        let bindings_col = v_flex()
            .flex_1()
            .min_w(rems(21.0))
            .gap_3()
            .children(custom_panel)
            .children(fixed_panel);

        v_flex().w_full().gap_3().children(paused_banner).child(
            v_flex()
                .w_full()
                .gap_3()
                .when(disabled, |v| v.opacity(0.55))
                .child(prefs_bar)
                .child(tabs_bar)
                .child(
                    h_flex()
                        .w_full()
                        .gap_4()
                        .flex_wrap()
                        .items_start()
                        .child(self.render_drawing_stage(&map, actual, shown, cx))
                        .child(bindings_col),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_lights_its_parts_including_left_stick() {
        let map = ButtonMap::default();
        let xbox = ControllerKind::Xbox;
        let screens = lit_parts(Place::Screens, &map, xbox);
        assert!(screens.contains(&Part::Button(PadButton::South)));
        assert!(screens.contains(&Part::Button(PadButton::Guide)));
        assert!(
            screens.contains(&Part::LeftStick),
            "Move uses both D-pad and left stick"
        );
        assert!(screens.contains(&Part::Button(PadButton::DPadUp)));

        let move_parts = parts_for_job(Job::Move, &map, xbox);
        assert!(move_parts.contains(&Part::LeftStick));
        assert!(move_parts.contains(&Part::Button(PadButton::DPadUp)));

        let guide = lit_parts(Place::Guide, &map, xbox);
        assert!(guide.contains(&Part::Button(PadButton::Guide)));
        assert!(
            guide.contains(&Part::Button(PadButton::West)),
            "Guide + X saves a clip"
        );
        assert!(!guide.contains(&Part::Button(PadButton::South)));
        assert!(!guide.contains(&Part::LeftStick));

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
                assert!(!job_title(e.job).is_empty());
            }
        }
    }
}
