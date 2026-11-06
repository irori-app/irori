//! One instance per configuration directory, using a lifetime lock and an
//! authenticated loopback channel on every desktop platform.
//!
//! A second instance sends its arguments as one JSON message and waits for
//! a one-byte acknowledgement. A waiting one (`--wait`) ends its message
//! with a newline instead of closing its side, and keeps the connection:
//! the running instance answers on it when the session ends, and its
//! closing tells the running instance that the waiter went away.
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use irori_services::StreamClient;

pub struct Instance {
    listener: TcpListener,
    token: String,
    _lock: File,
}
#[derive(Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
}
#[derive(Serialize, Deserialize)]
struct Invocation {
    token: String,
    args: Vec<String>,
    /// The sender stays connected until the session ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    wait: bool,
    /// The streaming client a waiting sender was started for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream: Option<StreamClient>,
}

/// What a second instance's arguments reached.
pub enum Forwarded {
    /// The running instance took them.
    Sent,
    /// The running instance took them and answers on this connection.
    Waiting(TcpStream),
}

/// The connection of a waiting second instance, held by the running one.
pub struct Waiter {
    stream: TcpStream,
    client: Option<StreamClient>,
}

impl std::fmt::Debug for Waiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waiter").finish_non_exhaustive()
    }
}

impl Waiter {
    /// The streaming client the waiting instance was started for.
    pub fn client(&self) -> Option<&StreamClient> {
        self.client.as_ref()
    }

    /// Block until the waiting instance goes away or closes its side.
    pub fn wait_for_hangup(&self) {
        let Ok(mut stream) = self.stream.try_clone() else {
            return;
        };
        let _ = stream.set_read_timeout(None);
        let mut buffer = [0; 64];
        while matches!(stream.read(&mut buffer), Ok(n) if n > 0) {}
    }

