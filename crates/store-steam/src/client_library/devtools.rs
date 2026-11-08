//! A small, read-only client of the Steam client's DevTools endpoint, the
//! Chrome DevTools protocol its Chromium (CEF) interface offers on this
//! computer while remote debugging is on.
//!
//! The port is the one Steam's web helper was started with
//! (`--remote-debugging-port=`), usually 8080; tools such as Millennium may
//! pick another. `http://127.0.0.1:<port>/json` lists the pages; the one
//! named `SharedJSContext` holds the client's stores. One expression is
//! evaluated there over the page's WebSocket and its value returned. Only
//! loopback addresses are ever contacted.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures::{SinkExt as _, StreamExt as _};
use irori_core::host;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::ClientLibraryError;

/// The port Steam uses unless told otherwise.
pub(crate) const DEFAULT_PORT: u16 = 8080;

/// How long one request to the endpoint may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the evaluation may take: a large library is serialized in one
/// go.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(20);

/// The largest page list read; a real one is a few kilobytes.
const MAX_LIST_BYTES: usize = 1 << 20;

/// The largest evaluated value accepted: tens of thousands of apps fit.
const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// Page titles of the shared context, newest first (older clients used the
/// others).
const SHARED_CONTEXT_TITLES: [&str; 4] = [
    "SharedJSContext",
    "Steam Shared Context presented by Valve™",
    "Steam",
    "SP",
];

/// What the process list says about the Steam client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Probe {
    /// The client (or its web helper) runs.
    pub running: bool,
    /// The debugging port its web helper was started with.
    pub port: Option<u16>,
}

/// The `--remote-debugging-port=` value among `args`.
pub(crate) fn port_from_args<'a>(args: impl IntoIterator<Item = &'a str>) -> Option<u16> {
    args.into_iter().find_map(|a| {
        let value = a.strip_prefix("--remote-debugging-port=")?;
        // Some builds list several ports; the first is the browser's.
        value
            .split(',')
            .next()?
            .trim()
            .parse()
            .ok()
            .filter(|p| *p > 0)
    })
}

/// Whether a program (path or name) belongs to the Steam client: the client
/// itself or its web helper.
fn is_steam_process(program: &str) -> bool {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "steam" | "steam.exe" | "steam_osx" | "steamwebhelper" | "steamwebhelper.exe"
    ) || name.starts_with("steam helper")
}

/// [`Probe`] over processes given as their argument lists.
pub(crate) fn probe_from<I, A>(processes: I) -> Probe
where
    I: IntoIterator<Item = A>,
    A: AsRef<[String]>,
{
    let mut probe = Probe::default();
    for args in processes {
        let args = args.as_ref();
        let Some(program) = args.first() else {
            continue;
        };
        if !is_steam_process(program) {
            continue;
        }
        probe.running = true;
        if probe.port.is_none() {
            probe.port = port_from_args(args.iter().map(String::as_str));
        }
    }
    probe
}

/// Look at the running processes (the host's, inside a Flatpak).
pub(crate) async fn probe() -> Result<Probe> {
    if cfg!(target_os = "linux") && host::is_flatpak() {
        let processes = host::processes(None)
            .await
            .context("Couldn't tell whether Steam is running")?;
        return Ok(probe_from(processes.iter().map(|p| p.args())));
    }
    tokio::task::spawn_blocking(|| {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
        probe_from(sys.processes().values().map(|p| {
            let mut args: Vec<String> = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            // The program's name stands first, so processes whose command
            // line can't be read still count as running.
            let name = p
                .exe()
                .and_then(|e| e.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.name().to_string_lossy().into_owned());
            args.insert(0, name);
            args
        }))
    })
    .await
    .context("Couldn't tell whether Steam is running")
}

/// One page of the `/json` list.
#[derive(Debug, Deserialize)]
struct Target {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "webSocketDebuggerUrl")]
    websocket: String,
}

/// The WebSocket address of the shared context in the page list `json` of
/// the endpoint on `port`.
pub(crate) fn shared_context(json: &str, port: u16) -> Result<String, ClientLibraryError> {
    let targets: Vec<Target> =
        serde_json::from_str(json).map_err(|_| ClientLibraryError::NotSteam { port })?;
    let steam = |t: &&Target| t.url.starts_with("https://steamloopback.host/");
    if !targets.iter().any(|t| steam(&t)) {
        return Err(ClientLibraryError::NotSteam { port });
    }
    let found = SHARED_CONTEXT_TITLES.iter().find_map(|title| {
        targets
            .iter()
            .filter(steam)
            // Older clients' names are also those of ordinary windows; the
            // shared page is the one showing Steam's index page.
            .find(|t| {
                t.title == *title
                    && !t.websocket.is_empty()
                    && (t.title == SHARED_CONTEXT_TITLES[0] || t.url.contains("/index.html"))
            })
    });
    let Some(target) = found else {
        return Err(ClientLibraryError::Starting);
    };
    if !is_loopback_ws(&target.websocket, port) {
        return Err(ClientLibraryError::NotSteam { port });
    }
    Ok(target.websocket.clone())
}

