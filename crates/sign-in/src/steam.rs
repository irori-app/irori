//! The Steam sign-in window: Valve's own sign-in page on
//! `store.steampowered.com` in a native browser engine (WebKitGTK, WKWebView,
//! or WebView2), with its real user agent and JavaScript.
//!
//! Irori never sees the account password or calls `IAuthenticationService`
//! directly. Once Steam completes the browser login (QR code or Steam Guard),
//! this window reads the resulting web session cookies (`steamLoginSecure` and
//! `steamRefresh_steam`) through `wry`'s native cookie manager, prints
//! `{"text": "<payload json>"}` to stdout, and exits 0. Closing the window
//! without signing in exits 2; bad arguments exit 1.
//!
//! Usage: irori-sign-in --steam-account [--url <url>] [--title <window title>]

use std::io::Write as _;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use serde_json::json;
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebViewBuilder};

use crate::{CANCELLED, RECHECK};

/// The first argument that makes `irori-sign-in` (or Irori in its sign-in
/// mode) show the official Steam web sign-in page.
pub const STEAM_ACCOUNT_FLAG: &str = "--steam-account";

/// Default official Steam login page.
const DEFAULT_URL: &str = "https://store.steampowered.com/login/";

/// Trusted Steam web hosts allowed for `--url`.
const TRUSTED_HOSTS: &[&str] = &[
    "store.steampowered.com",
    "steamcommunity.com",
    "login.steampowered.com",
];

const STORE_COOKIE_URL: &str = "https://store.steampowered.com/";
const LOGIN_COOKIE_URL: &str = "https://login.steampowered.com/";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    url: String,
    title: String,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let (mut url, mut title) = (None, None);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut dyn Iterator<Item = String>| {
            it.next().with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--url" => url = Some(value(&mut it)?),
            "--title" => title = Some(value(&mut it)?),
            other => bail!("unknown argument {other}"),
        }
    }
    let url = url.unwrap_or_else(|| DEFAULT_URL.into());
    if !is_trusted(&url) {
        bail!("--url must be an https address on Steam");
    }
    Ok(Args {
        url,
        title: title.unwrap_or_else(|| "Sign in to Steam".into()),
    })
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains(['@', ':', '\\']) {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

fn is_trusted(url: &str) -> bool {
    host_of(url).is_some_and(|host| TRUSTED_HOSTS.contains(&host.as_str()))
}

/// Split a Steam `steamLoginSecure` or `steamRefresh_steam` cookie value
/// (`<steamid64>%7C%7C<jwt>` or `<steamid64>||<jwt>`) into `(steamid, jwt)`.
fn parse_steam_cookie(raw: &str) -> Option<(u64, String)> {
    let trimmed = raw.trim();
    let (id_str, token) = trimmed
        .split_once("%7C%7C")
        .or_else(|| trimmed.split_once("%7c%7c"))
        .or_else(|| trimmed.split_once("||"))?;
    let steamid: u64 = id_str.parse().ok().filter(|&id| id > 0)?;
    if !looks_like_jwt(token) {
        return None;
    }
    Some((steamid, token.to_string()))
}

fn looks_like_jwt(token: &str) -> bool {
    let mut parts = token.split('.');
    let valid_part = |p: Option<&str>| {
        p.is_some_and(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='))
        })
    };
    valid_part(parts.next())
        && valid_part(parts.next())
        && valid_part(parts.next())
        && parts.next().is_none()
}

fn build_payload(steamid: u64, access_token: &str, refresh_cookie: Option<&str>) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("steamid".into(), json!(steamid.to_string()));
    obj.insert("accessToken".into(), json!(access_token));
    if let Some(cookie) = refresh_cookie {
        obj.insert("refreshCookie".into(), json!(cookie));
    }
    serde_json::Value::Object(obj).to_string()
}

fn read_steam_payload(webview: &wry::WebView) -> Option<String> {
    let store_cookies = webview.cookies_for_url(STORE_COOKIE_URL).ok()?;
    let login_secure = store_cookies
        .iter()
        .find(|c| c.name() == "steamLoginSecure")?
        .value();
    let (steamid, access_token) = parse_steam_cookie(login_secure)?;

    let refresh_cookie = webview
        .cookies_for_url(LOGIN_COOKIE_URL)
        .ok()
        .and_then(|cookies| {
            cookies
                .into_iter()
                .find(|c| c.name() == "steamRefresh_steam")
                .map(|c| c.value().to_string())
        })
        .filter(|raw| parse_steam_cookie(raw).is_some_and(|(id, _)| id == steamid));

    Some(build_payload(
        steamid,
        &access_token,
        refresh_cookie.as_deref(),
    ))
}

