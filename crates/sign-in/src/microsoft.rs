//! The Microsoft account sign-in window: Microsoft's own sign-in page
//! (`InlineLogin.srf`) as Windows shows it inside CloudExperienceHost, the
//! shell that hosts TokenBroker sign-ins.
//!
//! The page talks to its host instead of redirecting, so the window plays
//! that host:
//!
//! - The first request carries the `cxh-*` headers CloudExperienceHost sends,
//!   and the window presents itself with its user agent.
//! - A script aliases `window.external.notify` (how the page calls its host)
//!   to the window's IPC channel, on Microsoft's sign-in hosts only.
//! - When the page asks for `CloudExperienceHost.getContext`, the window
//!   answers through the page's `CloudExperienceHost.Bridge.dispatchMessage`.
//! - Once `https://login.live.com/ppsecure/post.srf` has loaded, the window
//!   reads the page's `ServerData`, which holds the signed-in account's
//!   tokens (its `DAProperty`).
//!
//! When the page hands over a `DAProperty`, the window prints
//! `{"text": "<its JSON>"}` (only the `DAProperty` fields) and exits 0, as the
//! other sign-in mode does. Closing the window exits 2; bad arguments exit 1.
//!
//! The sign-in starts from a fresh session. With `--profile`, the folder keeps
//! the session's cookies so a second window (Microsoft's approval page, opened
//! after the first sign-in) continues it; the caller removes the folder. On
//! macOS every window starts fresh.
//!
//! Usage: irori-sign-in --microsoft-account --url <url> [--title <window title>]
//! [--profile <dir>]

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::http::{HeaderMap, HeaderName, HeaderValue};
use wry::{PageLoadEvent, WebViewBuilder};

use crate::CANCELLED;

/// The first argument that makes `irori-sign-in` (or Irori in its sign-in
/// mode) show the Microsoft account sign-in.
pub const MICROSOFT_ACCOUNT_FLAG: &str = "--microsoft-account";

/// The user agent of the page inside CloudExperienceHost.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; MSAppHost/3.0) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.26100";

/// What the host says it can do, in its headers and its context.
const CAPABILITIES: &str =
    r#"{"PrivatePropertyBag":1,"PasswordlessConnect":1,"PreferAssociate":1,"ChromelessUI":0}"#;

/// The only hosts whose pages get the host bridge.
const TRUSTED_HOSTS: &[&str] = &[
    "login.live.com",
    "account.live.com",
    "login.microsoftonline.com",
    "login.microsoft.com",
    "account.microsoft.com",
    "sisu.xboxlive.com",
];

/// The page that holds the signed-in account's tokens once it has loaded.
const POST_SRF: &str = "https://login.live.com/ppsecure/post.srf";

/// The largest message the page may send (`ServerData` is a few kilobytes).
const MAX_MESSAGE: usize = 256 * 1024;

/// The fields of a `DAProperty`, all strings; `sDAToken` must not be empty.
const DA_FIELDS: [&str; 7] = [
    "sDAToken",
    "sDASessionKey",
    "sDAStartTime",
    "sDAExpires",
    "sSTSInlineFlowToken",
    "sSigninName",
    "K",
];

/// Reads the page's `ServerData` as JSON (`"null"` when it has none).
const SERVER_DATA: &str = "(function () { try { \
return JSON.stringify(typeof ServerData === 'undefined' ? null : ServerData); \
} catch (e) { return 'null'; } })()";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    url: String,
    title: String,
    profile: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let (mut url, mut title, mut profile) = (None, None, None);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut dyn Iterator<Item = String>| {
            it.next().with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--url" => url = Some(value(&mut it)?),
            "--title" => title = Some(value(&mut it)?),
            "--profile" => profile = Some(PathBuf::from(value(&mut it)?)),
            other => bail!("unknown argument {other}"),
        }
    }
    let url: String = url.context("--url is required")?;
    if !is_trusted(&url) {
        bail!("--url must be a Microsoft sign-in page");
    }
    Ok(Args {
        url,
        title: title.unwrap_or_else(|| "Sign in".into()),
        profile,
    })
}