/// Whether `address` is a plain WebSocket on this computer at `port`.
fn is_loopback_ws(address: &str, port: u16) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    url.scheme() == "ws" && loopback && url.port() == Some(port)
}

/// The page list of the endpoint on `port`. `None` when nothing listens.
pub(crate) async fn list(port: u16) -> Result<Option<String>> {
    match tokio::time::timeout(REQUEST_TIMEOUT, get(port, "/json")).await {
        Ok(Ok(body)) => Ok(Some(body)),
        Ok(Err(e)) => {
            if e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
                )
            }) {
                Ok(None)
            } else {
                Err(e)
            }
        }
        Err(_) => bail!("Steam's debugging port didn't answer in time"),
    }
}

/// A plain HTTP GET of `path` on `127.0.0.1:port`, written by hand so no
/// proxy setting can send it anywhere else.
async fn get(port: u16, path: &str) -> Result<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    (&mut stream)
        .take(MAX_LIST_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .await?;
    anyhow::ensure!(
        raw.len() <= MAX_LIST_BYTES,
        "Steam's page list is too large"
    );
    parse_response(&raw)
}

/// The body of an HTTP response with status 200.
fn parse_response(raw: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .context("Steam's debugging port gave an unreadable answer")?;
    let status = head.lines().next().unwrap_or_default();
    anyhow::ensure!(
        status.split_whitespace().nth(1) == Some("200"),
        "Steam's debugging port answered {status}"
    );
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase()
            .replace(' ', "")
            .starts_with("transfer-encoding:chunked")
    });
    anyhow::ensure!(!chunked, "Steam's debugging port gave an unreadable answer");
    Ok(body.to_string())
}

/// Evaluate `expression` in the page at `websocket` and return its value.
/// The expression must only read: nothing here changes the client.
pub(crate) async fn evaluate(websocket: &str, expression: &str) -> Result<serde_json::Value> {
    tokio::time::timeout(EVALUATE_TIMEOUT, evaluate_inner(websocket, expression))
        .await
        .map_err(|_| anyhow::anyhow!("Steam took too long to answer"))?
}

async fn evaluate_inner(websocket: &str, expression: &str) -> Result<serde_json::Value> {
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (mut ws, _) = tokio_tungstenite::connect_async_with_config(websocket, Some(config), false)
        .await
        .context("Couldn't connect to Steam's debugging port")?;
    let request = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": false,
            "silent": true,
        }
    });
    ws.send(Message::text(request.to_string())).await?;
    let mut answer = None;
    while let Some(message) = ws.next().await {
        let text = match message? {
            Message::Text(text) => text,
            Message::Close(_) => break,
            _ => continue,
        };
        let value: serde_json::Value =
            serde_json::from_str(text.as_str()).context("Steam gave an unreadable answer")?;
        // Events the page sends meanwhile carry no id.
        if value.get("id").and_then(serde_json::Value::as_u64) == Some(1) {
            answer = Some(value);
            break;
        }
    }
    let _ = ws.close(None).await;
    let answer = answer.context("Steam closed the connection without answering")?;
    evaluation_value(answer)
}

