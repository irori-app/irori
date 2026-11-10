use super::*;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn xml() -> &'static str {
    "<License><SPLicenseBlock>synthetic-license-block</SPLicenseBlock><LicenseInfo Type=\"Full\" LicenseUsage=\"Offline\"/><Binding Binding_Type=\"Device\"><DeviceId>synthetic-device</DeviceId></Binding></License>"
}
fn success() -> serde_json::Value {
    json!({"license":{"keys":[{"value":BASE64_STANDARD.encode(xml())}],"leases":[]}})
}

#[test]
fn content_ticket_content_request_preserves_jwe_and_other_license_fields() {
    let raw = "eyJhbGciOiJkaXIiLCJlbmMiOiJBMTI4Q0JDLUhTMjU2In0..aXY.Y2lwaGVy_dGV4dA.dGFn";
    for (input, expected) in [
        (format!("d={raw}"), raw),
        ("d=opaque".into(), "t=opaque"),
        ("t=compact&p=proof".into(), "t=compact&p=proof"),
    ] {
        let request = super::super::content_request(
            &reqwest::Client::new(),
            "device-unchanged".into(),
            crate::models::secrets::licensing_ticket(input),
            "reference-unchanged".into(),
            "11111111-1111-4111-8111-111111111111".into(),
            "US".into(),
        )
        .build()
        .unwrap();
        assert_eq!(request.headers()["authorization"], "device-unchanged");
        let body: serde_json::Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["market"], "US");
        assert_eq!(body["licenseVersion"], 4);
        assert_eq!(body["contentId"], "11111111-1111-4111-8111-111111111111");
        let user = &body["users"].as_object().unwrap().values().next().unwrap()[0];
        assert_eq!(user["identityValue"], expected);
        assert_eq!(user["localTicketReference"], "reference-unchanged");
        assert_eq!(user["identityType"], "Msa");
    }
}

#[test]
fn content_license_request_contract_is_unchanged() {
    let request = super::super::content_request(
        &reqwest::Client::new(),
        "synthetic-device-ticket".into(),
        "synthetic-user-ticket".into(),
        "synthetic-ticket-reference".into(),
        "11111111-1111-4111-8111-111111111111".into(),
        "US".into(),
    )
    .build()
    .unwrap();
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(
        request.url().as_str(),
        "https://licensing.mp.microsoft.com/v7.0/licenses/content"
    );
    assert_eq!(request.headers()["from"], "XboxLicenseManager");
    assert_eq!(
        request.headers()["authorization"],
        "synthetic-device-ticket"
    );
    assert_eq!(
        request.headers()["user-agent"],
        "XboxLm-PC/Microsoft.GamingServices_32.107.4002.0_x64__8wekyb3d8bbwe"
    );
    assert!(request.headers().contains_key("ms-cv"));
    assert_eq!(request.timeout(), Some(&std::time::Duration::from_secs(30)));
    let body: serde_json::Value =
        serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 9);
    assert_eq!(body["contentId"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(body["market"], "US");
    assert_eq!(body["licenseVersion"], 4);
    assert_eq!(body["concurrencyMode"], "Rude");
    assert_eq!(body["needKey"], true);
    assert_eq!(body["keyOnly"], true);
    assert_eq!(
        body["deviceContext"],
        json!({"hardwareManufacturer":"Public","hardwareType":"Public","mobileOperator":"Public"})
    );
    let users = body["users"].as_object().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        users.values().next().unwrap(),
        &json!([{"identityType":"Msa","identityValue":"synthetic-user-ticket","localTicketReference":"synthetic-ticket-reference"}])
    );
    let challenge = BASE64_STANDARD
        .decode(body["clientChallenge"].as_str().unwrap())
        .unwrap();
    let challenge = std::str::from_utf8(&challenge).unwrap();
    assert!(challenge.contains("<LicenseProtocolVersion>5</LicenseProtocolVersion>"));
    assert!(challenge.contains("<SigningKeyVersion>1</SigningKeyVersion>"));
    assert!(challenge.contains("<ClientVersion>2</ClientVersion>"));
}
fn stage(value: serde_json::Value) -> String {
    parse(&serde_json::to_vec(&value).unwrap())
        .err()
        .unwrap()
        .stage()
        .into()
}

