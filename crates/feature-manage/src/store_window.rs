//! The store window: a store's website in a window of its own, shown by the
//! sign-in helper's store mode (`irori-sign-in --store`), a process of its
//! own, so Irori's window never embeds a web engine. Each store has one
//! window, which keeps the store's cookies between visits; opening another
//! page of that store shows it there. When the window closes the store's
//! library refreshes, so a game just bought appears. Without the helper the
//! page opens in the browser.

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Global, Window};
use irori_core::Runner;
use irori_core::paths::Paths;
use irori_ui::i18n::{t, t_args};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::accounts::{ACCOUNT_STORES, store_cards};
use crate::login::window::{Helper, helper};
use crate::stores::store_site;

/// The first argument of the helper's store mode (`irori_sign_in::store::STORE_FLAG`).
const STORE_FLAG: &str = "--store";

/// The open store windows, each with the way to send it a page.
#[derive(Default)]
struct StoreWindows(HashMap<Runner, mpsc::UnboundedSender<String>>);

impl Global for StoreWindows {}

/// The stores the Stores page offers a store window for, in its order.
pub fn store_window_stores(cx: &App) -> Vec<Runner> {
    store_cards(cx)
        .into_iter()
        .filter(|c| ACCOUNT_STORES.contains(&c.runner) && !c.hidden)
        .map(|c| c.runner)
        .filter(|&runner| store_site(runner).is_some())
        .collect()
}

