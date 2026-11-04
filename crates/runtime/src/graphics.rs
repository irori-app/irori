//! The host's graphics, as far as per-game GPU options need it: the graphics
//! cards (`/sys/class/drm`, named from `pci.ids`), NVIDIA's driver and its
//! Smooth Motion layer, and lsfg-vk with Lossless Scaling's DLL. Also the
//! variables those options set ([`graphics_env`]).
//!
//! Everything here is Linux only; elsewhere [`HostGraphics::probe`] finds
//! nothing. Probing reads a few small sysfs and manifest files plus `pci.ids`,
//! so it runs on a blocking thread, never the UI thread or a runtime worker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use irori_core::{AppSettings, GameSettings};

use crate::env::{EnvMap, HostEnv};

/// PCI vendor ids.
const NVIDIA: u16 = 0x10de;
const AMD: u16 = 0x1002;
const INTEL: u16 = 0x8086;

/// Smooth Motion's Vulkan layer, shipped with NVIDIA's driver from 575.
const SMOOTH_MOTION_LAYER: &str = "VK_LAYER_NV_present";
/// The first NVIDIA driver with Smooth Motion on Linux.
pub const SMOOTH_MOTION_DRIVER: u32 = 575;
/// lsfg-vk 2's layer manifest. Version 1 used [`LSFG_V1_MANIFEST`] and
/// another configuration, which Irori doesn't write.
const LSFG_MANIFEST: &str = "VkLayer_LSFGVK_frame_generation.json";
const LSFG_V1_MANIFEST: &str = "VkLayer_LS_frame_generation.json";
/// Lossless Scaling's folder in a Steam library. Its "lsfg-vk" branch ships
/// `lsfg-vk.dll`; the regular build's `Lossless.dll` also works.
const LOSSLESS_FOLDER: &str = "Lossless Scaling";
const LOSSLESS_DLLS: [&str; 2] = ["lsfg-vk.dll", "Lossless.dll"];

/// One graphics card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuAdapter {
    /// PCI address (`0000:03:00.0`), what `iroriGpu` stores.
    pub slot: String,
    pub vendor: u16,
    pub device: u16,
    /// Kernel driver (`amdgpu`, `i915`, `xe`, `nouveau`, `nvidia`).
    pub driver: String,
    /// The card the firmware showed the boot screen on; usually the one
    /// driving the desktop.
    pub boot_vga: bool,
    /// Model from `pci.ids` ("Navi 31 [Radeon RX 7900 XT/7900 XTX]").
    pub model: Option<String>,
}

impl GpuAdapter {
    /// Vendor name, never translated; empty for vendors Irori doesn't know.
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor {
            NVIDIA => "NVIDIA",
            AMD => "AMD",
            INTEL => "Intel",
            0x1af4 => "Virtio",
            0x15ad => "VMware",
            0x1414 => "Microsoft",
            0x1a03 => "ASPEED",
            0x5143 => "Qualcomm",
            _ => "",
        }
    }

    /// `vendor:device` in hex, as Mesa's `MESA_VK_DEVICE_SELECT` takes it.
    pub fn pci_id(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor, self.device)
    }

    /// Whether NVIDIA's own driver runs the card (not nouveau or NVK).
    pub fn nvidia_driver(&self) -> bool {
        self.vendor == NVIDIA && self.driver == "nvidia"
    }

    /// Whether the card is a GeForce RTX 40 series (Ada) or newer, which
    /// Smooth Motion needs. NVIDIA numbers device ids by generation: Ada's
    /// start at 0x2684 (AD102), and every later generation is above them.
    pub fn ada_or_newer(&self) -> bool {
        self.vendor == NVIDIA && self.device >= 0x2684
    }

    /// Name for people: "AMD Radeon RX 7900 XT", "NVIDIA GeForce RTX 4070".
    /// `pci.ids` puts the marketing name in brackets after the chip's.
    pub fn name(&self) -> String {
        let model = self.model.as_deref().map(|m| {
            match (m.find('['), m.rfind(']')) {
                (Some(a), Some(b)) if b > a + 1 => &m[a + 1..b],
                _ => m,
            }
            .trim()
        });
        let vendor = self.vendor_name();
        match model {
            Some(m) if m.starts_with(vendor) => m.to_string(),
            Some(m) if vendor.is_empty() => m.to_string(),
            Some(m) => format!("{vendor} {m}"),
            None if vendor.is_empty() => self.pci_id(),
            None => format!("{vendor} {}", self.pci_id()),
        }
    }

    /// `DRI_PRIME` value selecting this card by its PCI address.
    pub fn dri_prime(&self) -> String {
        format!("pci-{}", self.slot.replace([':', '.'], "_"))
    }
}

