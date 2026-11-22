//! Lightweight Linux CPU, GPU and MangoHud availability detection without
//! spawning external processes or loading `pci.ids` into memory.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use irori_core::GpuSelection;

/// One graphics adapter detected via `/sys/class/drm/card*/device/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedGpu {
    pub slot: String,
    pub vendor_id: String,
    pub device_id: String,
    pub subvendor_id: Option<String>,
    pub subdevice_id: Option<String>,
    pub vram_bytes: Option<u64>,
    pub display_name: String,
    pub short_label: String,
    pub is_dedicated: bool,
}

/// Cached host HUD environment detected once on demand via [`OnceLock`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxHudEnv {
    pub has_vulkan_layer: bool,
    pub mangohud_bin_path: Option<PathBuf>,
    pub vkcube_bin_path: Option<PathBuf>,
    pub cpu_label: String,
    pub gpus: Vec<DetectedGpu>,
}

/// Target PCI hardware IDs for streaming lookup in `pci.ids`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciLookupTarget {
    pub vendor_id: String,
    pub device_id: String,
    pub subvendor_id: Option<String>,
    pub subdevice_id: Option<String>,
}

static LINUX_HUD_ENV: OnceLock<LinuxHudEnv> = OnceLock::new();

/// Candidate locations for the system PCI ID database.
const PCI_IDS_PATHS: [&str; 4] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/run/host/usr/share/hwdata/pci.ids",
    "/run/host/usr/share/misc/pci.ids",
];

const FLATPAK_MANGOHUD_EXT_DIR: &str = "/usr/lib/extensions/vulkan/MangoHud";
const FLATPAK_MANGOHUD_BIN: &str = "/usr/lib/extensions/vulkan/MangoHud/bin/mangohud";
const SYSTEM_VULKAN_IMPLICIT_LAYER_DIR: &str = "/usr/share/vulkan/implicit_layer.d";

/// Detect and cache CPU label, GPUs, MangoHud availability and `vkcube` once.
pub fn detect_linux_hud_env() -> &'static LinuxHudEnv {
    LINUX_HUD_ENV.get_or_init(detect_linux_hud_env_uncached)
}

/// Return the already-cached Linux HUD environment without performing I/O.
pub fn cached_linux_hud_env() -> Option<&'static LinuxHudEnv> {
    LINUX_HUD_ENV.get()
}

#[cfg(target_os = "linux")]
fn detect_linux_hud_env_uncached() -> LinuxHudEnv {
    let mangohud_bin_path = find_executable_in_path("mangohud").or_else(|| {
        let p = PathBuf::from(FLATPAK_MANGOHUD_BIN);
        is_executable_file(&p).then_some(p)
    });
    let has_vulkan_layer = mangohud_bin_path.is_some()
        || has_mangohud_layer_manifest(Path::new(SYSTEM_VULKAN_IMPLICIT_LAYER_DIR))
        || Path::new(FLATPAK_MANGOHUD_EXT_DIR).exists()
        || user_vulkan_layer_dirs()
            .iter()
            .any(|dir| has_mangohud_layer_manifest(dir));
    let vkcube_bin_path =
        find_executable_in_path("vkcube").or_else(|| find_executable_in_path("vkcube-wayland"));
    let cpu_label = shorten_cpu_label(&detect_cpu_model_linux());
    let gpus = detect_gpus_linux();

    LinuxHudEnv {
        has_vulkan_layer,
        mangohud_bin_path,
        vkcube_bin_path,
        cpu_label,
        gpus,
    }
}

#[cfg(not(target_os = "linux"))]
fn detect_linux_hud_env_uncached() -> LinuxHudEnv {
    LinuxHudEnv::default()
}

#[cfg(target_os = "linux")]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn user_vulkan_layer_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(xdg).join("vulkan/implicit_layer.d"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        let local = PathBuf::from(home).join(".local/share/vulkan/implicit_layer.d");
        if !dirs.contains(&local) {
            dirs.push(local);
        }
    }
    dirs
}

#[cfg(target_os = "linux")]
fn has_mangohud_layer_manifest(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(name) = name.to_str()
            && name.starts_with("MangoHud")
            && name.ends_with(".json")
        {
            return true;
        }
    }
    false
}

