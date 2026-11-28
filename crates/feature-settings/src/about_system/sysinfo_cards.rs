//! System & environment cards for `AboutSystemPanel`: CPU, memory bar,
//! cleaned GPU summary with expandable PCI metadata, OS/session badges,
//! structured display rows with expandable modes, and gaming tool readiness.

use gpui_kit::assets::IconName;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Div, Hsla, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, relative, rems,
};
use irori_runtime::sysinfo::{GpuInfo, SoftwareVersions, SteamDeckModel, SystemInfo, VulkanInfo};
use irori_services::displays::{Display, DisplayReport};
use irori_ui::i18n::{t, t_args};

use super::{
    AboutSystemPanel, action_btn, disclosure_toggle, info_tag, mono_xs, muted_tag, row_between,
    split_row, trunc_sm, two_col_grid, xs_muted,
};
use crate::displays::{CapabilityTone, hdr_badge, mode_compact, mode_size_entries, vrr_badge};
use crate::widgets::{muted, panel};

impl AboutSystemPanel {
    pub(super) fn render_system_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (copy_icon, copy_key) = if self.copied_sysinfo {
            (IconName::Check, "settings.common.copied")
        } else {
            (IconName::Copy, "settings.about.copy_system_info")
        };
        let on_refresh = cx.listener(|this, _, w, cx| this.load_sysinfo(w, cx));
        let on_copy = cx.listener(|this, _, w, cx| this.copy_sysinfo(w, cx));
        let refresh = action_btn(
            "sysinfo-refresh",
            IconName::RefreshCw,
            "settings.common.refresh",
        )
        .outline()
        .loading(self.loading)
        .on_click(on_refresh);
        let copy = action_btn("sysinfo-copy", copy_icon, copy_key)
            .outline()
            .disabled(self.info.is_none())
            .on_click(on_copy);
        let header = split_row()
            .pt_2()
            .pb_1()
            .gap_3()
            .child(irori_ui::style::caption(
                t("settings.about.system_section"),
                cx,
            ))
            .child(h_flex().gap_2().flex_shrink_0().child(refresh).child(copy));

        let Some(info) = &self.info else {
            let key = if self.loading {
                "settings.sysinfo.loading"
            } else {
                "settings.sysinfo.unavailable"
            };
            return v_flex()
                .gap_2()
                .child(header)
                .child(panel(cx).px_4().py_3().child(muted(t(key), cx)))
                .into_any_element();
        };

        let show_gpu = !info.gpus.is_empty() || cfg!(target_os = "linux");
        let multi_gpu = info.gpus.len() > 1;
        let gpu_card = show_gpu.then(|| self.render_graphics_card(info, multi_gpu, cx));
        let hardware = two_col_grid()
            .gap_2p5()
            .child(render_cpu_card(info, cx))
            .child(render_memory_card(info, cx))
            .children(gpu_card)
            .child(render_os_card(info, multi_gpu || !show_gpu, cx));

