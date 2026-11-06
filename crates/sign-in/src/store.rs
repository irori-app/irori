//! The store window: a store's website in a window of its own, with a toolbar
//! (back, forward, reload, the store's home, the address, open in the
//! browser), so a user can browse and buy without leaving Irori.
//!
//! Started with [`STORE_FLAG`] first. Unlike signing in, the store keeps its
//! cookies between windows, in `--profile` (on macOS 14 and later a data store
//! named after it; older macOS versions use the default one).
//!
//! It talks to Irori in JSON lines. On stdin, `{"open": "<url>"}` shows that
//! page and brings the window forward. On stdout, `{"external": "<url>"}` asks
//! Irori to open the page in the browser. Closing the window exits with 0.
//!
//! Usage: irori-sign-in --store --url <url> [--title <window title>]
//! [--profile <dir>] [--labels <json object>]

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::{Rect, WebView, WebViewBuilder};

/// The first argument that makes `irori-sign-in` (or Irori in its sign-in
/// mode) show a store instead of a sign-in page.
pub const STORE_FLAG: &str = "--store";

/// The toolbar's height, in logical pixels.
const TOOLBAR_HEIGHT: f64 = 44.;

const TOOLBAR_HTML: &str = include_str!("toolbar.html");

/// Lets the window follow addresses a page changes without loading (stores
/// are single-page apps) and gives it the navigation keys and mouse buttons
/// its engine leaves to the app. The page only asks the window to look:
/// the address shown is always read from the page itself.
fn page_script() -> String {
    // WebView2 handles Alt+arrows, F5 and the mouse's side buttons itself;
    // handling them again would go two pages back.
    let keys = cfg!(not(windows));
    let mouse = cfg!(target_os = "macos");
    format!(
        r#"(() => {{
  const post = (m) => {{ try {{ window.ipc.postMessage(m); }} catch (_) {{}} }};
  for (const name of ["pushState", "replaceState"]) {{
    const original = history[name];
    history[name] = function (...args) {{
      const result = original.apply(this, args);
      post("address");
      return result;
    }};
  }}
  addEventListener("popstate", () => post("address"));
  addEventListener("hashchange", () => post("address"));
  if ({mouse}) {{
    addEventListener("mouseup", (e) => {{
      if (e.button === 3) {{ e.preventDefault(); post("back"); }}
      if (e.button === 4) {{ e.preventDefault(); post("forward"); }}
    }});
  }}
  if ({keys}) {{
    const mac = navigator.platform.startsWith("Mac");
    addEventListener("keydown", (e) => {{
      const t = e.target;
      const editing = t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
      const primary = mac ? e.metaKey : e.ctrlKey;
      let command = null;
      if (primary && e.key === "[") command = "back";
      else if (primary && e.key === "]") command = "forward";
      else if (!mac && !editing && e.altKey && e.key === "ArrowLeft") command = "back";
      else if (!mac && !editing && e.altKey && e.key === "ArrowRight") command = "forward";
      else if (e.key === "F5" || (primary && e.key.toLowerCase() === "r")) command = "reload";
      if (command) {{ e.preventDefault(); post(command); }}
    }}, true);
  }}
}})();"#
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    url: String,
    title: String,
    profile: Option<PathBuf>,
    labels: Map<String, Value>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let (mut url, mut title, mut profile, mut labels) = (None, None, None, Map::new());
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut dyn Iterator<Item = String>| {
            it.next().with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--url" => url = Some(value(&mut it)?),
            "--title" => title = Some(value(&mut it)?),
            "--profile" => profile = Some(PathBuf::from(value(&mut it)?)),
            "--labels" => {
                labels = serde_json::from_str(&value(&mut it)?)
                    .context("--labels must be a JSON object")?;
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let url = url.context("--url is required")?;
    if !is_web(&url) {
        bail!("--url must be an http or https address");
    }
    Ok(Args {
        url,
        title: title.unwrap_or_else(|| "Store".into()),
        profile,
        labels,
    })
}

/// Only web pages are shown; anything else Irori sends is ignored.
fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// A command Irori sent on stdin.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Open(String),
}

fn parse_command(line: &str) -> Option<Command> {
    let value: Value = serde_json::from_str(line).ok()?;
    let url = value.get("open")?.as_str()?;
    is_web(url).then(|| Command::Open(url.to_string()))
}

/// What the toolbar, the page and Irori asked of the window.
#[derive(Debug)]
enum StoreEvent {
    Back,
    Forward,
    Reload,
    Home,
    External,
    /// The page's address may have changed.
    Address,
    Open(String),
}