/// An lsfg-vk installation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsfgVk {
    /// The layer manifest.
    pub manifest: PathBuf,
    /// lsfg-vk 2 or newer, which reads per-game settings from the
    /// environment. Irori only drives these.
    pub current: bool,
    /// Lossless Scaling's DLL, when Irori found it. lsfg-vk also looks in
    /// the usual Steam folders itself.
    pub dll: Option<PathBuf>,
}

/// What [`HostGraphics::probe`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostGraphics {
    /// Graphics cards, in the kernel's order.
    pub adapters: Vec<GpuAdapter>,
    /// The loaded NVIDIA driver's version ("580.95.05").
    pub nvidia_driver: Option<String>,
    /// NVIDIA's Smooth Motion layer is installed.
    pub smooth_motion_layer: bool,
    pub lsfg: Option<LsfgVk>,
}

impl HostGraphics {
    /// Probe everything (the settings screens). Blocking.
    pub fn probe(app: &AppSettings) -> Self {
        Self::probe_parts(app, &HostEnv::current(), true, true)
    }

    /// Probe only what `settings` uses (a launch). Blocking.
    pub fn probe_for(app: &AppSettings, settings: &GameSettings, host: &HostEnv) -> Self {
        Self::probe_parts(
            app,
            host,
            !settings.gpu.trim().is_empty(),
            settings.lsfg.enabled,
        )
    }

    fn probe_parts(app: &AppSettings, host: &HostEnv, gpus: bool, lsfg: bool) -> Self {
        if !cfg!(target_os = "linux") {
            return Self::default();
        }
        let mut found = Self::default();
        if gpus {
            let ids = PCI_IDS.iter().find_map(|p| std::fs::read_to_string(p).ok());
            found.adapters = adapters_in(Path::new("/sys/class/drm"), ids.as_deref());
            found.nvidia_driver = std::fs::read_to_string("/sys/module/nvidia/version")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            let dirs = implicit_layer_dirs(host, &crate::locations::home());
            found.smooth_motion_layer = has_layer(&dirs, SMOOTH_MOTION_LAYER);
        }
        if lsfg {
            let dirs = implicit_layer_dirs(host, &crate::locations::home());
            let libraries = crate::vdf::steam_libraries(&crate::locations::steam_path(app));
            found.lsfg = find_lsfg(&dirs, &libraries, host);
        }
        found
    }

    /// The card at a PCI address.
    pub fn adapter(&self, slot: &str) -> Option<&GpuAdapter> {
        let slot = slot.trim();
        self.adapters.iter().find(|a| a.slot == slot)
    }

    /// Whether a card runs on NVIDIA's own driver.
    pub fn has_nvidia_driver(&self) -> bool {
        self.adapters.iter().any(GpuAdapter::nvidia_driver)
    }

    /// The NVIDIA driver's major version (580 for "580.95.05").
    pub fn nvidia_major(&self) -> Option<u32> {
        self.nvidia_driver
            .as_deref()?
            .split('.')
            .next()?
            .parse()
            .ok()
    }

    /// Why Smooth Motion can't run here; `None` when it can.
    pub fn smooth_motion_blocker(&self) -> Option<SmoothMotionBlocker> {
        if !self.has_nvidia_driver() {
            return Some(SmoothMotionBlocker::NoNvidia);
        }
        if !self
            .adapters
            .iter()
            .any(|a| a.nvidia_driver() && a.ada_or_newer())
        {
            return Some(SmoothMotionBlocker::OldCard);
        }
        if !self.smooth_motion_layer {
            return Some(SmoothMotionBlocker::OldDriver);
        }
        None
    }

