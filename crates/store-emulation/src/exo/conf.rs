//! Cross-platform `dosbox.conf` translation and opt-in native script launching
//! for local eXo collections (`eXoDOS`, `eXoWin3x`, `eXoDemoscene`).
//!
//! Translates eXo's Windows-authored `dosbox.conf` into a generated config under
//! Irori's cache directory without modifying any file inside the user's eXo
//! collection folder.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::json_store;
use irori_core::process::CommandSpec;
use serde_json::Value;

use super::ExoEntryMeta;

/// Maximum `dosbox.conf` size read from disk (2 MiB).
const MAX_CONF_BYTES: u64 = 2 * 1024 * 1024;

/// Local MIDI assets found inside the user's own eXo collection (never downloaded).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LocalMidiAssets {
    pub mt32_dir: Option<PathBuf>,
    pub soundfont: Option<PathBuf>,
}

/// Base directory eXo `dosbox.conf` relative `.\` paths resolve against
/// (`<exo_root>/eXo` when present, else `<exo_root>`).
pub fn exo_host_base(exo: &ExoEntryMeta) -> PathBuf {
    let sub = exo.exo_root.join("eXo");
    if sub.is_dir() {
        sub
    } else {
        exo.exo_root.clone()
    }
}

/// Generate a DOSBox Staging-compatible `.conf` inside `launch_dir` for `exo`
/// and return its path. Never writes inside `exo.exo_root`.
pub fn prepare_dosbox_conf(
    exo: &ExoEntryMeta,
    launch_dir: &Path,
    app_name: &str,
    values: &BTreeMap<String, Value>,
) -> Result<PathBuf> {
    let host_base = exo_host_base(exo);
    let raw = match &exo.conf_path {
        Some(conf) if conf.is_file() => read_conf_bounded(conf)?,
        _ => fallback_conf(&exo.game_dir),
    };
    let midi = detect_local_midi(&exo.exo_root);
    let fullscreen = values
        .get("fullscreen")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let translated = translate_conf(&raw, &host_base, &midi, fullscreen);
    let safe_name = irori_core::paths::sanitize(app_name);
    let out_path = launch_dir.join(format!("{safe_name}.conf"));
    json_store::write_bytes(&out_path, translated.as_bytes())
        .with_context(|| format!("writing launch config {}", out_path.display()))?;
    Ok(out_path)
}

