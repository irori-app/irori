//! Lightweight Linux CPU, GPU and MangoHud availability detection without
//! spawning external processes or loading `pci.ids` into memory.

use std::io::BufRead;
use std::path::PathBuf;
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

/// Detect and cache CPU label, GPUs, MangoHud availability and `vkcube` once.
pub fn detect_linux_hud_env() -> &'static LinuxHudEnv {
    LINUX_HUD_ENV.get_or_init(LinuxHudEnv::default)
}

/// Clean a sysfs hex ID (`"0x1002\n"`) into lowercase hex digits without `0x`.
pub fn clean_sysfs_hex(raw: &str) -> Option<String> {
    let _ = raw;
    None
}

/// Parse the first CPU model line from a `/proc/cpuinfo`-formatted reader.
pub fn parse_cpu_model_from_reader<R: BufRead>(reader: R) -> Option<String> {
    let _ = reader;
    None
}

/// Shorten a full CPU model name toward <= 14 characters for the HUD label.
pub fn shorten_cpu_label(raw: &str) -> String {
    raw.trim().to_string()
}

/// Shorten a GPU model name by stripping redundant vendor/series prefixes.
pub fn shorten_gpu_label(raw: &str) -> String {
    raw.trim().to_string()
}

/// Friendly fallback GPU name when `pci.ids` is absent or lacks the device ID.
pub fn vendor_fallback_name(vendor_id: &str) -> &'static str {
    let _ = vendor_id;
    "GPU"
}

/// Classify a GPU as dedicated vs integrated without relying on `boot_vga`.
pub fn classify_gpu_dedicated(
    vendor_id: &str,
    slot: &str,
    vram_bytes: Option<u64>,
    name: &str,
) -> bool {
    let _ = (vendor_id, slot, vram_bytes, name);
    false
}

/// Stream-scan a `pci.ids` reader resolving vendor -> `\tdevice` -> `\t\tsubsys`
/// names with early return once all targets are resolved.
pub fn lookup_pci_names_from_reader<R: BufRead>(
    reader: R,
    targets: &[PciLookupTarget],
) -> Vec<Option<String>> {
    let _ = reader;
    vec![None; targets.len()]
}

/// Resolve the current `PCI_SLOT_NAME` for `selection`, returning `Some` only
/// when the host has 2 or more GPUs.
pub fn resolve_pci_dev(selection: &GpuSelection, gpus: &[DetectedGpu]) -> Option<String> {
    let _ = (selection, gpus);
    None
}

/// Choose the default short GPU label for `selection` given detected `gpus`.
pub fn default_gpu_short_label(selection: &GpuSelection, gpus: &[DetectedGpu]) -> String {
    let _ = (selection, gpus);
    "GPU".into()
}