        let tools = host_gaming_tools(&info.software);
        v_flex()
            .gap_2p5()
            .child(header)
            .child(hardware)
            .when_some(self.displays.as_ref(), |c, d| {
                c.child(self.render_displays_card(d, cx))
            })
            .when(!tools.is_empty(), |c| {
                c.child(render_tools_card(&tools, cx))
            })
            .into_any_element()
    }

    fn render_graphics_card(
        &self,
        info: &SystemInfo,
        multi_gpu: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = if multi_gpu {
            t_args(
                "settings.sysinfo.graphics_count",
                &[("count", &info.gpus.len().to_string())],
            )
        } else {
            t("settings.sysinfo.graphics")
        };
        let mut tags = wrap_row();
        if !multi_gpu && let Some(gpu) = info.gpus.first() {
            tags = tags.child(muted_tag(gpu_driver(gpu)));
        }
        if cfg!(target_os = "linux") {
            tags = tags.child(vulkan_tag(&info.vulkan));
        }

        let gpu_list = if info.gpus.is_empty() {
            v_flex().child(muted(t("settings.sysinfo.unavailable"), cx))
        } else {
            let rows: Vec<Div> = info
                .gpus
                .iter()
                .enumerate()
                .map(|(i, g)| render_gpu_entry(i, g, multi_gpu, self.pci_expanded, cx))
                .collect();
            v_flex().gap_2().children(rows)
        };
        let pci_toggle = (!info.gpus.is_empty()).then(|| {
            let key = if self.pci_expanded {
                "settings.sysinfo.pci_hide"
            } else {
                "settings.sysinfo.pci_show"
            };
            disclosure_toggle(
                "sysinfo-pci-toggle",
                self.pci_expanded,
                t(key),
                cx.listener(|this, _, _, cx| {
                    this.pci_expanded = !this.pci_expanded;
                    cx.notify();
                }),
            )
        });
        spec_card(cx)
            .when(multi_gpu, |c| c.col_span(2))
            .child(spec_header(IconName::Gpu, title, tags, cx))
            .child(gpu_list)
            .children(pci_toggle)
    }

    fn render_displays_card(&self, report: &DisplayReport, cx: &mut Context<Self>) -> AnyElement {
        if report.displays.is_empty() {
            let hdr = spec_header(
                IconName::Monitor,
                t("settings.display.label"),
                muted(t("settings.display.none"), cx),
                cx,
            );
            return panel(cx).px_4().py_3().child(hdr).into_any_element();
        }
        let bdr = cx.theme().border;
        let rows: Vec<AnyElement> = report
            .displays
            .iter()
            .enumerate()
            .map(|(i, d)| self.render_display_row(i, d, report.lists_modes, bdr, cx))
            .collect();
        panel(cx)
            .overflow_hidden()
            .children(rows)
            .into_any_element()
    }

    fn render_display_row(
        &self,
        ix: usize,
        d: &Display,
        lists_modes: bool,
        bdr: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = d.label();
        let mode_chip = if !d.enabled {
            t("settings.display.off")
        } else {
            d.current
                .as_ref()
                .map(mode_compact)
                .unwrap_or_else(|| t("settings.display.mode_unknown"))
        };
        let mut chips = wrap_row().child(muted_tag(mode_chip));
        for (lbl, tone) in [hdr_badge(d), vrr_badge(d)].into_iter().flatten() {
            chips = chips.child(match tone {
                CapabilityTone::Active => Tag::success().xsmall().outline().child(lbl),
                CapabilityTone::Capable => info_tag(lbl),
                CapabilityTone::Muted => muted_tag(lbl),
            });
        }
        let title = wrap_row()
            .gap_2()
            .min_w_0()
            .child(
                Icon::new(IconName::Monitor)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(spec_value(if name.is_empty() {
                t("settings.display.label")
            } else {
                name.into()
            }))
            .when(d.name.is_some() && !d.connector.is_empty(), |r| {
                r.child(muted_tag(d.connector.clone()))
            })
            .when(d.primary, |r| {
                r.child(info_tag(t("settings.display.main_short")))
            });

        let modes = (lists_modes && !d.modes.is_empty())
            .then(|| mode_size_entries(d))
            .filter(|e| !e.is_empty())
            .map(|e| self.render_modes_disclosure(ix, e, cx));
        v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .when(ix > 0, |r| r.border_t_1().border_color(bdr))
            .child(split_row().child(title).child(chips))
            .children(modes)
            .into_any_element()
    }

    fn render_modes_disclosure(
        &self,
        ix: usize,
        entries: Vec<(SharedString, SharedString, bool)>,
        cx: &mut Context<Self>,
    ) -> Collapsible {
        let open = self.modes_expanded.contains(&ix);
        let label = match (open, entries.len()) {
            (true, _) => t("settings.display.modes_hide"),
            (false, 1) => t("settings.display.modes_show_one"),
            (false, n) => t_args(
                "settings.display.modes_show_other",
                &[("count", &n.to_string())],
            ),
        };
        let click = cx.listener(move |this, _, _, cx| {
            if !this.modes_expanded.remove(&ix) {
                this.modes_expanded.insert(ix);
            }
            cx.notify();
        });
        let trigger = disclosure_toggle(("display-modes-toggle", ix), open, label, click);
        let mut box_ = Collapsible::new().w_full().open(open).child(trigger);
        if open {
            let th = cx.theme();
            let cur_lbl = t("settings.display.current_mode");
            let cells = entries.into_iter().map(|(res, hz, cur)| {
                let (bg, bdr) = if cur {
                    (th.primary.opacity(0.10), th.primary.opacity(0.40))
                } else {
                    (th.muted, th.border)
                };
                let cur_el = cur.then(|| {
                    div()
                        .text_xs()
                        .text_color(th.primary)
                        .flex_shrink_0()
                        .child(cur_lbl.clone())
                });
                let top = row_between()
                    .child(div().text_xs().font_semibold().truncate().child(res))
                    .children(cur_el);
                inset_box(bg, bdr, cx)
                    .min_w_0()
                    .child(top)
                    .when(!hz.is_empty(), |c| c.child(mono_xs(hz, cx)))
            });
            box_ = box_.content(irori_ui::motion::fade_in(
                ("display-modes-content", ix),
                two_col_grid().gap_1p5().pt_1p5().children(cells),
            ));
        }
        box_
    }
}