fn read_conf_bounded(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("reading metadata of {}", path.display()))?;
    if meta.len() > MAX_CONF_BYTES {
        bail!("{} exceeds the 2 MiB config size limit", path.display());
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn fallback_conf(game_dir: &Path) -> String {
    let dir = path_for_dosbox(game_dir);
    format!("[autoexec]\nmount c \"{dir}\"\nc:\n")
}

/// Pure translation from an eXo `dosbox.conf` to a DOSBox Staging-compatible config.
pub fn translate_conf(
    raw: &str,
    host_base: &Path,
    midi: &LocalMidiAssets,
    fullscreen: bool,
) -> String {
    let rewritten = rewrite_host_paths(raw, &|rel| {
        let resolved = resolve_host_rel(host_base, rel);
        trim_trailing_sep(&path_for_dosbox(&resolved))
    });

    let mut out = String::with_capacity(rewritten.len() + 256);
    let mut section = String::new();
    let mut uses_mt32 = false;
    let mut uses_default_gm = false;
    let mut has_mt32_romdir = false;
    let mut has_fluid_soundfont = false;

    for line in rewritten.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_ascii_lowercase();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if section == "autoexec" || trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            let val_lower = val.to_ascii_lowercase();

            // DOSBox-ECE output modes not accepted by DOSBox Staging.
            if key == "output" && matches!(val_lower.as_str(), "openglpp" | "surfacepp") {
                out.push_str("output = texture\n");
                continue;
            }
            // Strip ECE-only pixelshader directives that Staging rejects.
            if key == "pixelshader" {
                continue;
            }
            // Translate legacy `mt32.romdir` / `fluid.soundfont` keys.
            if key == "mt32.romdir" || (section == "mt32" && key == "romdir") {
                has_mt32_romdir = true;
                if section == "mt32" {
                    out.push_str(&format!("romdir = {val}\n"));
                } else {
                    // Emit under `[mt32]` in the override tail if outside `[mt32]`.
                    has_mt32_romdir = false;
                }
                continue;
            }
            if key == "fluid.soundfont" || (section == "fluid" && key == "soundfont") {
                has_fluid_soundfont = true;
                if section == "fluid" {
                    out.push_str(&format!("soundfont = {val}\n"));
                } else {
                    has_fluid_soundfont = false;
                }
                continue;
            }
            if key == "mididevice" {
                if val_lower == "mt32" {
                    uses_mt32 = true;
                } else if matches!(val_lower.as_str(), "default" | "win32" | "auto") {
                    uses_default_gm = true;
                }
            }
            // Let Irori's fullscreen toggle take effect cleanly in the override block.
            if section == "sdl" && key == "fullscreen" {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    // Append Irori's DOSBox Staging compatibility & option overrides.
    out.push_str("\n[sdl]\n");
    out.push_str(&format!(
        "fullscreen = {}\n",
        if fullscreen { "true" } else { "false" }
    ));

    if uses_mt32
        && !has_mt32_romdir
        && let Some(mt32_dir) = &midi.mt32_dir
    {
        out.push_str("\n[mt32]\n");
        out.push_str(&format!("romdir = {}\n", path_for_dosbox(mt32_dir)));
    }
    if uses_default_gm
        && !cfg!(windows)
        && !has_fluid_soundfont
        && let Some(sf) = &midi.soundfont
    {
        out.push_str("\n[midi]\nmididevice = fluidsynth\n");
        out.push_str("\n[fluid]\n");
        out.push_str(&format!("soundfont = {}\n", path_for_dosbox(sf)));
    }

    out
}

/// Rewrite ONLY `.\`-relative host paths inside `text`, leaving DOS guest paths
/// (`path=C:\;Z:\`, `cd \game`) untouched.
pub fn rewrite_host_paths(text: &str, resolve: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    let mut consumed = 0usize;
    while let Some(idx) = rest.find(".\\") {
        out.push_str(&rest[..idx]);
        let quoted = out.ends_with('"');
        let tail = &rest[idx + 2..];
        let end = tail
            .find(|c: char| if quoted { c == '"' } else { c.is_whitespace() })
            .unwrap_or(tail.len());
        let replacement = resolve(&tail[..end]);
        if !quoted && replacement.contains(' ') && on_mount_line(text, consumed + idx) {
            out.push('"');
            out.push_str(&replacement);
            out.push('"');
        } else {
            out.push_str(&replacement);
        }
        consumed += idx + 2 + end;
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn on_mount_line(text: &str, pos: usize) -> bool {
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let head = text[line_start..pos].trim_start().trim_start_matches('@');
    let cmd = head.split_whitespace().next().unwrap_or("");
    cmd.eq_ignore_ascii_case("mount") || cmd.eq_ignore_ascii_case("imgmount")
}

fn trim_trailing_sep(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path.to_string()
    } else {
        trimmed.to_string()
    }
}

fn path_for_dosbox(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Resolve a relative Windows-style token against `host_base`, falling back to
/// case-insensitive segment lookup on case-sensitive Unix filesystems.
fn resolve_host_rel(host_base: &Path, rel: &str) -> PathBuf {
    let normalized = rel.replace('\\', "/");
    let direct = host_base.join(&normalized);
    if direct.exists() {
        return direct;
    }
    if let Some(found) = resolve_case_insensitive(host_base, Path::new(&normalized)) {
        return found;
    }
    // Also try resolving against `host_base`'s parent if the path already starts with `eXo/`.
    if let Some(parent) = host_base.parent() {
        let alt = parent.join(&normalized);
        if alt.exists() {
            return alt;
        }
        if let Some(found) = resolve_case_insensitive(parent, Path::new(&normalized)) {
            return found;
        }
    }
    direct
}

fn resolve_case_insensitive(base: &Path, rel: &Path) -> Option<PathBuf> {
    let mut cur = base.to_path_buf();
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            if comp == Component::ParentDir {
                cur.pop();
            }
            continue;
        };
        let exact = cur.join(name);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let want = name.to_string_lossy().to_ascii_lowercase();
        let mut matches: Vec<_> = std::fs::read_dir(&cur)
            .ok()?
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().to_ascii_lowercase() == want)
            .collect();
        matches.sort();
        let picked = matches.into_iter().next()?;
        cur.push(picked);
    }
    Some(cur)
}

/// Discover local Roland MT-32 ROMs and General MIDI `.sf2` soundfonts already
/// stored inside the user's eXo collection folder.
pub fn detect_local_midi(exo_root: &Path) -> LocalMidiAssets {
    let mt32_candidates = [
        exo_root.join("eXo/emulators/dosbox/mt32"),
        exo_root.join("eXo/mt32"),
        exo_root.join("eXo/util/mt32"),
    ];
    let mt32_dir = mt32_candidates.into_iter().find(|d| has_mt32_roms(d));

    let sf_candidates = [
        exo_root.join("eXo/emulators/dosbox/soundfonts"),
        exo_root.join("eXo/soundfonts"),
        exo_root.join("eXo/util/soundfonts"),
        exo_root.join("eXo/emulators/dosbox"),
    ];
    let soundfont = sf_candidates.into_iter().find_map(|d| first_sf2(&d));

    LocalMidiAssets {
        mt32_dir,
        soundfont,
    }
}

fn has_mt32_roms(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    let names: Vec<String> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
        .collect();
    let has_mt32 =
        names.iter().any(|n| n == "mt32_control.rom") && names.iter().any(|n| n == "mt32_pcm.rom");
    let has_cm32l = names.iter().any(|n| n == "cm32l_control.rom")
        && names.iter().any(|n| n == "cm32l_pcm.rom");
    has_mt32 || has_cm32l
}

fn first_sf2(dir: &Path) -> Option<PathBuf> {
    let mut fonts: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("sf2"))
        })
        .collect();
    fonts.sort();
    fonts.into_iter().next()
}

