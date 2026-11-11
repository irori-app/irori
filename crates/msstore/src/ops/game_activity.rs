//! Optional Xbox activity read. No interactive login, presence or statistics writes.
use super::profile::{bounded, xbox_session};
use crate::tokens::TokenManager;
use serde_json::{Value, json};
use std::time::Duration;

fn decimal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}

fn user_id(value: &Value) -> Result<String, &'static str> {
    let users = value["profileUsers"].as_array().ok_or("profile-shape")?;
    if users.len() != 1 {
        return Err("profile-count");
    }
    users[0]["id"]
        .as_str()
        .filter(|id| decimal(id))
        .map(str::to_owned)
        .ok_or("profile-id")
}

fn minutes(value: &Value, xuid: &str, title_id: u32) -> Result<Option<f64>, &'static str> {
    let mut values = Vec::new();
    let mut read_lists = |lists: &Value| -> Result<(), &'static str> {
        if lists.is_null() {
            return Ok(());
        }
        for list in lists.as_array().ok_or("stats-lists")? {
            if list["arrangebyfield"].as_str() != Some("xuid")
                || list["arrangebyfieldid"].as_str() != Some(xuid)
            {
                return Err("stats-identity");
            }
            for stat in list["stats"].as_array().ok_or("stats-shape")? {
                if stat["name"].as_str() != Some("MinutesPlayed") {
                    continue;
                }
                if stat["xuid"].as_str() != Some(xuid) {
                    return Err("stats-user");
                }
                let raw = stat["value"].as_str().ok_or("stats-value")?;
                if raw.is_empty()
                    || raw.len() > 32
                    || !raw.bytes().all(|c| c.is_ascii_digit() || c == b'.')
                {
                    return Err("stats-value");
                }
                let n = raw.parse::<f64>().map_err(|_| "stats-value")?;
                if !n.is_finite() || !(0.0..=1_000_000_000.0).contains(&n) {
                    return Err("stats-range");
                }
                values.push(n);
            }
        }
        Ok(())
    };
    if !value.is_object()
        || (value.get("statlistscollection").is_none() && value.get("groups").is_none())
    {
        return Err("stats-envelope");
    }
    read_lists(&value["statlistscollection"])?;
    if let Some(groups) = value.get("groups") {
        for group in groups.as_array().ok_or("stats-groups")? {
            if let Some(id) = group.get("titleid").or_else(|| group.get("titleId"))
                && id.as_str() != Some(title_id.to_string().as_str())
            {
                return Err("stats-title");
            }
            read_lists(&group["statlistscollection"])?;
        }
    }
    if values.iter().any(|v| Some(v) != values.first()) {
        return Err("stats-conflict");
    }
    Ok(values.first().copied())
}

fn last_played(value: &Value, xuid: &str, title_id: u32) -> Result<Option<String>, &'static str> {
    if let Some(id) = value.get("xuid")
        && id.as_str() != Some(xuid)
    {
        return Err("title-user");
    }
    let titles = value["titles"].as_array().ok_or("title-shape")?;
    if titles.len() > 1 {
        return Err("title-count");
    }
    let Some(title) = titles.first() else {
        return Ok(None);
    };
    if title["titleId"].as_str() != Some(title_id.to_string().as_str()) {
        return Err("title-id");
    }
    if title["titleHistory"]["visible"] == false {
        return Ok(None);
    }
    let date = title["titleHistory"]["lastTimePlayed"].as_str();
    Ok(date
        .filter(|s| {
            (20..=40).contains(&s.len()) && s.is_ascii() && !s.chars().any(char::is_control)
        })
        .map(str::to_owned))
}

fn history_request(
    client: &reqwest::Client,
    authorization: &str,
    xuid: &str,
    title_id: u32,
) -> reqwest::RequestBuilder {
    client.get(format!("https://titlehub.xboxlive.com/users/xuid({xuid})/titles/titleid({title_id})/decoration/TitleHistory"))
        .header("Authorization", authorization).header("x-xbl-contract-version", "2")
        // Title Hub rejects requests without a valid locale, including history reads.
        .header("Accept-Language", "en-US")
}

async fn lookup(tokens: &TokenManager, title_id: u32) -> Result<Value, &'static str> {
    let snapshot = tokens
        .user_token_snapshot()
        .map_err(|_| "account-changed")?;
    let (client, authorization, account) = xbox_session(tokens).await?;
    let response = client
        .get("https://profile.xboxlive.com/users/me/profile/settings?settings=Gamertag")
        .header("Authorization", &authorization)
        .header("x-xbl-contract-version", "2")
        .send()
        .await
        .map_err(|_| "profile-request")?;
    let bytes = bounded(response, 128 * 1024).await?;
    let xuid = user_id(&serde_json::from_slice(&bytes).map_err(|_| "profile-json")?)?;
    // This POST is the documented batch-read shape, never statswrite/presence.
    let body = json!({"arrangebyfield":"xuid", "groups":[{"name":"Hero","titleId":title_id.to_string()}],
        "stats":[{"name":"MinutesPlayed","titleId":title_id.to_string()}], "xuids":[xuid]});
    let stats = async {
        let response = client
            .post("https://userstats.xboxlive.com/batch")
            .header("Authorization", &authorization)
            .header("x-xbl-contract-version", "2")
            .json(&body)
            .send()
            .await
            .map_err(|_| "stats-request")?;
        let bytes = bounded(response, 256 * 1024).await?;
        minutes(
            &serde_json::from_slice(&bytes).map_err(|_| "stats-json")?,
            &xuid,
            title_id,
        )
    };
    let history = async {
        let response = history_request(&client, &authorization, &xuid, title_id)
            .send()
            .await
            .map_err(|_| "title-request")?;
        let bytes = bounded(response, 256 * 1024).await?;
        last_played(
            &serde_json::from_slice(&bytes).map_err(|_| "title-json")?,
            &xuid,
            title_id,
        )
    };
    let (played, history) = tokio::join!(stats, history);
    // One unavailable statistic must not hide the other. Do not invent zero.
    let (played_ok, history_ok) = (played.is_ok(), history.is_ok());
    if !played_ok && !history_ok {
        return Err("activity-unavailable");
    }
    let played = played.ok().flatten();
    let history = history.ok().flatten();
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "account-changed")?;
    Ok(
        json!({"schema":1,"account":account,"titleId":title_id.to_string(),
        "minutesPlayed":played,"lastPlayed":history,"partial":!played_ok || !history_ok}),
    )
}

