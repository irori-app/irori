//! Settings lists that merge by key: environment variables by name, wrappers
//! by command and programs started alongside by path.
//!
//! A game's override of one of these lists holds only the game's own entries.
//! They are laid over the defaults' list ([`merge`]): an entry whose key matches
//! a default entry replaces it in place, the others follow the defaults, and an
//! entry marked [`REMOVED`] (`{"key": "DXVK_HUD", "iroriRemoved": true}`) takes
//! the default entry with its key out for that game. So a variable added to the
//! defaults later still reaches a game that set variables of its own.
//!
//! Lists written before this rule, which copied every default entry, read the
//! same: each of their entries replaces the default entry it copied.
//! Launch presets merge their lists into a game's the same way.

use serde_json::{Map, Value};

/// Settings keys whose lists merge by a key field, with that field.
pub const MERGED_LISTS: &[(&str, &str)] = &[
    ("enviromentOptions", "key"),
    ("wrapperOptions", "exe"),
    ("iroriLaunchPrograms", "path"),
];

/// Field of an override entry that takes the default entry with its key out.
pub const REMOVED: &str = "iroriRemoved";

/// The key field of a merged list, `None` for other settings keys.
pub fn key_field(key: &str) -> Option<&'static str> {
    MERGED_LISTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, field)| *field)
}

fn is_removal(entry: &Value) -> bool {
    entry.get(REMOVED).and_then(Value::as_bool) == Some(true)
}

/// `own` laid over `base`: each entry replaces the first entry of `base` with
/// its key that no earlier entry replaced, a removal takes that entry out, and
/// the rest follow in their order. Removals of keys `base` lacks do nothing.
pub fn merge(base: Option<&Value>, own: &Value, field: &str) -> Value {
    let mut slots: Vec<Option<Value>> = base
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .map(Some)
        .collect();
    let mut taken = vec![false; slots.len()];
    let mut added = Vec::new();
    for entry in own.as_array().into_iter().flatten() {
        let key = entry.get(field).filter(|k| !k.is_null());
        let slot = key.and_then(|key| {
            (0..slots.len())
                .find(|&i| !taken[i] && slots[i].as_ref().and_then(|e| e.get(field)) == Some(key))
        });
        match (slot, is_removal(entry)) {
            (Some(i), true) => {
                taken[i] = true;
                slots[i] = None;
            }
            (Some(i), false) => {
                taken[i] = true;
                slots[i] = Some(entry.clone());
            }
            (None, true) => {}
            (None, false) => added.push(entry.clone()),
        }
    }
    Value::Array(slots.into_iter().flatten().chain(added).collect())
}

/// The override that makes `base` read as `wanted`: the entries that differ
/// and removals of the default entries `wanted` left out. `None` when
/// `wanted` is `base`, so the game follows the defaults.
///
/// When the merge can't give `wanted`'s order (a default entry moved, keys
/// repeated), every default entry is removed and `wanted` follows whole, so
/// the game still reads exactly what was asked for.
pub fn own_entries(base: Option<&Value>, wanted: &Value, field: &str) -> Option<Value> {
    let empty = Vec::new();
    let defaults = base.and_then(Value::as_array).unwrap_or(&empty);
    let wanted_list = wanted.as_array().unwrap_or(&empty);
    if defaults == wanted_list {
        return None;
    }
    let mut used = vec![false; wanted_list.len()];
    let mut own = Vec::new();
    for default in defaults {
        let key = default.get(field);
        let found = (0..wanted_list.len())
            .find(|&i| !used[i] && key.is_some() && wanted_list[i].get(field) == key);
        match found {
            Some(i) => {
                used[i] = true;
                if &wanted_list[i] != default {
                    own.push(wanted_list[i].clone());
                }
            }
            None => own.push(removal(key, field)),
        }
    }
    own.extend(
        wanted_list
            .iter()
            .zip(&used)
            .filter(|(_, used)| !**used)
            .map(|(e, _)| e.clone()),
    );
    let own = Value::Array(own);
    if merge(base, &own, field) == Value::Array(wanted_list.clone()) {
        return Some(own);
    }
    let whole = defaults
        .iter()
        .map(|d| removal(d.get(field), field))
        .chain(wanted_list.iter().cloned())
        .collect();
    Some(Value::Array(whole))
}

