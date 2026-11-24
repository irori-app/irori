//! Guards how zbus is built into Irori on Linux.
//!
//! GPUI, ashpd and accesskit use zbus from GPUI's worker threads, Irori's
//! services from its Tokio runtime, and ashpd shares one session connection
//! between all of them. With zbus' `tokio` feature on (once turned on by
//! `secret-service` and `ksni`), a connection opened on a Tokio thread runs
//! its tasks with `tokio::spawn`, so the first GPUI worker to use it aborted
//! Irori ("there is no reactor running"), depending on which side opened the
//! shared connection first. Without that feature every connection drives
//! itself, whichever thread opened it.

use std::os::unix::net::UnixStream;

/// Two connected peers, opened inside a Tokio runtime as Irori's services
/// open theirs.
fn peers_opened_on_tokio() -> (zbus::Connection, zbus::Connection) {
    let (server, client) = UnixStream::pair().expect("socket pair");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let guid = zbus::Guid::generate();
        let server = zbus::connection::Builder::async_io_unix_stream(server)
            .server(guid)
            .expect("guid")
            .p2p()
            .build();
        let client = zbus::connection::Builder::async_io_unix_stream(client)
            .p2p()
            .build();
        futures::try_join!(server, client).expect("peer connections")
    })
}

#[test]
fn connection_opened_on_tokio_works_from_other_threads() {
    let (server, client) = peers_opened_on_tokio();
    // A thread without a Tokio runtime, as GPUI's workers are: starting the
    // object server spawns a task on the connection's executor.
    std::thread::spawn(move || {
        server.object_server();
        let reply = futures::executor::block_on(client.call_method(
            None::<&str>,
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        ));
        reply.expect("the peer answers");
    })
    .join()
    .expect("zbus needs no Tokio runtime on the calling thread");
}
