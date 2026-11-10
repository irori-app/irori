use super::*;
use std::os::fd::AsRawFd;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct Running {
    client: UnixStream,
    release: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<RequestEnd>,
    started: oneshot::Receiver<()>,
    dropped: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
    watch_fd: (i32, std::path::PathBuf),
    token: CancellationToken,
}
async fn frame(socket: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut header = [0; 8];
    socket.read_exact(&mut header).await?;
    assert_eq!(&header[..4], &crate::service::XML_MAGIC.to_le_bytes());
    assert_eq!(u16::from_le_bytes([header[4], header[5]]), 1);
    let mut body = vec![0; u16::from_le_bytes([header[6], header[7]]) as usize];
    socket.read_exact(&mut body).await?;
    Ok(body)
}
async fn reply(socket: &mut UnixStream, body: Vec<u8>) -> io::Result<()> {
    socket
        .write_all(&crate::service::encode_message(
            crate::service::XML_MAGIC,
            2,
            body,
        ))
        .await
}
fn request(body: &[u8]) -> Vec<u8> {
    crate::service::encode_message(crate::service::XML_MAGIC, 1, body.to_vec())
}
async fn start(
    pipeline: bool,
    budget: Duration,
    disabled: bool,
    fail_poll: bool,
    commit: Option<(Arc<irori_msstore_api::tokens::TokenManager>, bool)>,
) -> Running {
    let (mut client, mut server) = UnixStream::pair().unwrap();
    let mut peer = PeerMonitor::new(&server);
    let raw = peer.fd.as_ref().unwrap().as_raw_fd();
    let watch_fd = (
        raw,
        std::fs::read_link(format!("/proc/self/fd/{raw}")).unwrap(),
    );
    if disabled {
        peer.fd = None;
    }
    peer.fail_poll = fail_poll;
    let polls = peer.polls.clone();
    let token = CancellationToken::new();
    let worker_token = token.clone();
    let (release, gate) = oneshot::channel();
    let (begin, started) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let sentinel = dropped.clone();
    let task = tokio::spawn(async move {
        let operation = async {
            let body = frame(&mut server).await?;
            let _drop = Dropped(sentinel);
            let persist = |before_gate| {
                if let Some((tokens, before)) = &commit
                    && *before == before_gate
                {
                    tokens
                        .save_user_token(
                            "fixture-memory-only".into(),
                            irori_msstore_api::models::secrets::Token::Compact("committed".into()),
                        )
                        .unwrap();
                }
            };
            persist(true);
            let _ = begin.send(());
            let _ = gate.await;
            persist(false);
            reply(&mut server, body).await?;
            if pipeline {
                let next = frame(&mut server).await?;
                reply(&mut server, next).await?;
            }
            Ok(())
        };
        run_request(&peer, &worker_token, budget, operation).await
    });
    client.write_all(&request(b"one")).await.unwrap();
    Running {
        client,
        release: Some(release),
        task,
        started,
        dropped,
        polls,
        watch_fd,
        token,
    }
}
async fn began(r: &mut Running) {
    tokio::time::timeout(Duration::from_secs(1), &mut r.started)
        .await
        .unwrap()
        .unwrap();
}
async fn exact_reply(client: &mut UnixStream, body: &[u8]) {
    let expected = crate::service::encode_message(crate::service::XML_MAGIC, 2, body.to_vec());
    let mut actual = vec![0; expected.len()];
    tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut actual))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
}
fn assert_closed(watch: &(i32, std::path::PathBuf)) {
    let current = std::fs::read_link(format!("/proc/self/fd/{}", watch.0));
    assert!(
        current.as_ref().ok() != Some(&watch.1),
        "watch descriptor leaked"
    );
}
#[tokio::test]
async fn disconnect_drops_dispatch_before_release_and_cleans_descriptor() {
    let mut r = start(false, Duration::from_secs(3), false, false, None).await;
    began(&mut r).await;
    drop(r.client);
    let finished = tokio::time::timeout(Duration::from_millis(350), &mut r.task).await;
    let promptly = matches!(finished, Ok(Ok(RequestEnd::Stopped)));
    if !promptly {
        let _ = r.release.take().unwrap().send(());
        let _ = r.task.await;
    }
    assert!(
        promptly,
        "full disconnect retained pending dispatch until release/deadline"
    );
    assert!(r.dropped.load(Ordering::SeqCst));
    assert_closed(&r.watch_fd);
}
#[tokio::test]
async fn half_close_preserves_response_without_readable_eof_spin() {
    let mut r = start(false, Duration::from_secs(3), false, false, None).await;
    began(&mut r).await;
    r.client.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert!(!r.task.is_finished());
    assert!(!r.dropped.load(Ordering::SeqCst));
    let count = r.polls.load(Ordering::SeqCst);
    assert!(
        (2..=8).contains(&count),
        "unexpected monitor ticks: {count}"
    );
    r.release.take().unwrap().send(()).unwrap();
    exact_reply(&mut r.client, b"one").await;
    assert!(matches!(
        r.task.await.unwrap(),
        RequestEnd::Finished(Ok(Ok(())))
    ));
    assert_closed(&r.watch_fd);
}
#[tokio::test]
async fn unread_pipeline_preserves_bytes_and_does_not_spin() {
    let mut r = start(true, Duration::from_secs(3), false, false, None).await;
    began(&mut r).await;
    r.client.write_all(&request(b"two")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert!(!r.task.is_finished());
    let count = r.polls.load(Ordering::SeqCst);
    assert!(
        (2..=8).contains(&count),
        "unexpected monitor ticks: {count}"
    );
    r.release.take().unwrap().send(()).unwrap();
    exact_reply(&mut r.client, b"one").await;
    exact_reply(&mut r.client, b"two").await;
    assert!(matches!(
        r.task.await.unwrap(),
        RequestEnd::Finished(Ok(Ok(())))
    ));
    assert_closed(&r.watch_fd);
}
#[tokio::test]
async fn full_disconnect_cancels_even_with_unread_pipeline() {
    let mut r = start(true, Duration::from_secs(3), false, false, None).await;
    began(&mut r).await;
    r.client.write_all(&request(b"two")).await.unwrap();
    drop(r.client);
    let finished = tokio::time::timeout(Duration::from_millis(350), &mut r.task).await;
    let promptly = matches!(finished, Ok(Ok(RequestEnd::Stopped)));
    if !promptly {
        let _ = r.release.take().unwrap().send(());
        let _ = r.task.await;
    }
    assert!(promptly, "unread queued frame masked full disconnect");
    assert!(r.dropped.load(Ordering::SeqCst));
    assert_closed(&r.watch_fd);
}
#[tokio::test]
async fn disabled_and_failed_monitor_retain_deadline_without_spin() {
    for (disabled, fail_poll) in [(true, false), (false, true)] {
        let mut r = start(false, Duration::from_millis(180), disabled, fail_poll, None).await;
        began(&mut r).await;
        let outcome = tokio::time::timeout(Duration::from_secs(1), &mut r.task)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, RequestEnd::Finished(Err(_))));
        assert!(r.dropped.load(Ordering::SeqCst));
        let count = r.polls.load(Ordering::SeqCst);
        assert!(count <= 1, "failed monitor spun: {count}");
        assert_closed(&r.watch_fd);
    }
}
#[tokio::test]
async fn shutdown_stops_pending_dispatch_and_closes_watch() {
    let mut r = start(false, Duration::from_secs(3), false, false, None).await;
    began(&mut r).await;
    r.token.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(350), &mut r.task)
            .await
            .unwrap()
            .unwrap(),
        RequestEnd::Stopped
    ));
    assert!(r.dropped.load(Ordering::SeqCst));
    assert_closed(&r.watch_fd);
}
#[tokio::test]
async fn disconnect_does_not_cancel_unrelated_connection() {
    let mut first = start(false, Duration::from_secs(3), false, false, None).await;
    began(&mut first).await;
    let mut second = start(false, Duration::from_secs(3), false, false, None).await;
    began(&mut second).await;
    drop(first.client);
    let finished = tokio::time::timeout(Duration::from_millis(350), &mut first.task).await;
    let promptly = matches!(finished, Ok(Ok(RequestEnd::Stopped)));
    if !promptly {
        let _ = first.release.take().unwrap().send(());
        let _ = first.task.await;
    }
    assert!(!second.task.is_finished());
    assert!(!second.dropped.load(Ordering::SeqCst));
    second.release.take().unwrap().send(()).unwrap();
    exact_reply(&mut second.client, b"one").await;
    assert!(matches!(
        second.task.await.unwrap(),
        RequestEnd::Finished(Ok(Ok(())))
    ));
    assert!(promptly, "first connection did not cancel");
    assert_closed(&first.watch_fd);
    assert_closed(&second.watch_fd);
}
#[tokio::test]
async fn disconnect_keeps_already_committed_memory_refresh() {
    let tokens = Arc::new(irori_msstore_api::tokens::TokenManager::with_memory());
    let mut r = start(
        false,
        Duration::from_secs(3),
        false,
        false,
        Some((tokens.clone(), true)),
    )
    .await;
    began(&mut r).await;
    drop(r.client);
    let finished = tokio::time::timeout(Duration::from_millis(350), &mut r.task).await;
    let promptly = matches!(finished, Ok(Ok(RequestEnd::Stopped)));
    if !promptly {
        let _ = r.release.take().unwrap().send(());
        let _ = r.task.await;
    }
    assert!(
        matches!(tokens.get_user_token_for("fixture-memory-only").unwrap(), Some(irori_msstore_api::models::secrets::Token::Compact(value)) if value == "committed")
    );
    assert!(promptly);
    assert_closed(&r.watch_fd);
}

