//! The user's appearance file, `<config>/appearance.json`: colours, corner
//! radii and the other values of a gpui-kit theme, laid over whichever theme
//! shows (a picked one, the desktop's, the wallpaper's or Irori's own).
//!
//! ```json
//! {
//!   "radius": 4,
//!   "colors": { "primary.background": "#e0457b" },
//!   "dark":  { "colors": { "background": "#101014" } },
//!   "light": { "colors": { "background": "#fbf8f4" } }
//! }
//! ```
//!
//! Keys at the top apply in both colour modes; `light` and `dark` hold what
//! applies in one. The keys are a theme file's (`assets/themes/*.json`), so a
//! bundled theme's entries can be copied in; `name` and `mode` stay the
//! theme's own. Irori watches the file and applies each save after it has
//! settled, with no polling. A file that can't be used (bad JSON, a key no
//! theme has, a colour that doesn't parse) changes nothing: the look stays
//! as it was and a notice says what is wrong. Removing the file, or emptying
//! it, takes the overrides away. The system's high-contrast theme, an
//! accessibility choice, is never overridden.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::component::{ThemeConfig, try_parse_color};
use gpui_kit::{App, Global, Task};
use irori_core::events::{Notice, NoticeLevel};
use serde_json::{Map, Value};

use crate::wallpaper;

/// The file's name in Irori's configuration folder.
pub const FILE_NAME: &str = "appearance.json";

/// How long the file has to stay still before Irori reads it: editors save
/// in several steps (truncate, write, rename).
const SETTLE: Duration = Duration::from_millis(200);

/// Larger files are refused rather than read: a theme is a few kilobytes.
const MAX_BYTES: u64 = 1 << 20;

/// Keys a theme has that the file can't change.
const FIXED: &[&str] = &["name", "mode", "is_default"];

/// Where Irori looks for the file.
pub fn path() -> PathBuf {
    irori_core::Paths::get().config.join(FILE_NAME)
}

/// The overrides of a valid file.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Appearance {
    both: Map<String, Value>,
    light: Map<String, Value>,
    dark: Map<String, Value>,
}

impl Appearance {
    /// `base` with the overrides for its mode laid over it. Its name and
    /// mode stay, so the rest of the theme logic sees the same theme.
    pub(crate) fn over(&self, base: &ThemeConfig) -> ThemeConfig {
        let layer = if base.mode.is_dark() {
            &self.dark
        } else {
            &self.light
        };
        if self.both.is_empty() && layer.is_empty() {
            return base.clone();
        }
        let Ok(mut value) = serde_json::to_value(base) else {
            return base.clone();
        };
        merge(&mut value, &self.both);
        merge(&mut value, layer);
        match serde_json::from_value::<ThemeConfig>(value) {
            Ok(mut theme) => {
                theme.name = base.name.clone();
                theme.mode = base.mode;
                theme
            }
            Err(e) => {
                tracing::warn!("laying {FILE_NAME} over {}: {e}", base.name);
                base.clone()
            }
        }
    }
}

/// Lay `layer` over `target`, object by object. `null` leaves the theme's
/// own value.
fn merge(target: &mut Value, layer: &Map<String, Value>) {
    let Value::Object(target) = target else {
        return;
    };
    for (key, value) in layer {
        match (target.get_mut(key), value) {
            (_, Value::Null) => {}
            (Some(existing @ Value::Object(_)), Value::Object(inner)) => merge(existing, inner),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// What reading the file found.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Read {
    Missing,
    Found(Appearance),
    /// The file can't be used; why, in English (it names keys and lines).
    Invalid(String),
}

/// Read and check the file at `path`.
pub(crate) fn read(path: &Path) -> Read {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Read::Missing,
        Err(e) => return Read::Invalid(e.to_string()),
        Ok(meta) if meta.len() > MAX_BYTES => {
            return Read::Invalid(format!("the file is larger than {} KiB", MAX_BYTES / 1024));
        }
        Ok(_) => {}
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(appearance) => Read::Found(appearance),
            Err(e) => Read::Invalid(e),
        },
        Err(e) if e.kind() == ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Invalid(e.to_string()),
    }
}