#[derive(Debug)]
enum PageEvent {
    Loaded,
}

/// Run the official Steam web sign-in window with its arguments (after
/// [`STEAM_ACCOUNT_FLAG`]) and return the exit status. Signing in or closing
/// the window exits the process directly.
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
    let event_loop = EventLoopBuilder::<PageEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(&args.title)
        .with_inner_size(LogicalSize::new(560., 760.))
        .with_min_inner_size(LogicalSize::new(400., 520.))
        .build(&event_loop)
        .context("opening the Steam sign-in window")?;

    let loaded = event_loop.create_proxy();
    // Use the system webview's genuine user agent and fresh incognito storage.
    let builder = WebViewBuilder::new()
        .with_url(&args.url)
        .with_incognito(true)
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = loaded.send_event(PageEvent::Loaded);
            }
        })
        .with_new_window_req_handler(crate::popups::handler());

    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewBuilderExtUnix as _;
        let vbox = window
            .default_vbox()
            .context("the sign-in window has no container")?;
        builder.build_gtk(vbox)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + RECHECK);
        let check = matches!(
            event,
            Event::NewEvents(StartCause::ResumeTimeReached { .. })
                | Event::UserEvent(PageEvent::Loaded)
        );
        if check && let Some(payload) = read_steam_payload(&webview) {
            let line = json!({ "text": payload }).to_string();
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
            std::process::exit(0);
        }
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            std::process::exit(i32::from(CANCELLED));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_arguments_and_rejects_untrusted_urls() {
        let default = args(&[]).unwrap();
        assert_eq!(default.url, DEFAULT_URL);
        assert_eq!(default.title, "Sign in to Steam");

        let custom = args(&[
            "--url",
            "https://steamcommunity.com/login/home/",
            "--title",
            "Steam",
        ])
        .unwrap();
        assert_eq!(custom.url, "https://steamcommunity.com/login/home/");
        assert_eq!(custom.title, "Steam");

        assert!(args(&["--url", "http://store.steampowered.com/login/"]).is_err());
        assert!(args(&["--url", "https://example.com/login/"]).is_err());
        assert!(args(&["--url", "https://store.steampowered.com.evil.test/"]).is_err());
        assert!(args(&["--bogus"]).is_err());
    }

    #[test]
    fn parses_encoded_and_literal_steam_cookies() {
        let jwt = "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiI3NjU2MTE5Nzk2MDI2NTczMCJ9.sig_123";
        assert_eq!(
            parse_steam_cookie(&format!("76561197960265730%7C%7C{jwt}")),
            Some((76561197960265730, jwt.to_string()))
        );
        assert_eq!(
            parse_steam_cookie(&format!("76561197960265730%7c%7c{jwt}")),
            Some((76561197960265730, jwt.to_string()))
        );
        assert_eq!(
            parse_steam_cookie(&format!("76561197960265730||{jwt}")),
            Some((76561197960265730, jwt.to_string()))
        );
        assert_eq!(parse_steam_cookie("0||a.b.c"), None);
        assert_eq!(parse_steam_cookie("notanumber||a.b.c"), None);
        assert_eq!(parse_steam_cookie("76561197960265730||not-a-jwt"), None);
        assert_eq!(parse_steam_cookie("76561197960265730||a.b"), None);
    }

    #[test]
    fn builds_payload_with_and_without_refresh_cookie() {
        let with_refresh: serde_json::Value = serde_json::from_str(&build_payload(
            76561197960265730,
            "a.b.c",
            Some("76561197960265730%7C%7Cx.y.z"),
        ))
        .unwrap();
        assert_eq!(with_refresh["steamid"], "76561197960265730");
        assert_eq!(with_refresh["accessToken"], "a.b.c");
        assert_eq!(
            with_refresh["refreshCookie"],
            "76561197960265730%7C%7Cx.y.z"
        );

        let without_refresh: serde_json::Value =
            serde_json::from_str(&build_payload(76561197960265730, "a.b.c", None)).unwrap();
        assert!(without_refresh.get("refreshCookie").is_none());
    }
}