/// The headers CloudExperienceHost sends with the page's first request. The
/// correlation id is new for every window.
fn host_headers() -> HeaderMap {
    let correlation = uuid::Uuid::new_v4().to_string();
    let headers = [
        ("cxh-capabilities", CAPABILITIES),
        ("cxh-correlationid", correlation.as_str()),
        ("cxh-msabinaryversion", "55"),
        ("cxh-identityclientbinaryversion", "3"),
        (
            "cxh-osversioninfo",
            r#"{"platformId":2,"majorVersion":10,"minorVersion":0,"buildNumber":26100}"#,
        ),
        ("cxh-platform", "CloudExperienceHost.Platform.DESKTOP"),
        ("cxh-protocol", "TokenBroker"),
        ("cxh-source", "TokenBroker"),
        ("hostapp", "CloudExperienceHost"),
    ];
    let mut map = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        // Every value is plain ASCII, so neither conversion can fail.
        if let Ok(value) = HeaderValue::from_str(value) {
            map.insert(HeaderName::from_static(name), value);
        }
    }
    map
}

/// The host part of an `https` address without user or port, lower case;
/// `None` for anything else.
fn https_host(url: &str) -> Option<String> {
    let scheme = url.get(..8)?;
    if !scheme.eq_ignore_ascii_case("https://") {
        return None;
    }
    let rest = &url[8..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains(['@', ':', '\\']) {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

/// Whether `url` is a page of Microsoft's sign-in, the only pages the host
/// bridge serves.
fn is_trusted(url: &str) -> bool {
    https_host(url).is_some_and(|host| TRUSTED_HOSTS.contains(&host.as_str()))
}

/// Whether `url` is `post.srf`, whose `ServerData` holds the tokens.
fn is_post_srf(url: &str) -> bool {
    url.strip_prefix(POST_SRF)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['?', '#']))
}

/// Runs before every page in the main frame: on Microsoft's sign-in pages,
/// `window.external.notify` (how the page calls CloudExperienceHost) posts to
/// the window's IPC channel.
fn bridge_script() -> String {
    let hosts = Value::from(TRUSTED_HOSTS.to_vec());
    format!(
        r#"(function () {{
  if (window !== window.top) return;
  if (location.protocol !== 'https:' || location.port) return;
  if ({hosts}.indexOf(location.hostname) < 0) return;
  var notify = function (message) {{
    if (typeof message !== 'string') {{
      try {{ message = JSON.stringify(message); }} catch (e) {{ return; }}
    }}
    if (typeof message === 'string' && message.length <= {MAX_MESSAGE}) {{
      window.ipc.postMessage(message);
    }}
  }};
  var external = {{ notify: notify }};
  try {{ window.external = external; }} catch (e) {{}}
  if (window.external !== external) {{
    try {{
      Object.defineProperty(window, 'external', {{ value: external, configurable: true, writable: true }});
    }} catch (e) {{}}
  }}
}})();"#
    )
}

/// The answer to the page's `CloudExperienceHost.getContext`, passed back
/// through its bridge. `context` is the page's own correlation value and goes
/// in as data, never as code.
fn context_reply(context: &str) -> String {
    let message = json!({
        "type": "callback",
        "value": {
            "name": "CloudExperienceHost.getContext",
            "args": ["CloudExperienceHost", "TokenBroker", "TokenBroker", CAPABILITIES],
            "context": context,
        },
    })
    .to_string();
    // A JSON string is a JavaScript string literal.
    let literal = Value::String(message).to_string();
    format!(
        "(function () {{ var dispatch = window[\"CloudExperienceHost.Bridge.dispatchMessage\"]; \
if (typeof dispatch === 'function') dispatch({literal}); }})()"
    )
}

/// What a message from the page asks of the window.
#[derive(Debug, PartialEq, Eq)]
enum Message {
    /// The signed-in account's `DAProperty`, as the JSON the window prints.
    Token(String),
    /// `CloudExperienceHost.getContext`, with the page's context value.
    GetContext(String),
    /// Anything else.
    Other,
}

