//! Synthetic ciphertext only. No account, license, game files or executable runs.
use super::*;
use crate::crypt::encrypt_page_xts;
use std::io::Cursor;

fn fixture(length: u64) -> (XvdFile, SegmentFile, [u8; 32], Vec<u8>, Vec<u8>) {
    let content = SegmentFile {
        offset: 0,
        length,
        keep_encrypted: true,
        data_hashs: vec![],
    };
    let stored = content.stored_length().unwrap() as usize;
    let mut bytes = XvdHeader::buffer();
    bytes[512..520].copy_from_slice(b"msft-xvd");
    bytes[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
    bytes[652..656].copy_from_slice(&4u32.to_le_bytes());
    let header = XvdHeader::try_from_array(&bytes).unwrap();
    let layout = header.layout();
    let region = XvcRegionId::Other(7);
    let id = Uuid::nil();
    let units: Vec<u32> = (0..stored / PAGE_SIZE)
        .map(|i| (i * 7 + 17) as u32)
        .collect();
    let xvd = XvdFile {
        header,
        layout,
        encrypted_section_infos: vec![EncryptedSectionInfo {
            section_offset: 0,
            section_length: stored as u64,
            header_id: region,
            vduid: id,
            data_units: Some(units.clone()),
            first_segment_index: 0,
            data_hashs: vec![],
        }],
    };
    let key: [u8; 32] = std::array::from_fn(|i| i as u8 + 1);
    let tweak_key: [u8; 16] = key[..16].try_into().unwrap();
    let data_key: [u8; 16] = key[16..].try_into().unwrap();
    let tweak_cipher = Aes128Enc::new((&tweak_key).into());
    let data_cipher = Aes128Enc::new((&data_key).into());
    let plain: Vec<u8> = (0..stored).map(|i| (i % 251) as u8).collect();
    let mut encrypted = plain.clone();
    for (i, page) in encrypted.chunks_exact_mut(PAGE_SIZE).enumerate() {
        encrypt_page_xts(
            page.try_into().unwrap(),
            TweakGenerator::new(region, id).with_data_unit(units[i]),
            &tweak_cipher,
            &data_cipher,
        );
    }
    (
        xvd,
        content,
        key,
        plain[..length as usize].to_vec(),
        encrypted,
    )
}

#[test]
fn streaming_stored_length_distinguishes_plain_and_ciphertext_and_rejects_overflow() {
    for (length, stored) in [
        (0, 0),
        (1, 4096),
        (4095, 4096),
        (4096, 4096),
        (4097, 8192),
        (8192, 8192),
    ] {
        let mut file = SegmentFile {
            offset: 0,
            length,
            keep_encrypted: true,
            data_hashs: vec![],
        };
        assert_eq!(file.stored_length().unwrap(), stored);
        file.keep_encrypted = false;
        assert_eq!(file.stored_length().unwrap(), length);
    }
    let mut file = SegmentFile {
        offset: 0,
        length: u64::MAX,
        keep_encrypted: true,
        data_hashs: vec![],
    };
    assert!(file.stored_length().is_err());
    file.keep_encrypted = false;
    assert_eq!(file.stored_length().unwrap(), u64::MAX);
}

#[tokio::test]
async fn streaming_throughput_decrypts_large_batches_with_exact_unpadded_plaintext() {
    let (xvd, mut file, key, plain, ciphertext) = fixture(1024 * 1024 + 107);
    file.keep_encrypted = false;
    let stored = ciphertext.len();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(format!("HTTP/1.1 206 Test\r\nContent-Length: {stored}\r\nContent-Range: bytes 0-{}/{stored}\r\nConnection: close\r\n\r\n",stored-1).as_bytes()).await.unwrap();
        socket.write_all(&ciphertext).await.unwrap();
    });
    let mut output = Vec::new();
    xvd.download_file_http(
        &crate::http_client().unwrap(),
        &format!("http://{address}/fixture"),
        &mut output,
        &file,
        key,
        |_, _| {},
        None,
        0,
        |_| Ok(()),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(output, plain);
}

