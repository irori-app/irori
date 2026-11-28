//! Local loopback WebSocket JSON-RPC client for `aria2c`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Map, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Maximum JSON-RPC WebSocket message/frame size (16 MiB).
const MAX_WS_BYTES: usize = 16 * 1024 * 1024;
/// Timeout for a single loopback JSON-RPC call.
const RPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Push notifications sent by `aria2c` over WebSocket JSON-RPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Aria2Event {
    DownloadStart { gid: String },
    DownloadPause { gid: String },
    DownloadStop { gid: String },
    DownloadComplete { gid: String },
    DownloadError { gid: String },
    BtDownloadComplete { gid: String },
}

/// One file reported by `aria2.tellStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Aria2FileStatus {
    pub path: PathBuf,
    pub selected: bool,
    pub length: u64,
    pub completed_length: u64,
}

/// Snapshot returned by `aria2.tellStatus`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Aria2TaskStatus {
    pub status: String,
    pub total_length: u64,
    pub completed_length: u64,
    pub download_speed: u64,
    pub connections: u32,
    pub num_seeders: Option<u32>,
    pub error_message: Option<String>,
    pub dir: PathBuf,
    pub followed_by: Vec<String>,
    pub bittorrent_name: Option<String>,
    pub files: Vec<Aria2FileStatus>,
}

struct OutboundRpc {
    method: String,
    params: Vec<Value>,
    reply: oneshot::Sender<Result<Value>>,
}

/// Connected WebSocket JSON-RPC client for a running `aria2c` instance.
pub(crate) struct Aria2Client {
    tx: mpsc::Sender<OutboundRpc>,
    events: broadcast::Sender<Aria2Event>,
    alive: Arc<AtomicBool>,
    next_id: AtomicU64,
}

