//! Synthetic loopback responses only, never device provisioning or a keyring.
use super::*;
use std::io::{Read, Write};

fn valid_response() -> String {
    let license = "<License><SPLicenseBlock>synthetic-block</SPLicenseBlock><LicenseInfo Type=\"Device\"/><Binding Binding_Type=\"Machine\"/></License>";
    format!(
        "<DeviceAddResponse Success=\"true\"><puid>synthetic-puid</puid><DeviceTpmKeyState>0</DeviceTpmKeyState>{license}<KeyHolderLicense>{license}</KeyHolderLicense><HWDeviceID>synthetic-hardware</HWDeviceID><GlobalDeviceID>synthetic-device</GlobalDeviceID><LicenseKeySequence>1</LicenseKeySequence><LicenseSignatureKeyVersion>1</LicenseSignatureKeyVersion><ServerInfo ServerTime=\"2026-10-28T00:00:00Z\">synthetic</ServerInfo></DeviceAddResponse>"
    )
}

async fn response(
    status: u16,
    body: Vec<u8>,
    chunked: bool,
) -> Result<DeviceAddResponse, DeviceCredentialError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        // A client that stops reading an oversized body leaves this write
        // waiting on a full socket buffer; give up instead of hanging.
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        let headers = if chunked {
            format!(
                "HTTP/1.1 {status} Test\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
        } else {
            format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
        };
        if stream.write_all(headers.as_bytes()).is_ok() {
            if chunked {
                let _ = stream.write_all(format!("{:x}\r\n", body.len()).as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.write_all(b"\r\n0\r\n\r\n");
            } else {
                let _ = stream.write_all(&body);
            }
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let received = client
        .get(format!("http://{address}/synthetic"))
        .send()
        .await
        .unwrap();
    let result = device_credential_response(received).await;
    // Join off the runtime: the test runtime has one thread, and the client's
    // connection only closes (ending the server's write) while it runs.
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
    result
}

#[tokio::test]
async fn credential_device_response_accepts_complete_registration_without_rewriting_identity() {
    let parsed = response(200, valid_response().into_bytes(), false)
        .await
        .unwrap();
    assert_eq!(parsed.puid, "synthetic-puid");
    assert_eq!(parsed.hw_device_id, "synthetic-hardware");
    assert_eq!(parsed.license.splicense_block, "synthetic-block");
}

#[tokio::test]
async fn credential_device_response_http_refusals_are_errors_not_panics_or_payload_logs() {
    for status in [400, 401, 403, 429, 500, 503] {
        let error = response(status, b"PRIVATE-SERVER-RESPONSE".to_vec(), false)
            .await
            .unwrap_err();
        assert!(matches!(error, DeviceCredentialError::Transport(_)));
        assert!(!error.to_string().contains("PRIVATE"));
    }
}

#[tokio::test]
async fn credential_device_response_rejects_malformed_incomplete_and_unsuccessful_payloads() {
    for body in [
        vec![],
        b"<html>PRIVATE-SERVER-RESPONSE</html>".to_vec(),
        vec![0xff],
        valid_response()
            .replace("Success=\"true\"", "Success=\"false\"")
            .into_bytes(),
        valid_response().replace("synthetic-block", "").into_bytes(),
        valid_response().replace("synthetic-puid", "").into_bytes(),
    ] {
        let error = response(200, body, false).await.unwrap_err();
        assert!(matches!(error, DeviceCredentialError::InvalidResponse));
        assert!(!error.to_string().contains("PRIVATE"));
    }
}

#[tokio::test]
async fn credential_device_response_bounds_bodies_with_or_without_content_length() {
    for chunked in [false, true] {
        assert!(matches!(
            response(200, vec![b'x'; DEVICE_RESPONSE_LIMIT + 1], chunked).await,
            Err(DeviceCredentialError::InvalidResponse)
        ));
    }
}