/// Minutes played and last played time of an Xbox title for the player
/// (schema 1), within 25 seconds. A statistic Xbox doesn't answer is null.
pub async fn activity(tokens: &TokenManager, title_id: u32) -> Result<Value, &'static str> {
    if title_id == 0 {
        return Err("invalid-title");
    }
    tokio::time::timeout(Duration::from_secs(25), lookup(tokens, title_id))
        .await
        .unwrap_or(Err("activity-timeout"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn list(value: Value) -> Value {
        json!({"arrangebyfield":"xuid","arrangebyfieldid":"123","stats":[{"name":"MinutesPlayed","xuid":"123","value":value}]})
    }
    #[test]
    fn activity_reads_root_and_hero_lists_without_double_counting() {
        let root = json!({"statlistscollection":[list(json!("2052"))]});
        assert_eq!(minutes(&root, "123", 42).unwrap(), Some(2052.0));
        let grouped = json!({"groups":[{"titleid":"42","statlistscollection":[list(json!("2052"))]}],"statlistscollection":[list(json!("2052"))]});
        assert_eq!(minutes(&grouped, "123", 42).unwrap(), Some(2052.0));
    }
    #[test]
    fn activity_distinguishes_missing_from_real_zero_and_fractions() {
        assert_eq!(
            minutes(&json!({"statlistscollection":[]}), "123", 42).unwrap(),
            None
        );
        for (raw, n) in [("0", 0.0), ("0.5", 0.5)] {
            assert_eq!(
                minutes(
                    &json!({"statlistscollection":[list(json!(raw))]}),
                    "123",
                    42
                )
                .unwrap(),
                Some(n)
            );
        }
    }
    #[test]
    fn activity_rejects_wrong_users_titles_conflicts_and_unsafe_values() {
        for raw in [
            json!("-1"),
            json!("NaN"),
            json!("1e5"),
            json!("1000000001"),
            json!("1\n"),
            json!(""),
            json!(42),
        ] {
            assert!(minutes(&json!({"statlistscollection":[list(raw)]}), "123", 42).is_err());
        }
        assert!(
            minutes(
                &json!({"statlistscollection":[list(json!("1")),list(json!("2"))]}),
                "123",
                42
            )
            .is_err()
        );
        assert!(
            minutes(
                &json!({"statlistscollection":[list(json!("1"))]}),
                "999",
                42
            )
            .is_err()
        );
        assert!(
            minutes(
                &json!({"groups":[{"titleid":"43","statlistscollection":[]}]}),
                "123",
                42
            )
            .is_err()
        );
        assert!(minutes(&json!({}), "123", 42).is_err());
    }
    #[test]
    fn activity_binds_history_and_profile_to_single_identity() {
        assert_eq!(
            user_id(&json!({"profileUsers":[{"id":"123"}]})).unwrap(),
            "123"
        );
        for v in [
            json!({"profileUsers":[]}),
            json!({"profileUsers":[{"id":"a"}]}),
            json!({"profileUsers":[{"id":"123"},{"id":"456"}]}),
        ] {
            assert!(user_id(&v).is_err());
        }
        let v = json!({"xuid":"123","titles":[{"titleId":"42","titleHistory":{"lastTimePlayed":"2026-10-19T10:00:00Z","visible":true}}]});
        assert!(last_played(&v, "123", 42).unwrap().is_some());
        assert!(last_played(&v, "456", 42).is_err());
        assert!(last_played(&v, "123", 43).is_err());
    }

    #[tokio::test]
    async fn activity_history_reads_date_from_locale_required_service() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            let valid = request.lines().any(|line| line == "accept-language: en-us");
            let history = request
                .starts_with("get /users/xuid(123)/titles/titleid(42)/decoration/titlehistory ");
            let (status, body) = if valid && history {
                (
                    "200 OK",
                    r#"{"xuid":"123","titles":[{"titleId":"42","titleHistory":{"lastTimePlayed":"2026-10-19T10:00:00.123456700Z","visible":true}}]}"#,
                )
            } else if valid {
                (
                    "200 OK",
                    r#"{"xuid":"123","titles":[{"titleId":"42","titleHistory":null}]}"#,
                )
            } else {
                (
                    "400 Bad Request",
                    r#"["Request does not contain the Accept-Language header, with valid locale value"]"#,
                )
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let mut request = history_request(&client, "synthetic-test-authorization", "123", 42)
            .build()
            .unwrap();
        // Redirect only the test's transport, retaining the production request headers and path.
        *request.url_mut() =
            reqwest::Url::parse(&format!("http://{address}{}", request.url().path())).unwrap();
        let response = client.execute(request).await.unwrap();
        let bytes = bounded(response, 256 * 1024).await;
        worker.join().unwrap();
        let value: Value =
            serde_json::from_slice(&bytes.expect("history service rejected the request")).unwrap();
        assert_eq!(
            last_played(&value, "123", 42).unwrap().as_deref(),
            Some("2026-10-19T10:00:00.123456700Z")
        );
    }
}