/// Check the file's text. An empty file overrides nothing.
pub(crate) fn parse(text: &str) -> Result<Appearance, String> {
    if text.trim().is_empty() {
        return Ok(Appearance::default());
    }
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Value::Object(top) = value else {
        return Err("the file must hold a JSON object".into());
    };
    let mut appearance = Appearance::default();
    for (key, value) in top {
        match key.as_str() {
            "$schema" => {}
            "light" | "dark" => {
                let Value::Object(inner) = value else {
                    return Err(format!("“{key}” must be an object"));
                };
                check_level(&inner, Some(&key))?;
                if key == "light" {
                    appearance.light = inner;
                } else {
                    appearance.dark = inner;
                }
            }
            _ => {
                appearance.both.insert(key, value);
            }
        }
    }
    check_level(&appearance.both, None)?;
    Ok(appearance)
}

/// Check one level's keys and values against a theme's.
fn check_level(level: &Map<String, Value>, mode: Option<&str>) -> Result<(), String> {
    let at = |key: &str| match mode {
        Some(mode) => format!("{mode}.{key}"),
        None => key.to_string(),
    };
    let known = serde_json::to_value(ThemeConfig::default()).unwrap_or_default();
    for key in level.keys() {
        if FIXED.contains(&key.as_str()) {
            return Err(format!("“{}” stays the theme’s own", at(key)));
        }
        if matches!(key.as_str(), "light" | "dark" | "$schema") && mode.is_some() {
            return Err(format!("“{}” belongs at the top of the file", at(key)));
        }
        if known.get(key).is_none() {
            return Err(format!("“{}” is not a theme key", at(key)));
        }
    }
    if let Some(colors) = level.get("colors") {
        let Value::Object(colors) = colors else {
            return Err(format!("“{}” must be an object", at("colors")));
        };
        let known = &known["colors"];
        for (key, value) in colors {
            let name = at(&format!("colors.{key}"));
            if known.get(key).is_none() {
                return Err(format!("“{name}” is not a theme colour"));
            }
            match value {
                Value::Null => {}
                Value::String(color) if try_parse_color(color).is_ok() => {}
                _ => return Err(format!("“{name}” is not a colour: {value}")),
            }
        }
    }
    let values: Map<String, Value> = level
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    serde_json::from_value::<ThemeConfig>(Value::Object(values))
        .map_err(|e| format!("{}: {e}", mode.unwrap_or("the file")))?;
    Ok(())
}

/// What to do after a read, so a file saved again with the same contents,
/// or with the same mistake, does nothing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Step {
    Apply(Appearance),
    /// The file went away (or was emptied): back to the theme as it is.
    Clear,
    /// Keep the look and tell the user.
    Report(String),
}

/// Follows the reads of the file.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    shown: Option<Appearance>,
    reported: Option<String>,
}

impl Tracker {
    pub(crate) fn next(&mut self, read: Read) -> Option<Step> {
        match read {
            Read::Invalid(error) => {
                if self.reported.as_ref() == Some(&error) {
                    return None;
                }
                self.reported = Some(error.clone());
                Some(Step::Report(error))
            }
            Read::Missing => {
                self.reported = None;
                self.shown.take().map(|_| Step::Clear)
            }
            Read::Found(appearance) if appearance == Appearance::default() => {
                self.next(Read::Missing)
            }
            Read::Found(appearance) => {
                self.reported = None;
                if self.shown.as_ref() == Some(&appearance) {
                    return None;
                }
                self.shown = Some(appearance.clone());
                Some(Step::Apply(appearance))
            }
        }
    }
}