/// Clean a sysfs hex ID (`"0x1002\n"`) into lowercase hex digits without `0x`.
pub fn clean_sysfs_hex(raw: &str) -> Option<String> {
    let cleaned = raw
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase();
    if cleaned.is_empty() || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        None
    } else {
        Some(cleaned)
    }
}

/// Parse the first CPU model line from a `/proc/cpuinfo`-formatted reader.
///
/// Stops reading immediately on the first `model name` line. On ARM64/SoC
/// layouts without `model name`, falls back to the `Model` field if present.
pub fn parse_cpu_model_from_reader<R: BufRead>(mut reader: R) -> Option<String> {
    let mut line = String::with_capacity(128);
    let mut arm_model_fallback: Option<String> = None;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("model name") {
            return Some(val.to_string());
        }
        // ARM64 / Raspberry Pi cpuinfo uses uppercase `Model`, distinct from
        // x86's lowercase numeric `model` stepping field.
        if key == "Model"
            && arm_model_fallback.is_none()
            && !val.chars().all(|c| c.is_ascii_digit())
        {
            arm_model_fallback = Some(val.to_string());
        }
    }
    arm_model_fallback
}

/// Resolve a CPU model string from cpuinfo, devicetree model, or host arch.
fn resolve_cpu_model_with_fallbacks<R: BufRead>(
    cpuinfo: Option<R>,
    devicetree_model: Option<&str>,
    arch: &str,
) -> String {
    if let Some(model) = cpuinfo.and_then(parse_cpu_model_from_reader) {
        return model;
    }
    if let Some(dt) = devicetree_model {
        let trimmed = dt.trim_end_matches('\0').trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    arch.to_string()
}

#[cfg(target_os = "linux")]
fn detect_cpu_model_linux() -> String {
    let cpuinfo = File::open("/proc/cpuinfo")
        .or_else(|_| File::open("/run/host/proc/cpuinfo"))
        .ok()
        .map(BufReader::new);
    let dt = std::fs::read_to_string("/sys/firmware/devicetree/base/model").ok();
    resolve_cpu_model_with_fallbacks(cpuinfo, dt.as_deref(), std::env::consts::ARCH)
}

/// Shorten a full CPU model name toward <= 14 characters for the HUD label.
pub fn shorten_cpu_label(raw: &str) -> String {
    let before_at = raw.split('@').next().unwrap_or(raw);
    let cleaned = before_at
        .replace("(R)", "")
        .replace("(r)", "")
        .replace("(TM)", "")
        .replace("(tm)", "");

    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        let lower = t.to_ascii_lowercase();

        // Drop trailing "with Radeon Graphics" / "w/ Radeon Graphics" suffixes.
        if lower == "with" || lower == "w/" {
            break;
        }
        // Drop generation prefix like "13th Gen".
        if i + 1 < tokens.len()
            && tokens[i + 1].eq_ignore_ascii_case("gen")
            && is_ordinal_token(&lower)
        {
            i += 2;
            continue;
        }
        // Drop "CPU", "Processor", and "<N>-Core" tokens.
        if lower == "cpu" || lower == "processor" || is_core_count_token(&lower) {
            i += 1;
            continue;
        }
        kept.push(t);
        i += 1;
    }

    let joined = kept.join(" ");
    let stripped = strip_cpu_brand_prefix(&joined);
    if stripped.is_empty() {
        "CPU".to_string()
    } else {
        stripped.to_string()
    }
}

