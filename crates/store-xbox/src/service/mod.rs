//! The account service a game's `xgameruntime.dll` talks to, ported from the
//! Nexus backend's `xodus-service`: it answers the runtime's requests for
//! Microsoft account tickets (XUser sign-in), Store licence tokens and
//! package update checks with the signed-in account.
//!
//! One service runs inside Irori for each game session. It listens on a
//! private Unix socket (`xodus.sock` in a folder only the user can open) and
//! on a loopback TCP port whose clients must first send the session's random
//! 64-character key (compared in constant time) before their bytes reach the
//! socket, as Nexus Launcher's bridge does. The game learns both through its
//! environment ([`Session::env`]). Requests are framed as a little-endian
//! `u32` magic, a `u16` type and a `u16` length; each has 92 seconds.

mod account_metadata;
mod context;
mod router;
mod store_license;
mod store_updates;
mod xml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_msstore_api::models::secrets::Token;
use irori_msstore_api::tokens::TokenManager;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UnixListener};
use tokio_util::sync::CancellationToken;

pub(crate) const XML_MAGIC: u32 = 0x5844_5358;
pub(crate) const PROTO_MAGIC: u32 = 0x5844_5350;
/// Bytes of the bridge key (hex of 32 random bytes).
const KEY_LENGTH: usize = 64;
/// How long a bridge client has to send the key.
const KEY_TIMEOUT: Duration = Duration::from_secs(10);

/// Requests the runtime sends (Xodus' protocol buffer enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum XodusMessageType {
    #[default]
    Unknown = 0,
    Ping = 1,
    Pong = 2,
    MsaTokenRequest = 3,
    MsaTokenResponse = 4,
    StoreLicenseTokenRequest = 5,
    StoreLicenseTokenResponse = 6,
    StorePackageUpdatesRequest = 9,
    StorePackageUpdatesResponse = 10,
}

impl TryFrom<i32> for XodusMessageType {
    type Error = ();
    fn try_from(value: i32) -> std::result::Result<Self, ()> {
        Ok(match value {
            0 => Self::Unknown,
            1 => Self::Ping,
            2 => Self::Pong,
            3 => Self::MsaTokenRequest,
            4 => Self::MsaTokenResponse,
            5 => Self::StoreLicenseTokenRequest,
            6 => Self::StoreLicenseTokenResponse,
            9 => Self::StorePackageUpdatesRequest,
            10 => Self::StorePackageUpdatesResponse,
            _ => return Err(()),
        })
    }
}

pub(crate) fn encode_message(magic: u32, msg_type: u16, message: Vec<u8>) -> Vec<u8> {
    // Frames carry a u16 length; longer answers can't be sent and go empty.
    let message = if message.len() > u16::MAX as usize {
        tracing::warn!("account service answer too long to send");
        Vec::new()
    } else {
        message
    };
    let mut buffer = Vec::with_capacity(8 + message.len());
    buffer.extend(magic.to_le_bytes());
    buffer.extend(msg_type.to_le_bytes());
    buffer.extend((message.len() as u16).to_le_bytes());
    buffer.extend(message);
    buffer
}

/// No runtime request uses the protocol buffer framing yet (Xodus leaves it
/// unimplemented and would panic); the connection is closed instead.
pub(crate) async fn proto_handle(
    _socket: &mut tokio::net::UnixStream,
    _context: &mut context::SimpleContext,
) -> tokio::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "protocol buffer requests are not supported",
    ))
}

/// A running account service; dropping it stops the service and removes its
/// socket folder.
pub struct Session {
    stop: CancellationToken,
    dir: tempfile::TempDir,
    port: u16,
    key: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Longest socket folder path that leaves room for the private folder and
/// `xodus.sock` within a Unix socket address (108 bytes with its NUL).
const MAX_SOCKET_BASE: usize = 107 - "/xbox-service-XXXXXX/xodus.sock".len();

/// Where the socket folder goes: `preferred`, unless its path is too long
/// for a socket address (a deep cache folder), then the user's runtime
/// folder, else the temporary folder.
fn socket_base(preferred: &Path, runtime: Option<PathBuf>) -> PathBuf {
    let fits = |p: &Path| p.as_os_str().len() <= MAX_SOCKET_BASE;
    if fits(preferred) {
        return preferred.to_path_buf();
    }
    runtime
        .map(|r| r.join("irori-xbox"))
        .filter(|p| fits(p))
        .unwrap_or_else(|| std::env::temp_dir().join("irori-xbox"))
}

impl Session {
    /// Start a service for the account in `tokens`, its socket folder inside
    /// `runtime_dir`. Needs a registered device.
    pub async fn start(tokens: TokenManager, runtime_dir: &Path) -> Result<Self> {
        let Token::Legacy(device) = tokens
            .get_device_sts_token()
            .context("Sign in to the Xbox store first")?
        else {
            anyhow::bail!("The device sign-in is not usable. Sign in to the Xbox store again.");
        };
        let runtime_dir = &socket_base(
            runtime_dir,
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        );
        std::fs::create_dir_all(runtime_dir)
            .with_context(|| format!("Could not create {}", runtime_dir.display()))?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("xbox-service-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir_in(runtime_dir)?;
        let socket_path = dir.path().join("xodus.sock");
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Could not open {}", socket_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        }
        let bridge = TcpListener::bind("127.0.0.1:0").await?;
        let port = bridge.local_addr()?.port();
        let key = random_key();
        let stop = CancellationToken::new();
        let context = context::SimpleContext::new(device, Arc::new(tokens));
        tokio::spawn(serve(listener, context, stop.clone()));
        tokio::spawn(bridge_loop(bridge, socket_path, key.clone(), stop.clone()));
        Ok(Self {
            stop,
            dir,
            port,
            key,
        })
    }

    /// The variables that point the game's runtime at this service.
    pub fn env(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "XODUS_SOCKET".to_string(),
                self.socket().to_string_lossy().into_owned(),
            ),
            ("XODUS_BRIDGE_PORT".to_string(), self.port.to_string()),
            ("XODUS_BRIDGE_KEY".to_string(), self.key.clone()),
            ("XODUS_SAVE_SYNC".to_string(), "auto".to_string()),
        ])
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("xodus.sock")
    }
}

