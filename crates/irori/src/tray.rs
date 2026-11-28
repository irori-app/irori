//! System tray (StatusNotifierItem) with quick access to recent games, and
//! "Save clip" while the replay buffer records.

use futures::channel::mpsc::UnboundedSender;
use irori_core::GameId;
use irori_ui::i18n::t;
use ksni::TrayMethods as _;

use crate::ShellCommand;

const ICON_SVG: &[u8] = include_bytes!("../../../packaging/irori.svg");
const TRAY_ICON_SIZES: [u32; 2] = [32, 64];

fn render_icon(tree: &resvg::usvg::Tree, size: u32, monochrome: bool) -> Option<ksni::Icon> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    // Leave subtle optical padding so the full-bleed rounded tile matches
    // neighboring tray icons instead of filling the entire slot edge-to-edge.
    let inset = size / 32;
    let inner = size - inset * 2;
    let transform = resvg::tiny_skia::Transform::from_scale(
        inner as f32 / tree.size().width(),
        inner as f32 / tree.size().height(),
    )
    .post_translate(inset as f32, inset as f32);
    resvg::render(tree, transform, &mut pixmap.as_mut());

    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for pixel in pixmap.pixels() {
        let c = pixel.demultiply();
        let [r, g, b] = if monochrome {
            // Integer BT.709 luma keeps the dark-tray option monochrome on panels
            // that expect a neutral silhouette.
            let luma =
                ((u16::from(c.red()) * 54 + u16::from(c.green()) * 183 + u16::from(c.blue()) * 19)
                    >> 8) as u8;
            [luma, luma, luma]
        } else {
            [c.red(), c.green(), c.blue()]
        };
        data.extend_from_slice(&[c.alpha(), r, g, b]);
    }

    let side = i32::try_from(size).ok()?;
    Some(ksni::Icon {
        width: side,
        height: side,
        data,
    })
}

fn render_tray_icons(monochrome: bool) -> Vec<ksni::Icon> {
    let tree = match resvg::usvg::Tree::from_data(ICON_SVG, &Default::default()) {
        Ok(tree) => tree,
        Err(e) => {
            tracing::warn!("could not parse tray icon SVG: {e}");
            return Vec::new();
        }
    };
    TRAY_ICON_SIZES
        .into_iter()
        .filter_map(|size| render_icon(&tree, size, monochrome))
        .collect()
}

struct TrayIcon {
    name: String,
    pixmap: Vec<ksni::Icon>,
}

fn build_tray_icon(dark_icon: bool, generic_icon: bool) -> TrayIcon {
    if generic_icon {
        let name = if dark_icon {
            "applications-games-symbolic"
        } else {
            "applications-games"
        };
        return TrayIcon {
            name: name.into(),
            pixmap: Vec::new(),
        };
    }
    let pixmap = render_tray_icons(dark_icon);
    let name = if pixmap.is_empty() {
        "irori".into()
    } else {
        // Leave `icon_name` empty when providing pixmaps so StatusNotifier
        // hosts always show Irori's embedded mark even if a desktop icon
        // theme has no `irori` entry (e.g. AppImage or `cargo run`).
        String::new()
    };
    TrayIcon { name, pixmap }
}

pub struct IroriTray {
    tx: UnboundedSender<ShellCommand>,
    recent: Vec<(GameId, String)>,
    icon: TrayIcon,
    icon_rev: u64,
    /// The replay buffer records, so a clip can be saved.
    replay: bool,
}

impl IroriTray {
    fn apply_icon(&mut self, icon: TrayIcon, rev: u64) {
        if rev >= self.icon_rev {
            self.icon_rev = rev;
            self.icon = icon;
        }
    }
}

/// Handle to update the tray menu.
pub type TrayHandle = ksni::Handle<IroriTray>;

impl ksni::Tray for IroriTray {
    fn id(&self) -> String {
        "irori".into()
    }

    fn title(&self) -> String {
        "Irori".into()
    }