fn is_ordinal_token(lower: &str) -> bool {
    for suffix in ["st", "nd", "rd", "th"] {
        if let Some(num) = lower.strip_suffix(suffix)
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

fn is_core_count_token(lower: &str) -> bool {
    lower
        .strip_suffix("-core")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn strip_cpu_brand_prefix(s: &str) -> &str {
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = strip_prefix_ci(s, &lower, "intel core ultra ") {
        // Preserve "Ultra ..." so Core Ultra models stay recognizable.
        if !rest.is_empty() {
            return &s["intel core ".len()..];
        }
    }
    for prefix in ["intel core ", "intel ", "amd ", "qualcomm "] {
        if let Some(rest) = strip_prefix_ci(s, &lower, prefix)
            && !rest.is_empty()
        {
            return rest;
        }
    }
    s
}

fn strip_prefix_ci<'a>(orig: &'a str, lower: &str, prefix_lower: &str) -> Option<&'a str> {
    lower
        .starts_with(prefix_lower)
        .then(|| orig[prefix_lower.len()..].trim())
}

/// Shorten a GPU model name by stripping redundant vendor/series prefixes.
pub fn shorten_gpu_label(raw: &str) -> String {
    let cleaned = raw
        .replace("(R)", "")
        .replace("(r)", "")
        .replace("(TM)", "")
        .replace("(tm)", "");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return "GPU".to_string();
    }

    let lower = trimmed.to_ascii_lowercase();
    // Preserve short vendor fallback names ("AMD GPU", "NVIDIA GPU", "Intel GPU").
    if matches!(lower.as_str(), "amd gpu" | "nvidia gpu" | "intel gpu") {
        return trimmed.to_string();
    }

    let mut cur = trimmed;
    for prefix in [
        "nvidia corporation ",
        "nvidia geforce ",
        "geforce ",
        "nvidia ",
        "amd radeon ",
        "advanced micro devices ",
        "amd ",
        "intel corporation ",
        "intel ",
    ] {
        let cur_lower = cur.to_ascii_lowercase();
        if let Some(rest) = strip_prefix_ci(cur, &cur_lower, prefix)
            && !rest.is_empty()
            && !rest.eq_ignore_ascii_case("gpu")
            && !rest.eq_ignore_ascii_case("graphics")
        {
            cur = rest;
        }
    }

    // Also strip a leading "Radeon " when followed by a specific series token
    // (e.g., "Radeon RX 9070 XT" -> "RX 9070 XT", while keeping "Radeon Graphics").
    let cur_lower = cur.to_ascii_lowercase();
    if let Some(rest) = strip_prefix_ci(cur, &cur_lower, "radeon ") {
        let rest_lower = rest.to_ascii_lowercase();
        if !rest.is_empty()
            && !rest_lower.starts_with("graphics")
            && !rest_lower.eq_ignore_ascii_case("gpu")
        {
            cur = rest;
        }
    }

    let normalized = cur.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        "GPU".to_string()
    } else {
        normalized
    }
}

/// Friendly fallback GPU name when `pci.ids` is absent or lacks the device ID.
pub fn vendor_fallback_name(vendor_id: &str) -> &'static str {
    match clean_sysfs_hex(vendor_id).as_deref() {
        Some("1002") => "AMD GPU",
        Some("10de") => "NVIDIA GPU",
        Some("8086") => "Intel GPU",
        _ => "GPU",
    }
}

/// Classify a GPU as dedicated vs integrated without relying on `boot_vga`.
pub fn classify_gpu_dedicated(
    vendor_id: &str,
    slot: &str,
    vram_bytes: Option<u64>,
    name: &str,
) -> bool {
    const THREE_GIB: u64 = 3 * 1024 * 1024 * 1024;
    const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;

    let vendor =
        clean_sysfs_hex(vendor_id).unwrap_or_else(|| vendor_id.trim().to_ascii_lowercase());
    let name_lower = name.to_ascii_lowercase();

    // (a) NVIDIA is always treated as discrete on desktop/laptop Linux.
    if vendor == "10de" {
        return true;
    }

    // (b) Dedicated VRAM heuristic from sysfs `mem_info_vram_total`.
    if let Some(vram) = vram_bytes {
        if vram >= FOUR_GIB {
            return true;
        }
        if vram < THREE_GIB {
            return false;
        }
    }

    // (c) Intel iGPU vs Intel Arc dGPU.
    if vendor == "8086" {
        if slot.to_ascii_lowercase().starts_with("0000:00:02.")
            || name_lower.contains("uhd")
            || name_lower.contains("iris")
            || name_lower.contains("hd graphics")
        {
            return false;
        }
        if name_lower.contains("arc") {
            return true;
        }
        let non_root_bus = slot
            .split(':')
            .nth(1)
            .is_some_and(|bus| !bus.eq_ignore_ascii_case("00"));
        if non_root_bus && vram_bytes.is_some_and(|v| v > 0) {
            return true;
        }
        return false;
    }

    // (d) Explicit AMD discrete series naming fallback when VRAM size is absent.
    name_lower.contains("radeon rx") || name_lower.contains("radeon pro")
}