/// Show `runner`'s store, at `url` or its home page, in the store window.
/// `url` must be a web address (see [`irori_ui::web_url`]). A store without
/// a store window shows `url` in the browser.
pub fn open_store_window(runner: Runner, url: Option<String>, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(site) = store_site(runner) else {
        // Steam, added games and the launchers have no store window.
        if let Some(url) = url.as_deref().and_then(irori_ui::web_url) {
            cx.open_url(&url);
        }
        return;
    };
    let url = url
        .as_deref()
        .and_then(irori_ui::web_url)
        .unwrap_or_else(|| site.url.to_string());
    let windows = cx.default_global::<StoreWindows>();
    if let Some(open) = windows.0.get(&runner) {
        if open.unbounded_send(url.clone()).is_ok() {
            return;
        }
        windows.0.remove(&runner);
    }
    let Some(helper) = helper() else {
        cx.open_url(&url);
        return;
    };

    let (pages, commands) = mpsc::unbounded();
    cx.global_mut::<StoreWindows>().0.insert(runner, pages);
    let (external_tx, mut external) = mpsc::unbounded();
    let args = window_args(runner, url);
    let b = irori_ui::backend(cx);
    let task = irori_ui::run(cx, async move {
        let shown = show(helper, args, commands, external_tx).await;
        // Whatever happened in the window, a purchase may have.
        if b.store(runner).is_some_and(|s| s.user().is_some()) {
            let _ = b.refresh_store(runner, false).await;
        }
        shown
    });
    window
        .spawn(cx, async move |cx| {
            while let Some(url) = external.next().await {
                let _ = cx.update(|_, cx| cx.open_url(&url));
            }
            let result = task.await;
            let _ = cx.update(|window, cx| {
                cx.global_mut::<StoreWindows>().0.remove(&runner);
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t_args(
                            "manage.store_window.failed",
                            &[("store", runner.store_name())],
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
}

/// The toolbar's and the menu's texts, by the helper's names for them.
fn labels() -> Map<String, Value> {
    [
        ("back", t("manage.store_window.back")),
        ("forward", t("manage.store_window.forward")),
        ("reload", t("manage.store_window.reload")),
        ("home", t("manage.store_window.home")),
        ("external", t("manage.store_window.open_in_browser")),
        ("insecure", t("manage.store_window.not_secure")),
        ("edit", t("manage.store_window.edit")),
        ("undo", t("manage.store_window.undo")),
        ("redo", t("manage.store_window.redo")),
        ("cut", t("manage.store_window.cut")),
        ("copy", t("manage.store_window.copy")),
        ("paste", t("manage.store_window.paste")),
        ("select_all", t("manage.store_window.select_all")),
        ("close_window", t("manage.store_window.close_window")),
        ("quit", t("manage.store_window.quit")),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_string(), Value::String(text.to_string())))
    .collect()
}

/// The helper's arguments after [`STORE_FLAG`].
fn window_args(runner: Runner, url: String) -> Vec<String> {
    let profile = Paths::get().store_window_dir(runner);
    vec![
        "--url".into(),
        url,
        "--title".into(),
        runner.store_name().to_string(),
        "--profile".into(),
        profile.to_string_lossy().into_owned(),
        "--labels".into(),
        Value::Object(labels()).to_string(),
    ]
}

/// Run the store window until it closes: send it the pages from `pages`,
/// and pass on the pages it asks to open in the browser.
async fn show(
    helper: Helper,
    args: Vec<String>,
    mut pages: mpsc::UnboundedReceiver<String>,
    external: mpsc::UnboundedSender<String>,
) -> Result<()> {
    let mut command = tokio::process::Command::new(&helper.program);
    // `irori-sign-in` is a console program; its output comes through pipes.
    irori_core::process::hide_console(command.as_std_mut());
    let mut child = command
        .args(helper.args)
        .arg(STORE_FLAG)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {}", helper.program.display()))?;
    let mut stdin = child.stdin.take().context("no input to the store window")?;
    let mut out = BufReader::new(
        child
            .stdout
            .take()
            .context("no output from the store window")?,
    )
    .lines();
    let err = child
        .stderr
        .take()
        .context("no output from the store window")?;
    // Only the last line says why the window stopped; the rest is the page's.
    let last_error = tokio::spawn(async move {
        let mut lines = BufReader::new(err).lines();
        let mut last = None;
        while let Ok(Some(line)) = lines.next_line().await {
            last = Some(line);
        }
        last
    });
    let mut sending = true;
    loop {
        tokio::select! {
            line = out.next_line() => match line {
                Ok(Some(line)) => {
                    if let Some(url) = parse_external(&line) {
                        let _ = external.unbounded_send(url);
                    }
                }
                _ => break,
            },
            page = pages.next(), if sending => match page {
                Some(url) => {
                    let line = format!("{}\n", serde_json::json!({ "open": url }));
                    let _ = stdin.write_all(line.as_bytes()).await;
                    let _ = stdin.flush().await;
                }
                None => sending = false,
            },
        }
    }
    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    let reason = last_error.await.ok().flatten();
    bail!(
        "the store window failed: {}",
        reason.as_deref().unwrap_or("it stopped unexpectedly")
    )
}

/// The page from the helper's `{"external": "..."}` line, when it is a web
/// address.
fn parse_external(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    irori_ui::web_url(value.get("external")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_pages_are_web_addresses() {
        assert_eq!(
            parse_external(r#"{"external":"https://www.gog.com/en/game/hades"}"#),
            Some("https://www.gog.com/en/game/hades".into())
        );
        assert_eq!(parse_external(r#"{"external":"file:///etc/passwd"}"#), None);
        assert_eq!(parse_external(r#"{"external":"smb://share/x"}"#), None);
        assert_eq!(parse_external("a page's log line"), None);
    }

    #[test]
    fn labels_name_what_the_window_reads() {
        crate::i18n::register();
        let labels = labels();
        for name in [
            "back",
            "forward",
            "reload",
            "home",
            "external",
            "insecure",
            "edit",
            "undo",
            "redo",
            "cut",
            "copy",
            "paste",
            "select_all",
            "close_window",
            "quit",
        ] {
            let text = labels[name].as_str().unwrap();
            assert!(!text.is_empty() && !text.starts_with("manage."), "{name}");
        }
    }

    /// A stand-in for the helper: `body` runs as a shell script.
    #[cfg(unix)]
    fn fake_helper(dir: &std::path::Path, body: &str) -> Helper {
        use std::os::unix::fs::PermissionsExt as _;
        let program = dir.join("irori-sign-in");
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Helper { program, args: &[] }
    }

    #[cfg(unix)]
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[cfg(unix)]
    #[test]
    fn pages_go_in_and_browser_requests_come_out() {
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("seen");
        let helper = fake_helper(
            dir.path(),
            &format!(
                r#"echo "$@" > "{seen}.args"
read line
echo "$line" > "{seen}"
echo 'a page said something'
echo '{{"external":"https://www.gog.com/en/game/hades"}}'
echo '{{"external":"file:///etc/passwd"}}'"#,
                seen = seen.display()
            ),
        );
        let (pages, commands) = mpsc::unbounded();
        let (external_tx, external) = mpsc::unbounded();
        pages
            .unbounded_send("https://www.gog.com/en/game/stardew_valley".into())
            .unwrap();
        let args = vec!["--url".into(), "https://www.gog.com/".into()];
        block_on(show(helper, args, commands, external_tx)).unwrap();
        drop(pages);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("seen.args"))
                .unwrap()
                .trim(),
            "--store --url https://www.gog.com/"
        );
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().trim(),
            r#"{"open":"https://www.gog.com/en/game/stardew_valley"}"#
        );
        let opened: Vec<String> = block_on(external.collect());
        assert_eq!(opened, ["https://www.gog.com/en/game/hades"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_window_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(
            dir.path(),
            "echo 'loading the page' >&2\necho 'no display' >&2\nexit 1",
        );
        let (_pages, commands) = mpsc::unbounded();
        let (external_tx, _external) = mpsc::unbounded();
        let error = block_on(show(helper, Vec::new(), commands, external_tx)).unwrap_err();
        assert_eq!(format!("{error:#}"), "the store window failed: no display");
    }
}