#[test]
fn content_license_device_limit_is_code_based_and_never_echoes_provider_data() {
    let denial = |code| {
        json!({"satisfactionFailure":{"code":code,
        "description":"private@example.test token=SECRET https://private.invalid/",
        "alternateContentIds":["never-retry"],"remediationAction":"https://private.invalid/"}})
    };
    let error = parse(&serde_json::to_vec(&denial(501i64)).unwrap())
        .err()
        .unwrap();
    assert_eq!(error.stage(), "content-device-limit");
    assert!(
        error
            .to_string()
            .contains("Microsoft Store device limit reached")
    );
    assert!(
        error
            .to_string()
            .contains("https://account.microsoft.com/devices/content")
    );
    assert!(!format!("{error:?} {error}").contains("SECRET"));
    assert!(!error.to_string().contains("private"));
    for code in [i64::MIN, -1, 0, 1, 500, 502, i64::MAX] {
        assert_eq!(stage(denial(code)), "content-not-entitled");
    }
    for code in [json!("501"), json!(501.0), json!(null)] {
        assert_eq!(
            stage(json!({"satisfactionFailure":{"code":code,"description":"private"}})),
            "content-schema"
        );
    }
    let mut ambiguous = success();
    ambiguous["satisfactionFailure"] = denial(501)["satisfactionFailure"].clone();
    assert_eq!(stage(ambiguous), "content-schema");
}

#[tokio::test]
async fn content_license_device_limit_http_is_not_an_http501_and_never_retries() {
    let body =
        serde_json::to_vec(&json!({"satisfactionFailure":{"code":501,"description":"private"}}))
            .unwrap();
    for (status, expected) in [
        (200, "content-device-limit"),
        (501, "content-http"),
        (401, "content-http"),
    ] {
        let (url, server) = server(
            format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            body.clone(),
        )
        .await;
        let error = fetch(reqwest::Client::new().get(url)).await.err().unwrap();
        assert_eq!(error.stage(), expected);
        assert!(!error.to_string().contains("private"));
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[test]
fn content_license_success_preserves_existing_first_key_and_extra_fields() {
    let mut fixture = success();
    fixture["license"]["keys"]
        .as_array_mut()
        .unwrap()
        .push(json!({"value":"not-selected"}));
    fixture["license"]["leases"] = json!([{"value":"unchanged-lease"}]);
    fixture["extraField"] = json!("ignored");
    let bytes = serde_json::to_vec(&fixture).unwrap();
    let (content, license) = parse(&bytes).unwrap();
    let old: LicenseContentResponse = serde_json::from_slice(&bytes).unwrap();
    let LicenseContentResponse::Success { license: old } = old else {
        panic!("fixture");
    };
    let old_xml = BASE64_STANDARD.decode(&old.keys[0].value).unwrap();
    let old_license: License =
        quick_xml::de::from_str(std::str::from_utf8(&old_xml).unwrap()).unwrap();
    assert_eq!(content.keys.len(), old.keys.len());
    assert_eq!(content.keys[0].value, old.keys[0].value);
    assert_eq!(content.keys[1].value, old.keys[1].value);
    assert_eq!(content.leases[0].value, old.leases[0].value);
    assert_eq!(license.splicense_block, old_license.splicense_block);
    assert_eq!(license.binding.device_id, old_license.binding.device_id);
}

#[test]
fn content_license_denials_are_not_successes_or_alternative_package_retries() {
    let denial = json!({"satisfactionFailure":{"code":1,"description":"private@example.test t=SECRET",
        "alternateContentIds":["do-not-retry"],"remediationProductSkus":[{"productId":"do-not-guess"}]}});
    assert_eq!(stage(denial.clone()), "content-not-entitled");
    let mut ambiguous = success();
    ambiguous["satisfactionFailure"] = denial["satisfactionFailure"].clone();
    assert_eq!(stage(ambiguous), "content-schema");
    let error = parse(&serde_json::to_vec(&denial).unwrap()).err().unwrap();
    assert!(!format!("{error} {error:?}").contains("SECRET"));
    assert!(!format!("{error} {error:?}").contains("example.test"));
}

#[test]
fn content_license_rejects_unknown_schema_without_relaxing_required_fields() {
    for value in [
        json!(null),
        json!([]),
        json!({"error":"private"}),
        json!({"license":null}),
        json!({"license":{"keys":[]}}),
        json!({"license":{"keys":"invalid","leases":[]}}),
        json!({"license":{"keys":[{}],"leases":[]}}),
        json!({"satisfactionFailure":{"code":"private","description":"private"}}),
    ] {
        assert_eq!(stage(value), "content-schema");
    }
    assert_eq!(
        parse(b"<html>private-token</html>").err().unwrap().stage(),
        "content-json"
    );
    assert_eq!(parse(b"").err().unwrap().stage(), "content-json");
}

#[test]
fn content_license_malformed_keys_are_errors_not_panics() {
    assert_eq!(
        stage(json!({"license":{"keys":[],"leases":[]}})),
        "content-empty-keys"
    );
    for (key, expected) in [
        (String::new(), "content-empty-keys"),
        ("not base64".into(), "content-base64"),
        (BASE64_STANDARD.encode([0xff, 0xfe]), "content-utf8"),
        (
            BASE64_STANDARD.encode("<broken private='SECRET'>"),
            "content-xml",
        ),
        (
            BASE64_STANDARD.encode("<License><SPLicenseBlock/></License>"),
            "content-xml",
        ),
    ] {
        let fixture = json!({"license":{"keys":[{"value":key}],"leases":[]}});
        let error = parse(&serde_json::to_vec(&fixture).unwrap()).err().unwrap();
        assert_eq!(error.stage(), expected);
        assert!(!format!("{error} {error:?}").contains("SECRET"));
    }
    assert_eq!(
        parse(&vec![b'x'; MAX_BODY + 1]).err().unwrap().stage(),
        "content-size"
    );
}

async fn server(headers: String, body: Vec<u8>) -> (String, tokio::task::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await.unwrap();
        socket.write_all(headers.as_bytes()).await.unwrap();
        let _ = socket.write_all(&body).await;
        1
    });
    (format!("http://{address}/synthetic"), task)
}