/// Extract the marketing name from a `pci.ids` device line: prefer the text
/// inside `[...]` when non-empty, otherwise return the trimmed device name.
fn extract_device_display_name(raw_name: &str) -> String {
    let trimmed = raw_name.trim();
    if let Some(open) = trimmed.find('[')
        && let Some(close_rel) = trimmed[open + 1..].find(']')
    {
        let inside = trimmed[open + 1..open + 1 + close_rel].trim();
        if !inside.is_empty() {
            return inside.to_string();
        }
    }
    trimmed.to_string()
}

/// Stream-scan a `pci.ids` reader resolving vendor -> `\tdevice` -> `\t\tsubsys`
/// names with early return once all targets are resolved.
pub fn lookup_pci_names_from_reader<R: BufRead>(
    mut reader: R,
    targets: &[PciLookupTarget],
) -> Vec<Option<String>> {
    if targets.is_empty() {
        return Vec::new();
    }

    let mut results: Vec<Option<String>> = vec![None; targets.len()];
    let mut resolved_final = vec![false; targets.len()];
    let mut remaining = targets.len();
    let mut active_device_targets: Vec<usize> = Vec::new();

    let mut in_matching_vendor = false;
    let mut current_vendor_id = String::new();
    let mut buf = String::with_capacity(256);

    let finalize_active =
        |active: &mut Vec<usize>, resolved: &mut [bool], rem: &mut usize| -> bool {
            for idx in active.drain(..) {
                if !resolved[idx] {
                    resolved[idx] = true;
                    *rem -= 1;
                }
            }
            *rem == 0
        };

    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = buf.trim_end_matches(['\r', '\n']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // PCI device classes section starts with `C `; no more vendor blocks follow.
        if line.starts_with("C ") {
            finalize_active(
                &mut active_device_targets,
                &mut resolved_final,
                &mut remaining,
            );
            break;
        }

        if let Some(sub_rest) = line.strip_prefix("\t\t") {
            if active_device_targets.is_empty() {
                continue;
            }
            let Some((subv, after_subv)) = split_first_ws(sub_rest) else {
                continue;
            };
            let Some((subd, after_subd)) = split_first_ws(after_subv.trim_start()) else {
                continue;
            };
            let subname = after_subd.trim();
            if subname.is_empty() {
                continue;
            }

            let mut i = 0;
            while i < active_device_targets.len() {
                let idx = active_device_targets[i];
                let t = &targets[idx];
                let matches_sub = t
                    .subvendor_id
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case(subv))
                    && t.subdevice_id
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(subd));
                if matches_sub {
                    results[idx] = Some(subname.to_string());
                    resolved_final[idx] = true;
                    remaining -= 1;
                    active_device_targets.swap_remove(i);
                } else {
                    i += 1;
                }
            }
            if remaining == 0 {
                return results;
            }
        } else if let Some(dev_rest) = line.strip_prefix('\t') {
            if finalize_active(
                &mut active_device_targets,
                &mut resolved_final,
                &mut remaining,
            ) {
                return results;
            }
            if !in_matching_vendor {
                continue;
            }
            let Some((dev_id, dev_name)) = split_first_ws(dev_rest) else {
                continue;
            };
            let dev_name = dev_name.trim();
            if dev_name.is_empty() {
                continue;
            }

            for (idx, t) in targets.iter().enumerate() {
                if resolved_final[idx] {
                    continue;
                }
                if t.vendor_id.eq_ignore_ascii_case(&current_vendor_id)
                    && t.device_id.eq_ignore_ascii_case(dev_id)
                {
                    results[idx] = Some(extract_device_display_name(dev_name));
                    if t.subvendor_id.is_some() && t.subdevice_id.is_some() {
                        active_device_targets.push(idx);
                    } else {
                        resolved_final[idx] = true;
                        remaining -= 1;
                    }
                }
            }
            if remaining == 0 {
                return results;
            }
        } else {
            if finalize_active(
                &mut active_device_targets,
                &mut resolved_final,
                &mut remaining,
            ) {
                return results;
            }
            let Some((vend_id, _)) = split_first_ws(line) else {
                in_matching_vendor = false;
                continue;
            };
            let needed = targets
                .iter()
                .enumerate()
                .any(|(idx, t)| !resolved_final[idx] && t.vendor_id.eq_ignore_ascii_case(vend_id));
            if needed {
                in_matching_vendor = true;
                current_vendor_id.clear();
                current_vendor_id.push_str(vend_id);
            } else {
                in_matching_vendor = false;
            }
        }
    }

    finalize_active(
        &mut active_device_targets,
        &mut resolved_final,
        &mut remaining,
    );
    results
}

