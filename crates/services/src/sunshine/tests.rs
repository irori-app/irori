//! The sync against a stand-in for Sunshine's web API that keeps its
//! applications as Sunshine does: sorted by name after every change.

use std::sync::Arc;

use base64::Engine as _;
use irori_depot::test_server::{Req, Resp, TestServer};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::*;

const PROGRAM: &str = "/opt/Irori/irori";
const AUTH: &str = "Basic YW5hOnNlY3JldA=="; // ana:secret

#[derive(Default)]
struct FakeSunshine {
    apps: Vec<Value>,
    covers: Vec<(String, Vec<u8>)>,
}

fn serve(fake: Arc<Mutex<FakeSunshine>>) -> TestServer {
    TestServer::start(move |req: &Req| {
        if req.header("authorization") != Some(AUTH) {
            return Resp::status(401);
        }
        let mut fake = fake.lock();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/apps") => Resp::json(json!({ "apps": fake.apps, "env": {} })),
            ("POST", "/api/apps") => {
                let mut app = body;
                let index = app["index"].as_i64().unwrap();
                app.as_object_mut().unwrap().remove("index");
                if index == -1 {
                    fake.apps.push(app);
                } else {
                    fake.apps[index as usize] = app;
                }
                fake.apps
                    .sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Resp::json(json!({ "status": true }))
            }
            ("POST", "/api/covers/upload") => {
                let key = body["key"].as_str().unwrap().to_string();
                let data = base64::engine::general_purpose::STANDARD
                    .decode(body["data"].as_str().unwrap())
                    .unwrap();
                fake.covers.push((key.clone(), data));
                Resp::json(json!({ "status": true, "path": format!("/covers/{key}.png") }))
            }
            ("DELETE", path) => {
                let index: usize = path.rsplit('/').next().unwrap().parse().unwrap();
                fake.apps.remove(index);
                Resp::json(json!({ "status": true }))
            }
            _ => Resp::status(404),
        }
    })
}

async fn client(server: &TestServer, password: &str) -> Result<Client> {
    Client::connect_at(
        server.base(),
        SunshineLogin {
            username: "ana".into(),
            password: password.into(),
        },
        None,
    )
    .await
}

fn game(id: &str, name: &str) -> Wanted {
    let (runner, app) = id.split_once(':').unwrap();
    Wanted {
        id: id.into(),
        name: name.into(),
        cmd: plan::command_line(PROGRAM, &["--launch", &format!("{runner}/{app}"), "--wait"])
            .unwrap(),
        cover: None,
    }
}