#[tokio::test]
async fn content_license_actual_http_success_preserves_license() {
    let bytes = serde_json::to_vec(&success()).unwrap();
    let (url, server) = server(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", bytes.len()), bytes).await;
    let (_, license) = fetch(reqwest::Client::new().get(url)).await.unwrap();
    assert_eq!(license.splicense_block, "synthetic-license-block");
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn content_license_status_precedes_body_and_never_retries_denied_license() {
    for status in [301, 400, 401, 403, 404, 429, 500, 503] {
        // Even a success-shaped body on HTTP403/500 is never accepted.
        let bytes = serde_json::to_vec(&success()).unwrap();
        let (url, server) = server(
            format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            ),
            bytes,
        )
        .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let error = fetch(client.get(url)).await.err().unwrap();
        assert_eq!(error.stage(), "content-http");
        assert!(error.to_string().contains(&status.to_string()));
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[tokio::test]
async fn content_license_actual_http_json_and_schema_failures_are_distinct() {
    for (bytes, expected) in [
        (
            b"<html>private@example.test</html>".to_vec(),
            "content-json",
        ),
        (br#"{"unknown":"private"}"#.to_vec(), "content-schema"),
        (
            serde_json::to_vec(&json!({"satisfactionFailure":{"code":1,"description":"private"}}))
                .unwrap(),
            "content-not-entitled",
        ),
    ] {
        let (url, server) = server(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            ),
            bytes,
        )
        .await;
        let error = fetch(reqwest::Client::new().get(url)).await.err().unwrap();
        assert_eq!(error.stage(), expected);
        assert!(!error.to_string().contains("private"));
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[tokio::test]
async fn content_license_bounds_both_declared_and_streamed_body_and_rejects_truncation() {
    for (headers, body, expected) in [
        (
            format!("Content-Length: {}\r\n", MAX_BODY + 1),
            vec![],
            "content-size",
        ),
        (String::new(), vec![b'x'; MAX_BODY + 1], "content-size"),
        (
            "Content-Length: 100\r\n".into(),
            b"truncated".to_vec(),
            "content-read",
        ),
    ] {
        let (url, server) = server(
            format!("HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"),
            body,
        )
        .await;
        assert_eq!(
            fetch(reqwest::Client::new().get(url))
                .await
                .err()
                .unwrap()
                .stage(),
            expected
        );
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[tokio::test]
async fn content_license_transport_timeout_and_cancellation_have_no_background_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(30))
        .build()
        .unwrap();
    let error = fetch(client.get(format!("http://{address}/fixture")))
        .await
        .err()
        .unwrap();
    assert_eq!(error.stage(), "content-timeout");
    server.abort();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert_eq!(
        fetch(reqwest::Client::new().get(format!("http://{address}/fixture")))
            .await
            .err()
            .unwrap()
            .stage(),
        "content-request"
    );
    // fetch owns its request future, it never spawns a detached retry.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let task = fetch(reqwest::Client::new().get(format!("http://{address}/fixture")));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), task)
            .await
            .is_err()
    );
    server.await.unwrap();
}