fn split_first_ws(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(char::is_whitespace)?;
    Some((&s[..idx], &s[idx..]))
}

#[cfg(target_os = "linux")]
fn lookup_pci_names_from_system(targets: &[PciLookupTarget]) -> Vec<Option<String>> {
    if targets.is_empty() {
        return Vec::new();
    }
    for path in PCI_IDS_PATHS {
        if let Ok(file) = File::open(path) {
            return lookup_pci_names_from_reader(BufReader::new(file), targets);
        }
    }
    vec![None; targets.len()]
}

#[cfg(target_os = "linux")]
fn detect_gpus_linux() -> Vec<DetectedGpu> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };

    let mut card_dirs: Vec<(u32, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(num_str) = name.strip_prefix("card")
            && !num_str.is_empty()
            && num_str.chars().all(|c| c.is_ascii_digit())
            && let Ok(num) = num_str.parse::<u32>()
        {
            card_dirs.push((num, entry.path()));
        }
    }
    card_dirs.sort_by_key(|(num, _)| *num);

    struct RawCard {
        slot: String,
        vendor_id: String,
        device_id: String,
        subvendor_id: Option<String>,
        subdevice_id: Option<String>,
        vram_bytes: Option<u64>,
    }

    let mut raw_cards = Vec::new();
    for (_, card_path) in card_dirs {
        let dev = card_path.join("device");
        if !dev.is_dir() {
            continue;
        }
        let Some(vendor_id) = read_sysfs_hex(&dev.join("vendor")) else {
            continue;
        };
        let Some(device_id) = read_sysfs_hex(&dev.join("device")) else {
            continue;
        };
        let subvendor_id = read_sysfs_hex(&dev.join("subsystem_vendor"));
        let subdevice_id = read_sysfs_hex(&dev.join("subsystem_device"));
        let slot = read_pci_slot(&dev);
        let Some(slot) = slot.filter(|s| !s.is_empty()) else {
            continue;
        };
        let vram_bytes = std::fs::read_to_string(dev.join("mem_info_vram_total"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());

        raw_cards.push(RawCard {
            slot,
            vendor_id,
            device_id,
            subvendor_id,
            subdevice_id,
            vram_bytes,
        });
    }

    let targets: Vec<PciLookupTarget> = raw_cards
        .iter()
        .map(|c| PciLookupTarget {
            vendor_id: c.vendor_id.clone(),
            device_id: c.device_id.clone(),
            subvendor_id: c.subvendor_id.clone(),
            subdevice_id: c.subdevice_id.clone(),
        })
        .collect();
    let names = lookup_pci_names_from_system(&targets);

    raw_cards
        .into_iter()
        .zip(names)
        .map(|(card, pci_name)| {
            let display_name =
                pci_name.unwrap_or_else(|| vendor_fallback_name(&card.vendor_id).to_string());
            let short_label = shorten_gpu_label(&display_name);
            let is_dedicated =
                classify_gpu_dedicated(&card.vendor_id, &card.slot, card.vram_bytes, &display_name);
            DetectedGpu {
                slot: card.slot,
                vendor_id: card.vendor_id,
                device_id: card.device_id,
                subvendor_id: card.subvendor_id,
                subdevice_id: card.subdevice_id,
                vram_bytes: card.vram_bytes,
                display_name,
                short_label,
                is_dedicated,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn read_sysfs_hex(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    clean_sysfs_hex(&raw)
}

#[cfg(target_os = "linux")]
fn read_pci_slot(device_dir: &Path) -> Option<String> {
    if let Ok(uevent) = std::fs::read_to_string(device_dir.join("uevent")) {
        for line in uevent.lines() {
            if let Some(slot) = line.strip_prefix("PCI_SLOT_NAME=") {
                let slot = slot.trim();
                if !slot.is_empty() {
                    return Some(slot.to_string());
                }
            }
        }
    }
    std::fs::read_link(device_dir)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn find_gpu_by_hardware_id<'a>(
    gpus: &'a [DetectedGpu],
    vendor_id: &str,
    device_id: &str,
    subvendor_id: Option<&str>,
    subdevice_id: Option<&str>,
) -> Option<&'a DetectedGpu> {
    if let (Some(sv), Some(sd)) = (subvendor_id, subdevice_id)
        && let Some(exact) = gpus.iter().find(|g| {
            g.vendor_id.eq_ignore_ascii_case(vendor_id)
                && g.device_id.eq_ignore_ascii_case(device_id)
                && g.subvendor_id
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case(sv))
                && g.subdevice_id
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(sd))
        })
    {
        return Some(exact);
    }
    gpus.iter().find(|g| {
        g.vendor_id.eq_ignore_ascii_case(vendor_id) && g.device_id.eq_ignore_ascii_case(device_id)
    })
}

/// Resolve the current `PCI_SLOT_NAME` for `selection`, returning `Some` only
/// when the host has 2 or more GPUs.
pub fn resolve_pci_dev(selection: &GpuSelection, gpus: &[DetectedGpu]) -> Option<String> {
    if gpus.len() < 2 {
        return None;
    }
    match selection {
        GpuSelection::AllGpus => None,
        GpuSelection::AutoDedicated => gpus.iter().find(|g| g.is_dedicated).map(|g| g.slot.clone()),
        GpuSelection::ByHardwareId {
            vendor_id,
            device_id,
            subvendor_id,
            subdevice_id,
        } => find_gpu_by_hardware_id(
            gpus,
            vendor_id,
            device_id,
            subvendor_id.as_deref(),
            subdevice_id.as_deref(),
        )
        .map(|g| g.slot.clone()),
    }
}

/// Choose the default short GPU label for `selection` given detected `gpus`.
pub fn default_gpu_short_label(selection: &GpuSelection, gpus: &[DetectedGpu]) -> String {
    let chosen = match selection {
        GpuSelection::ByHardwareId {
            vendor_id,
            device_id,
            subvendor_id,
            subdevice_id,
        } => find_gpu_by_hardware_id(
            gpus,
            vendor_id,
            device_id,
            subvendor_id.as_deref(),
            subdevice_id.as_deref(),
        )
        .or_else(|| gpus.iter().find(|g| g.is_dedicated))
        .or_else(|| gpus.first()),
        GpuSelection::AutoDedicated | GpuSelection::AllGpus => gpus
            .iter()
            .find(|g| g.is_dedicated)
            .or_else(|| gpus.first()),
    };
    chosen
        .map(|g| g.short_label.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("GPU")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn clean_sysfs_hex_strips_prefix_and_whitespace() {
        assert_eq!(clean_sysfs_hex("0x1002\n"), Some("1002".into()));
        assert_eq!(clean_sysfs_hex("  0X10DE\r\n"), Some("10de".into()));
        assert_eq!(clean_sysfs_hex("8086"), Some("8086".into()));
        assert_eq!(clean_sysfs_hex("0x\n"), None);
        assert_eq!(clean_sysfs_hex("not-hex"), None);
    }

    #[test]
    fn cpu_parsing_stops_early_and_supports_arm64_fallbacks() {
        let x86 = "\
processor\t: 0
vendor_id\t: GenuineIntel
model\t\t: 183
model name\t: Intel(R) Core(TM) i5-14600K
cpu MHz\t\t: 3500.000
";
        let mut cursor = Cursor::new(x86);
        assert_eq!(
            parse_cpu_model_from_reader(&mut cursor),
            Some("Intel(R) Core(TM) i5-14600K".into())
        );
        // Stopped immediately after `model name`, before reading `cpu MHz`.
        assert!((cursor.position() as usize) < x86.len());

        let arm_cpuinfo = "\
processor\t: 0
BogoMIPS\t: 48.00
Model\t\t: Raspberry Pi 5 Model B Rev 1.0
";
        assert_eq!(
            parse_cpu_model_from_reader(Cursor::new(arm_cpuinfo)),
            Some("Raspberry Pi 5 Model B Rev 1.0".into())
        );

        let asahi_cpuinfo = "processor\t: 0\nBogoMIPS\t: 48.00\n";
        assert_eq!(
            resolve_cpu_model_with_fallbacks(
                Some(Cursor::new(asahi_cpuinfo)),
                Some("Apple MacBook Pro (14-inch, M2 Pro)\0\0"),
                "aarch64"
            ),
            "Apple MacBook Pro (14-inch, M2 Pro)"
        );

        assert_eq!(
            resolve_cpu_model_with_fallbacks(Some(Cursor::new(asahi_cpuinfo)), None, "aarch64"),
            "aarch64"
        );
    }

    #[test]
    fn shorten_cpu_and_gpu_labels() {
        assert_eq!(
            shorten_cpu_label("Intel(R) Core(TM) i5-14600K"),
            "i5-14600K"
        );
        assert_eq!(
            shorten_cpu_label("13th Gen Intel(R) Core(TM) i7-13700K CPU @ 3.40GHz"),
            "i7-13700K"
        );
        assert_eq!(
            shorten_cpu_label("AMD Ryzen 7 7800X3D 8-Core Processor"),
            "Ryzen 7 7800X3D"
        );
        assert_eq!(
            shorten_cpu_label("AMD Ryzen 7 7840U w/ Radeon 780M Graphics"),
            "Ryzen 7 7840U"
        );

        assert_eq!(shorten_gpu_label("AMD Radeon RX 9070 XT"), "RX 9070 XT");
        assert_eq!(shorten_gpu_label("Radeon RX 9070 XT"), "RX 9070 XT");
        assert_eq!(
            shorten_gpu_label("NVIDIA GeForce RTX 4070 Super"),
            "RTX 4070 Super"
        );
        assert_eq!(shorten_gpu_label("Intel Arc B580"), "Arc B580");
        assert_eq!(shorten_gpu_label("AMD GPU"), "AMD GPU");
    }

    #[test]
    fn streaming_pci_ids_resolves_subdevice_and_bracket_fallback_with_early_exit() {
        let pci_ids = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t164e  Raphael
\t7550  Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]
\t\t1002 0e3a  Radeon RX 9070 XT
\t\t1da2 4800  Sapphire Pulse Radeon RX 9070
10de  NVIDIA Corporation
\t2786  AD104 [GeForce RTX 4070]
C 00  Unclassified device
";
        let targets = vec![
            PciLookupTarget {
                vendor_id: "1002".into(),
                device_id: "7550".into(),
                subvendor_id: Some("1002".into()),
                subdevice_id: Some("0e3a".into()),
            },
            PciLookupTarget {
                vendor_id: "1002".into(),
                device_id: "7550".into(),
                subvendor_id: Some("ffff".into()),
                subdevice_id: Some("ffff".into()),
            },
        ];

        let mut cursor = Cursor::new(pci_ids);
        let names = lookup_pci_names_from_reader(&mut cursor, &targets);
        assert_eq!(
            names,
            vec![
                Some("Radeon RX 9070 XT".into()),
                Some("Radeon RX 9070/9070 XT/9070 GRE".into()),
            ]
        );
        // Should return early as soon as vendor 10de starts (finalizing target 1),
        // without scanning the rest of the database.
        assert!((cursor.position() as usize) < pci_ids.len());

        // Fallback when pci.ids is empty/missing.
        assert_eq!(
            lookup_pci_names_from_reader(Cursor::new(""), &targets),
            vec![None, None]
        );
        assert_eq!(vendor_fallback_name("0x1002\n"), "AMD GPU");
        assert_eq!(vendor_fallback_name("10de"), "NVIDIA GPU");
        assert_eq!(vendor_fallback_name("8086"), "Intel GPU");
        assert_eq!(vendor_fallback_name("9999"), "GPU");
    }

    #[test]
    fn classify_gpu_dedicated_ignores_boot_vga_and_separates_dgpu_from_igpu() {
        let gib = 1024 * 1024 * 1024_u64;
        // AMD RX 9070 XT (16 GiB) vs AMD Raphael iGPU (512 MiB)
        assert!(classify_gpu_dedicated(
            "1002",
            "0000:03:00.0",
            Some(16 * gib),
            "Radeon RX 9070 XT"
        ));
        assert!(!classify_gpu_dedicated(
            "1002",
            "0000:10:00.0",
            Some(512 * 1024 * 1024),
            "Raphael"
        ));
        // AMD dGPU without VRAM sysfs file falls back to name
        assert!(classify_gpu_dedicated(
            "1002",
            "0000:03:00.0",
            None,
            "AMD Radeon RX 6700 XT"
        ));
        // NVIDIA always dedicated even without VRAM sysfs
        assert!(classify_gpu_dedicated(
            "10de",
            "0000:01:00.0",
            None,
            "GeForce RTX 4070"
        ));
        // Intel UHD/Iris on 00:02.0 -> iGPU; Intel Arc on discrete bus -> dGPU
        assert!(!classify_gpu_dedicated(
            "8086",
            "0000:00:02.0",
            None,
            "UHD Graphics 770"
        ));
        assert!(!classify_gpu_dedicated(
            "8086",
            "0000:00:02.0",
            None,
            "Intel Arc Graphics"
        ));
        assert!(classify_gpu_dedicated(
            "8086",
            "0000:03:00.0",
            None,
            "Intel Arc B580"
        ));
    }

    #[test]
    fn resolve_pci_dev_and_default_label_for_single_and_multi_gpu() {
        let dgpu = DetectedGpu {
            slot: "0000:03:00.0".into(),
            vendor_id: "1002".into(),
            device_id: "7550".into(),
            subvendor_id: Some("1002".into()),
            subdevice_id: Some("0e3a".into()),
            vram_bytes: Some(16 * 1024 * 1024 * 1024),
            display_name: "Radeon RX 9070 XT".into(),
            short_label: "RX 9070 XT".into(),
            is_dedicated: true,
        };
        let igpu = DetectedGpu {
            slot: "0000:10:00.0".into(),
            vendor_id: "1002".into(),
            device_id: "164e".into(),
            subvendor_id: None,
            subdevice_id: None,
            vram_bytes: Some(512 * 1024 * 1024),
            display_name: "Raphael".into(),
            short_label: "Raphael".into(),
            is_dedicated: false,
        };

        // Single GPU -> never emit pci_dev.
        assert_eq!(
            resolve_pci_dev(&GpuSelection::AutoDedicated, std::slice::from_ref(&dgpu)),
            None
        );
        assert_eq!(
            default_gpu_short_label(&GpuSelection::AutoDedicated, std::slice::from_ref(&dgpu)),
            "RX 9070 XT"
        );

        // Multi-GPU -> AutoDedicated picks dGPU slot even when iGPU is listed first.
        let both = vec![igpu.clone(), dgpu.clone()];
        assert_eq!(
            resolve_pci_dev(&GpuSelection::AutoDedicated, &both),
            Some("0000:03:00.0".into())
        );
        assert_eq!(
            default_gpu_short_label(&GpuSelection::AutoDedicated, &both),
            "RX 9070 XT"
        );
        assert_eq!(resolve_pci_dev(&GpuSelection::AllGpus, &both), None);

        let by_id = GpuSelection::ByHardwareId {
            vendor_id: "1002".into(),
            device_id: "164e".into(),
            subvendor_id: None,
            subdevice_id: None,
        };
        assert_eq!(resolve_pci_dev(&by_id, &both), Some("0000:10:00.0".into()));
        assert_eq!(default_gpu_short_label(&by_id, &both), "Raphael");
    }
}