fn names(fake: &Mutex<FakeSunshine>) -> Vec<String> {
    fake.lock()
        .apps
        .iter()
        .map(|a| a["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn adds_updates_and_removes_only_irori_applications() {
    let fake = Arc::new(Mutex::new(FakeSunshine::default()));
    fake.lock().apps = vec![
        json!({"name": "Desktop", "image-path": "desktop.png"}),
        json!({"name": "Steam Big Picture", "cmd": "steam steam://open/bigpicture"}),
    ];
    let server = serve(fake.clone());
    let client = client(&server, "secret").await.unwrap();
    let mut state = SunshineState::default();

    let wanted = vec![game("gog:1", "Hades"), game("gog:2", "Celeste")];
    let report = sync(&client, &wanted, PROGRAM, &mut state).await.unwrap();
    assert_eq!((report.added, report.updated, report.removed), (2, 0, 0));
    assert_eq!(
        names(&fake),
        ["Celeste", "Desktop", "Hades", "Steam Big Picture"]
    );

    // Nothing changed, nothing to do.
    let report = sync(&client, &wanted, PROGRAM, &mut state).await.unwrap();
    assert_eq!((report.added, report.updated, report.removed), (0, 0, 0));

    // The user tunes Hades in Sunshine; a rename keeps it.
    {
        let mut fake = fake.lock();
        let hades = fake.apps.iter_mut().find(|a| a["name"] == "Hades").unwrap();
        hades["prep-cmd"] = json!([{"do": "set-resolution 1280x800", "undo": ""}]);
    }
    let wanted = vec![game("gog:1", "Hades II")];
    let report = sync(&client, &wanted, PROGRAM, &mut state).await.unwrap();
    assert_eq!((report.added, report.updated, report.removed), (0, 1, 1));
    assert_eq!(names(&fake), ["Desktop", "Hades II", "Steam Big Picture"]);
    let hades = fake.lock().apps[1].clone();
    assert_eq!(hades["prep-cmd"][0]["do"], "set-resolution 1280x800");
    assert_eq!(hades[plan::MARK], "gog:1");
    assert_eq!(hades["wait-all"], true);
    assert_eq!(hades["auto-detach"], false);

    // Turning the setting off takes Irori's out and leaves the rest.
    let report = sync(&client, &[], PROGRAM, &mut state).await.unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(names(&fake), ["Desktop", "Steam Big Picture"]);
}

#[tokio::test]
async fn covers_are_given_once_and_the_users_own_picture_stays() {
    let dir = tempfile::tempdir().unwrap();
    let art = dir.path().join("hades.jpg");
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(300, 400))
        .write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    std::fs::write(&art, jpeg).unwrap();

    let fake = Arc::new(Mutex::new(FakeSunshine::default()));
    let server = serve(fake.clone());
    let client = client(&server, "secret").await.unwrap();
    let mut state = SunshineState::default();
    let mut hades = game("gog:1", "Hades");
    hades.cover = Some(format!("file://{}", art.display()));

    sync(&client, std::slice::from_ref(&hades), PROGRAM, &mut state)
        .await
        .unwrap();
    {
        let fake = fake.lock();
        assert_eq!(fake.covers.len(), 1);
        assert_eq!(fake.covers[0].0, "irori-gog-1");
        assert_eq!(&fake.covers[0].1[1..4], b"PNG");
        assert_eq!(fake.apps[0]["image-path"], "/covers/irori-gog-1.png");
    }
    assert_eq!(state.covers["gog:1"].path, "/covers/irori-gog-1.png");

    // The user picks their own picture in Sunshine; Irori leaves it.
    fake.lock().apps[0]["image-path"] = json!("/home/ana/hades.png");
    let report = sync(&client, std::slice::from_ref(&hades), PROGRAM, &mut state)
        .await
        .unwrap();
    assert_eq!(report.updated, 0);
    assert_eq!(fake.lock().apps[0]["image-path"], "/home/ana/hades.png");
    assert_eq!(fake.lock().covers.len(), 1);
}

#[tokio::test]
async fn a_wrong_password_says_so() {
    let fake = Arc::new(Mutex::new(FakeSunshine::default()));
    let server = serve(fake);
    let Err(error) = client(&server, "wrong").await else {
        panic!("a wrong password should be refused");
    };
    assert_eq!(
        format!("{error:#}"),
        "Sunshine refused the user name or password"
    );
}

#[tokio::test]
async fn sunshine_not_running_says_so() {
    // Nothing listens on a port that was just freed.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let Err(error) = Client::connect_at(
        format!("http://127.0.0.1:{port}"),
        SunshineLogin::default(),
        None,
    )
    .await
    else {
        panic!("nothing should answer");
    };
    assert!(format!("{error:#}").starts_with("Sunshine isn't running"));
}

#[derive(Default)]
struct FakeApollo {
    apps: Vec<Value>,
    sign_ins: usize,
    next_uuid: usize,
}

const SESSION: &str = "c0ffee";

