//! Actionable `Installed & on this system` pane inside the Wine manager's
//! `Builds` view: origin filters, local search, watched custom folders,
//! deduplicated Irori-managed rows (with inline update / rollback and an
//! overflow menu), and collapsible system-detected origin groups.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_core::{Event, WineInstallation};
use irori_runtime::manager::{Release, ReleaseKind};
use irori_ui::components::{EmptyState, StoreLogo};
use irori_ui::i18n::t;
use irori_ui::models;
use irori_ui::wine_installs::{cancel_install, release_key, start_install};

use super::installed::{OriginFilter, WineOrigin, kind_label, shorten_home};
use super::ui::{
    PillTone, choice_pill, eid, ellipsis_menu, muted_xs, pane_shell, pill, section_band,
    view_menu_item,
};
use super::{SystemState, WineManagerView, release_is_default, render_op};
use crate::common::{backend_op, plain_tooltip};
use crate::format;

impl WineManagerView {
    fn origin_pill(
        &self,
        id: &'static str,
        label: SharedString,
        val: OriginFilter,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        choice_pill(
            id,
            label,
            self.origin_filter == val,
            cx,
            cx.listener(move |this, _, _, cx| {
                if this.origin_filter != val {
                    this.origin_filter = val;
                    cx.notify();
                }
            }),
        )
    }

