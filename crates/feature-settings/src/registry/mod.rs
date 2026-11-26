//! The field registry: one static description per editable setting.
//!
//! A field is addressed by its JSON path in the serialized settings
//! (`AppSettings` for application fields, `GameSettings` for launch fields;
//! the two share keys because `AppSettings` flattens its game defaults). A
//! dotted path reaches into an object (`gamescope.fpsLimiter`,
//! `iroriUi.fontSize`). The first segment of a game field's path is the key
//! stored in a game's sparse override map.
//!
//! Labels, help, group names, tags, placeholders and option names are
//! translation keys (`settings.field.<id>.label`, …) resolved when rendered,
//! so a language change applies on the next frame. Keywords stay English and
//! the search also matches the English text, so a query works whichever
//! language the interface uses.
//!
//! The registry drives rendering order, grouping and the settings search.

mod fields_app;
mod fields_general;
mod fields_launch;
mod fields_runtime;
mod options;
mod types;

#[cfg(test)]
mod tests;

pub use types::{
    ActionKind, ChoiceOption, ChoiceValue, Condition, CustomKind, FieldKind, FieldSection,
    FieldSpec, Label, Need, NeedContext, PanelKind, Scope, TextKind,
};

const SECTIONS: &[&[FieldSpec]] = &[
    fields_general::FIELDS,
    fields_app::APPEARANCE,
    fields_app::ADVANCED,
    fields_runtime::FIELDS,
    fields_launch::FIELDS,
    fields_app::SAVES,
];

const fn total_fields(sections: &[&[FieldSpec]]) -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < sections.len() {
        total += sections[i].len();
        i += 1;
    }
    total
}

const fn concat_fields<const N: usize>(sections: &[&[FieldSpec]]) -> [FieldSpec; N] {
    let mut out = [sections[0][0]; N];
    let mut out_idx = 0;
    let mut s = 0;
    while s < sections.len() {
        let section = sections[s];
        let mut i = 0;
        while i < section.len() {
            out[out_idx] = section[i];
            out_idx += 1;
            i += 1;
        }
        s += 1;
    }
    out
}

const TOTAL_FIELDS: usize = total_fields(SECTIONS);

static FIELDS_DATA: [FieldSpec; TOTAL_FIELDS] = concat_fields(SECTIONS);

/// Every field, in display order.
pub static FIELDS: &[FieldSpec] = &FIELDS_DATA;

/// The field with this path.
pub fn find(path: &str) -> Option<&'static FieldSpec> {
    FIELDS.iter().find(|f| f.path == path)
}

/// Fields of one section, in display order.
pub fn section_fields(section: FieldSection) -> impl Iterator<Item = &'static FieldSpec> {
    FIELDS
        .iter()
        .filter(move |f| f.supported_on_host() && f.section == section)
}

/// Fields matching a search query, in display order. Empty for a blank query.
pub fn search(query: &str) -> impl Iterator<Item = &'static FieldSpec> + '_ {
    FIELDS
        .iter()
        .filter(move |f| f.supported_on_host() && f.matches(query))
}