/// Read a message from the page (or its `ServerData`).
fn parse_message(body: &str) -> Message {
    if body.len() > MAX_MESSAGE {
        return Message::Other;
    }
    let Ok(Value::Object(value)) = serde_json::from_str::<Value>(body) else {
        return Message::Other;
    };
    if let Some(token) = da_property(&value) {
        return Message::Token(token);
    }
    let invoke = value.get("type").and_then(Value::as_str) == Some("invoke");
    let call = value.get("value");
    let name = call.and_then(|v| v.get("name")).and_then(Value::as_str);
    let context = call.and_then(|v| v.get("context")).and_then(Value::as_str);
    match (invoke, name, context) {
        (true, Some("CloudExperienceHost.getContext"), Some(context)) => {
            Message::GetContext(context.to_string())
        }
        _ => Message::Other,
    }
}

/// The `DAProperty` fields of `value` as a JSON object, when it has them all.
fn da_property(value: &Map<String, Value>) -> Option<String> {
    let mut out = Map::new();
    for field in DA_FIELDS {
        let text = value.get(field)?.as_str()?;
        out.insert(field.to_string(), Value::String(text.to_string()));
    }
    let token = out.get("sDAToken").and_then(Value::as_str);
    token
        .is_some_and(|t| !t.is_empty())
        .then(|| Value::Object(out).to_string())
}

/// What the page reported to the event loop.
#[derive(Debug)]
enum PageEvent {
    /// The page asked for its host's context.
    Context(String),
    /// A page finished loading at this address.
    Loaded(String),
    /// The signed-in account's `DAProperty`.
    Token(String),
}

