//! What a Windows installer run from Add game is doing: its output as it
//! writes it (the app's operation log, read every second while the panel
//! shows), and, on request, the programs it put in the prefix, found by
//! comparing the prefix's program folders with how they were before it
//! ran, so the user can pick the game's program instead of browsing for it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task,
    UniformListScrollHandle, Window, div, rems, uniform_list,
};
use irori_ui::game_log::{self, LineLevel, LogTail};
use irori_ui::i18n::t;

/// Lines of output kept.
const MAX_LINES: usize = 400;
/// How often the output is read again while the panel shows.
const REFRESH_EVERY: Duration = Duration::from_secs(1);
/// Folders below a program folder looked through for programs.
const SCAN_DEPTH: usize = 4;
/// Programs listed at most, so a huge prefix stays quick to show.
const MAX_FOUND: usize = 200;

/// The program folders of a prefix (`drive_c`, or `pfx/drive_c` for Proton).
fn program_roots(prefix: &Path) -> Vec<PathBuf> {
    ["drive_c", "pfx/drive_c"]
        .iter()
        .map(|c| prefix.join(c))
        .filter(|c| c.is_dir())
        .flat_map(|c| {
            ["Program Files", "Program Files (x86)", "GOG Games", "Games"].map(|p| c.join(p))
        })
        .filter(|p| p.is_dir())
        .collect()
}

/// Every `.exe` under the prefix's program folders. Blocking.
pub(crate) fn programs_in(prefix: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for root in program_roots(prefix) {
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() && depth < SCAN_DEPTH {
                    stack.push((path, depth + 1));
                } else if kind.is_file()
                    && path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                {
                    out.insert(path);
                }
            }
        }
    }
    out
}

/// The programs `after` has that `before` didn't, likeliest game first:
/// uninstallers, setups, crash reporters and redistributables left out,
/// shallower paths (the game's own folder) before deeper ones.
pub(crate) fn new_programs(before: &HashSet<PathBuf>, after: &HashSet<PathBuf>) -> Vec<PathBuf> {
    const NOT_GAMES: &[&str] = &[
        "unins",
        "uninst",
        "setup",
        "install",
        "crash",
        "report",
        "redist",
        "vcredist",
        "dxsetup",
        "dotnet",
        "update",
        "helper",
        "launcherpatcher",
    ];
    let mut found: Vec<PathBuf> = after
        .difference(before)
        .filter(|p| {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            !NOT_GAMES.iter().any(|w| name.contains(w))
        })
        .cloned()
        .collect();
    found.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    found.truncate(MAX_FOUND);
    found
}

/// The installer's output, followed while it shows.
pub(crate) struct InstallerLog {
    tail: Arc<LogTail>,
    scroll: UniformListScrollHandle,
    _refresh: Task<()>,
}

impl InstallerLog {
    pub(crate) fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let refresh = cx.spawn_in(window, async move |this, cx| {
            loop {
                let path = path.clone();
                // The file is read on the backend's runtime, never here.
                let Ok(read) = cx.update(|_, cx| {
                    irori_ui::run(cx, async move {
                        Ok(game_log::read_tail(&path, MAX_LINES).await?)
                    })
                }) else {
                    break;
                };
                let tail = read.await.ok().flatten();
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(tail) = tail else { return };
                        let changed = tail.lines.len() != this.tail.lines.len()
                            || tail.lines.last() != this.tail.lines.last();
                        if changed {
                            this.tail = Arc::new(tail);
                            let last = this.tail.lines.len().saturating_sub(1);
                            this.scroll
                                .scroll_to_item(last, gpui_kit::ScrollStrategy::Bottom);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        });
        Self {
            tail: Arc::new(LogTail::default()),
            scroll: UniformListScrollHandle::new(),
            _refresh: refresh,
        }
    }
}

impl Render for InstallerLog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, warning) = (theme.muted_foreground, theme.danger, theme.warning);
        let mono = theme.mono_font_family.clone();
        let tail = self.tail.clone();
        let count = tail.lines.len();
        let body = if count == 0 {
            div()
                .p_2()
                .text_xs()
                .text_color(muted)
                .child(t("library.sideload.installer_log.waiting"))
                .into_any_element()
        } else {
            uniform_list("installer-log", count, move |range, _, _| {
                range
                    .filter_map(|i| tail.lines.get(i).cloned())
                    .map(|(level, line): (LineLevel, SharedString)| {
                        let colour = match level {
                            LineLevel::Error => Some(danger),
                            LineLevel::Warning => Some(warning),
                            LineLevel::Info => None,
                        };
                        div()
                            .px_2()
                            .text_xs()
                            .whitespace_nowrap()
                            .truncate()
                            .when_some(colour, |d, c| d.text_color(c))
                            .child(line)
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .h(rems(10.))
            .into_any_element()
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .text_xs()
                    .text_color(muted)
                    .child(t("library.sideload.installer_log.title")),
            )
            .child(
                div()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .font_family(mono)
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_programs_put_the_game_first() {
        let set =
            |paths: &[&str]| -> HashSet<PathBuf> { paths.iter().map(PathBuf::from).collect() };
        let before = set(&["/p/drive_c/Program Files/Old/old.exe"]);
        let after = set(&[
            "/p/drive_c/Program Files/Old/old.exe",
            "/p/drive_c/Program Files/Game/bin/x64/Game-Win64-Shipping.exe",
            "/p/drive_c/Program Files/Game/Game.exe",
            "/p/drive_c/Program Files/Game/unins000.exe",
            "/p/drive_c/Program Files/Game/_CommonRedist/vcredist_x64.exe",
            "/p/drive_c/Program Files/Game/CrashReporter.exe",
        ]);
        let found = new_programs(&before, &after);
        assert_eq!(
            found,
            [
                PathBuf::from("/p/drive_c/Program Files/Game/Game.exe"),
                PathBuf::from("/p/drive_c/Program Files/Game/bin/x64/Game-Win64-Shipping.exe"),
            ]
        );
        assert!(new_programs(&after, &after).is_empty());
    }

    #[test]
    fn finds_programs_in_the_prefix_s_program_folders() {
        let root = std::env::temp_dir().join(format!("irori-installer-log-{}", std::process::id()));
        let prefix = root.as_path();
        let game = prefix.join("pfx/drive_c/Program Files (x86)/Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("Game.EXE"), b"").unwrap();
        std::fs::write(game.join("readme.txt"), b"").unwrap();
        std::fs::create_dir_all(prefix.join("pfx/drive_c/windows")).unwrap();
        std::fs::write(prefix.join("pfx/drive_c/windows/notepad.exe"), b"").unwrap();
        let found = programs_in(prefix);
        assert_eq!(found.len(), 1);
        assert!(found.contains(&game.join("Game.EXE")));
        assert!(programs_in(&prefix.join("missing")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
