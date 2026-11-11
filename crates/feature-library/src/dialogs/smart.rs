//! The smart category editor: a name, whether games must pass all rules or
//! any of them, the rules, and the "Leave out" rules, with a live count of
//! the games that match.
//!
//! Rows are edited as they were read: what a newer Irori added to a row
//! stays with it, a rule this version can't read stays as a row of its own
//! (shown as such, and removable), and a match mode this version can't read
//! stays until the user picks one. The editor reopens a category with the
//! mode it was saved with, whatever its rows.

use std::collections::BTreeSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div,
};
use irori_core::{GameMode, Runner};
use irori_services::{Condition, EffectiveStatus, Match, Rule, SmartCategory};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::categories::{NameError, validate_name};
use super::{dialog_width, text};

/// The kinds of rule the editor offers, in its menu's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Store,
    Installed,
    Status,
    Playtime,
    LastPlayed,
    Hltb,
    Mode,
    Tags,
    Genre,
    AgeRating,
    System,
    Category,
    Favourite,
    Query,
}

impl Kind {
    pub const ALL: [Kind; 14] = [
        Kind::Store,
        Kind::Installed,
        Kind::Status,
        Kind::Playtime,
        Kind::LastPlayed,
        Kind::Hltb,
        Kind::Mode,
        Kind::Tags,
        Kind::Genre,
        Kind::AgeRating,
        Kind::System,
        Kind::Category,
        Kind::Favourite,
        Kind::Query,
    ];

    pub fn of(rule: &Rule) -> Kind {
        match rule {
            Rule::Store { .. } => Kind::Store,
            Rule::Installed { .. } => Kind::Installed,
            Rule::Status { .. } => Kind::Status,
            Rule::Playtime { .. } => Kind::Playtime,
            Rule::LastPlayed { .. } => Kind::LastPlayed,
            Rule::Hltb { .. } => Kind::Hltb,
            Rule::Mode { .. } => Kind::Mode,
            Rule::Tags { .. } => Kind::Tags,
            Rule::Genre { .. } => Kind::Genre,
            Rule::AgeRating { .. } => Kind::AgeRating,
            Rule::System { .. } => Kind::System,
            Rule::Category { .. } => Kind::Category,
            Rule::Favourite {} => Kind::Favourite,
            Rule::Query { .. } => Kind::Query,
        }
    }

    pub fn label(self) -> SharedString {
        t(match self {
            Kind::Store => "library.smart.rule.store",
            Kind::Installed => "library.smart.rule.installed",
            Kind::Status => "library.smart.rule.status",
            Kind::Playtime => "library.smart.rule.playtime",
            Kind::LastPlayed => "library.smart.rule.last_played",
            Kind::Hltb => "library.smart.rule.hltb",
            Kind::Mode => "library.smart.rule.mode",
            Kind::Tags => "library.smart.rule.tags",
            Kind::Genre => "library.smart.rule.genre",
            Kind::AgeRating => "library.smart.rule.age_rating",
            Kind::System => "library.smart.rule.system",
            Kind::Category => "library.smart.rule.category",
            Kind::Favourite => "library.smart.rule.favourite",
            Kind::Query => "library.smart.rule.query",
        })
    }

    /// A new row of this kind, before the user picks its values.
    fn new_rule(self) -> Rule {
        match self {
            Kind::Store => Rule::Store {
                stores: BTreeSet::new(),
            },
            Kind::Installed => Rule::Installed { installed: true },
            Kind::Status => Rule::Status {
                statuses: BTreeSet::new(),
            },
            Kind::Playtime => Rule::Playtime {
                min_minutes: None,
                max_minutes: None,
            },
            Kind::LastPlayed => Rule::LastPlayed {
                min_days: None,
                max_days: None,
            },
            Kind::Hltb => Rule::Hltb {
                min_hours: None,
                max_hours: None,
            },
            Kind::Mode => Rule::Mode {
                modes: BTreeSet::new(),
            },
            Kind::Tags => Rule::Tags { tags: Vec::new() },
            Kind::Genre => Rule::Genre { genres: Vec::new() },
            Kind::AgeRating => Rule::AgeRating {
                max_age: 12,
                include_unrated: false,
            },
            Kind::System => Rule::System {
                systems: BTreeSet::new(),
            },
            Kind::Category => Rule::Category {
                names: BTreeSet::new(),
            },
            Kind::Favourite => Rule::Favourite {},
            Kind::Query => Rule::Query {
                query: String::new(),
            },
        }
    }