fn vulkan_tag(vk: &VulkanInfo) -> Tag {
    if !vk.available() {
        return Tag::warning()
            .xsmall()
            .outline()
            .child(format!("Vulkan · {}", t("settings.sysinfo.not_available")));
    }
    let lbl: SharedString = match &vk.instance_version {
        Some(v) => format!("Vulkan {v}").into(),
        None => format!("Vulkan · {}", t("settings.sysinfo.available")).into(),
    };
    muted_tag(lbl)
}

fn render_cpu_card(info: &SystemInfo, cx: &App) -> Div {
    let threads = if info.cpu.cores == 1 {
        t("settings.sysinfo.threads_one")
    } else {
        t_args(
            "settings.sysinfo.threads_other",
            &[("count", &info.cpu.cores.to_string())],
        )
    };
    let name = clean_cpu_model(&info.cpu.model);
    let title = if name.is_empty() {
        "-".to_string()
    } else {
        name
    };
    spec_card(cx)
        .child(spec_header(
            IconName::Cpu,
            t("settings.sysinfo.cpu"),
            muted_tag(threads),
            cx,
        ))
        .child(spec_value(title))
}

fn render_memory_card(info: &SystemInfo, cx: &App) -> Div {
    let th = cx.theme();
    let (pct, ratio) = memory_percent(info.memory.used, info.memory.total);
    let used = irori_core::util::format_bytes(info.memory.used);
    let total = irori_core::util::format_bytes(info.memory.total);
    let pct_tag = (info.memory.total > 0).then(|| {
        muted_tag(t_args(
            "settings.sysinfo.memory_percent",
            &[("percent", &pct.to_string())],
        ))
    });
    let bar = (info.memory.total > 0).then(|| {
        let fill = div()
            .h_full()
            .w(relative(ratio))
            .rounded_full()
            .bg(th.primary);
        div()
            .w_full()
            .h(rems(0.32))
            .rounded_full()
            .bg(th.muted)
            .border_1()
            .border_color(th.border)
            .overflow_hidden()
            .child(fill)
    });
    let hdr = spec_header(
        IconName::MemoryStick,
        t("settings.sysinfo.memory"),
        h_flex().children(pct_tag),
        cx,
    );
    let val = t_args(
        "settings.sysinfo.memory_value",
        &[("used", &used), ("total", &total)],
    );
    spec_card(cx)
        .child(hdr)
        .child(spec_value(val))
        .children(bar)
}

fn render_gpu_entry(
    ix: usize,
    gpu: &GpuInfo,
    multi_gpu: bool,
    pci_expanded: bool,
    cx: &App,
) -> Div {
    let th = cx.theme();
    let row = split_row()
        .child(spec_value(clean_gpu_name(gpu)).flex_1())
        .when(multi_gpu, |r| {
            let badges = h_flex()
                .gap_1p5()
                .flex_shrink_0()
                .when(gpu.boot_vga, |t_| {
                    t_.child(info_tag(t("settings.sysinfo.gpu_primary")))
                })
                .child(muted_tag(gpu_driver(gpu)));
            r.child(badges)
        });
    let pci = pci_expanded.then(|| {
        let raw_name = match (&gpu.vendor_name, &gpu.device_name) {
            (Some(v), Some(d)) => format!("{v} {d}"),
            (None, Some(d)) => d.clone(),
            (Some(v), None) => v.clone(),
            (None, None) => format!("PCI {}:{}", gpu.vendor_id, gpu.device_id),
        };
        let slot = (!gpu.slot.is_empty()).then(|| format!("Slot {}", gpu.slot));
        let ids = (!gpu.device_id.is_empty() || !gpu.vendor_id.is_empty())
            .then(|| format!("D={} V={}", gpu.device_id, gpu.vendor_id));
        let meta = [slot, ids]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        inset_box(th.muted, th.border, cx)
            .font_family(th.mono_font_family.clone())
            .text_xs()
            .text_color(th.muted_foreground)
            .child(raw_name)
            .when(!meta.is_empty(), |b| b.child(meta))
    });
    v_flex()
        .gap_1()
        .when(ix > 0, |d| d.pt_2().border_t_1().border_color(th.border))
        .child(row)
        .children(pci)
}