/// The overrides in force, read by [`crate::theme`].
pub(crate) struct Override {
    pub(crate) appearance: Rc<Appearance>,
    /// Counted across saves, so new colours under the same theme name apply.
    pub(crate) revision: u64,
}

impl Global for Override {}

/// Keeps the watch running.
struct Follow {
    _task: Task<()>,
}

impl Global for Follow {}

/// Apply the user's appearance file and follow its changes.
pub(crate) fn follow(cx: &mut App) {
    follow_file(path(), cx);
}

/// [`follow`] for the file at `path`. The first read is on the UI thread,
/// as the desktop's theme files are, so the first window already shows the
/// user's colours; the file is small and local.
pub(crate) fn follow_file(path: PathBuf, cx: &mut App) {
    // Watching before the first read, so no save falls between the two.
    let watch = wallpaper::Watch::new(&path);
    let mut tracker = Tracker::default();
    if let Some(step) = tracker.next(read(&path)) {
        act(step, &path, cx);
    }
    let task = cx.spawn(async move |cx| {
        let Some((mut watch, mut changes)) = watch else {
            return;
        };
        while changes.next().await.is_some() {
            cx.background_executor().timer(SETTLE).await;
            while changes.try_recv().is_ok() {}
            watch.rewatch();
            let file = path.clone();
            let found = cx
                .background_executor()
                .spawn(async move { read(&file) })
                .await;
            if let Some(step) = tracker.next(found) {
                let path = path.clone();
                cx.update(|cx| act(step, &path, cx));
            }
        }
    });
    cx.set_global(Follow { _task: task });
}

fn act(step: Step, path: &Path, cx: &mut App) {
    match step {
        Step::Apply(appearance) => {
            let revision = cx.try_global::<Override>().map_or(0, |o| o.revision) + 1;
            tracing::info!("applying {}", path.display());
            cx.set_global(Override {
                appearance: Rc::new(appearance),
                revision,
            });
            reapply(cx);
        }
        Step::Clear => {
            if cx.has_global::<Override>() {
                cx.remove_global::<Override>();
            }
            reapply(cx);
        }
        Step::Report(error) => {
            tracing::warn!("{} not applied: {error}", path.display());
            let file = path.display().to_string();
            let Some(backend) = crate::state::try_backend(cx) else {
                return;
            };
            // Held until the UI listens when this is the first read.
            backend.bus.startup_notice(
                Notice::new(NoticeLevel::Warning, "The appearance file wasn’t applied")
                    .message(format!("{file}: {error}"))
                    .keys(
                        "ui.appearance.invalid",
                        Some("ui.appearance.invalid_detail"),
                        &[("file", &file), ("error", &error)],
                    ),
            );
        }
    }
}

