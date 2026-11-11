use super::*;

#[tokio::test]
async fn streaming_resume_terminal_http_error_checkpoints_only_written_pages_then_restarts_exactly()
{
    let payload: Vec<u8> = (0..16384).map(|n| (n % 251) as u8).collect();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = payload.clone();
    let task = tokio::spawn(async move {
        for (start, status, length) in [(0, 206, 5000), (5000, 403, 0), (4096, 206, 12288)] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .to_lowercase()
                    .contains(&format!("range: bytes={start}-16383\r\n"))
            );
            let declared = if status == 206 { 16384 - start } else { 0 };
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {declared}\r\nContent-Range: bytes {start}-16383/16384\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            socket
                .write_all(&body[start..start + length])
                .await
                .unwrap();
        }
    });
    let client = crate::http_client().unwrap();
    let url = format!("http://{address}/private?SECRET");
    let file = SegmentFile {
        offset: 0,
        length: 16384,
        data_hashs: vec![],
        keep_encrypted: false,
    };
    let mut output = vec![];
    let mut offsets = vec![];
    let error = XvdFile::download_segment_http(
        &[],
        &client,
        &url,
        &mut output,
        &file,
        [0; 32],
        |_, _| {},
        None,
        0,
        |p| {
            offsets.push(p);
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<TransferError>(),
        Some(&TransferError::new("transfer-http", 403))
    );
    assert_eq!(offsets, vec![4096]);
    assert_eq!(output, &payload[..4096]);
    XvdFile::download_segment_http(
        &[],
        &client,
        &url,
        &mut output,
        &file,
        [0; 32],
        |_, _| {},
        None,
        4096,
        |p| {
            offsets.push(p);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(offsets, vec![4096, 16384]);
    assert_eq!(output, payload);
    task.await.unwrap();
}

#[tokio::test]
async fn streaming_resume_disk_error_does_not_retry_forever_or_commit_partial_writes() {
    struct Broken;
    impl tokio::io::AsyncWrite for Broken {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::other("synthetic disk failure")))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 206 Test\r\nContent-Length: 4096\r\nContent-Range: bytes 0-4095/4096\r\nConnection: close\r\n\r\n").await.unwrap();
        socket.write_all(&[0; 4096]).await.unwrap();
    });
    let client = crate::http_client().unwrap();
    let url = format!("http://{address}/package");
    let file = SegmentFile {
        offset: 0,
        length: 4096,
        data_hashs: vec![],
        keep_encrypted: false,
    };
    let mut committed = false;
    let result = timeout(
        std::time::Duration::from_secs(2),
        XvdFile::download_segment_http(
            &[],
            &client,
            &url,
            &mut Broken,
            &file,
            [0; 32],
            |_, _| {},
            None,
            0,
            |_| {
                committed = true;
                Ok(())
            },
        ),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(!committed);
    server.await.unwrap();
}

#[tokio::test]
async fn streaming_resume_preserves_bytes_across_pause_and_process_restart_offset() {
    for initial_offset in [0u64, 4096] {
        let payload: Vec<u8> = (0..16384).map(|n| (n % 251) as u8).collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = payload.clone();
        let starts = if initial_offset == 0 {
            vec![0usize, 4096]
        } else {
            vec![4096]
        };
        let server = tokio::spawn(async move {
            for start in starts {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let byte = socket.read_u8().await.unwrap();
                    request.push(byte);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap().to_lowercase();
                assert!(request.contains(&format!("range: bytes={start}-16383\r\n")));
                let headers = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-16383/16384\r\nConnection: close\r\n\r\n",
                    body.len() - start
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                let end = if start == 0 { 4096 } else { body.len() };
                socket.write_all(&body[start..end]).await.unwrap();
            }
        });
        let (control, pause) = tokio::sync::watch::channel(false);
        let (checkpoints, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let resume = control.clone();
        let restarter = tokio::spawn(async move {
            if initial_offset == 0 {
                assert_eq!(observed.recv().await, Some(4096));
                resume.send(false).unwrap();
            }
        });
        let mut output = payload[..initial_offset as usize].to_vec();
        let file = SegmentFile {
            offset: 0,
            length: 16384,
            data_hashs: vec![],
            keep_encrypted: false,
        };
        let mut did_pause = initial_offset != 0;
        let client = crate::http_client().unwrap();
        let url = format!("http://{address}/package");
        let transfer = XvdFile::download_segment_http(
            &[],
            &client,
            &url,
            &mut output,
            &file,
            [0; 32],
            |position, _| {
                if !did_pause && position == 4096 {
                    did_pause = true;
                    control.send(true).unwrap();
                }
            },
            Some(pause),
            initial_offset,
            |position| {
                checkpoints.send(position).ok();
                Ok(())
            },
        );
        timeout(tokio::time::Duration::from_secs(5), transfer)
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
        restarter.await.unwrap();
        assert_eq!(output, payload);
    }
}