    /// Why lsfg-vk frame generation can't run here; `None` when it can.
    pub fn lsfg_blocker(&self) -> Option<LsfgBlocker> {
        match &self.lsfg {
            None => Some(LsfgBlocker::NotInstalled),
            Some(l) if !l.current => Some(LsfgBlocker::Outdated),
            Some(l) if l.dll.is_none() => Some(LsfgBlocker::NoLosslessScaling),
            Some(_) => None,
        }
    }
}

/// Why Smooth Motion is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothMotionBlocker {
    /// No card runs on NVIDIA's own driver.
    NoNvidia,
    /// NVIDIA cards older than the GeForce RTX 40 series.
    OldCard,
    /// The driver has no Smooth Motion layer (older than 575).
    OldDriver,
}

/// Why lsfg-vk frame generation is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LsfgBlocker {
    NotInstalled,
    /// lsfg-vk 1, which takes its settings another way.
    Outdated,
    /// Lossless Scaling (whose DLL lsfg-vk runs) isn't in a Steam library.
    NoLosslessScaling,
}

/// Where distributions keep `pci.ids`.
const PCI_IDS: [&str; 3] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

/// Graphics cards under a `/sys/class/drm`-like folder: every `cardN` whose
/// device is a PCI device. Connectors (`card0-DP-1`) and render nodes are
/// skipped.
pub fn adapters_in(drm: &Path, pci_ids: Option<&str>) -> Vec<GpuAdapter> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<(u32, GpuAdapter)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("card")?.parse().ok()?;
            let device = e.path().join("device");
            let uevent = std::fs::read_to_string(device.join("uevent")).ok()?;
            let mut adapter = parse_uevent(&uevent)?;
            adapter.boot_vga =
                std::fs::read_to_string(device.join("boot_vga")).is_ok_and(|v| v.trim() == "1");
            Some((n, adapter))
        })
        .collect();
    cards.sort_by_key(|(n, _)| *n);
    let mut adapters: Vec<GpuAdapter> = Vec::new();
    for (_, a) in cards {
        // One card can show up twice (a simple framebuffer handed over).
        if !adapters.iter().any(|b| b.slot == a.slot) {
            adapters.push(a);
        }
    }
    if let Some(text) = pci_ids {
        let wanted: Vec<(u16, u16)> = adapters.iter().map(|a| (a.vendor, a.device)).collect();
        let names = model_names(text, &wanted);
        for a in &mut adapters {
            a.model = names.get(&(a.vendor, a.device)).cloned();
        }
    }
    adapters
}

/// A PCI device's `uevent`: `PCI_SLOT_NAME`, `PCI_ID` and `DRIVER`.
fn parse_uevent(text: &str) -> Option<GpuAdapter> {
    let mut adapter = GpuAdapter::default();
    let mut ids = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "PCI_SLOT_NAME" => adapter.slot = value.trim().to_string(),
            "DRIVER" => adapter.driver = value.trim().to_string(),
            "PCI_ID" => {
                let (v, d) = value.trim().split_once(':')?;
                ids = Some((
                    u16::from_str_radix(v, 16).ok()?,
                    u16::from_str_radix(d, 16).ok()?,
                ));
            }
            _ => {}
        }
    }
    let (vendor, device) = ids?;
    if adapter.slot.is_empty() {
        return None;
    }
    adapter.vendor = vendor;
    adapter.device = device;
    Some(adapter)
}

/// Device names from a `pci.ids` file for the `(vendor, device)` pairs
/// wanted. Vendors start a line; their devices follow, indented by one tab
/// (two tabs are subsystems).
pub fn model_names(text: &str, wanted: &[(u16, u16)]) -> HashMap<(u16, u16), String> {
    let mut names = HashMap::new();
    let mut vendor: Option<u16> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            if rest.starts_with('\t') {
                continue;
            }
            let Some(v) = vendor else { continue };
            let Some((id, name)) = rest.split_once(char::is_whitespace) else {
                continue;
            };
            if let Ok(device) = u16::from_str_radix(id, 16)
                && wanted.contains(&(v, device))
            {
                names.insert((v, device), name.trim().to_string());
            }
            continue;
        }
        // A new vendor, or the device classes that end the vendor list.
        vendor = line
            .split_once(char::is_whitespace)
            .and_then(|(id, _)| u16::from_str_radix(id, 16).ok())
            .filter(|v| wanted.iter().any(|(w, _)| w == v));
        if line.starts_with("C ") {
            break;
        }
    }
    names
}