/// Run the Microsoft account sign-in window with its arguments (after
/// [`MICROSOFT_ACCOUNT_FLAG`]) and return the exit status. Signing in or
/// closing the window exits the process directly.
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
        .with_inner_size(LogicalSize::new(520., 760.))
        .with_min_inner_size(LogicalSize::new(380., 480.))
        .build(&event_loop)
        .context("opening the sign-in window")?;

    // macOS keeps site data in named stores rather than folders: every
    // window there starts fresh.
    let profile = args.profile.filter(|_| !cfg!(target_os = "macos"));
    let mut context = profile.map(|dir| wry::WebContext::new(Some(dir)));
    let builder = match context.as_mut() {
        Some(context) => WebViewBuilder::new_with_web_context(context),
        // A fresh session each time: nothing from an earlier sign-in is kept.
        None => WebViewBuilder::new().with_incognito(true),
    };

    let messages = event_loop.create_proxy();
    let loaded = event_loop.create_proxy();
    let builder = builder
        .with_url_and_headers(&args.url, host_headers())
        .with_user_agent(USER_AGENT)
        .with_initialization_script_for_main_only(bridge_script(), true)
        .with_ipc_handler(move |request| {
            // Only Microsoft's sign-in pages speak to the host.
            if !is_trusted(&request.uri().to_string()) {
                return;
            }
            let event = match parse_message(request.body()) {
                Message::Token(token) => PageEvent::Token(token),
                Message::GetContext(context) => PageEvent::Context(context),
                Message::Other => return,
            };
            let _ = messages.send_event(event);
        })
        .with_on_page_load_handler(move |event, address| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = loaded.send_event(PageEvent::Loaded(address));
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

    let proxy = event_loop.create_proxy();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(PageEvent::Context(context)) => {
                if let Err(e) = webview.evaluate_script(&context_reply(&context)) {
                    eprintln!("irori-sign-in: answering the sign-in page failed: {e}");
                }
            }
            Event::UserEvent(PageEvent::Loaded(address)) if is_post_srf(&address) => {
                let proxy = proxy.clone();
                let result = webview.evaluate_script_with_callback(SERVER_DATA, move |json| {
                    let data = serde_json::from_str::<String>(&json).unwrap_or_default();
                    if let Message::Token(token) = parse_message(&data) {
                        let _ = proxy.send_event(PageEvent::Token(token));
                    }
                });
                if let Err(e) = result {
                    eprintln!("irori-sign-in: reading the sign-in page failed: {e}");
                }
            }
            Event::UserEvent(PageEvent::Token(token)) => {
                let line = json!({ "text": token }).to_string();
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
                std::process::exit(0);
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => std::process::exit(i32::from(CANCELLED)),
            _ => {}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIN: &str = "https://login.live.com/ppsecure/InlineLogin.srf?id=80604&hosted=1";

    fn args(list: &[&str]) -> Result<Args> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn arguments() {
        let parsed = args(&["--url", LOGIN, "--title", "T", "--profile", "/tmp/p"]).unwrap();
        assert_eq!(
            parsed,
            Args {
                url: LOGIN.into(),
                title: "T".into(),
                profile: Some(PathBuf::from("/tmp/p")),
            }
        );
        let plain = args(&["--url", LOGIN]).unwrap();
        assert_eq!(plain.title, "Sign in");
        assert_eq!(plain.profile, None);
        assert!(args(&[]).is_err());
        assert!(args(&["--url"]).is_err());
        assert!(args(&["--url", LOGIN, "--capture", "x"]).is_err());
        assert!(args(&["--url", "https://example.com/"]).is_err());
        assert!(args(&["--url", "http://login.live.com/"]).is_err());
    }

    #[test]
    fn the_mode_is_chosen_by_its_first_argument() {
        let run = |list: &[&str]| crate::main(list.iter().map(|s| s.to_string()));
        // Bad arguments exit 1 before any window opens.
        assert_eq!(run(&[MICROSOFT_ACCOUNT_FLAG]), 1);
        assert_eq!(run(&[MICROSOFT_ACCOUNT_FLAG, "--url", LOGIN, "--bogus"]), 1);
        assert_eq!(
            run(&[MICROSOFT_ACCOUNT_FLAG, "--url", "https://example.com/"]),
            1
        );
    }

    #[test]
    fn trusted_hosts() {
        assert!(is_trusted(LOGIN));
        assert!(is_trusted("https://LOGIN.live.com"));
        assert!(is_trusted("https://account.live.com/proofs?x=1"));
        assert!(is_trusted("https://login.live.com#frag"));
        assert!(!is_trusted("http://login.live.com/"));
        assert!(!is_trusted("https://login.live.com.example.com/"));
        assert!(!is_trusted("https://example.com/login.live.com"));
        assert!(!is_trusted("https://login.live.com:8443/"));
        assert!(!is_trusted("https://user@login.live.com/"));
        assert!(!is_trusted("https://"));
        assert!(!is_trusted("login.live.com"));
    }

    #[test]
    fn post_srf_is_recognised() {
        assert!(is_post_srf("https://login.live.com/ppsecure/post.srf"));
        assert!(is_post_srf(
            "https://login.live.com/ppsecure/post.srf?id=80604&contextid=1"
        ));
        assert!(!is_post_srf("https://login.live.com/ppsecure/post.srfx"));
        assert!(!is_post_srf(
            "https://login.live.com/ppsecure/InlineLogin.srf"
        ));
        assert!(!is_post_srf("https://example.com/ppsecure/post.srf"));
    }

    #[test]
    fn host_headers_identify_cloud_experience_host() {
        let first = host_headers();
        let second = host_headers();
        assert_eq!(first.len(), 9);
        assert_eq!(first["hostapp"], "CloudExperienceHost");
        assert_eq!(first["cxh-protocol"], "TokenBroker");
        assert_eq!(first["cxh-source"], "TokenBroker");
        assert_eq!(
            first["cxh-platform"],
            "CloudExperienceHost.Platform.DESKTOP"
        );
        assert_eq!(first["cxh-capabilities"], CAPABILITIES);
        for name in [
            "cxh-msabinaryversion",
            "cxh-identityclientbinaryversion",
            "cxh-osversioninfo",
        ] {
            assert_eq!(first[name], second[name], "{name}");
        }
        let id = |h: &HeaderMap| {
            let id = uuid::Uuid::parse_str(h["cxh-correlationid"].to_str().unwrap()).unwrap();
            assert_eq!(id.get_version_num(), 4);
            id
        };
        assert_ne!(id(&first), id(&second));
    }

    fn server_data(token: &str) -> Value {
        json!({
            "sDAToken": token,
            "sDASessionKey": "key",
            "sDAStartTime": "2026-11-09T10:00:00Z",
            "sDAExpires": "2026-11-10T10:00:00Z",
            "sSTSInlineFlowToken": "flow",
            "sSigninName": "user@example.com",
            "K": "0001",
            "urlPost": "https://login.live.com/ppsecure/post.srf",
            "arrProofData": [1, 2, 3],
        })
    }

    #[test]
    fn a_da_property_is_captured_with_only_its_fields() {
        let Message::Token(text) = parse_message(&server_data("t0ken").to_string()) else {
            panic!("a DAProperty");
        };
        let value: Map<String, Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(value.len(), DA_FIELDS.len());
        assert_eq!(value["sDAToken"], "t0ken");
        assert_eq!(value["sSigninName"], "user@example.com");
        assert_eq!(value["K"], "0001");
        assert!(!value.contains_key("urlPost"));
    }

    #[test]
    fn incomplete_da_properties_are_ignored() {
        assert_eq!(parse_message(&server_data("").to_string()), Message::Other);
        let mut missing = server_data("t");
        missing
            .as_object_mut()
            .unwrap()
            .remove("sSTSInlineFlowToken");
        assert_eq!(parse_message(&missing.to_string()), Message::Other);
        let mut number = server_data("t");
        number["K"] = json!(1);
        assert_eq!(parse_message(&number.to_string()), Message::Other);
        assert_eq!(parse_message("null"), Message::Other);
        assert_eq!(parse_message("not json"), Message::Other);
        assert_eq!(parse_message("[1]"), Message::Other);
        let huge = json!({ "pad": "x".repeat(MAX_MESSAGE) }).to_string();
        assert_eq!(parse_message(&huge), Message::Other);
    }

    #[test]
    fn get_context_requests_are_recognised() {
        let context = "quoted \"context\" \\ value";
        let invoke = json!({
            "type": "invoke",
            "value": { "name": "CloudExperienceHost.getContext", "context": context },
        });
        assert_eq!(
            parse_message(&invoke.to_string()),
            Message::GetContext(context.into())
        );
        let other = json!({
            "type": "invoke",
            "value": { "name": "CloudExperienceHost.telemetry", "context": "c" },
        });
        assert_eq!(parse_message(&other.to_string()), Message::Other);
        let callback = json!({
            "type": "callback",
            "value": { "name": "CloudExperienceHost.getContext", "context": "c" },
        });
        assert_eq!(parse_message(&callback.to_string()), Message::Other);
    }

    #[test]
    fn the_context_reply_passes_the_context_as_data() {
        let context = "\"); alert(1); (\"";
        let script = context_reply(context);
        assert!(script.contains(r#"window["CloudExperienceHost.Bridge.dispatchMessage"]"#));
        assert!(!script.contains(context));
        // The argument is a string literal holding the callback message.
        let start = script.find("dispatch(\"").unwrap() + "dispatch(".len();
        let end = script.rfind("); })()").unwrap();
        let message: String = serde_json::from_str(&script[start..end]).unwrap();
        let message: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(message["type"], "callback");
        assert_eq!(message["value"]["name"], "CloudExperienceHost.getContext");
        assert_eq!(message["value"]["context"], context);
        assert_eq!(message["value"]["args"][1], "TokenBroker");
        assert_eq!(message["value"]["args"][3], CAPABILITIES);
    }

    #[test]
    fn the_bridge_script_aliases_notify_to_ipc() {
        let script = bridge_script();
        assert!(script.contains("window.external = external"));
        assert!(script.contains("notify: notify"));
        assert!(script.contains("window.ipc.postMessage(message)"));
        assert!(script.contains("window !== window.top"));
        for host in TRUSTED_HOSTS {
            assert!(script.contains(&format!("\"{host}\"")), "{host}");
        }
        assert!(script.contains(&MAX_MESSAGE.to_string()));
    }

    #[test]
    fn server_data_script_reads_server_data() {
        assert!(SERVER_DATA.contains("JSON.stringify("));
        assert!(SERVER_DATA.contains("ServerData"));
    }
}