    /// Whether the row's values are typed (two fields: text, or a range).
    fn typed(self) -> bool {
        matches!(
            self,
            Kind::Playtime | Kind::LastPlayed | Kind::Hltb | Kind::Tags | Kind::Genre | Kind::Query
        )
    }
}

/// The text fields of a typed rule: its text, or its range's low and high
/// end (playtime in hours).
pub fn texts_of(rule: &Rule) -> (String, String) {
    let n = |v: Option<u64>| v.map(|v| v.to_string()).unwrap_or_default();
    match rule {
        Rule::Playtime {
            min_minutes,
            max_minutes,
        } => (
            n(min_minutes.map(|m| m / 60)),
            n(max_minutes.map(|m| m / 60)),
        ),
        Rule::LastPlayed { min_days, max_days } => {
            (n(min_days.map(u64::from)), n(max_days.map(u64::from)))
        }
        Rule::Hltb {
            min_hours,
            max_hours,
        } => (n(min_hours.map(u64::from)), n(max_hours.map(u64::from))),
        Rule::Tags { tags } => (tags.join(", "), String::new()),
        Rule::Genre { genres } => (genres.join(", "), String::new()),
        Rule::Query { query } => (query.clone(), String::new()),
        _ => (String::new(), String::new()),
    }
}

/// `rule` with the typed fields' values; other kinds keep their values.
/// A number that doesn't read leaves its end of the range open.
pub fn with_texts(rule: &Rule, a: &str, b: &str) -> Rule {
    let number = |s: &str| s.trim().parse::<u32>().ok();
    let list = |s: &str| -> Vec<String> {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    };
    match rule {
        Rule::Playtime { .. } => Rule::Playtime {
            min_minutes: number(a).map(|h| u64::from(h) * 60),
            max_minutes: number(b).map(|h| u64::from(h) * 60 + 59),
        },
        Rule::LastPlayed { .. } => Rule::LastPlayed {
            min_days: number(a),
            max_days: number(b),
        },
        Rule::Hltb { .. } => Rule::Hltb {
            min_hours: number(a),
            max_hours: number(b),
        },
        Rule::Tags { .. } => Rule::Tags { tags: list(a) },
        Rule::Genre { .. } => Rule::Genre { genres: list(a) },
        Rule::Query { .. } => Rule::Query {
            query: a.trim().to_owned(),
        },
        other => other.clone(),
    }
}

/// One row being edited.
struct Row {
    /// The row as it was read: what a newer Irori added to it, or the
    /// whole of a rule this version can't read.
    original: Condition,
    /// `None` for a rule this version can't read (kept as it is).
    rule: Option<Rule>,
    a: Entity<InputState>,
    b: Entity<InputState>,
}

impl Row {
    fn condition(&self, cx: &App) -> Condition {
        match &self.rule {
            None => self.original.clone(),
            Some(rule) => {
                self.original
                    .with_rule(with_texts(rule, &text(&self.a, cx), &text(&self.b, cx)))
            }
        }
    }
}

/// Which list a row is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Rules,
    LeaveOut,
}

/// The editor of one smart category, new or saved.
pub struct SmartEditor {
    /// The saved category's id; `None` for a new one.
    id: Option<String>,
    /// The category as opened, for what the editor doesn't change (its
    /// pin, colour and what a newer Irori added).
    base: SmartCategory,
    name: Entity<InputState>,
    matching: Match,
    rules: Vec<Row>,
    leave_out: Vec<Row>,
    next_row: usize,
    /// The last counted draft, with what it was counted against.
    preview: Option<(SmartCategory, (u64, usize, u64), usize)>,
    _subscriptions: Vec<Subscription>,
}