/// Folders the Vulkan loader reads implicit layers from, in its order.
pub fn implicit_layer_dirs(host: &HostEnv, home: &Path) -> Vec<PathBuf> {
    let var = |key: &str| host.get(key).filter(|v| !v.is_empty());
    let list = |key: &str, default: &str| -> Vec<PathBuf> {
        var(key)
            .unwrap_or(default)
            .split(':')
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    let mut roots = Vec::new();
    roots.push(var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from));
    roots.extend(list("XDG_CONFIG_DIRS", "/etc/xdg"));
    roots.push(PathBuf::from("/etc"));
    roots.push(var("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from));
    roots.extend(list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    let mut dirs: Vec<PathBuf> = roots
        .into_iter()
        .map(|r| r.join("vulkan/implicit_layer.d"))
        .collect();
    if let Some(extra) = var("VK_ADD_IMPLICIT_LAYER_PATH") {
        dirs.extend(
            extra
                .split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        );
    }
    dirs.dedup();
    dirs
}

/// Whether a manifest in `dirs` declares the layer `name`.
fn has_layer(dirs: &[PathBuf], name: &str) -> bool {
    let quoted = format!("\"{name}\"");
    dirs.iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|text| text.contains(&quoted)))
}

/// lsfg-vk's manifest in `dirs` and Lossless Scaling's DLL: the one the
/// user points lsfg-vk at (`LSFGVK_DLL_PATH`), else the one in a Steam
/// library.
pub fn find_lsfg(dirs: &[PathBuf], libraries: &[PathBuf], host: &HostEnv) -> Option<LsfgVk> {
    let manifest = |name: &str| dirs.iter().map(|d| d.join(name)).find(|p| p.is_file());
    let (manifest, current) = match manifest(LSFG_MANIFEST) {
        Some(m) => (m, true),
        None => (manifest(LSFG_V1_MANIFEST)?, false),
    };
    let dll = host
        .get("LSFGVK_DLL_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            libraries.iter().find_map(|lib| {
                let folder = lib.join("steamapps/common").join(LOSSLESS_FOLDER);
                LOSSLESS_DLLS
                    .iter()
                    .map(|name| folder.join(name))
                    .find(|p| p.is_file())
            })
        });
    Some(LsfgVk {
        manifest,
        current,
        dll,
    })
}

/// Variables of the GPU and frame generation options, for a Linux launch:
/// the chosen graphics card, Smooth Motion and lsfg-vk. Variables the user
/// set for the game are left out, so theirs win; the host environment's do
/// not, because these options are chosen per game.
pub fn graphics_env(settings: &GameSettings, graphics: &HostGraphics) -> EnvMap {
    let mut env = EnvMap::new();
    let slot = settings.gpu.trim();
    if !slot.is_empty() {
        match graphics.adapter(slot) {
            Some(card) => env.extend(select_card(card, &graphics.adapters)),
            None => tracing::warn!("graphics card {slot} not found; the system picks one"),
        }
    }
    if settings.smooth_motion {
        env.insert("NVPRESENT_ENABLE_SMOOTH_MOTION".into(), "1".into());
    }
    let lsfg = &settings.lsfg;
    if lsfg.enabled {
        // An environment profile: lsfg-vk ignores its configuration file's
        // profiles for this game.
        env.insert("LSFGVK_ENV".into(), "1".into());
        env.insert(
            "LSFGVK_MULTIPLIER".into(),
            lsfg.multiplier.clamp(2, 20).to_string(),
        );
        env.insert(
            "LSFGVK_FLOW_SCALE".into(),
            (f64::from(lsfg.flow_scale.clamp(25, 100)) / 100.0).to_string(),
        );
        if lsfg.performance_mode {
            env.insert("LSFGVK_PERFORMANCE_MODE".into(), "1".into());
        }
        if let Some(dll) = graphics.lsfg.as_ref().and_then(|l| l.dll.as_ref()) {
            env.insert("LSFGVK_DLL_PATH".into(), dll.to_string_lossy().into_owned());
        }
    }
    env.retain(|key, _| {
        !settings
            .environment_options
            .iter()
            .any(|v| v.key.trim() == key)
    });
    env
}