/// Apply the user's preferences again; the defaults where there is no
/// backend to hold them.
fn reapply(cx: &mut App) {
    if crate::state::try_backend(cx).is_some() {
        crate::theme::reapply(cx);
    } else {
        crate::theme::apply(&irori_core::settings::UiPrefs::default(), None, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::ThemeMode;

    use super::*;

    fn theme(mode: ThemeMode) -> ThemeConfig {
        let set: gpui_kit::component::ThemeSet =
            serde_json::from_str(include_str!("../../../assets/themes/irori.json")).unwrap();
        set.themes
            .into_iter()
            .find(|t| t.mode == mode)
            .expect("a theme of that mode")
    }

    #[test]
    fn overrides_apply_per_mode_and_keep_the_rest() {
        let a = parse(
            r##"{
                "$schema": "x",
                "radius": 3,
                "colors": { "primary.background": "#112233", "border": null },
                "dark": { "colors": { "background": "#000000" } },
                "light": { "colors": { "background": "#ffffff" }, "shadow": false }
            }"##,
        )
        .unwrap();
        let dark = theme(ThemeMode::Dark);
        let over = a.over(&dark);
        assert_eq!(over.name, dark.name);
        assert_eq!(over.mode, ThemeMode::Dark);
        assert_eq!(over.radius, Some(3));
        assert_eq!(over.colors.primary.as_deref(), Some("#112233"));
        assert_eq!(over.colors.background.as_deref(), Some("#000000"));
        // `null` and untouched keys keep the theme's own.
        assert_eq!(over.colors.border, dark.colors.border);
        assert_eq!(over.colors.foreground, dark.colors.foreground);
        assert_eq!(over.shadow, dark.shadow);

        let light = theme(ThemeMode::Light);
        let over = a.over(&light);
        assert_eq!(over.colors.background.as_deref(), Some("#ffffff"));
        assert_eq!(over.shadow, Some(false));
    }

    #[test]
    fn nothing_to_lay_over_is_the_theme_itself() {
        let dark = theme(ThemeMode::Dark);
        let over = parse("  \n").unwrap().over(&dark);
        assert_eq!(
            serde_json::to_value(&over).unwrap(),
            serde_json::to_value(&dark).unwrap()
        );
        // Only the light mode is overridden: dark stays as it was.
        let over = parse(r##"{"light": {"radius": 1}}"##).unwrap().over(&dark);
        assert_eq!(
            serde_json::to_value(&over).unwrap(),
            serde_json::to_value(&dark).unwrap()
        );
    }

    #[test]
    fn mistakes_are_named() {
        let error = |text: &str| parse(text).unwrap_err();
        assert!(error("{ \"radius\": 3,").contains("line 1"));
        assert!(error("[1, 2]").contains("JSON object"));
        assert!(error(r#"{"radios": 3}"#).contains("“radios”"));
        assert!(
            error(r##"{"colors": {"primry.background": "#fff"}}"##)
                .contains("“colors.primry.background”")
        );
        assert!(
            error(r#"{"dark": {"colors": {"background": "nope"}}}"#)
                .contains("“dark.colors.background”")
        );
        assert!(error(r#"{"colors": {"background": 12}}"#).contains("not a colour"));
        assert!(error(r#"{"radius": "big"}"#).contains("invalid type"));
        assert!(error(r#"{"name": "Mine"}"#).contains("“name”"));
        assert!(error(r#"{"dark": {"mode": "light"}}"#).contains("“dark.mode”"));
        assert!(error(r#"{"dark": 1}"#).contains("“dark” must be an object"));
        assert!(error(r#"{"dark": {"light": {}}}"#).contains("top of the file"));
        assert!(error(r#"{"colors": "red"}"#).contains("must be an object"));
    }

    #[test]
    fn reads_tell_missing_from_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        assert_eq!(read(&file), Read::Missing);
        std::fs::write(&file, r##"{"colors": {"ring": "#ff0000"}}"##).unwrap();
        assert!(matches!(read(&file), Read::Found(_)));
        std::fs::write(&file, "{").unwrap();
        assert!(matches!(read(&file), Read::Invalid(_)));
        std::fs::write(&file, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(matches!(read(&file), Read::Invalid(e) if e.contains("larger")));
    }

    #[test]
    fn the_tracker_applies_changes_and_reports_each_mistake_once() {
        let a = parse(r##"{"radius": 2}"##).unwrap();
        let b = parse(r##"{"radius": 4}"##).unwrap();
        let mut t = Tracker::default();
        assert_eq!(t.next(Read::Missing), None);
        assert_eq!(t.next(Read::Found(a.clone())), Some(Step::Apply(a.clone())));
        // Saved again unchanged.
        assert_eq!(t.next(Read::Found(a.clone())), None);
        // A mistake keeps the look and is told once, until it changes.
        assert_eq!(
            t.next(Read::Invalid("x".into())),
            Some(Step::Report("x".into()))
        );
        assert_eq!(t.next(Read::Invalid("x".into())), None);
        assert_eq!(
            t.next(Read::Invalid("y".into())),
            Some(Step::Report("y".into()))
        );
        // Fixed back to what shows: nothing to redo.
        assert_eq!(t.next(Read::Found(a.clone())), None);
        assert_eq!(
            t.next(Read::Invalid("x".into())),
            Some(Step::Report("x".into()))
        );
        assert_eq!(t.next(Read::Found(b.clone())), Some(Step::Apply(b)));
        // Emptied or removed: the overrides go, once.
        assert_eq!(
            t.next(Read::Found(Appearance::default())),
            Some(Step::Clear)
        );
        assert_eq!(t.next(Read::Missing), None);
        assert_eq!(t.next(Read::Found(a.clone())), Some(Step::Apply(a)));
        assert_eq!(t.next(Read::Missing), Some(Step::Clear));
    }

    /// Runs the app until `done` holds, letting real time pass for the file
    /// watch and the app's clock for the settling delay.
    fn wait_for(cx: &mut gpui_kit::TestAppContext, mut done: impl FnMut(&mut App) -> bool) -> bool {
        for _ in 0..100 {
            cx.run_until_parked();
            if cx.update(&mut done) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            cx.executor().advance_clock(Duration::from_millis(300));
        }
        false
    }

    #[gpui_kit::test]
    fn saves_apply_live_and_a_broken_file_keeps_the_look(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::ActiveTheme as _;

        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let file = dir.join(FILE_NAME);
        let colors = |hex: &str| format!(r#"{{"colors": {{"background": "{hex}"}}}}"#);
        std::fs::write(&file, colors("#123456")).unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(cx);
            crate::theme::apply(&irori_core::settings::UiPrefs::default(), None, cx);
            follow_file(file.clone(), cx);
        });
        // GPUI keeps colours as HSL, which can move a channel by one.
        let shows = |cx: &mut App, hex: u32| {
            let (shown, wanted) = (cx.theme().background.to_rgb(), gpui_kit::rgb(hex));
            [shown.r - wanted.r, shown.g - wanted.g, shown.b - wanted.b]
                .iter()
                .all(|d| d.abs() < 1.5 / 255.0)
        };
        assert!(cx.update(|cx| shows(cx, 0x123456)), "the first read");

        // Saved the way editors do: a new file renamed over the old one.
        let partial = dir.join("appearance.json.tmp");
        std::fs::write(&partial, colors("#654321")).unwrap();
        std::fs::rename(&partial, &file).unwrap();
        assert!(
            wait_for(cx, |cx| shows(cx, 0x654321)),
            "the save never applied"
        );

        // A mistake keeps the look as it is.
        std::fs::write(&file, r##"{"colors": {"background": "#zz"}}"##).unwrap();
        std::fs::write(&file, colors("not a colour")).unwrap();
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(50));
            cx.executor().advance_clock(Duration::from_millis(300));
            cx.run_until_parked();
        }
        assert!(
            cx.update(|cx| shows(cx, 0x654321)),
            "a broken file changed the look"
        );

        // Removed: the theme's own colours return.
        std::fs::remove_file(&file).unwrap();
        assert!(
            wait_for(cx, |cx| !shows(cx, 0x654321)),
            "removing the file kept its colours"
        );
    }

    #[test]
    fn every_bundled_theme_survives_the_round_trip() {
        let a = parse(r##"{"colors": {"ring": "#ff0000"}}"##).unwrap();
        for json in crate::theme::bundled() {
            let set: gpui_kit::component::ThemeSet = serde_json::from_str(json).unwrap();
            for t in set.themes {
                let over = a.over(&t);
                assert_eq!(over.colors.ring.as_deref(), Some("#ff0000"), "{}", t.name);
                let mut before = serde_json::to_value(&t).unwrap();
                let mut after = serde_json::to_value(&over).unwrap();
                before["colors"]["ring"] = Value::Null;
                after["colors"]["ring"] = Value::Null;
                assert_eq!(before, after, "{}", t.name);
            }
        }
    }
}