/// The value of a `Runtime.evaluate` answer.
pub(crate) fn evaluation_value(answer: serde_json::Value) -> Result<serde_json::Value> {
    if answer.get("error").is_some() {
        return Err(ClientLibraryError::Unsupported.into());
    }
    let result = answer
        .get("result")
        .context("Steam gave an unreadable answer")?;
    if result.get("exceptionDetails").is_some() {
        return Err(ClientLibraryError::Unsupported.into());
    }
    Ok(result
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_the_port_from_the_web_helper() {
        assert_eq!(
            port_from_args(["steamwebhelper", "-lang=en", "--remote-debugging-port=8080"]),
            Some(8080)
        );
        assert_eq!(
            port_from_args(["x", "--remote-debugging-port=41234,8081"]),
            Some(41234)
        );
        assert_eq!(port_from_args(["x", "--remote-debugging-port=0"]), None);
        assert_eq!(port_from_args(["x", "--remote-debugging-port=nope"]), None);
        assert_eq!(port_from_args(["x"]), None);
    }

    #[test]
    fn tells_steam_apart_from_other_programs() {
        let none = probe_from([args(&["firefox", "--remote-debugging-port=9222"])]);
        assert_eq!(
            none,
            Probe::default(),
            "another browser's port is not Steam's"
        );
        let off = probe_from([
            args(&["/home/u/.local/share/Steam/ubuntu12_32/steam"]),
            args(&[
                "/home/u/.local/share/Steam/ubuntu12_64/steamwebhelper",
                "-lang=en",
            ]),
        ]);
        assert_eq!(
            off,
            Probe {
                running: true,
                port: None
            }
        );
        let on = probe_from([
            args(&[r"C:\Program Files (x86)\Steam\steam.exe"]),
            args(&[
                r"C:\Program Files (x86)\Steam\bin\cef\cef.win64\steamwebhelper.exe",
                "--remote-debugging-port=8080",
            ]),
        ]);
        assert_eq!(on.port, Some(8080));
        let mac = probe_from([args(&["Steam Helper", "--remote-debugging-port=8080"])]);
        assert_eq!(mac.port, Some(8080));
    }

    const LIST: &str = r#"[
        {"description": "", "id": "A1", "title": "Steam", "type": "page",
         "url": "https://steamloopback.host/routes/library/home",
         "webSocketDebuggerUrl": "ws://127.0.0.1:8080/devtools/page/A1"},
        {"description": "", "id": "B2", "title": "SharedJSContext", "type": "page",
         "url": "https://steamloopback.host/index.html",
         "webSocketDebuggerUrl": "ws://127.0.0.1:8080/devtools/page/B2"},
        {"id": "C3", "title": "Store", "type": "page",
         "url": "https://store.steampowered.com/",
         "webSocketDebuggerUrl": "ws://127.0.0.1:8080/devtools/page/C3"}
    ]"#;

    #[test]
    fn finds_the_shared_context() {
        assert_eq!(
            shared_context(LIST, 8080).unwrap(),
            "ws://127.0.0.1:8080/devtools/page/B2"
        );
        // Older clients name it differently; any of the names on Steam's
        // own pages does.
        let old = r#"[{"title": "Steam Shared Context presented by Valve™",
            "url": "https://steamloopback.host/index.html",
            "webSocketDebuggerUrl": "ws://localhost:8080/devtools/page/X"}]"#;
        assert_eq!(
            shared_context(old, 8080).unwrap(),
            "ws://localhost:8080/devtools/page/X"
        );
    }

    #[test]
    fn refuses_what_isnt_steam_on_this_computer() {
        let other = r#"[{"title": "SharedJSContext", "url": "https://example.com/",
            "webSocketDebuggerUrl": "ws://127.0.0.1:8080/devtools/page/X"}]"#;
        assert!(matches!(
            shared_context(other, 8080),
            Err(ClientLibraryError::NotSteam { port: 8080 })
        ));
        assert!(matches!(
            shared_context("<html>", 8080),
            Err(ClientLibraryError::NotSteam { .. })
        ));
        let elsewhere = r#"[{"title": "SharedJSContext", "url": "https://steamloopback.host/index.html",
            "webSocketDebuggerUrl": "ws://192.168.1.5:8080/devtools/page/X"}]"#;
        assert!(matches!(
            shared_context(elsewhere, 8080),
            Err(ClientLibraryError::NotSteam { .. })
        ));
        let other_port = r#"[{"title": "SharedJSContext", "url": "https://steamloopback.host/index.html",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9999/devtools/page/X"}]"#;
        assert!(shared_context(other_port, 8080).is_err());
        let starting = r#"[{"title": "Steam", "url": "https://steamloopback.host/routes/",
            "webSocketDebuggerUrl": "ws://127.0.0.1:8080/devtools/page/A"}]"#;
        assert!(matches!(
            shared_context(starting, 8080),
            Err(ClientLibraryError::Starting)
        ));
    }

    #[test]
    fn reads_plain_http_answers() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[]";
        assert_eq!(parse_response(ok).unwrap(), "[]");
        assert!(parse_response(b"HTTP/1.1 404 Not Found\r\n\r\n").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n[]")
                .is_err()
        );
        assert!(parse_response(b"garbage").is_err());
    }

    #[test]
    fn reads_evaluation_answers() {
        let ok = serde_json::json!({"id": 1, "result": {"result": {"type": "object", "value": {"v": 1}}}});
        assert_eq!(evaluation_value(ok).unwrap()["v"], 1);
        let thrown = serde_json::json!({"id": 1, "result": {"result": {}, "exceptionDetails": {"text": "x"}}});
        assert!(
            evaluation_value(thrown)
                .unwrap_err()
                .downcast_ref::<ClientLibraryError>()
                .is_some()
        );
        let refused = serde_json::json!({"id": 1, "error": {"code": -32601}});
        assert!(evaluation_value(refused).is_err());
    }
}
