use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const ROOT: &str = "9AAAAAAAAAAA";
const BASE: &str = "9BBBBBBBBBBB";
const UPGRADE: &str = "9CCCCCCCCCCC";
const CID: &str = "11111111-1111-4111-8111-111111111111";
fn game(id: &str) -> Value {
    json!({"ProductId":id,"ProductType":"Game","LocalizedProperties":[{"ProductTitle":"Synthetic base"}],
        "DisplaySkuAvailabilities":[{"Sku":{"Properties":{"Packages":[{"ContentId":CID,"PackageFormat":"MSIXVC",
        "PlatformDependencies":[{"PlatformName":"Windows.Desktop"}]}]}}}]})
}
fn bundle(id: &str, children: Vec<String>) -> Value {
    json!({"ProductId":id,"ProductType":"Game","DisplaySkuAvailabilities":[{"Sku":{"Properties":{
        "BundledSkus":children.iter().map(|child|json!({"BigId":child})).collect::<Vec<_>>()}}}]})
}
fn server(
    routes: BTreeMap<String, (u16, Value)>,
    count: usize,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/products", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && captured.lock().unwrap().len() < count {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("synthetic server: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.starts_with("GET /products/"));
            let target = first.split_whitespace().nth(1).unwrap();
            let id = target
                .strip_prefix("/products/")
                .unwrap()
                .split('?')
                .next()
                .unwrap();
            assert!(target.ends_with("?market=US&languages=en,neutral"));
            captured.lock().unwrap().push(id.to_string());
            let (status, body) = routes
                .get(id)
                .cloned()
                .unwrap_or((404, json!({"error":"must-not-fetch-reverse-license-offer"})));
            let body = body.to_string();
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (endpoint, requests, worker)
}

#[tokio::test]
async fn premium_http_only_reads_declared_bundle_members() {
    let upgrade = json!({"ProductId":UPGRADE,"ProductType":"GameAddOn","DisplaySkuAvailabilities":[{
        "Sku":{"Properties":{}},"Availabilities":[{"LicensingData":{"SatisfyingEntitlementKeys":[{
            "EntitlementKeys":[format!("big:{ROOT}:0010"),"big:9DDDDDDDDDDD:0010"]}]}}]}]});
    let routes = BTreeMap::from([
        (
            ROOT.into(),
            (
                200,
                json!({"Product":bundle(ROOT,vec![BASE.into(),UPGRADE.into()])}),
            ),
        ),
        (BASE.into(), (200, json!({"Product":game(BASE)}))),
        (UPGRADE.into(), (200, json!({"Product":upgrade}))),
    ]);
    let (endpoint, requests, worker) = server(routes, 3);
    let plan = resolve_from(
        &reqwest::Client::builder().no_proxy().build().unwrap(),
        ROOT,
        "US",
        &endpoint,
    )
    .await
    .unwrap();
    worker.join().unwrap();
    let mut requests = requests.lock().unwrap().clone();
    requests.sort();
    assert_eq!(requests, vec![ROOT, BASE, UPGRADE]);
    assert_eq!(plan.choices.len(), 1);
    assert_eq!(plan.choices[0].product_id, BASE);
    assert_eq!(plan.select(None).unwrap(), CID);
}

#[tokio::test]
async fn premium_http_reports_exact_status_without_exposing_response_body() {
    for status in [400, 401, 403, 404, 410, 429, 500, 503] {
        let (endpoint, requests, worker) = server(
            BTreeMap::from([(
                ROOT.into(),
                (
                    status,
                    json!({"private":"synthetic-response-body-must-not-be-disclosed"}),
                ),
            )]),
            1,
        );
        let error = resolve_from(
            &reqwest::Client::builder().no_proxy().build().unwrap(),
            ROOT,
            "US",
            &endpoint,
        )
        .await
        .err()
        .unwrap();
        worker.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(
            error,
            format!("Store metadata returned HTTP {status} for product {ROOT} (market US)")
        );
        assert!(!error.contains("synthetic-response"));
        assert!(!error.contains("unavailable for this market"));
    }
}

#[tokio::test]
async fn premium_unavailable_declared_member_still_stops_resolution() {
    let routes = BTreeMap::from([
        (
            ROOT.into(),
            (200, json!({"Product":bundle(ROOT,vec![UPGRADE.into()])})),
        ),
        (
            UPGRADE.into(),
            (404, json!({"error":"synthetic-unavailable-real-member"})),
        ),
    ]);
    let (endpoint, requests, worker) = server(routes, 2);
    let error = resolve_from(
        &reqwest::Client::builder().no_proxy().build().unwrap(),
        ROOT,
        "US",
        &endpoint,
    )
    .await
    .err()
    .unwrap();
    worker.join().unwrap();
    assert_eq!(requests.lock().unwrap().as_slice(), [ROOT, UPGRADE]);
    assert_eq!(
        error,
        format!("Store metadata returned HTTP 404 for product {UPGRADE} (market US)")
    );
}

#[test]
fn premium_nested_bundles_keep_real_addon_choices_and_explicit_selection() {
    let nested = "9DDDDDDDDDDD";
    let mut addon = game(UPGRADE);
    addon["ProductType"] = json!("GameAddOn");
    addon["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["ContentId"] =
        json!("22222222-2222-4222-8222-222222222222");
    let products = BTreeMap::from([
        (ROOT.into(), bundle(ROOT, vec![BASE.into(), nested.into()])),
        (BASE.into(), game(BASE)),
        (
            nested.into(),
            bundle(nested, vec![UPGRADE.into(), BASE.into()]),
        ),
        (UPGRADE.into(), addon),
    ]);
    let plan = from_products(ROOT, &products).unwrap();
    assert_eq!(plan.choices.len(), 2);
    assert!(plan.select(None).is_err());
    assert_eq!(plan.select(Some(CID)).unwrap(), CID);
    assert_eq!(
        plan.select(Some("22222222-2222-4222-8222-222222222222"))
            .unwrap(),
        "22222222-2222-4222-8222-222222222222"
    );
    assert!(
        plan.select(Some("33333333-3333-4333-8333-333333333333"))
            .is_err()
    );
}

#[test]
fn premium_bundle_depth_and_product_limits_still_fail_closed() {
    for total in [MAX_PRODUCTS, MAX_PRODUCTS + 1] {
        let ids = (1..total).map(|i| format!("9{i:011}")).collect::<Vec<_>>();
        let mut products = ids
            .iter()
            .map(|id| (id.clone(), game(id)))
            .collect::<BTreeMap<_, _>>();
        products.insert(ROOT.into(), bundle(ROOT, ids));
        assert_eq!(
            from_products(ROOT, &products).is_ok(),
            total == MAX_PRODUCTS
        );
    }
    for depth in [MAX_DEPTH, MAX_DEPTH + 1] {
        let ids = (0..=depth).map(|i| format!("9{i:011}")).collect::<Vec<_>>();
        let mut products = BTreeMap::new();
        for (i, id) in ids.iter().enumerate() {
            products.insert(
                id.clone(),
                if i == depth {
                    game(id)
                } else {
                    bundle(id, vec![ids[i + 1].clone()])
                },
            );
        }
        assert_eq!(
            from_products(&ids[0], &products).is_ok(),
            depth == MAX_DEPTH
        );
    }
}