impl Aria2Client {
    /// Connect to an `aria2c` WebSocket JSON-RPC endpoint (`ws://127.0.0.1:<port>/jsonrpc`).
    pub(crate) async fn connect(ws_url: &str, secret: String) -> Result<Self> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WS_BYTES))
            .max_frame_size(Some(MAX_WS_BYTES));
        let (ws, _) = tokio_tungstenite::connect_async_with_config(ws_url, Some(config), false)
            .await
            .context("Couldn't connect to local aria2 WebSocket RPC")?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::channel::<OutboundRpc>(128);
        let (events, _) = broadcast::channel::<Aria2Event>(256);
        let events_tx = events.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let alive_pump = alive.clone();
        let token = format!("token:{secret}");

        tokio::spawn(async move {
            let mut pending: HashMap<String, oneshot::Sender<Result<Value>>> = HashMap::new();
            let mut seq: u64 = 0;

            loop {
                tokio::select! {
                    req = rx.recv() => {
                        let Some(req) = req else {
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        };
                        seq = seq.saturating_add(1);
                        let id = format!("irori-{seq}");
                        let mut full_params = Vec::with_capacity(req.params.len() + 1);
                        full_params.push(Value::String(token.clone()));
                        full_params.extend(req.params);
                        let payload = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "method": req.method,
                            "params": full_params,
                        });
                        pending.insert(id.clone(), req.reply);
                        if let Err(err) = sink.send(Message::text(payload.to_string())).await {
                            if let Some(reply) = pending.remove(&id) {
                                let _ = reply.send(Err(anyhow::anyhow!("aria2 RPC send failed: {err}")));
                            }
                            break;
                        }
                    }
                    msg = stream.next() => {
                        let Some(msg) = msg else { break };
                        match msg {
                            Ok(Message::Text(text)) => {
                                handle_incoming_json(text.as_str(), &mut pending, &events_tx);
                            }
                            Ok(Message::Ping(data)) => {
                                if sink.send(Message::Pong(data)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(err) => {
                                tracing::debug!("aria2 WebSocket stream ended: {err}");
                                break;
                            }
                        }
                    }
                }
            }

            alive_pump.store(false, Ordering::Release);
            for (_, reply) in pending.drain() {
                let _ = reply.send(Err(anyhow::anyhow!("aria2 RPC connection closed")));
            }
        });

        Ok(Self {
            tx,
            events,
            alive,
            next_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire) && !self.tx.is_closed()
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Aria2Event> {
        self.events.subscribe()
    }

    pub(crate) async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        let _seq = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(OutboundRpc {
                method: method.to_string(),
                params,
                reply: reply_tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("aria2 RPC channel is closed"))?;
        tokio::time::timeout(RPC_TIMEOUT, reply_rx)
            .await
            .map_err(|_| anyhow::anyhow!("aria2 RPC call '{method}' timed out"))?
            .map_err(|_| anyhow::anyhow!("aria2 RPC closed before answering '{method}'"))?
    }

    pub(crate) async fn get_version(&self) -> Result<String> {
        let val = self.call("aria2.getVersion", Vec::new()).await?;
        Ok(val
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string())
    }

    pub(crate) async fn add_uri(
        &self,
        uris: &[&str],
        options: Map<String, Value>,
    ) -> Result<String> {
        let uri_list = Value::Array(
            uris.iter()
                .map(|u| Value::String((*u).to_string()))
                .collect(),
        );
        let val = self
            .call("aria2.addUri", vec![uri_list, Value::Object(options)])
            .await?;
        val.as_str()
            .map(ToString::to_string)
            .context("aria2.addUri did not return a GID")
    }

    pub(crate) async fn add_torrent(
        &self,
        torrent_bytes: &[u8],
        options: Map<String, Value>,
    ) -> Result<String> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(torrent_bytes);
        let val = self
            .call(
                "aria2.addTorrent",
                vec![
                    Value::String(encoded),
                    Value::Array(Vec::new()),
                    Value::Object(options),
                ],
            )
            .await?;
        val.as_str()
            .map(ToString::to_string)
            .context("aria2.addTorrent did not return a GID")
    }

    pub(crate) async fn tell_status(&self, gid: &str) -> Result<Aria2TaskStatus> {
        let val = self
            .call("aria2.tellStatus", vec![Value::String(gid.to_string())])
            .await?;
        Ok(parse_task_status(&val))
    }

    pub(crate) async fn change_option(&self, gid: &str, options: Map<String, Value>) -> Result<()> {
        let _ = self
            .call(
                "aria2.changeOption",
                vec![Value::String(gid.to_string()), Value::Object(options)],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn change_global_download_limit(&self, bytes_per_sec: u64) -> Result<()> {
        let mut opts = Map::new();
        opts.insert(
            "max-overall-download-limit".into(),
            Value::String(bytes_per_sec.to_string()),
        );
        let _ = self
            .call("aria2.changeGlobalOption", vec![Value::Object(opts)])
            .await?;
        Ok(())
    }

    /// Immediately stop a download/seeding GID and purge its result from aria2.
    pub(crate) async fn stop_and_purge(&self, gid: &str) {
        if self
            .call("aria2.forceRemove", vec![Value::String(gid.to_string())])
            .await
            .is_err()
        {
            let _ = self
                .call("aria2.remove", vec![Value::String(gid.to_string())])
                .await;
        }
        let _ = self
            .call(
                "aria2.removeDownloadResult",
                vec![Value::String(gid.to_string())],
            )
            .await;
    }

    pub(crate) async fn remove_download_result(&self, gid: &str) {
        let _ = self
            .call(
                "aria2.removeDownloadResult",
                vec![Value::String(gid.to_string())],
            )
            .await;
    }

    pub(crate) async fn force_shutdown(&self) -> Result<()> {
        let _ = self.call("aria2.forceShutdown", Vec::new()).await?;
        Ok(())
    }
}

fn handle_incoming_json(
    text: &str,
    pending: &mut HashMap<String, oneshot::Sender<Result<Value>>>,
    events: &broadcast::Sender<Aria2Event>,
) {
    let Ok(val) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("ignoring unreadable JSON from aria2 WebSocket");
        return;
    };
    if let Some(id) = val.get("id").and_then(Value::as_str) {
        if let Some(reply) = pending.remove(id) {
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("aria2 RPC error");
                let _ = reply.send(Err(anyhow::anyhow!("{msg}")));
            } else {
                let _ = reply.send(Ok(val.get("result").cloned().unwrap_or(Value::Null)));
            }
        }
        return;
    }
    let Some(method) = val.get("method").and_then(Value::as_str) else {
        return;
    };
    let Some(gid) = val
        .get("params")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("gid"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return;
    };
    let event = match method {
        "aria2.onDownloadStart" => Aria2Event::DownloadStart { gid },
        "aria2.onDownloadPause" => Aria2Event::DownloadPause { gid },
        "aria2.onDownloadStop" => Aria2Event::DownloadStop { gid },
        "aria2.onDownloadComplete" => Aria2Event::DownloadComplete { gid },
        "aria2.onDownloadError" => Aria2Event::DownloadError { gid },
        "aria2.onBtDownloadComplete" => Aria2Event::BtDownloadComplete { gid },
        _ => return,
    };
    let _ = events.send(event);
}

fn parse_u64_field(val: &Value, key: &str) -> u64 {
    val.get(key)
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| v.as_u64())
        })
        .unwrap_or(0)
}

fn parse_task_status(val: &Value) -> Aria2TaskStatus {
    let status = val
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let followed_by = val
        .get("followedBy")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let bittorrent_name = val
        .get("bittorrent")
        .and_then(|bt| bt.get("info"))
        .and_then(|info| info.get("name"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let files = val
        .get("files")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|f| Aria2FileStatus {
                    path: PathBuf::from(f.get("path").and_then(Value::as_str).unwrap_or_default()),
                    selected: f
                        .get("selected")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s != "false"),
                    length: parse_u64_field(f, "length"),
                    completed_length: parse_u64_field(f, "completedLength"),
                })
                .collect()
        })
        .unwrap_or_default();
    let num_seeders = val.get("numSeeders").and_then(|v| {
        v.as_str()
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| v.as_u64().map(|n| n as u32))
    });
    let error_message = val
        .get("errorMessage")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            val.get("errorCode")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty() && *c != "0")
                .map(|c| format!("aria2 error code {c}"))
        });
    Aria2TaskStatus {
        status,
        total_length: parse_u64_field(val, "totalLength"),
        completed_length: parse_u64_field(val, "completedLength"),
        download_speed: parse_u64_field(val, "downloadSpeed"),
        connections: parse_u64_field(val, "connections") as u32,
        num_seeders,
        error_message,
        dir: PathBuf::from(val.get("dir").and_then(Value::as_str).unwrap_or_default()),
        followed_by,
        bittorrent_name,
        files,
    }
}
