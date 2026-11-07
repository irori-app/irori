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

fn client(server: &TestServer, password: &str) -> Client {
    Client::at(
        server.base(),
        SunshineLogin {
            username: "ana".into(),
            password: password.into(),
        },
    )
    .unwrap()
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
    let client = client(&server, "secret");
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
    let client = client(&server, "secret");
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
    let client = client(&server, "wrong");
    let mut state = SunshineState::default();
    let error = sync(&client, &[game("gog:1", "Hades")], PROGRAM, &mut state)
        .await
        .unwrap_err();
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
    let client = Client::at(format!("http://127.0.0.1:{port}"), SunshineLogin::default()).unwrap();
    let error = client.apps().await.unwrap_err();
    assert!(format!("{error:#}").starts_with("Sunshine isn't running"));
}