/// Apollo's web API: a session cookie from `/api/login`, applications by
/// `uuid`, kept in the order they came.
fn serve_apollo(fake: Arc<Mutex<FakeApollo>>) -> TestServer {
    TestServer::start(move |req: &Req| {
        let mut fake = fake.lock();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if (req.method.as_str(), req.path.as_str()) == ("POST", "/api/login") {
            if body["username"] == "ana" && body["password"] == "secret" {
                fake.sign_ins += 1;
                let mut resp = Resp::status(200);
                resp.headers.push((
                    "Set-Cookie".into(),
                    format!("auth={SESSION}; Secure; SameSite=Strict; Path=/"),
                ));
                return resp;
            }
            return Resp::status(401);
        }
        if req.header("cookie") != Some(&format!("auth={SESSION}")) {
            return Resp::status(401);
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/apps") => Resp::json(json!({ "apps": fake.apps, "host_uuid": "h" })),
            ("POST", "/api/apps") => {
                let mut app = body;
                assert!(app.get("index").is_none(), "Apollo names apps by uuid");
                let uuid = app["uuid"].as_str().unwrap_or_default().to_string();
                if uuid.is_empty() {
                    fake.next_uuid += 1;
                    app["uuid"] = json!(format!("uuid-{}", fake.next_uuid));
                    fake.apps.push(app);
                } else {
                    let slot = fake.apps.iter_mut().find(|a| a["uuid"] == uuid).unwrap();
                    *slot = app;
                }
                Resp::json(json!({ "status": true }))
            }
            ("POST", "/api/apps/delete") => {
                let uuid = body["uuid"].as_str().unwrap().to_string();
                fake.apps.retain(|a| a["uuid"] != uuid);
                Resp::json(json!({ "status": true }))
            }
            ("POST", "/api/covers/upload") => Resp::json(json!({
                "status": true,
                "path": format!("C:/Program Files/Apollo/config/covers/{}.png", body["key"].as_str().unwrap()),
            })),
            _ => Resp::status(404),
        }
    })
}

#[tokio::test]
async fn apollo_signs_in_once_and_changes_apps_by_uuid() {
    let fake = Arc::new(Mutex::new(FakeApollo::default()));
    fake.lock().apps = vec![json!({"name": "Desktop", "uuid": "desktop"})];
    let server = serve_apollo(fake.clone());
    let login = SunshineLogin {
        username: "ana".into(),
        password: "secret".into(),
    };
    let client = Client::connect_at(server.base(), login.clone(), None)
        .await
        .unwrap();
    assert_eq!(client.host(), StreamHost::Apollo);
    let mut state = SunshineState::default();
    let wanted = vec![game("gog:1", "Hades"), game("gog:2", "Celeste")];
    let report = sync(&client, &wanted, PROGRAM, &mut state).await.unwrap();
    assert_eq!((report.added, report.removed), (2, 0));
    assert_eq!(names_of(&fake.lock().apps), ["Desktop", "Hades", "Celeste"]);

    // The next sync reuses the session: a browser stays signed in.
    let session = client.session();
    let client = Client::connect_at(server.base(), login.clone(), session)
        .await
        .unwrap();
    let report = sync(&client, &[game("gog:1", "Hades II")], PROGRAM, &mut state)
        .await
        .unwrap();
    assert_eq!((report.updated, report.removed), (1, 1));
    assert_eq!(fake.lock().sign_ins, 1);
    let apps = fake.lock().apps.clone();
    assert_eq!(names_of(&apps), ["Desktop", "Hades II"]);
    assert_eq!(apps[1]["uuid"], "uuid-1");

    // A wrong password is said plainly.
    let wrong = SunshineLogin {
        password: "nope".into(),
        ..login
    };
    let Err(error) = Client::connect_at(server.base(), wrong, None).await else {
        panic!("a wrong password should be refused");
    };
    assert_eq!(
        format!("{error:#}"),
        "Sunshine refused the user name or password"
    );
}

fn names_of(apps: &[Value]) -> Vec<String> {
    apps.iter()
        .map(|a| a["name"].as_str().unwrap().to_string())
        .collect()
}