fn render_tools_card(tools: &[(&'static str, Option<&str>)], cx: &App) -> AnyElement {
    let th = cx.theme();
    let (ready, total) = (
        tools.iter().filter(|(_, v)| v.is_some()).count(),
        tools.len(),
    );
    let ready_lbl = t_args(
        "settings.sysinfo.tools_ready",
        &[("ready", &ready.to_string()), ("total", &total.to_string())],
    );
    let ready_tag = if ready == total {
        Tag::success().xsmall().outline().child(ready_lbl)
    } else {
        muted_tag(ready_lbl)
    };
    let missing = t("settings.sysinfo.not_installed");
    let cells: Vec<Div> = tools
        .iter()
        .enumerate()
        .map(|(ix, &(name, ver))| {
            let ok = ver.is_some();
            let dot = div()
                .flex_none()
                .size_2()
                .rounded_full()
                .when(ok, |d| d.bg(th.success))
                .when(!ok, |d| d.border_1().border_color(th.muted_foreground));
            let detail = match ver {
                Some(v) => mono_xs(v.to_string(), cx),
                None => xs_muted(missing.clone(), cx).truncate(),
            };
            let lbl = trunc_sm(name)
                .font_medium()
                .when(!ok, |d| d.text_color(th.muted_foreground));
            let row = h_flex()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .child(dot)
                .child(lbl);
            v_flex()
                .min_w_0()
                .gap_1()
                .px_3()
                .py_2p5()
                .when(!ok, |c| c.bg(th.muted.opacity(0.6)))
                .when(ix > 0, |c| c.border_l_1().border_color(th.border))
                .child(row)
                .child(detail.pl_3p5())
        })
        .collect();
    let header = spec_header(
        IconName::Gamepad2,
        t("settings.sysinfo.gaming_tools"),
        ready_tag,
        cx,
    )
    .px_4()
    .py_2()
    .bg(th.muted)
    .border_b_1()
    .border_color(th.border);
    let grid = div()
        .grid()
        .grid_cols(total.clamp(1, 5) as u16)
        .w_full()
        .children(cells);
    panel(cx)
        .overflow_hidden()
        .child(header)
        .child(grid)
        .into_any_element()
}

fn render_os_card(info: &SystemInfo, full_span: bool, cx: &App) -> Div {
    let os_title = match &info.os.version {
        Some(v) if !v.trim().is_empty() => format!("{} {}", info.os.name, v.trim()),
        _ => info.os.name.clone(),
    };
    let mut tags = wrap_row();
    if cfg!(target_os = "linux") {
        let extras = [
            info.desktop.as_deref(),
            info.session_type.as_deref(),
            info.is_flatpak.then_some("Flatpak"),
            (!info.is_flatpak && info.is_appimage).then_some("AppImage"),
        ];
        for val in extras.into_iter().flatten().filter(|s| !s.is_empty()) {
            tags = tags.child(muted_tag(val.to_string()));
        }
        if let Some((model, _)) = info.steam_deck {
            let key = match model {
                SteamDeckModel::Lcd => "settings.sysinfo.steam_deck_lcd",
                SteamDeckModel::Oled => "settings.sysinfo.steam_deck_oled",
            };
            tags = tags.child(info_tag(t(key)));
        }
    }
    let kernel = (!info.os.kernel.is_empty()).then(|| {
        mono_xs(
            t_args(
                "settings.sysinfo.kernel_value",
                &[("kernel", &info.os.kernel)],
            ),
            cx,
        )
    });
    spec_card(cx)
        .when(full_span, |c| c.col_span(2))
        .child(spec_header(
            IconName::Terminal,
            t("settings.sysinfo.os"),
            tags,
            cx,
        ))
        .child(spec_value(os_title))
        .children(kernel)
}

fn wrap_row() -> Div {
    h_flex().flex_wrap().items_center().gap_1p5()
}
fn inset_box(bg: Hsla, bdr: Hsla, cx: &App) -> Div {
    v_flex()
        .gap_0p5()
        .px_2p5()
        .py_1p5()
        .rounded(cx.theme().radius)
        .border_1()
        .bg(bg)
        .border_color(bdr)
}
fn spec_card(cx: &App) -> Div {
    panel(cx).min_w_0().gap_1p5().px_3p5().py_3()
}
fn spec_value(text: impl IntoElement) -> Div {
    div().text_sm().font_semibold().min_w_0().child(text)
}
fn spec_header(icon: IconName, label: SharedString, right: impl IntoElement, cx: &App) -> Div {
    let fg = cx.theme().muted_foreground;
    let kicker = h_flex()
        .items_center()
        .gap_1p5()
        .child(Icon::new(icon).xsmall().text_color(fg))
        .child(irori_ui::style::caption(label, cx));
    split_row().child(kicker).child(right)
}

fn gpu_driver(gpu: &GpuInfo) -> SharedString {
    match (&gpu.driver, &gpu.driver_version) {
        (Some(d), Some(v)) => format!("{d} {v}").into(),
        (Some(d), None) => d.clone().into(),
        _ => t("settings.sysinfo.no_driver"),
    }
}

/// Remove trademark noise from CPU brand strings while keeping model tokens intact.
pub(crate) fn clean_cpu_model(raw: &str) -> String {
    raw.replace("(R)", "")
        .replace("(TM)", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Shorten PCI database vendor and bracketed device names into a readable GPU title.
pub(crate) fn clean_gpu_name(gpu: &GpuInfo) -> String {
    let vendor = gpu
        .vendor_name
        .as_deref()
        .map(shorten_vendor)
        .filter(|v| !v.is_empty());
    if let Some(dev) = gpu.device_name.as_deref().filter(|s| !s.trim().is_empty()) {
        let model = dev
            .split_once('[')
            .and_then(|(_, r)| r.rsplit_once(']'))
            .map(|(m, _)| m.trim())
            .filter(|m| !m.is_empty())
            .unwrap_or(dev.trim());
        let dup = vendor.is_some_and(|v| {
            model
                .to_ascii_lowercase()
                .starts_with(&v.to_ascii_lowercase())
        });
        return match vendor.filter(|_| !dup) {
            Some(v) => format!("{v} {model}"),
            None => model.to_string(),
        };
    }
    match vendor {
        Some(v) => format!("{v} ({}:{})", gpu.vendor_id, gpu.device_id),
        None => format!("PCI {}:{}", gpu.vendor_id, gpu.device_id),
    }
}

fn shorten_vendor(vendor: &str) -> &str {
    match vendor.to_ascii_lowercase().as_str() {
        l if l.contains("advanced micro devices") || l.contains("[amd") => "AMD",
        l if l.contains("nvidia") => "NVIDIA",
        l if l.contains("intel") => "Intel",
        _ => vendor.trim(),
    }
}

/// Percentage `(0..=100, 0.0..=1.0)` of memory in use, safe when `total == 0`.
pub(crate) fn memory_percent(used: u64, total: u64) -> (u8, f32) {
    if total == 0 {
        return (0, 0.0);
    }
    let ratio = (used as f64 / total as f64).clamp(0.0, 1.0);
    ((ratio * 100.0).round() as u8, ratio as f32)
}

/// Host-relevant gaming helper tools shown in the readiness strip.
pub(crate) fn host_gaming_tools(software: &SoftwareVersions) -> Vec<(&'static str, Option<&str>)> {
    [
        ("Wine", software.wine.as_deref()),
        ("umu-launcher", software.umu.as_deref()),
        ("gamescope", software.gamescope.as_deref()),
        ("MangoHud", software.mangohud.as_deref()),
        ("GameMode", software.gamemode.as_deref()),
    ]
    .into_iter()
    .filter(|(name, _)| cfg!(target_os = "linux") || (*name == "Wine" && !cfg!(windows)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_helpers_clean_cpu_gpu_and_memory() {
        assert_eq!(
            clean_cpu_model("Intel(R) Core(TM) i5-14600K"),
            "Intel Core i5-14600K"
        );
        assert_eq!(
            clean_cpu_model("AMD   Ryzen(TM) 7 7800X3D"),
            "AMD Ryzen 7 7800X3D"
        );
        assert_eq!(
            (
                memory_percent(0, 0),
                memory_percent(50, 200),
                memory_percent(400, 200)
            ),
            ((0, 0.0), (25, 0.25), (100, 1.0))
        );
        let gpu = |v: Option<&str>, d: Option<&str>| GpuInfo {
            slot: "0000:03:00.0".into(),
            vendor_id: "1002".into(),
            device_id: "7550".into(),
            vendor_name: v.map(str::to_string),
            device_name: d.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(
            clean_gpu_name(&gpu(
                Some("Advanced Micro Devices, Inc. [AMD/ATI]"),
                Some("Navi 48 [Radeon RX 9070]")
            )),
            "AMD Radeon RX 9070"
        );
        assert_eq!(
            clean_gpu_name(&gpu(
                Some("NVIDIA Corporation"),
                Some("AD103 [GeForce RTX 4080 SUPER]")
            )),
            "NVIDIA GeForce RTX 4080 SUPER"
        );
        assert_eq!(clean_gpu_name(&gpu(None, None)), "PCI 1002:7550");
    }
}