#[tokio::test]
async fn streaming_encrypted_download_and_partial_resume_keep_exact_ciphertext_for_launch_decryption()
 {
    for length in [1u64, 4095, 4096, 4097, 5000, 8192, 8193, 1024 * 1024 + 107] {
        for initial in [0u64, 4096].into_iter().filter(|&n| n < length) {
            let (xvd, content, key, plain, ciphertext) = fixture(length);
            let stored = ciphertext.len();
            let body = ciphertext.clone();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                assert!(
                    String::from_utf8(request)
                        .unwrap()
                        .to_ascii_lowercase()
                        .contains(&format!("range: bytes={initial}-{}\r\n", stored - 1))
                );
                socket.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {initial}-{}/{stored}\r\nConnection: close\r\n\r\n",stored-initial as usize,stored-1).as_bytes()).await.unwrap();
                for chunk in body[initial as usize..].chunks(137) {
                    socket.write_all(chunk).await.unwrap();
                }
            });
            let mut downloaded = ciphertext[..initial as usize].to_vec();
            let mut committed = vec![];
            let client = crate::http_client().unwrap();
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                xvd.download_file_http(
                    &client,
                    &format!("http://{address}/synthetic"),
                    &mut downloaded,
                    &content,
                    key,
                    |_, _| {},
                    None,
                    initial,
                    |n| {
                        committed.push(n);
                        Ok(())
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
            server.await.unwrap();
            assert_eq!(downloaded.len(), stored);
            assert_eq!(downloaded, ciphertext);
            assert_eq!(committed.last(), Some(&length));
            let mut image = vec![];
            xvd.mount_mem_fd(
                &mut Cursor::new(&downloaded),
                &mut image,
                &content,
                key,
                |_, _| {},
            )
            .await
            .unwrap();
            assert_eq!(image.len(), length as usize);
            assert_eq!(image, plain);
        }
    }
}

#[tokio::test]
async fn streaming_plain_output_remains_unpadded_and_short_ciphertext_returns_error() {
    let (xvd, mut content, key, plain, ciphertext) = fixture(5000);
    let mut image = vec![];
    let result = xvd
        .mount_mem_fd(
            &mut Cursor::new(&ciphertext[..5000]),
            &mut image,
            &content,
            key,
            |_, _| {},
        )
        .await;
    assert_eq!(
        result
            .unwrap_err()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    content.keep_encrypted = false;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 8192\r\nContent-Range: bytes 0-8191/8192\r\nConnection: close\r\n\r\n").await.unwrap();
        socket.write_all(&ciphertext).await.unwrap();
    });
    let mut downloaded = vec![];
    let client = crate::http_client().unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        xvd.download_file_http(
            &client,
            &format!("http://{address}/synthetic"),
            &mut downloaded,
            &content,
            key,
            |_, _| {},
            None,
            0,
            |_| Ok(()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    server.await.unwrap();
    assert_eq!(downloaded.len(), 5000);
    assert_eq!(downloaded, plain);
}

#[test]
fn streaming_update_verifier_checks_licensed_plaintext_ciphertext_wrong_keys_and_modified_pages() {
    use sha2_11::{Digest, Sha256};
    let (xvd, mut content, key, plain, ciphertext) = fixture(8192);
    content.data_hashs = ciphertext
        .chunks_exact(PAGE_SIZE)
        .map(|p| Sha256::digest(p)[..20].try_into().unwrap())
        .collect();
    assert!(
        xvd.stored_page_verifier(&content, key)
            .matches(0, ciphertext[..PAGE_SIZE].try_into().unwrap())
    );
    content.keep_encrypted = false;
    let page: &[u8; PAGE_SIZE] = plain[..PAGE_SIZE].try_into().unwrap();
    assert!(xvd.stored_page_verifier(&content, key).matches(0, page));
    assert!(!xvd.stored_page_verifier(&content, [0; 32]).matches(0, page));
    let mut corrupt = *page;
    corrupt[200] ^= 1;
    assert!(!xvd.stored_page_verifier(&content, key).matches(0, &corrupt));
    assert!(!xvd.stored_page_verifier(&content, key).matches(1, page));
    assert!(!xvd.stored_page_verifier(&content, key).matches(2, page));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "synthetic encrypted loopback/disk benchmark, not Internet bandwidth"]
async fn streaming_encrypted_loopback_file_benchmark() {
    let (xvd, mut file, key, plain, ciphertext) = fixture(64 * 1024 * 1024 + 107);
    file.keep_encrypted = false;
    let length = file.length;
    let stored = ciphertext.len();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(format!("HTTP/1.1 206 Test\r\nContent-Length: {stored}\r\nContent-Range: bytes 0-{}/{stored}\r\nConnection: close\r\n\r\n",stored-1).as_bytes()).await.unwrap();
        socket.write_all(&ciphertext).await.unwrap();
    });
    let target =
        std::env::temp_dir().join(format!("nexus-encrypted-throughput-{}.bin", Uuid::new_v4()));
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&target)
        .await
        .unwrap();
    let client = crate::http_client().unwrap();
    let start = std::time::Instant::now();
    xvd.download_file_http(
        &client,
        &format!("http://{address}/fixture"),
        &mut output,
        &file,
        key,
        |_, _| {},
        None,
        0,
        |_| Ok(()),
    )
    .await
    .unwrap();
    output.sync_all().await.unwrap();
    let seconds = start.elapsed().as_secs_f64();
    server.await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), plain);
    drop(output);
    std::fs::remove_file(target).unwrap();
    eprintln!(
        "XTS_HTTP_BENCH bytes={length} seconds={seconds:.6} MiBps={:.2}",
        length as f64 / 1048576.0 / seconds
    );
}