/// Variables that make OpenGL and Vulkan use `card`.
///
/// Mesa takes the card's PCI address in `DRI_PRIME` (OpenGL, and Vulkan
/// through its device-select layer). `MESA_VK_DEVICE_SELECT` with `!` then
/// hides the other cards from Vulkan, since DXVK and VKD3D-Proton pick a
/// discrete card over an integrated one whatever order they're listed in;
/// it names cards by model, so it is left out when two cards are the same
/// model. NVIDIA's driver renders on its card through PRIME render offload
/// and steps aside with `__VK_LAYER_NV_optimus` when another card is
/// chosen.
fn select_card(card: &GpuAdapter, all: &[GpuAdapter]) -> EnvMap {
    let mut env = EnvMap::new();
    let mut set = |k: &str, v: String| {
        env.insert(k.to_string(), v);
    };
    let same_model = all
        .iter()
        .filter(|a| a.vendor == card.vendor && a.device == card.device)
        .count();
    if same_model == 1 {
        set("MESA_VK_DEVICE_SELECT", format!("{}!", card.pci_id()));
    }
    if card.nvidia_driver() {
        if !card.boot_vga {
            set("__NV_PRIME_RENDER_OFFLOAD", "1".into());
            set("__VK_LAYER_NV_optimus", "NVIDIA_only".into());
            set("__GLX_VENDOR_LIBRARY_NAME", "nvidia".into());
        }
    } else {
        set("DRI_PRIME", card.dri_prime());
        if all.iter().any(GpuAdapter::nvidia_driver) {
            set("__VK_LAYER_NV_optimus", "non_NVIDIA_only".into());
            set("__GLX_VENDOR_LIBRARY_NAME", "mesa".into());
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::EnvVar;

    const PCI_IDS_SAMPLE: &str = "\
# comment
10de  NVIDIA Corporation
\t2684  AD102 [GeForce RTX 4090]
\t1c03  GP106 [GeForce GTX 1060 6GB]
\t\t1043 85ac  Some subsystem
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t164e  Raphael
\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]
14e4  Broadcom Inc. and subsidiaries
\t164e  NetXtreme II BCM57710 10-Gigabit PCIe [Everest]
C 00  Unclassified device
\t00  Non-VGA unclassified device
";

    fn card(slot: &str, vendor: u16, device: u16, driver: &str, boot: bool) -> GpuAdapter {
        GpuAdapter {
            slot: slot.into(),
            vendor,
            device,
            driver: driver.into(),
            boot_vga: boot,
            model: None,
        }
    }

    fn write_card(drm: &Path, name: &str, uevent: &str, boot_vga: Option<&str>) {
        let device = drm.join(name).join("device");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join("uevent"), uevent).unwrap();
        if let Some(b) = boot_vga {
            std::fs::write(device.join("boot_vga"), b).unwrap();
        }
    }

    #[test]
    fn cards_come_from_drm_and_pci_ids() {
        let dir = tempfile::tempdir().unwrap();
        let drm = dir.path();
        write_card(
            drm,
            "card1",
            "DRIVER=amdgpu\nPCI_ID=1002:744C\nPCI_SLOT_NAME=0000:03:00.0\n",
            Some("1\n"),
        );
        write_card(
            drm,
            "card0",
            "DRIVER=amdgpu\nPCI_ID=1002:164E\nPCI_SLOT_NAME=0000:18:00.0\n",
            Some("0\n"),
        );
        // A connector, a render node and a card without a PCI device.
        std::fs::create_dir_all(drm.join("card1-DP-1")).unwrap();
        std::fs::create_dir_all(drm.join("renderD128/device")).unwrap();
        write_card(drm, "card2", "DRIVER=simple-framebuffer\n", None);

        let cards = adapters_in(drm, Some(PCI_IDS_SAMPLE));
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].slot, "0000:18:00.0");
        assert_eq!(cards[0].name(), "AMD Raphael");
        assert!(!cards[0].boot_vga);
        assert_eq!(cards[1].pci_id(), "1002:744c");
        assert_eq!(
            cards[1].name(),
            "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M"
        );
        assert!(cards[1].boot_vga);
        assert_eq!(cards[1].dri_prime(), "pci-0000_03_00_0");

        let unnamed = adapters_in(drm, None);
        assert_eq!(unnamed[1].name(), "AMD 1002:744c");
    }

    #[test]
    fn pci_ids_lookup_stays_within_the_vendor() {
        let names = model_names(PCI_IDS_SAMPLE, &[(0x1002, 0x164e), (0x10de, 0x2684)]);
        assert_eq!(names[&(0x1002, 0x164e)], "Raphael");
        assert_eq!(names[&(0x10de, 0x2684)], "AD102 [GeForce RTX 4090]");
        assert_eq!(names.len(), 2);
        let nvidia = GpuAdapter {
            model: Some("AD102 [GeForce RTX 4090]".into()),
            ..card("0000:01:00.0", NVIDIA, 0x2684, "nvidia", false)
        };
        assert_eq!(nvidia.name(), "NVIDIA GeForce RTX 4090");
        assert!(nvidia.ada_or_newer());
        assert!(!card("x", NVIDIA, 0x1c03, "nvidia", false).ada_or_newer());
    }

    #[test]
    fn choosing_a_mesa_card() {
        let cards = vec![
            card("0000:18:00.0", AMD, 0x164e, "amdgpu", false),
            card("0000:03:00.0", AMD, 0x744c, "amdgpu", true),
        ];
        let graphics = HostGraphics {
            adapters: cards,
            ..Default::default()
        };
        let gs = GameSettings {
            gpu: "0000:18:00.0".into(),
            ..Default::default()
        };
        let env = graphics_env(&gs, &graphics);
        assert_eq!(env["DRI_PRIME"], "pci-0000_18_00_0");
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "1002:164e!");
        assert!(!env.contains_key("__NV_PRIME_RENDER_OFFLOAD"));
        assert!(!env.contains_key("__VK_LAYER_NV_optimus"));
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn two_cards_of_one_model_are_told_apart_by_address() {
        let graphics = HostGraphics {
            adapters: vec![
                card("0000:03:00.0", AMD, 0x744c, "amdgpu", true),
                card("0000:04:00.0", AMD, 0x744c, "amdgpu", false),
            ],
            ..Default::default()
        };
        let gs = GameSettings {
            gpu: "0000:04:00.0".into(),
            ..Default::default()
        };
        let env = graphics_env(&gs, &graphics);
        assert_eq!(env["DRI_PRIME"], "pci-0000_04_00_0");
        assert!(!env.contains_key("MESA_VK_DEVICE_SELECT"));
    }

    #[test]
    fn choosing_between_nvidia_and_an_integrated_card() {
        let graphics = HostGraphics {
            adapters: vec![
                card("0000:00:02.0", INTEL, 0xa780, "i915", true),
                card("0000:01:00.0", NVIDIA, 0x2786, "nvidia", false),
            ],
            ..Default::default()
        };
        let mut gs = GameSettings {
            gpu: "0000:01:00.0".into(),
            ..Default::default()
        };
        let env = graphics_env(&gs, &graphics);
        assert_eq!(env["__NV_PRIME_RENDER_OFFLOAD"], "1");
        assert_eq!(env["__VK_LAYER_NV_optimus"], "NVIDIA_only");
        assert_eq!(env["__GLX_VENDOR_LIBRARY_NAME"], "nvidia");
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "10de:2786!");
        assert!(!env.contains_key("DRI_PRIME"));

        gs.gpu = "0000:00:02.0".into();
        let env = graphics_env(&gs, &graphics);
        assert_eq!(env["DRI_PRIME"], "pci-0000_00_02_0");
        assert_eq!(env["__VK_LAYER_NV_optimus"], "non_NVIDIA_only");
        assert_eq!(env["__GLX_VENDOR_LIBRARY_NAME"], "mesa");

        // NVIDIA driving the desktop needs no offload.
        let mut desktop = graphics.clone();
        desktop.adapters[0].boot_vga = false;
        desktop.adapters[1].boot_vga = true;
        gs.gpu = "0000:01:00.0".into();
        let env = graphics_env(&gs, &desktop);
        assert!(!env.contains_key("__NV_PRIME_RENDER_OFFLOAD"));
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "10de:2786!");

        // A card that is gone leaves the choice to the system.
        gs.gpu = "0000:09:00.0".into();
        assert!(graphics_env(&gs, &graphics).is_empty());
    }

    #[test]
    fn frame_generation_variables() {
        let graphics = HostGraphics {
            lsfg: Some(LsfgVk {
                manifest: "/m.json".into(),
                current: true,
                dll: Some("/lib/steamapps/common/Lossless Scaling/lsfg-vk.dll".into()),
            }),
            ..Default::default()
        };
        let mut gs = GameSettings {
            smooth_motion: true,
            ..Default::default()
        };
        gs.lsfg.enabled = true;
        gs.lsfg.multiplier = 3;
        gs.lsfg.flow_scale = 85;
        gs.lsfg.performance_mode = true;
        let env = graphics_env(&gs, &graphics);
        assert_eq!(env["NVPRESENT_ENABLE_SMOOTH_MOTION"], "1");
        assert_eq!(env["LSFGVK_ENV"], "1");
        assert_eq!(env["LSFGVK_MULTIPLIER"], "3");
        assert_eq!(env["LSFGVK_FLOW_SCALE"], "0.85");
        assert_eq!(env["LSFGVK_PERFORMANCE_MODE"], "1");
        assert_eq!(
            env["LSFGVK_DLL_PATH"],
            "/lib/steamapps/common/Lossless Scaling/lsfg-vk.dll"
        );

        // Out-of-range values are brought into lsfg-vk's range.
        gs.lsfg.multiplier = 1;
        gs.lsfg.flow_scale = 100;
        gs.lsfg.performance_mode = false;
        let env = graphics_env(&gs, &HostGraphics::default());
        assert_eq!(env["LSFGVK_MULTIPLIER"], "2");
        assert_eq!(env["LSFGVK_FLOW_SCALE"], "1");
        assert!(!env.contains_key("LSFGVK_PERFORMANCE_MODE"));
        assert!(!env.contains_key("LSFGVK_DLL_PATH"));

        assert!(graphics_env(&GameSettings::default(), &graphics).is_empty());
    }

    #[test]
    fn the_users_variables_win() {
        let graphics = HostGraphics {
            adapters: vec![card("0000:03:00.0", AMD, 0x744c, "amdgpu", true)],
            ..Default::default()
        };
        let mut gs = GameSettings {
            gpu: "0000:03:00.0".into(),
            smooth_motion: true,
            environment_options: vec![
                EnvVar {
                    key: " DRI_PRIME ".into(),
                    value: "1".into(),
                },
                EnvVar {
                    key: "LSFGVK_MULTIPLIER".into(),
                    value: "4".into(),
                },
            ],
            ..Default::default()
        };
        gs.lsfg.enabled = true;
        let env = graphics_env(&gs, &graphics);
        assert!(!env.contains_key("DRI_PRIME"));
        assert!(!env.contains_key("LSFGVK_MULTIPLIER"));
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "1002:744c!");
        assert_eq!(env["LSFGVK_FLOW_SCALE"], "1");
    }

    // The folders are the Linux loader's, read only on Linux; elsewhere
    // `Path::join` would write them with another separator.
    #[cfg(target_os = "linux")]
    #[test]
    fn layer_folders_follow_the_loader() {
        let host = HostEnv::from_pairs([
            ("XDG_DATA_HOME", "/data"),
            ("XDG_DATA_DIRS", "/a:/b"),
            ("VK_ADD_IMPLICIT_LAYER_PATH", "/extra"),
        ]);
        let dirs = implicit_layer_dirs(&host, Path::new("/home/u"));
        let shown: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        assert_eq!(
            shown,
            [
                "/home/u/.config/vulkan/implicit_layer.d",
                "/etc/xdg/vulkan/implicit_layer.d",
                "/etc/vulkan/implicit_layer.d",
                "/data/vulkan/implicit_layer.d",
                "/a/vulkan/implicit_layer.d",
                "/b/vulkan/implicit_layer.d",
                "/extra",
            ]
        );
    }

    #[test]
    fn lsfg_and_smooth_motion_detection() {
        let dir = tempfile::tempdir().unwrap();
        let layers = dir.path().join("layers");
        std::fs::create_dir_all(&layers).unwrap();
        let dirs = vec![layers.clone()];
        let library = dir.path().join("lib");
        let libraries = vec![library.clone()];
        let none = HostEnv::default();

        assert_eq!(find_lsfg(&dirs, &libraries, &none), None);
        std::fs::write(layers.join(LSFG_V1_MANIFEST), "{}").unwrap();
        let v1 = find_lsfg(&dirs, &libraries, &none).unwrap();
        assert!(!v1.current);

        std::fs::write(layers.join(LSFG_MANIFEST), "{}").unwrap();
        let v2 = find_lsfg(&dirs, &libraries, &none).unwrap();
        assert!(v2.current && v2.dll.is_none());
        let graphics = HostGraphics {
            lsfg: Some(v2),
            ..Default::default()
        };
        assert_eq!(
            graphics.lsfg_blocker(),
            Some(LsfgBlocker::NoLosslessScaling)
        );

        let folder = library.join("steamapps/common/Lossless Scaling");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Lossless.dll"), "").unwrap();
        let found = find_lsfg(&dirs, &libraries, &none).unwrap();
        assert_eq!(found.dll, Some(folder.join("Lossless.dll")));
        std::fs::write(folder.join("lsfg-vk.dll"), "").unwrap();
        let found = find_lsfg(&dirs, &libraries, &none).unwrap();
        assert_eq!(found.dll, Some(folder.join("lsfg-vk.dll")));

        let own = dir.path().join("own.dll");
        std::fs::write(&own, "").unwrap();
        let host = HostEnv::from_pairs([("LSFGVK_DLL_PATH", own.to_string_lossy().as_ref())]);
        assert_eq!(find_lsfg(&dirs, &[library], &host).unwrap().dll, Some(own));

        assert!(!has_layer(&dirs, SMOOTH_MOTION_LAYER));
        std::fs::write(
            layers.join("nvidia_layers.json"),
            r#"{"layer": {"name": "VK_LAYER_NV_present"}}"#,
        )
        .unwrap();
        assert!(has_layer(&dirs, SMOOTH_MOTION_LAYER));
    }

    #[test]
    fn smooth_motion_needs_nvidia_ada_and_a_new_driver() {
        let mut graphics = HostGraphics {
            adapters: vec![card("0000:03:00.0", AMD, 0x744c, "amdgpu", true)],
            ..Default::default()
        };
        assert_eq!(
            graphics.smooth_motion_blocker(),
            Some(SmoothMotionBlocker::NoNvidia)
        );
        graphics
            .adapters
            .push(card("0000:01:00.0", NVIDIA, 0x2204, "nvidia", false));
        assert_eq!(
            graphics.smooth_motion_blocker(),
            Some(SmoothMotionBlocker::OldCard)
        );
        graphics.adapters[1].device = 0x2b85;
        assert_eq!(
            graphics.smooth_motion_blocker(),
            Some(SmoothMotionBlocker::OldDriver)
        );
        graphics.smooth_motion_layer = true;
        assert_eq!(graphics.smooth_motion_blocker(), None);
        graphics.nvidia_driver = Some("580.95.05".into());
        assert_eq!(graphics.nvidia_major(), Some(580));
        // nouveau doesn't count.
        graphics.adapters[1].driver = "nouveau".into();
        assert_eq!(
            graphics.smooth_motion_blocker(),
            Some(SmoothMotionBlocker::NoNvidia)
        );
    }
}