/// Build the opt-in command to run an eXo game's own native launch script
/// (`.command`/`.bsh` on Unix, `.bat` on Windows).
///
/// Only called when the user explicitly opts in via `iroriExoUseNativeScript`.
pub fn native_script_command(exo: &ExoEntryMeta) -> Result<CommandSpec> {
    let script = exo.script_path.as_ref().with_context(|| {
        format!(
            "No native eXo launch script was found in {}",
            exo.config_dir.display()
        )
    })?;
    if !script.is_file() {
        bail!("eXo launch script {} was not found", script.display());
    }
    let root_canon = std::fs::canonicalize(&exo.exo_root)
        .with_context(|| format!("resolving {}", exo.exo_root.display()))?;
    let script_canon =
        std::fs::canonicalize(script).with_context(|| format!("resolving {}", script.display()))?;
    if !script_canon.starts_with(&root_canon) {
        bail!(
            "Refusing to run eXo script outside collection root: {}",
            script.display()
        );
    }

    let mut spec = build_platform_script_command(&script_canon);
    spec.cwd = Some(exo.config_dir.clone());
    Ok(spec)
}

#[cfg(windows)]
fn build_platform_script_command(script: &Path) -> CommandSpec {
    irori_core::process::program_command(script)
}

#[cfg(not(windows))]
fn build_platform_script_command(script: &Path) -> CommandSpec {
    // eXo `.command` and `.bsh` wrappers rely on bash features; fall back to
    // `/bin/sh` only when bash is absent.
    let shell = if Path::new("/bin/bash").is_file() {
        "/bin/bash"
    } else if Path::new("/usr/bin/env").is_file() {
        return CommandSpec::new("/usr/bin/env")
            .arg("bash")
            .arg(script.as_os_str());
    } else {
        "/bin/sh"
    };
    CommandSpec::new(shell).arg(script.as_os_str())
}