/// Toolbar buttons and the page's own requests; `trusted` is the toolbar,
/// the only one that may open the browser or go home.
fn parse_request(message: &str, trusted: bool) -> Option<StoreEvent> {
    Some(match message {
        "back" => StoreEvent::Back,
        "forward" => StoreEvent::Forward,
        "reload" => StoreEvent::Reload,
        "address" => StoreEvent::Address,
        "home" if trusted => StoreEvent::Home,
        "external" if trusted => StoreEvent::External,
        _ => return None,
    })
}

/// The toolbar, with its labels set from `labels` (as data, never markup).
fn toolbar_html(labels: &Map<String, Value>) -> String {
    let json = Value::Object(labels.clone())
        .to_string()
        .replace("</", "<\\/");
    TOOLBAR_HTML.replace("/*LABELS*/", &json)
}

/// A data store identifier for `profile`, the same on every run.
// Only macOS names data stores, but the tests check it everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn store_identifier(profile: &std::path::Path) -> [u8; 16] {
    // FNV-1a, twice with different offsets: stable across builds, unlike
    // the standard library's hasher.
    let fnv = |offset: u64| {
        profile.to_string_lossy().bytes().fold(offset, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    };
    let mut id = [0; 16];
    id[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_le_bytes());
    id[8..].copy_from_slice(&fnv(0x6c62_272e_07bb_0142).to_le_bytes());
    id
}

/// Run the store window with its arguments (after [`STORE_FLAG`]) and return
/// the exit status. Closing the window exits the process directly.
pub fn main(args: impl IntoIterator<Item = String>) -> u8 {
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("irori-sign-in: {e:#}");
            return 1;
        }
    };
    match run(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("irori-sign-in: {e:#}");
            1
        }
    }
}

fn run(args: Args) -> Result<()> {
    let event_loop = EventLoopBuilder::<StoreEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(&args.title)
        .with_inner_size(LogicalSize::new(1280., 820.))
        .with_min_inner_size(LogicalSize::new(720., 480.))
        .build(&event_loop)
        .context("opening the store window")?;
    #[cfg(target_os = "macos")]
    let _menu = crate::menu::install(&args.labels);

    // Linux and Windows keep the profile in a folder (macOS names a data
    // store instead); both views share it, since WebView2 runs one browser
    // per folder.
    let folder = if cfg!(target_os = "macos") {
        None
    } else {
        args.profile.clone()
    };
    let mut context = wry::WebContext::new(folder);

    let (toolbar_bounds, page_bounds) = bounds(&window);
    let toolbar_events = event_loop.create_proxy();
    let toolbar = WebViewBuilder::new_with_web_context(&mut context)
        .with_html(toolbar_html(&args.labels))
        .with_ipc_handler(move |request| {
            if let Some(event) = parse_request(request.body(), true) {
                let _ = toolbar_events.send_event(event);
            }
        })
        // The toolbar is Irori's own page: links never open from it.
        .with_navigation_handler(|url| url.starts_with("about:") || url.starts_with("data:"))
        .with_bounds(toolbar_bounds);
    let toolbar = build(toolbar, &window)?;

    let page_events = event_loop.create_proxy();
    let loads = event_loop.create_proxy();
    let page = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&args.url)
        .with_initialization_script_for_main_only(page_script(), true)
        .with_ipc_handler(move |request| {
            if let Some(event) = parse_request(request.body(), false) {
                let _ = page_events.send_event(event);
            }
        })
        .with_on_page_load_handler(move |_, _| {
            let _ = loads.send_event(StoreEvent::Address);
        })
        .with_new_window_req_handler(crate::popups::handler())
        .with_bounds(page_bounds);
    #[cfg(target_os = "macos")]
    let page = match &args.profile {
        Some(profile) => {
            use wry::WebViewBuilderExtDarwin as _;
            page.with_data_store_identifier(store_identifier(profile))
        }
        None => page,
    };
    let page = build(page, &window)?;

    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewExtUnix as _;
        // The window's box packs both views to fill it; the toolbar keeps
        // its own height.
        if let Some(vbox) = window.default_vbox() {
            let bar = toolbar.webview();
            bar.set_size_request(-1, TOOLBAR_HEIGHT as i32);
            vbox.set_child_packing(&bar, false, false, 0, gtk::PackType::Start);
        }
    }

    listen_for_commands(event_loop.create_proxy());
    let home = args.url;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(event) => handle(event, &window, &toolbar, &page, &home),
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } => {
                let (toolbar_bounds, page_bounds) = bounds(&window);
                let _ = toolbar.set_bounds(toolbar_bounds);
                let _ = page.set_bounds(page_bounds);
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => std::process::exit(0),
            _ => {}
        }
    })
}