fn random_key() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn serve(listener: UnixListener, context: context::SimpleContext, stop: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stop.cancelled() => return,
        };
        match accepted {
            Ok((socket, _)) => {
                let (context, stop) = (context.clone(), stop.clone());
                tokio::spawn(async move { router::route(socket, stop, context).await });
            }
            Err(e) => {
                tracing::warn!("account service: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Accept bridge clients: the key first, then a plain pipe to the socket.
async fn bridge_loop(listener: TcpListener, socket: PathBuf, key: String, stop: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stop.cancelled() => return,
        };
        let Ok((client, _)) = accepted else {
            continue;
        };
        let (socket, key, stop) = (socket.clone(), key.clone(), stop.clone());
        tokio::spawn(async move {
            tokio::select! {
                _ = bridge(client, &socket, &key) => {}
                _ = stop.cancelled() => {}
            }
        });
    }
}

async fn bridge(mut client: TcpStream, socket: &Path, key: &str) -> std::io::Result<()> {
    let mut supplied = [0u8; KEY_LENGTH];
    tokio::time::timeout(KEY_TIMEOUT, client.read_exact(&mut supplied))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    if !same_key(&supplied, key.as_bytes()) {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    let mut upstream = tokio::net::UnixStream::connect(socket).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Constant-time comparison of the bridge key.
fn same_key(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_msstore_api::models::secrets::LegacyToken;
    use tokio::io::AsyncWriteExt;

    fn tokens_with_device() -> TokenManager {
        let tokens = TokenManager::with_memory();
        let device = LegacyToken {
            key_name: None,
            token: "synthetic-device".into(),
            binary_secret: None,
            tpm_key: None,
            lifetime: irori_msstore_api::models::soap::Timestamp {
                id: None,
                created: "2026-01-01T00:00:00Z".into(),
                expires: "2099-01-01T00:00:00Z".into(),
            },
        };
        tokens
            .save_device_token(
                irori_msstore_api::tokens::PASSPORT_STS.into(),
                Token::Legacy(device),
            )
            .unwrap();
        tokens
    }

    async fn ping(stream: &mut (impl AsyncReadExt + AsyncWriteExt + Unpin)) -> Vec<u8> {
        stream
            .write_all(&encode_message(XML_MAGIC, 1, b"ping".to_vec()))
            .await
            .unwrap();
        let mut answer = [0u8; 12];
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut answer))
            .await
            .unwrap()
            .unwrap();
        answer.to_vec()
    }

    #[tokio::test]
    async fn socket_and_keyed_bridge_answer_and_stop_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::start(tokens_with_device(), dir.path())
            .await
            .unwrap();
        let env = session.env();
        let socket = PathBuf::from(&env["XODUS_SOCKET"]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let expected = encode_message(XML_MAGIC, 2, b"ping".to_vec());
        let mut unix = tokio::net::UnixStream::connect(&socket).await.unwrap();
        assert_eq!(ping(&mut unix).await, expected);

        let port: u16 = env["XODUS_BRIDGE_PORT"].parse().unwrap();
        let mut good = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        good.write_all(env["XODUS_BRIDGE_KEY"].as_bytes())
            .await
            .unwrap();
        assert_eq!(ping(&mut good).await, expected);

        let mut bad = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        bad.write_all(&[b'0'; KEY_LENGTH]).await.unwrap();
        bad.write_all(&encode_message(XML_MAGIC, 1, b"ping".to_vec()))
            .await
            .unwrap();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), bad.read(&mut byte))
            .await
            .unwrap();
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "a wrong key reached the service"
        );

        let folder = socket.parent().unwrap().to_path_buf();
        drop(session);
        assert!(!folder.exists());
    }

    #[test]
    fn message_types_round_trip_and_unknown_is_refused() {
        assert_eq!(
            XodusMessageType::try_from(3),
            Ok(XodusMessageType::MsaTokenRequest)
        );
        assert!(XodusMessageType::try_from(7).is_err());
        assert!(same_key(b"ab", b"ab") && !same_key(b"ab", b"ac") && !same_key(b"a", b"ab"));
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;

    #[test]
    fn a_deep_folder_moves_the_socket_somewhere_shorter() {
        let short = Path::new("/home/u/.cache/irori/stores/xbox/service");
        assert_eq!(socket_base(short, None), short);
        let deep = PathBuf::from(format!("/home/u/{}/service", "x".repeat(100)));
        assert_eq!(
            socket_base(&deep, Some("/run/user/1000".into())),
            Path::new("/run/user/1000/irori-xbox")
        );
        assert_eq!(
            socket_base(&deep, None),
            std::env::temp_dir().join("irori-xbox")
        );
        let socket = socket_base(&deep, Some("/run/user/1000".into()))
            .join("xbox-service-AbCdEf/xodus.sock");
        assert!(socket.as_os_str().len() < 108);
    }
}
