use std::future::Future;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::time::Duration;
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

// A separate safe duplicate avoids borrowing the stream while its request owns
// &mut UnixStream. It never reads, writes or changes flags, and dies with route.
pub(super) struct PeerMonitor {
    fd: Option<OwnedFd>,
    #[cfg(test)]
    polls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    fail_poll: bool,
}
impl PeerMonitor {
    pub(super) fn new(socket: &UnixStream) -> Self {
        Self {
            fd: socket
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| {
                    tracing::debug!(
                        "Peer monitor unavailable, retaining request deadline: {error}"
                    );
                    error
                })
                .ok(),
            #[cfg(test)]
            polls: Default::default(),
            #[cfg(test)]
            fail_poll: false,
        }
    }
    #[cfg(target_os = "linux")]
    fn hung_up(&self) -> io::Result<bool> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        #[cfg(test)]
        {
            self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_poll {
                return Err(io::Error::other("synthetic poll failure"));
            }
        }
        let fd = self.fd.as_ref().expect("enabled peer monitor");
        let mut fds = [PollFd::new(fd.as_fd(), PollFlags::empty())];
        match poll(&mut fds, PollTimeout::ZERO) {
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => return Ok(false),
            Err(error) => return Err(io::Error::from(error)),
        }
        let events = fds[0]
            .revents()
            .ok_or_else(|| io::Error::other("unknown poll status"))?;
        Self::disconnect_status(events)
    }
    #[cfg(target_os = "linux")]
    fn disconnect_status(events: nix::poll::PollFlags) -> io::Result<bool> {
        use nix::poll::PollFlags;
        if events.contains(PollFlags::POLLNVAL) {
            return Err(io::Error::other("invalid peer monitor descriptor"));
        }
        // RDHUP/EOF also describes a legal client shutdown(write). HUP alone
        // confirms both directions are gone, even with unread pipelined bytes.
        // A concurrent socket error does not invalidate that HUP evidence.
        if events.contains(PollFlags::POLLHUP) {
            return Ok(true);
        }
        if events.contains(PollFlags::POLLERR) {
            return Err(io::Error::other("peer monitor socket error"));
        }
        Ok(false)
    }
    async fn disconnected(&self) {
        #[cfg(target_os = "linux")]
        if self.fd.is_some() {
            loop {
                match self.hung_up() {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::debug!(
                            "Peer monitor disabled, retaining request deadline: {error}"
                        );
                        break;
                    }
                }
                // No readiness wait: queued data and half-close remain readable
                // forever. A zero-time poll once per tick cannot spin on them.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        std::future::pending::<()>().await;
    }
}

pub(super) enum RequestEnd {
    Stopped,
    Finished(Result<io::Result<()>, tokio::time::error::Elapsed>),
}
pub(super) async fn run_request(
    peer: &PeerMonitor,
    token: &CancellationToken,
    budget: Duration,
    request: impl Future<Output = io::Result<()>>,
) -> RequestEnd {
    tokio::select! {
        biased;
        _ = token.cancelled() => RequestEnd::Stopped,
        _ = peer.disconnected() => RequestEnd::Stopped,
        result = tokio::time::timeout(budget, request) => RequestEnd::Finished(result),
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "peer_tests.rs"]
mod tests;