fn handle(event: StoreEvent, window: &Window, toolbar: &WebView, page: &WebView, home: &str) {
    let result = match event {
        StoreEvent::Back => page.evaluate_script("history.back()"),
        StoreEvent::Forward => page.evaluate_script("history.forward()"),
        StoreEvent::Reload => page.reload(),
        StoreEvent::Home => page.load_url(home),
        StoreEvent::Open(url) => {
            window.set_focus();
            page.load_url(&url)
        }
        StoreEvent::External => {
            if let Ok(url) = page.url() {
                let line = serde_json::json!({ "external": url }).to_string();
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
            Ok(())
        }
        StoreEvent::Address => page.url().and_then(|url| {
            let url = Value::String(url).to_string();
            toolbar.evaluate_script(&format!("irori.setAddress({url})"))
        }),
    };
    if let Err(e) = result {
        eprintln!("irori-sign-in: {e}");
    }
}

/// Add a view to the window: on Linux to the window's box, which lays the
/// views out top to bottom (WebKitGTK can't be a child view on Wayland).
fn build(builder: WebViewBuilder<'_>, window: &Window) -> Result<WebView> {
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewBuilderExtUnix as _;
        let vbox = window
            .default_vbox()
            .context("the store window has no container")?;
        Ok(builder.build_gtk(vbox)?)
    }
    #[cfg(not(target_os = "linux"))]
    Ok(builder.build_as_child(window)?)
}

/// Where the toolbar and the page sit in the window. On Linux the window's
/// box lays them out and these are ignored.
fn bounds(window: &Window) -> (Rect, Rect) {
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let toolbar = Rect {
        position: LogicalPosition::new(0., 0.).into(),
        size: LogicalSize::new(size.width, TOOLBAR_HEIGHT).into(),
    };
    let page = Rect {
        position: LogicalPosition::new(0., TOOLBAR_HEIGHT).into(),
        size: LogicalSize::new(size.width, (size.height - TOOLBAR_HEIGHT).max(0.)).into(),
    };
    (toolbar, page)
}

/// Read Irori's commands from stdin on a thread of their own. When Irori
/// goes away stdin closes and the window stays until the user closes it.
fn listen_for_commands(proxy: EventLoopProxy<StoreEvent>) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Some(Command::Open(url)) = parse_command(&line)
                && proxy.send_event(StoreEvent::Open(url)).is_err()
            {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn arguments() {
        let parsed = args(&[
            "--url",
            "https://www.gog.com/",
            "--title",
            "GOG",
            "--profile",
            "/tmp/gog",
            "--labels",
            r#"{"back":"Înapoi"}"#,
        ])
        .unwrap();
        assert_eq!(parsed.url, "https://www.gog.com/");
        assert_eq!(parsed.title, "GOG");
        assert_eq!(parsed.profile, Some(PathBuf::from("/tmp/gog")));
        assert_eq!(parsed.labels["back"], "Înapoi");
        assert!(args(&["--url", "file:///etc/passwd"]).is_err());
        assert!(args(&["--url", "https://x", "--labels", "[1]"]).is_err());
        assert!(args(&["--title", "T"]).is_err());
        assert!(args(&["--capture", "x"]).is_err());
    }

    #[test]
    fn commands_open_only_web_pages() {
        assert_eq!(
            parse_command(r#"{"open":"https://store.epicgames.com/p/hades"}"#),
            Some(Command::Open("https://store.epicgames.com/p/hades".into()))
        );
        assert_eq!(parse_command(r#"{"open":"javascript:alert(1)"}"#), None);
        assert_eq!(parse_command(r#"{"open":"file:///etc/passwd"}"#), None);
        assert_eq!(parse_command("not json"), None);
        assert_eq!(parse_command(r#"{"close":true}"#), None);
    }

    #[test]
    fn pages_cannot_open_the_browser_or_go_home() {
        assert!(matches!(
            parse_request("back", false),
            Some(StoreEvent::Back)
        ));
        assert!(matches!(
            parse_request("address", false),
            Some(StoreEvent::Address)
        ));
        assert!(parse_request("external", false).is_none());
        assert!(parse_request("home", false).is_none());
        assert!(matches!(
            parse_request("external", true),
            Some(StoreEvent::External)
        ));
        assert!(parse_request("anything", true).is_none());
    }

    #[test]
    fn labels_stay_data() {
        let mut labels = Map::new();
        labels.insert("back".into(), "</script><script>alert(1)".into());
        let html = toolbar_html(&labels);
        assert!(!html.contains("/*LABELS*/"));
        assert!(!html.contains("</script><script>alert(1)"));
        assert!(html.contains(r#"<\/script><script>alert(1)"#));
    }

    #[test]
    fn store_identifiers_are_stable_and_distinct() {
        let epic = store_identifier(std::path::Path::new("/data/store-window/legendary"));
        assert_eq!(
            epic,
            store_identifier(std::path::Path::new("/data/store-window/legendary"))
        );
        assert_ne!(
            epic,
            store_identifier(std::path::Path::new("/data/store-window/gog"))
        );
    }
}