    fn icon_name(&self) -> String {
        self.icon.name.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.pixmap.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(ShellCommand::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{StandardItem, SubMenu};
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            StandardItem {
                label: t("shell.tray.show").to_string(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.unbounded_send(ShellCommand::Show);
                }),
                ..Default::default()
            }
            .into(),
        ];
        if self.replay {
            items.push(
                StandardItem {
                    label: t("shell.tray.save_clip").to_string(),
                    icon_name: "media-record".into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.unbounded_send(ShellCommand::SaveClip);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        if !self.recent.is_empty() {
            items.push(
                SubMenu {
                    label: t("shell.tray.recent").to_string(),
                    submenu: self
                        .recent
                        .iter()
                        .map(|(id, title)| {
                            let id = id.clone();
                            StandardItem {
                                label: title.clone(),
                                activate: Box::new(move |t: &mut Self| {
                                    let _ = t.tx.unbounded_send(ShellCommand::Launch(id.clone()));
                                }),
                                ..Default::default()
                            }
                            .into()
                        })
                        .collect(),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.extend([
            StandardItem {
                label: t("shell.nav.big_picture").to_string(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.unbounded_send(ShellCommand::BigPicture);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: t("shell.tray.quit").to_string(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.unbounded_send(ShellCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

/// Start the tray on the backend runtime. Returns `None` when no tray host is
/// available (the app then quits on window close regardless of settings).
pub async fn start(
    tx: UnboundedSender<ShellCommand>,
    dark_icon: bool,
    generic_icon: bool,
) -> Option<TrayHandle> {
    let tray = IroriTray {
        tx,
        recent: Vec::new(),
        icon: build_tray_icon(dark_icon, generic_icon),
        icon_rev: 0,
        replay: false,
    };
    match tray.spawn().await {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::info!("system tray unavailable: {e}");
            None
        }
    }
}

/// Switch the tray icon between Irori's embedded logo and the desktop theme's
/// generic games icon (with monochrome/symbolic variants when `dark_icon` is on).
/// Ignores stale updates with a lower `rev` so rapid toggles always converge.
pub async fn set_icon(handle: &TrayHandle, dark_icon: bool, generic_icon: bool, rev: u64) {
    let icon = build_tray_icon(dark_icon, generic_icon);
    handle.update(move |t| t.apply_icon(icon, rev)).await;
}

/// Replace the recent games shown in the tray menu.
pub async fn set_recent(handle: &TrayHandle, recent: Vec<(GameId, String)>) {
    handle.update(move |t| t.recent = recent).await;
}

/// Show or hide "Save clip".
pub async fn set_replay(handle: &TrayHandle, replay: bool) {
    handle.update(move |t| t.replay = replay).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    #[test]
    fn renders_color_and_monochrome_tray_pixmaps() {
        let color = render_tray_icons(false);
        assert_eq!(color.len(), 2);
        assert_eq!((color[0].width, color[0].height), (32, 32));
        assert_eq!((color[1].width, color[1].height), (64, 64));

        for icon in &color {
            let expected_len = usize::try_from(icon.width * icon.height * 4).unwrap();
            assert_eq!(icon.data.len(), expected_len);
            // Outer corners and inset padding edge are transparent outside the rounded tile.
            assert_eq!(icon.data[0], 0);
            let top_edge = (icon.width / 2) as usize * 4;
            assert_eq!(icon.data[top_edge], 0);
            // Center of the 1024x1024 SVG is the warm orange flame.
            let center = ((icon.height / 2) * icon.width + (icon.width / 2)) as usize * 4;
            let [a, r, g, b] = icon.data[center..center + 4] else {
                unreachable!();
            };
            assert_eq!(a, 255);
            assert!(r != g || g != b);
        }

        let monochrome = render_tray_icons(true);
        assert_eq!(monochrome.len(), 2);
        for icon in &monochrome {
            let expected_len = usize::try_from(icon.width * icon.height * 4).unwrap();
            assert_eq!(icon.data.len(), expected_len);
            assert_eq!(icon.data[0], 0);
            for &[_, r, g, b] in icon.data.as_chunks::<4>().0 {
                assert_eq!(r, g);
                assert_eq!(g, b);
            }
            let center = ((icon.height / 2) * icon.width + (icon.width / 2)) as usize * 4;
            assert_eq!(icon.data[center], 255);
        }

        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let mut with_pixmap = IroriTray {
            tx: tx.clone(),
            recent: Vec::new(),
            icon: build_tray_icon(false, false),
            icon_rev: 0,
            replay: false,
        };
        assert_eq!(with_pixmap.icon_name(), "");
        assert_eq!(with_pixmap.icon_pixmap().len(), 2);

        // Live updates switch cleanly and ignore stale out-of-order revisions.
        with_pixmap.apply_icon(build_tray_icon(false, true), 2);
        assert_eq!(with_pixmap.icon_name(), "applications-games");
        assert!(with_pixmap.icon_pixmap().is_empty());
        with_pixmap.apply_icon(build_tray_icon(false, false), 1);
        assert_eq!(with_pixmap.icon_name(), "applications-games");
        assert!(with_pixmap.icon_pixmap().is_empty());

        let generic_dark = IroriTray {
            tx: tx.clone(),
            recent: Vec::new(),
            icon: build_tray_icon(true, true),
            icon_rev: 0,
            replay: false,
        };
        assert_eq!(generic_dark.icon_name(), "applications-games-symbolic");
        assert!(generic_dark.icon_pixmap().is_empty());

        let fallback = IroriTray {
            tx,
            recent: Vec::new(),
            icon: TrayIcon {
                name: "irori".into(),
                pixmap: Vec::new(),
            },
            icon_rev: 0,
            replay: false,
        };
        assert_eq!(fallback.icon_name(), "irori");
        assert!(fallback.icon_pixmap().is_empty());
    }
}