    /// Tell the waiting instance how the session ended, and let it go.
    pub fn finish(&self, error: Option<&str>) {
        let line = serde_json::json!({ "done": true, "error": error }).to_string();
        let mut stream = &self.stream;
        let _ = stream.write_all(format!("{line}\n").as_bytes());
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// How the session a waiting instance asked for ended: `Ok` when it ran,
/// else why it couldn't, or that the running instance closed first.
pub fn read_outcome(stream: TcpStream) -> std::result::Result<(), String> {
    let mut line = String::new();
    let _ = stream.set_read_timeout(None);
    let read = std::io::BufRead::read_line(&mut std::io::BufReader::new(stream), &mut line);
    if read.is_err() || line.trim().is_empty() {
        return Err("Irori closed before the session ended".into());
    }
    let value: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
    match value.get("error").and_then(serde_json::Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

pub fn acquire(args: &[String]) -> Result<Option<Instance>> {
    let dir = &irori_core::Paths::get().config;
    Ok(match acquire_in(dir, args, None)? {
        Acquired::Primary(instance) => Some(instance),
        Acquired::Forwarded(_) => None,
    })
}

/// Hand `args` (and the streaming `client` the caller was started for) to
/// the running instance and stay connected, or become the running instance
/// when there is none.
pub fn acquire_waiting(
    args: &[String],
    client: Option<StreamClient>,
) -> Result<std::result::Result<TcpStream, Instance>> {
    let dir = &irori_core::Paths::get().config;
    Ok(match acquire_in(dir, args, Some(client))? {
        Acquired::Primary(instance) => Err(instance),
        Acquired::Forwarded(Forwarded::Waiting(stream)) => Ok(stream),
        Acquired::Forwarded(Forwarded::Sent) => {
            bail!("The running Irori is older and can't wait for a game; restart it")
        }
    })
}

enum Acquired {
    Primary(Instance),
    Forwarded(Forwarded),
}

/// `waiting` is `Some` for a waiting instance, with its streaming client.
fn acquire_in(
    dir: &Path,
    args: &[String],
    waiting: Option<Option<StreamClient>>,
) -> Result<Acquired> {
    let wait = waiting.is_some();
    std::fs::create_dir_all(dir)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let lock = options.open(dir.join("instance.lock"))?;
    let endpoint_file = dir.join("instance.json");
    // The lock holder may still be publishing its endpoint. Never delete an
    // active instance's lock or start a second writer to the same settings.
    for _ in 0..40 {
        match lock.try_lock() {
            Ok(()) => {
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
                let token = uuid::Uuid::new_v4().to_string();
                let endpoint = Endpoint {
                    port: listener.local_addr()?.port(),
                    token: token.clone(),
                };
                let mut private = OpenOptions::new();
                private.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    private.mode(0o600);
                }
                private
                    .open(&endpoint_file)?
                    .write_all(&serde_json::to_vec(&endpoint)?)?;
                return Ok(Acquired::Primary(Instance {
                    listener,
                    token,
                    _lock: lock,
                }));
            }
            Err(TryLockError::WouldBlock) => {
                if let Ok(Some(endpoint)) = irori_core::json_store::read::<Endpoint>(&endpoint_file)
                {
                    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
                    if let Ok(mut stream) =
                        TcpStream::connect_timeout(&address, Duration::from_millis(200))
                    {
                        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                        let invocation = Invocation {
                            token: endpoint.token,
                            args: args.to_vec(),
                            wait,
                            stream: waiting.clone().flatten(),
                        };
                        stream.write_all(&serde_json::to_vec(&invocation)?)?;
                        if wait {
                            stream.write_all(b"\n")?;
                        } else {
                            stream.shutdown(std::net::Shutdown::Write)?;
                        }
                        let mut ack = [0];
                        if stream.read_exact(&mut ack).is_ok() && ack[0] == 1 {
                            return Ok(Acquired::Forwarded(if wait {
                                Forwarded::Waiting(stream)
                            } else {
                                Forwarded::Sent
                            }));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(TryLockError::Error(e)) => {
                return Err(e).context("Could not lock Irori's configuration");
            }
        }
    }
    bail!("Another Irori instance is starting or is not responding")
}

fn receive(mut stream: TcpStream, token: &str) -> Option<(Vec<String>, Option<Waiter>)> {
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    // One deadline for the whole message: connections are served one at a
    // time, and a client trickling bytes must not hold up the links behind it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(left)).ok()?;
        match std::io::Read::read(&mut stream, &mut buffer).ok()? {
            0 => break,
            n if bytes.len() + n > 64 * 1024 => return None,
            n => bytes.extend_from_slice(&buffer[..n]),
        }
        // A waiting instance ends its message with a newline and stays.
        if bytes.ends_with(b"\n") {
            break;
        }
    }
    let invocation: Invocation = serde_json::from_slice(&bytes).ok()?;
    if invocation.token != token {
        return None;
    }
    stream.write_all(&[1]).ok()?;
    let waiter = invocation.wait.then(|| Waiter {
        stream,
        client: invocation.stream,
    });
    Some((invocation.args, waiter))
}

pub fn serve(
    instance: Instance,
    on_args: impl Fn(Vec<String>, Option<Waiter>) + Send + 'static,
) -> Result<()> {
    std::thread::Builder::new()
        .name("irori-instance".into())
        .spawn(move || {
            let _lock = instance._lock;
            for stream in instance.listener.incoming().flatten() {
                if let Some((args, waiter)) = receive(stream, &instance.token) {
                    on_args(args, waiter);
                }
            }
        })
        .context("Could not start Irori's instance listener")?;
    Ok(())
}

// Locks are released by the OS on process exit, including crashes. Leaving
// the stable lock inode avoids races with another instance acquiring it.
pub fn release() {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwards_arguments_without_losing_spaces_or_empty_values() {
        let dir = tempfile::tempdir().unwrap();
        let primary = primary(dir.path());
        let worker = std::thread::spawn(move || {
            let _lock = primary._lock;
            let (stream, _) = primary.listener.accept().unwrap();
            receive(stream, &primary.token).unwrap()
        });
        let args = vec!["--launch".into(), "gog/game with spaces".into(), "".into()];
        assert!(matches!(
            acquire_in(dir.path(), &args, None).unwrap(),
            Acquired::Forwarded(Forwarded::Sent)
        ));
        let (received, waiter) = worker.join().unwrap();
        assert_eq!(received, args);
        assert!(waiter.is_none());
        assert!(matches!(
            acquire_in(dir.path(), &[], None).unwrap(),
            Acquired::Primary(_)
        ));
    }

    fn deck() -> StreamClient {
        StreamClient {
            width: 1280,
            height: 800,
            fps: "60".into(),
            hdr: false,
        }
    }

    fn primary(dir: &Path) -> Instance {
        match acquire_in(dir, &[], None).unwrap() {
            Acquired::Primary(instance) => instance,
            Acquired::Forwarded(_) => panic!("no instance should be running"),
        }
    }

    #[test]
    fn a_waiting_instance_hears_how_the_session_ended() {
        let dir = tempfile::tempdir().unwrap();
        let primary = primary(dir.path());
        let worker = std::thread::spawn(move || {
            let _lock = primary._lock;
            let (stream, _) = primary.listener.accept().unwrap();
            let (args, waiter) = receive(stream, &primary.token).unwrap();
            let waiter = waiter.unwrap();
            assert_eq!(waiter.client(), Some(&deck()));
            waiter.finish(Some("Hades isn't installed"));
            args
        });
        let args = vec!["--launch".into(), "gog/1".into(), "--wait".into()];
        let Acquired::Forwarded(Forwarded::Waiting(stream)) =
            acquire_in(dir.path(), &args, Some(Some(deck()))).unwrap()
        else {
            panic!("the waiting instance should stay connected");
        };
        assert_eq!(read_outcome(stream), Err("Hades isn't installed".into()));
        assert_eq!(worker.join().unwrap(), args);
    }

    #[test]
    fn the_running_instance_hears_a_waiter_hang_up() {
        let dir = tempfile::tempdir().unwrap();
        let primary = primary(dir.path());
        let worker = std::thread::spawn(move || {
            let _lock = primary._lock;
            let (stream, _) = primary.listener.accept().unwrap();
            let (_, waiter) = receive(stream, &primary.token).unwrap();
            // Returns once the waiter is gone; a hang here fails the test.
            waiter.unwrap().wait_for_hangup();
        });
        let Acquired::Forwarded(Forwarded::Waiting(stream)) =
            acquire_in(dir.path(), &["--big-picture".into()], Some(None)).unwrap()
        else {
            panic!("the waiting instance should stay connected");
        };
        drop(stream);
        worker.join().unwrap();
    }
    #[test]
    fn rejects_unauthenticated_invocations() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .write_all(br#"{"token":"wrong","args":["--launch","gog/1"]}"#)
            .unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(receive(listener.accept().unwrap().0, "secret").is_none());
    }
}