impl SmartEditor {
    pub(crate) fn new(
        id: Option<String>,
        base: SmartCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("library.smart.name_placeholder"))
                .default_value(base.name.clone())
        });
        let library = models(cx).library.clone();
        let subscriptions = vec![
            cx.observe(&library, |_, _, cx| cx.notify()),
            cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.save(window, cx),
                _ => {}
            }),
        ];
        let mut this = Self {
            id,
            matching: base.matching.clone(),
            rules: Vec::new(),
            leave_out: Vec::new(),
            next_row: 0,
            preview: None,
            name,
            base: base.clone(),
            _subscriptions: subscriptions,
        };
        for c in &base.rules {
            this.push_row(List::Rules, c.clone(), window, cx);
        }
        for c in &base.leave_out {
            this.push_row(List::LeaveOut, c.clone(), window, cx);
        }
        this
    }

    fn push_row(
        &mut self,
        list: List,
        original: Condition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (a_text, b_text) = original.rule.as_ref().map(texts_of).unwrap_or_default();
        let placeholders = match original.rule.as_ref().map(Kind::of) {
            Some(Kind::Tags | Kind::Genre) => {
                (t("library.smart.list_placeholder"), SharedString::default())
            }
            Some(Kind::Query) => (
                t("library.smart.query_placeholder"),
                SharedString::default(),
            ),
            _ => (t("library.smart.at_least"), t("library.smart.at_most")),
        };
        let a = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholders.0)
                .default_value(a_text)
        });
        let b = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholders.1)
                .default_value(b_text)
        });
        for input in [&a, &b] {
            self._subscriptions
                .push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }));
        }
        self.next_row += 1;
        let row = Row {
            rule: original.rule.clone(),
            original,
            a,
            b,
        };
        match list {
            List::Rules => self.rules.push(row),
            List::LeaveOut => self.leave_out.push(row),
        }
    }

    fn rows_mut(&mut self, list: List) -> &mut Vec<Row> {
        match list {
            List::Rules => &mut self.rules,
            List::LeaveOut => &mut self.leave_out,
        }
    }

    fn add(&mut self, list: List, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row(list, Condition::new(kind.new_rule()), window, cx);
        cx.notify();
    }

    /// Change a row's kind: its values start over, what a newer Irori added
    /// to the row stays.
    fn set_kind(
        &mut self,
        list: List,
        ix: usize,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows_mut(list).get(ix) else {
            return;
        };
        if row.rule.as_ref().map(Kind::of) == Some(kind) {
            return;
        }
        let original = row.original.with_rule(kind.new_rule());
        self.rows_mut(list).remove(ix);
        let len = self.rows_mut(list).len();
        self.push_row(list, original, window, cx);
        // Back to its place.
        let rows = self.rows_mut(list);
        let row = rows.remove(len);
        rows.insert(ix, row);
        cx.notify();
    }

    fn edit_rule(
        &mut self,
        list: List,
        ix: usize,
        f: impl FnOnce(&mut Rule),
        cx: &mut Context<Self>,
    ) {
        if let Some(Some(rule)) = self.rows_mut(list).get_mut(ix).map(|r| r.rule.as_mut()) {
            f(rule);
            cx.notify();
        }
    }

    fn remove(&mut self, list: List, ix: usize, cx: &mut Context<Self>) {
        let rows = self.rows_mut(list);
        if ix < rows.len() {
            rows.remove(ix);
            cx.notify();
        }
    }

    /// How its rules combine, as shown.
    #[cfg(test)]
    pub(crate) fn matching(&self) -> &Match {
        &self.matching
    }

    /// The category as it would be saved.
    pub(crate) fn draft(&self, cx: &App) -> SmartCategory {
        let mut draft = self.base.clone();
        draft.name = text(&self.name, cx);
        draft.matching = self.matching.clone();
        draft.rules = self.rules.iter().map(|r| r.condition(cx)).collect();
        draft.leave_out = self.leave_out.iter().map(|r| r.condition(cx)).collect();
        draft
    }

    fn name_result(&self, cx: &App) -> Result<String, NameError> {
        let user = &models(cx).library.read(cx).user;
        let existing: Vec<String> = user
            .categories_in_order()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        let renaming = self.id.as_ref().map(|_| self.base.name.as_str());
        match validate_name(&text(&self.name, cx), &existing, renaming) {
            Err(NameError::Unchanged) => Ok(self.base.name.clone()),
            other => other,
        }
    }

    /// How many games the draft holds, counted again only when the draft
    /// or the library changed.
    fn count(&mut self, draft: &SmartCategory, cx: &App) -> usize {
        let b = backend(cx);
        let hltb = b.hltb_times();
        let lib = models(cx).library.read(cx);
        let key = (
            lib.snapshot.revision,
            std::sync::Arc::as_ptr(&lib.user) as usize,
            hltb.revision,
        );
        if let Some((d, k, n)) = &self.preview
            && d == draft
            && *k == key
        {
            return *n;
        }
        let n = irori_services::smart_categories::count_matches(
            draft,
            &lib.snapshot,
            &lib.user,
            &hltb.hours,
            chrono::Local::now().date_naive(),
            &chrono::Local,
        );
        self.preview = Some((draft.clone(), key, n));
        n
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(name) = self.name_result(cx) else {
            return;
        };
        let mut category = self.draft(cx);
        category.name = name.clone();
        let id = self.id.clone();
        let b = backend(cx);
        irori_ui::state::run_infallible(cx, async move {
            b.userdata.save_smart_category(id.as_deref(), category);
        })
        .detach();
        window.close_dialog(cx);
        window.push_notification(
            Notification::success(t_args("library.smart.saved", &[("name", &name)])),
            cx,
        );
    }

    fn render_rows(&self, list: List, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = match list {
            List::Rules => &self.rules,
            List::LeaveOut => &self.leave_out,
        };
        rows.iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(list, ix, row, cx))
            .collect()
    }

    fn render_row(&self, list: List, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let prefix = match list {
            List::Rules => "rule",
            List::LeaveOut => "leave-out",
        };
        let id = |part: &str| SharedString::from(format!("smart-{prefix}-{ix}-{part}"));
        let editor = cx.entity().downgrade();
        let remove = Button::new(id("remove"))
            .small()
            .ghost()
            .icon(IconName::X)
            .tooltip(t("library.smart.remove_rule"))
            .on_click(cx.listener(move |this, _, _, cx| this.remove(list, ix, cx)));
        let Some(rule) = &row.rule else {
            return h_flex()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("library.smart.unknown_rule")),
                )
                .child(remove)
                .into_any_element();
        };
        let kind = Kind::of(rule);
        let kind_button = Button::new(id("kind"))
            .small()
            .outline()
            .label(kind.label())
            .dropdown_caret(true)
            .dropdown_menu({
                let editor = editor.clone();
                move |menu, _, _| {
                    Kind::ALL.into_iter().fold(menu, |menu, k| {
                        let editor = editor.clone();
                        menu.item(PopupMenuItem::new(k.label()).checked(k == kind).on_click(
                            move |_, window, cx| {
                                let _ = editor
                                    .update(cx, |this, cx| this.set_kind(list, ix, k, window, cx));
                            },
                        ))
                    })
                }
            });
        let value: AnyElement = if kind.typed() {
            let ranged = matches!(kind, Kind::Playtime | Kind::LastPlayed | Kind::Hltb);
            let unit = match kind {
                Kind::LastPlayed => Some(t("library.smart.unit.days_ago")),
                Kind::Playtime | Kind::Hltb => Some(t("library.smart.unit.hours")),
                _ => None,
            };
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().flex_1().min_w_0().child(Input::new(&row.a).small()))
                .when(ranged, |el| {
                    el.child(div().text_sm().child("–"))
                        .child(div().flex_1().min_w_0().child(Input::new(&row.b).small()))
                })
                .when_some(unit, |el, unit| {
                    el.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(unit),
                    )
                })
                .into_any_element()
        } else {
            self.render_choice(list, ix, rule, cx)
        };
        h_flex()
            .w_full()
            .gap_2()
            .flex_wrap()
            .child(kind_button)
            .child(value)
            .child(remove)
            .into_any_element()
    }

    /// The values of a rule picked from a list.
    fn render_choice(
        &self,
        list: List,
        ix: usize,
        rule: &Rule,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = cx.entity().downgrade();
        let id = SharedString::from(format!("smart-{list:?}-{ix}-value"));
        let lib = models(cx).library.read(cx);
        // (label, whether picked, how picking it changes the rule)
        type Pick = Box<dyn Fn(&mut Rule)>;
        let mut options: Vec<(SharedString, bool, Pick)> = Vec::new();
        match rule {
            Rule::Store { stores } => {
                for r in Runner::ALL {
                    options.push((
                        r.store_name().into(),
                        stores.contains(&r),
                        Box::new(move |rule| {
                            if let Rule::Store { stores } = rule
                                && !stores.remove(&r)
                            {
                                stores.insert(r);
                            }
                        }),
                    ));
                }
            }
            Rule::Installed { installed } => {
                for (v, key) in [
                    (true, "library.smart.installed_yes"),
                    (false, "library.smart.installed_no"),
                ] {
                    options.push((
                        t(key),
                        *installed == v,
                        Box::new(move |rule| *rule = Rule::Installed { installed: v }),
                    ));
                }
            }
            Rule::Status { statuses } => {
                for s in EffectiveStatus::ALL {
                    options.push((
                        irori_ui::play_status::label(s),
                        statuses.contains(&s),
                        Box::new(move |rule| {
                            if let Rule::Status { statuses } = rule
                                && !statuses.remove(&s)
                            {
                                statuses.insert(s);
                            }
                        }),
                    ));
                }
            }
            Rule::Mode { modes } => {
                for m in GameMode::ALL {
                    options.push((
                        irori_ui::game_mode::label(m),
                        modes.contains(&m),
                        Box::new(move |rule| {
                            if let Rule::Mode { modes } = rule
                                && !modes.remove(&m)
                            {
                                modes.insert(m);
                            }
                        }),
                    ));
                }
            }
            Rule::AgeRating {
                max_age,
                include_unrated,
            } => {
                for limit in crate::filter::AGE_LIMITS {
                    let unrated = *include_unrated;
                    options.push((
                        t_args("library.smart.age_up_to", &[("age", &limit.to_string())]),
                        *max_age == limit,
                        Box::new(move |rule| {
                            *rule = Rule::AgeRating {
                                max_age: limit,
                                include_unrated: unrated,
                            }
                        }),
                    ));
                }
                let (age, unrated) = (*max_age, *include_unrated);
                options.push((
                    t("library.smart.age_include_unrated"),
                    unrated,
                    Box::new(move |rule| {
                        *rule = Rule::AgeRating {
                            max_age: age,
                            include_unrated: !unrated,
                        }
                    }),
                ));
            }
            Rule::System { systems } => {
                let mut present = crate::filter::present_systems(&lib.snapshot);
                for s in systems {
                    if !present.contains(s) {
                        present.push(s.clone());
                    }
                }
                for s in present {
                    let label =
                        irori_engines::systems::get(&s).map_or(s.clone(), |sys| sys.short.clone());
                    let picked = systems.contains(&s);
                    options.push((
                        label.into(),
                        picked,
                        Box::new(move |rule| {
                            if let Rule::System { systems } = rule
                                && !systems.remove(&s)
                            {
                                systems.insert(s.clone());
                            }
                        }),
                    ));
                }
            }
            Rule::Category { names } => {
                let mut all: Vec<String> = lib.user.categories.keys().cloned().collect();
                for n in names {
                    if !all.contains(n) {
                        all.push(n.clone());
                    }
                }
                for name in all {
                    let picked = names.contains(&name);
                    options.push((
                        name.clone().into(),
                        picked,
                        Box::new(move |rule| {
                            if let Rule::Category { names } = rule
                                && !names.remove(&name)
                            {
                                names.insert(name.clone());
                            }
                        }),
                    ));
                }
            }
            Rule::Favourite {} => {
                return div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t("library.smart.favourite_hint"))
                    .into_any_element();
            }
            _ => return div().flex_1().into_any_element(),
        }
        let picked: Vec<SharedString> = options
            .iter()
            .filter(|(_, on, _)| *on)
            .map(|(l, _, _)| l.clone())
            .collect();
        let label: SharedString = if picked.is_empty() {
            t("library.smart.choose")
        } else {
            picked.join(&t("library.filter.list_separator")).into()
        };
        let options = std::rc::Rc::new(options);
        div()
            .flex_1()
            .min_w_0()
            .child(
                Button::new(id)
                    .small()
                    .outline()
                    .w_full()
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        options
                            .iter()
                            .enumerate()
                            .fold(menu, |menu, (n, (label, on, _))| {
                                let (editor, options) = (editor.clone(), options.clone());
                                menu.item(PopupMenuItem::new(label.clone()).checked(*on).on_click(
                                    move |_, _, cx| {
                                        let options = options.clone();
                                        let _ = editor.update(cx, |this, cx| {
                                            this.edit_rule(
                                                list,
                                                ix,
                                                |rule| (options[n].2)(rule),
                                                cx,
                                            )
                                        });
                                    },
                                ))
                            })
                    }),
            )
            .into_any_element()
    }

    fn render_add(&self, list: List, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = cx.entity().downgrade();
        let id = match list {
            List::Rules => "smart-add-rule",
            List::LeaveOut => "smart-add-leave-out",
        };
        Button::new(id)
            .small()
            .ghost()
            .icon(IconName::Plus)
            .label(t("library.smart.add_rule"))
            .dropdown_menu(move |menu, _, _| {
                Kind::ALL.into_iter().fold(menu, |menu, kind| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(kind.label()).on_click(move |_, window, cx| {
                            let _ = editor.update(cx, |this, cx| this.add(list, kind, window, cx));
                        }),
                    )
                })
            })
    }
}