#[test]
fn hangup_takes_precedence_over_socket_error_but_not_invalid_fd() {
    use nix::poll::PollFlags;
    assert!(!PeerMonitor::disconnect_status(PollFlags::POLLIN).unwrap());
    assert!(PeerMonitor::disconnect_status(PollFlags::POLLHUP | PollFlags::POLLERR).unwrap());
    assert!(PeerMonitor::disconnect_status(PollFlags::POLLERR).is_err());
    assert!(PeerMonitor::disconnect_status(PollFlags::POLLHUP | PollFlags::POLLNVAL).is_err());
}

#[tokio::test]
async fn disconnect_before_commit_does_not_run_later_memory_refresh() {
    let tokens = Arc::new(irori_msstore_api::tokens::TokenManager::with_memory());
    let mut r = start(
        false,
        Duration::from_secs(3),
        false,
        false,
        Some((tokens.clone(), false)),
    )
    .await;
    began(&mut r).await;
    drop(r.client);
    let finished = tokio::time::timeout(Duration::from_millis(350), &mut r.task).await;
    let promptly = matches!(finished, Ok(Ok(RequestEnd::Stopped)));
    if !promptly {
        let _ = r.release.take().unwrap().send(());
        let _ = r.task.await;
    }
    assert!(
        tokens
            .get_user_token_for("fixture-memory-only")
            .unwrap()
            .is_none()
    );
    assert!(promptly);
    assert_closed(&r.watch_fd);
}