    pub(super) fn render_installed_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, bg, radius) = {
            let t = cx.theme();
            (t.border, t.background, t.radius)
        };
        let app = models(cx).settings.read(cx).app.clone();
        let filters = h_flex()
            .gap_1()
            .flex_wrap()
            .flex_1()
            .child(self.origin_pill(
                "wine-origin-all",
                t("manage.wine.installed.filter.all"),
                OriginFilter::All,
                cx,
            ))
            .child(self.origin_pill(
                "wine-origin-irori",
                t("manage.wine.installed.filter.irori"),
                OriginFilter::Irori,
                cx,
            ))
            .when(cfg!(target_os = "linux"), |r| {
                r.child(self.origin_pill(
                    "wine-origin-steam",
                    t("manage.wine.installed.filter.steam"),
                    OriginFilter::Steam,
                    cx,
                ))
            })
            .child(self.origin_pill(
                "wine-origin-other",
                t("manage.wine.installed.filter.other"),
                OriginFilter::Other,
                cx,
            ));

        let header = v_flex()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .truncate()
                            .child(t("manage.wine.pane.installed")),
                    )
                    .child(
                        Button::new("add-path")
                            .outline()
                            .small()
                            .icon(IconName::FolderPlus)
                            .label(t("manage.wine.custom.add"))
                            .on_click(cx.listener(|this, _, w, cx| this.add_custom_path(w, cx))),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .child(filters)
                    .child(
                        div()
                            .w(rems(10.5))
                            .flex_shrink_0()
                            .child(Input::new(&self.installed_search).small()),
                    ),
            )
            .when(!app.custom_wine_paths.is_empty(), |col| {
                col.child(
                    h_flex()
                        .gap_1p5()
                        .flex_wrap()
                        .items_center()
                        .child(muted_xs(
                            format!("{}:", t("manage.wine.custom.watched")),
                            cx,
                        ))
                        .children(app.custom_wine_paths.iter().enumerate().map(|(ix, p)| {
                            let (remove, full) = (p.clone(), SharedString::from(p.clone()));
                            h_flex()
                                .id(eid("custom-chip", ix))
                                .gap_1()
                                .pl_2()
                                .pr_1()
                                .py_0p5()
                                .rounded(radius)
                                .bg(bg)
                                .border_1()
                                .border_color(border)
                                .items_center()
                                .tooltip(move |w, cx| plain_tooltip(full.clone(), w, cx))
                                .child(muted_xs(shorten_home(p), cx).max_w(rems(13.)))
                                .child(
                                    Button::new(eid("remove-custom", ix))
                                        .ghost()
                                        .xsmall()
                                        .compact()
                                        .icon(IconName::X)
                                        .tooltip(t("manage.wine.custom.remove"))
                                        .on_click(move |_, _, cx| {
                                            let remove = remove.clone();
                                            backend_op(cx, move |b| {
                                                b.settings.update_app(|s| {
                                                    s.custom_wine_paths.retain(|x| x != &remove)
                                                });
                                                b.bus.emit(Event::WineVersionsChanged);
                                            });
                                        }),
                                )
                        })),
                )
            });
        pane_shell(
            "wine-installed-pane",
            "wine-installed-scroll",
            header,
            self.render_installed_body(&app, cx),
            cx,
        )
    }

    fn render_installed_body(
        &self,
        app: &irori_core::AppSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let default_bin = app.game.wine_version.bin.as_str();
        let share_steam = cfg!(target_os = "linux") && app.download_proton_to_steam;
        let previous: Vec<String> = models(cx)
            .components
            .read(cx)
            .report
            .previous
            .iter()
            .map(|p| p.version.clone())
            .collect();
        let query = self.installed_search.read(cx).value().trim().to_lowercase();

        let Some(SystemState::Loaded {
            managed, detected, ..
        }) = &self.system
        else {
            return h_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.system.loading"))
                .into_any_element();
        };
        if managed.is_empty() && detected.is_empty() && previous.is_empty() {
            return div()
                .p_4()
                .child(
                    EmptyState::new(IconName::Wine, t("manage.wine.system.empty.title"))
                        .description(t("manage.wine.system.empty.description")),
                )
                .into_any_element();
        }
        let f_managed: Vec<&Release> = if self.origin_filter.includes_managed() {
            managed
                .iter()
                .filter(|r| {
                    query.is_empty()
                        || r.version.to_lowercase().contains(&query)
                        || r.kind.to_lowercase().contains(&query)
                })
                .collect()
        } else {
            Vec::new()
        };
        let seen_ver: HashSet<&str> = f_managed.iter().map(|r| r.version.as_str()).collect();
        let orphans: Vec<String> = if self.origin_filter.includes_managed() {
            previous
                .iter()
                .filter(|p| {
                    !seen_ver.contains(p.as_str())
                        && (query.is_empty() || p.to_lowercase().contains(&query))
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let f_detected: Vec<(WineOrigin, Vec<&WineInstallation>)> = detected
            .iter()
            .filter(|(o, _)| self.origin_filter.matches_detected(*o))
            .filter_map(|(o, list)| {
                let items: Vec<&WineInstallation> = list
                    .iter()
                    .filter(|w| {
                        query.is_empty()
                            || w.name.to_lowercase().contains(&query)
                            || w.bin.to_lowercase().contains(&query)
                            || o.label().to_lowercase().contains(&query)
                    })
                    .collect();
                (!items.is_empty()).then_some((*o, items))
            })
            .collect();

        if f_managed.is_empty() && f_detected.is_empty() && orphans.is_empty() {
            return div()
                .p_6()
                .text_sm()
                .text_color(muted)
                .child(t("manage.wine.installed.no_match"))
                .into_any_element();
        }
        let mut col = v_flex().w_full();
        if !f_managed.is_empty() {
            col = col.child(section_band(
                t("manage.wine.installed.section_managed"),
                f_managed.len(),
                cx,
            ));
            for (ix, r) in f_managed.iter().enumerate() {
                let has_rb = previous.iter().any(|p| p == &r.version);
                col =
                    col.child(self.render_managed_row(ix, r, default_bin, share_steam, has_rb, cx));
            }
        }
        if !orphans.is_empty() {
            col = col.child(section_band(
                t("manage.updates.section.previous"),
                orphans.len(),
                cx,
            ));
            for ver in orphans {
                let (target, sel) = (ver.clone(), ver.clone());
                col = col.child(
                    h_flex()
                        .debug_selector(move || format!("rollback-{sel}"))
                        .w_full()
                        .gap_3()
                        .px_4()
                        .py_2p5()
                        .border_t_1()
                        .border_color(border)
                        .items_center()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .truncate()
                                        .child(ver.clone()),
                                )
                                .child(muted_xs(t("manage.updates.previous_detail"), cx)),
                        )
                        .child(
                            Button::new(eid("go-back", &ver))
                                .small()
                                .outline()
                                .icon(IconName::Undo2)
                                .label(t("manage.updates.go_back"))
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.confirm_go_back(target.clone(), w, cx)
                                })),
                        ),
                );
            }
        }
        if !f_detected.is_empty() {
            let total: usize = f_detected.iter().map(|(_, l)| l.len()).sum();
            col = col.child(section_band(
                t("manage.wine.installed.section_detected"),
                total,
                cx,
            ));
            for (o, list) in f_detected {
                col =
                    col.child(self.render_origin_group(o, list, default_bin, query.is_empty(), cx));
            }
        }
        col.into_any_element()
    }

    fn render_origin_group(
        &self,
        origin: WineOrigin,
        list: Vec<&WineInstallation>,
        default_bin: &str,
        can_collapse: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let collapsed = can_collapse && self.collapsed_origins.contains(&origin);
        let icon = match origin {
            WineOrigin::Steam => StoreLogo::Steam.icon().text_color(muted),
            WineOrigin::Lutris => StoreLogo::Lutris.icon().text_color(muted),
            WineOrigin::Custom => Icon::new(IconName::Folder).xsmall().text_color(muted),
            WineOrigin::System => Icon::new(IconName::HardDrive).xsmall().text_color(muted),
            WineOrigin::Irori | WineOrigin::Bottles | WineOrigin::Other => {
                Icon::new(IconName::Wine).xsmall().text_color(muted)
            }
        };
        let chev = if collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        let mut grp = v_flex().w_full().border_t_1().border_color(border).child(
            h_flex()
                .id(eid("origin-group", format!("{origin:?}")))
                .w_full()
                .px_4()
                .py_2()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed_origins.remove(&origin) {
                        this.collapsed_origins.insert(origin);
                    }
                    cx.notify();
                }))
                .child(Icon::new(chev).xsmall().text_color(muted))
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .font_medium()
                        .truncate()
                        .child(origin.label()),
                )
                .child(muted_xs(list.len().to_string(), cx)),
        );
        if !collapsed {
            for (ix, w) in list.into_iter().enumerate() {
                grp = grp.child(self.render_detected_row(origin, ix, w, default_bin, cx));
            }
        }
        grp.into_any_element()
    }

    fn render_managed_row(
        &self,
        ix: usize,
        release: &Release,
        default_bin: &str,
        share_steam: bool,
        has_rb: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (border, info) = (cx.theme().border, cx.theme().info);
        let key = release_key(release);
        let op = self.ops.read(cx).running.get(&key).map(|op| op.snapshot());
        let is_default = release_is_default(release, default_bin);
        let has_update = release.has_update == Some(true);
        let in_steam = share_steam && release.release_kind().is_some_and(ReleaseKind::is_proton);
        let size = if release.disksize > 0 {
            release.disksize
        } else {
            release.downsize
        };
        let meta = [
            format::size(size),
            (!release.date.is_empty()).then(|| irori_ui::dates::localized_date(&release.date)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let actions: AnyElement = if op.is_some() {
            let k = key.clone();
            Button::new(eid("managed-cancel", &key))
                .small()
                .outline()
                .label(t("manage.wine.cancel"))
                .on_click(move |_, _, cx| cancel_install(&k, cx))
                .into_any_element()
        } else {
            let (view, wine_def, dir, notes) = (
                cx.entity().downgrade(),
                release.installation(),
                release.install_dir.clone().unwrap_or_default(),
                irori_ui::web_url(&release.release_notes_link),
            );
            let (ver_rb, rel_upd, rel_rem) =
                (release.version.clone(), release.clone(), release.clone());
            let mrb = ver_rb.clone();
            let mut bar = h_flex().gap_1().flex_shrink_0().items_center();
            if has_update {
                bar = bar.child(
                    Button::new(eid("managed-upd", &key))
                        .primary()
                        .small()
                        .label(t("manage.wine.update"))
                        .on_click(move |_, _, cx| {
                            start_install(rel_upd.clone(), false, cx);
                        }),
                );
            } else if has_rb {
                bar = bar.child(
                    Button::new(eid("go-back", &ver_rb))
                        .outline()
                        .small()
                        .icon(IconName::Undo2)
                        .label(t("manage.updates.go_back"))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.confirm_go_back(ver_rb.clone(), w, cx)
                        })),
                );
            }
            bar.child(ellipsis_menu(
                eid("managed-menu", &key),
                move |mut menu, _, _| {
                    if !is_default && let Some(w) = wine_def.clone() {
                        menu = menu.item(view_menu_item(
                            t("manage.wine.action.set_default"),
                            IconName::Star,
                            view.clone(),
                            move |this, win, cx| {
                                this.set_default_wine(w.clone(), win, cx);
                            },
                        ));
                    }
                    if has_rb {
                        let rb = mrb.clone();
                        menu = menu.item(view_menu_item(
                            t("manage.updates.go_back"),
                            IconName::Undo2,
                            view.clone(),
                            move |this, win, cx| {
                                this.confirm_go_back(rb.clone(), win, cx);
                            },
                        ));
                    }
                    if !dir.is_empty() {
                        let d = dir.clone();
                        menu = menu.item(
                            PopupMenuItem::new(t("manage.wine.open_folder"))
                                .icon(IconName::FolderOpen)
                                .on_click(move |_, _, cx| cx.open_with_system(Path::new(&d))),
                        );
                    }
                    if let Some(url) = notes.clone() {
                        menu = menu.item(
                            PopupMenuItem::new(t("manage.wine.release_notes"))
                                .icon(IconName::ExternalLink)
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        );
                    }
                    let rem = rel_rem.clone();
                    menu.separator().item(view_menu_item(
                        t("manage.wine.remove"),
                        IconName::Trash,
                        view.clone(),
                        move |this, win, cx| {
                            this.confirm_remove(rem.clone(), win, cx);
                        },
                    ))
                },
            ))
            .into_any_element()
        };

        let head = h_flex()
            .gap_1p5()
            .flex_wrap()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .truncate()
                    .child(release.version.clone()),
            )
            .when(is_default, |h| {
                h.child(pill(
                    t("manage.wine.tag.default"),
                    PillTone::Accent,
                    true,
                    cx,
                ))
            })
            .when(in_steam, |h| {
                h.child(pill(
                    t("manage.wine.tag.in_steam"),
                    PillTone::Info,
                    false,
                    cx,
                ))
            })
            .when(has_update && op.is_none(), |h| {
                h.child(pill(
                    t("manage.wine.tag.update_available"),
                    PillTone::Warning,
                    true,
                    cx,
                ))
            });
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(head)
            .when(!meta.is_empty(), |c| c.child(muted_xs(meta, cx)))
            .when(has_rb && op.is_none(), |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(info)
                        .truncate()
                        .child(t("manage.updates.previous_detail")),
                )
            })
            .children(op.as_ref().map(|live| render_op(live, cx)));
        let sel_ver = release.version.clone();
        h_flex()
            .id(eid("managed-row", &key))
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .items_start()
            .when(has_rb, |r| {
                r.debug_selector(move || format!("rollback-{sel_ver}"))
            })
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .child(left)
            .child(actions)
            .into_any_element()
    }

    fn render_detected_row(
        &self,
        origin: WineOrigin,
        ix: usize,
        w: &WineInstallation,
        default_bin: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_default = !default_bin.is_empty() && w.bin == default_bin;
        let reveal = Path::new(&w.bin)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&w.bin));
        let (full_bin, view, wine_def) = (
            SharedString::from(w.bin.clone()),
            cx.entity().downgrade(),
            w.clone(),
        );
        let head = h_flex()
            .gap_1p5()
            .flex_wrap()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .truncate()
                    .child(w.name.clone()),
            )
            .when(is_default, |h| {
                h.child(pill(
                    t("manage.wine.tag.default"),
                    PillTone::Accent,
                    true,
                    cx,
                ))
            })
            .child(pill(kind_label(w.kind).into(), PillTone::Muted, false, cx));
        let path_el = muted_xs(shorten_home(&w.bin), cx)
            .id(eid("sys-path", format!("{origin:?}-{ix}")))
            .tooltip(move |win, cx| plain_tooltip(full_bin.clone(), win, cx));
        let menu = ellipsis_menu(
            eid("sys-menu", format!("{origin:?}-{ix}")),
            move |mut menu, _, _| {
                if !is_default {
                    let wine = wine_def.clone();
                    menu = menu.item(view_menu_item(
                        t("manage.wine.action.set_default"),
                        IconName::Star,
                        view.clone(),
                        move |this, win, cx| {
                            this.set_default_wine(wine.clone(), win, cx);
                        },
                    ));
                }
                let rev = reveal.clone();
                menu.item(
                    PopupMenuItem::new(t("manage.wine.open_folder"))
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, _, cx| cx.reveal_path(&rev)),
                )
            },
        );
        h_flex()
            .w_full()
            .gap_3()
            .pl_8()
            .pr_4()
            .py_2p5()
            .border_t_1()
            .border_color(cx.theme().border)
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(head)
                    .child(path_el),
            )
            .child(menu)
            .into_any_element()
    }
}