impl Render for SmartEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.draft(cx);
        let count = self.count(&draft, cx);
        let name = self.name_result(cx);
        let muted = cx.theme().muted_foreground;
        let modes = [
            (Match::All, "library.smart.match_all"),
            (Match::Any, "library.smart.match_any"),
        ];
        let unknown_mode = matches!(self.matching, Match::Unknown(_));
        let rules = self.render_rows(List::Rules, cx);
        let leave_out = self.render_rows(List::LeaveOut, cx);
        let has_rules = !rules.is_empty();
        v_flex()
            .gap_3()
            .child(v_flex().gap_1().child(Input::new(&self.name)).when(
                name == Err(NameError::Exists),
                |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(NameError::Exists.message()),
                    )
                },
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(div().text_sm().child(t("library.smart.show_games")))
                    .child(
                        ButtonGroup::new("smart-match")
                            .small()
                            .outline()
                            .children(modes.iter().enumerate().map(|(ix, (m, key))| {
                                Button::new(SharedString::from(format!("smart-match-{ix}")))
                                    .label(t(key))
                                    .selected(self.matching == *m)
                            }))
                            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                                if let Some(&ix) = clicked.first() {
                                    this.matching = if ix == 0 { Match::All } else { Match::Any };
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .when(unknown_mode, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("library.smart.unknown_match")),
                )
            })
            .child(
                v_flex()
                    .id("smart-rules")
                    .gap_2()
                    .max_h_64()
                    .overflow_y_scroll()
                    .when(!has_rules, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("library.smart.no_rules")),
                        )
                    })
                    .children(rules),
            )
            .child(self.render_add(List::Rules, cx))
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .child(t("library.smart.leave_out")),
            )
            .child(
                v_flex()
                    .id("smart-leave-out")
                    .gap_2()
                    .max_h_48()
                    .overflow_y_scroll()
                    .children(leave_out),
            )
            .child(self.render_add(List::LeaveOut, cx))
            .child(div().text_sm().text_color(muted).child(match count {
                1 => t("library.smart.matches_one"),
                n => t_args("library.smart.matches_other", &[("count", &n.to_string())]),
            }))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("smart-cancel")
                            .ghost()
                            .label(t("library.common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("smart-save")
                            .primary()
                            .label(t("library.smart.save"))
                            .disabled(name.is_err())
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// Open the editor: of saved smart category `id`, else of a new one
/// starting from `draft` (a saved view's rules) or empty. A category this
/// version can't read at all isn't opened: it can only be kept or deleted.
pub fn open_smart_category_editor(
    id: Option<String>,
    draft: Option<SmartCategory>,
    window: &mut Window,
    cx: &mut App,
) {
    let saved = id.as_ref().and_then(|id| {
        models(cx)
            .library
            .read(cx)
            .user
            .smart_categories
            .get(id)
            .cloned()
    });
    let (id, base) = match (id, saved) {
        (Some(id), Some(saved)) if !saved.is_unreadable() => (Some(id), saved),
        (Some(_), _) => return,
        (None, _) => (None, draft.unwrap_or_default()),
    };
    let title = match &id {
        Some(_) => t_args("library.smart.edit_title", &[("name", &base.name)]),
        None => t("library.smart.new_title"),
    };
    let view = cx.new(|cx| SmartEditor::new(id, base, window, cx));
    let width = dialog_width(window, 38.);
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let input = view.read(cx).name.clone();
            input.focus_handle(cx).focus(window, cx);
        }
        dialog.title(title.clone()).w(width).child(view.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_read_back_as_they_were_written() {
        let playtime = with_texts(&Kind::Playtime.new_rule(), "2", "10");
        assert_eq!(
            playtime,
            Rule::Playtime {
                min_minutes: Some(120),
                max_minutes: Some(659)
            }
        );
        assert_eq!(texts_of(&playtime), ("2".into(), "10".into()));
        let days = with_texts(&Kind::LastPlayed.new_rule(), "30", "not a number");
        assert_eq!(
            days,
            Rule::LastPlayed {
                min_days: Some(30),
                max_days: None
            }
        );
        let tags = with_texts(&Kind::Tags.new_rule(), " Co-op , ,Puzzle", "");
        assert_eq!(texts_of(&tags), ("Co-op, Puzzle".into(), String::new()));
        // Kinds picked from lists keep their values.
        let installed = Rule::Installed { installed: false };
        assert_eq!(with_texts(&installed, "x", "y"), installed);
        for kind in Kind::ALL {
            assert_eq!(Kind::of(&kind.new_rule()), kind);
        }
    }
}