/// A list written before lists merged, when it replaced `base` whole, as
/// the override that still reads exactly as it did: `None` when merging
/// already gives the same list (it copied every default entry it meant to
/// keep), else its own entries and removals of the default entries it left
/// out.
pub fn from_whole_list(base: Option<&Value>, old: &Value, field: &str) -> Option<Value> {
    if !old.is_array() || merge(base, old, field) == *old {
        return None;
    }
    // `own_entries` is `None` only when `old` equals `base`, which merges
    // to itself and returned above.
    own_entries(base, old, field)
}

fn removal(key: Option<&Value>, field: &str) -> Value {
    let mut entry = Map::new();
    entry.insert(field.to_string(), key.cloned().unwrap_or(Value::Null));
    entry.insert(REMOVED.to_string(), Value::Bool(true));
    Value::Object(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &[(&str, &str)]) -> Value {
        Value::Array(
            pairs
                .iter()
                .map(|(k, v)| json!({"key": k, "value": v}))
                .collect(),
        )
    }

    #[test]
    fn own_entries_follow_and_replace_the_defaults() {
        let defaults = env(&[("FOO", "1"), ("BAR", "2")]);
        let own = env(&[("BAR", "3"), ("XALIA", "0")]);
        assert_eq!(
            merge(Some(&defaults), &own, "key"),
            env(&[("FOO", "1"), ("BAR", "3"), ("XALIA", "0")])
        );
    }

    #[test]
    fn a_removal_takes_a_default_out() {
        let defaults = env(&[("FOO", "1"), ("BAR", "2")]);
        let own = json!([{"key": "FOO", "iroriRemoved": true}]);
        assert_eq!(merge(Some(&defaults), &own, "key"), env(&[("BAR", "2")]));
        assert_eq!(merge(None, &own, "key"), json!([]));
    }

    #[test]
    fn a_list_that_copied_the_defaults_reads_the_same() {
        let defaults = env(&[("FOO", "1"), ("BAR", "2")]);
        let copied = env(&[("FOO", "1"), ("BAR", "5"), ("OWN", "x")]);
        assert_eq!(merge(Some(&defaults), &copied, "key"), copied);
    }

    #[test]
    fn own_entries_keep_only_what_differs() {
        let defaults = env(&[("FOO", "1"), ("BAR", "2")]);
        assert_eq!(own_entries(Some(&defaults), &defaults, "key"), None);
        let wanted = env(&[("BAR", "3"), ("NEW", "x")]);
        let own = own_entries(Some(&defaults), &wanted, "key").expect("differs");
        assert_eq!(
            own,
            json!([
                {"key": "FOO", "iroriRemoved": true},
                {"key": "BAR", "value": "3"},
                {"key": "NEW", "value": "x"},
            ])
        );
        assert_eq!(merge(Some(&defaults), &own, "key"), wanted);
    }

    #[test]
    fn a_new_order_is_kept_whole() {
        let defaults = json!([{"exe": "a", "args": ""}, {"exe": "b", "args": ""}]);
        let wanted = json!([{"exe": "b", "args": ""}, {"exe": "a", "args": ""}]);
        let own = own_entries(Some(&defaults), &wanted, "exe").expect("differs");
        assert_eq!(merge(Some(&defaults), &own, "exe"), wanted);
    }

    #[test]
    fn repeated_keys_survive() {
        let defaults = json!([{"exe": "env", "args": "A=1"}]);
        let wanted = json!([
            {"exe": "env", "args": "A=1"},
            {"exe": "env", "args": "B=2"},
        ]);
        let own = own_entries(Some(&defaults), &wanted, "exe").expect("differs");
        assert_eq!(merge(Some(&defaults), &own, "exe"), wanted);
    }
}
