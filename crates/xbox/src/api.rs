//! [`Client`]: the token chain (Microsoft account → Xbox Live user token →
//! XSTS) and the data services the XSTS token opens.

use std::fmt;
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::auth::{Pkce, REDIRECT_URL, SCOPES};
use crate::model::{self, Achievement, AchievementPage, Title, TitleHub};

/// Server base addresses (overridable for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Microsoft account sign-in: `oauth20_authorize.srf`, `oauth20_token.srf`.
    pub login: String,
    pub user_auth: String,
    pub xsts: String,
    pub titlehub: String,
    pub userstats: String,
    pub achievements: String,
    /// The people the account follows.
    pub social: String,
    /// Gamertags and pictures.
    pub profile: String,
    /// Online state and what is being played.
    pub presence: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            login: "https://login.live.com".into(),
            user_auth: "https://user.auth.xboxlive.com".into(),
            xsts: "https://xsts.auth.xboxlive.com".into(),
            titlehub: "https://titlehub.xboxlive.com".into(),
            userstats: "https://userstats.xboxlive.com".into(),
            achievements: "https://achievements.xboxlive.com".into(),
            social: "https://social.xboxlive.com".into(),
            profile: "https://profile.xboxlive.com".into(),
            presence: "https://userpresence.xboxlive.com".into(),
        }
    }
}

impl Endpoints {
    /// Every service at one address (a test server).
    pub fn all_at(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self {
            login: base.clone(),
            user_auth: base.clone(),
            xsts: base.clone(),
            titlehub: base.clone(),
            userstats: base.clone(),
            achievements: base.clone(),
            social: base.clone(),
            profile: base.clone(),
            presence: base,
        }
    }
}

/// The Xbox network refused something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum XboxError {
    /// Microsoft refused the code or the saved refresh token: only signing
    /// in again fixes it.
    #[error("Microsoft refused the saved sign-in: {0}")]
    SignInRefused(String),
    /// The account can't use Xbox services (no Xbox profile yet, a child
    /// account outside a family, a country without Xbox).
    #[error("{0}")]
    NoXboxAccess(String),
    /// The XSTS token ran out or was refused; a new one fixes it.
    #[error("The Xbox sign-in ran out")]
    Unauthorized,
    #[error("The Xbox network is busy; try again in a minute")]
    TooManyRequests,
    #[error("The Xbox network answered with HTTP status {0}")]
    Status(u16),
}

/// What the Microsoft account's token endpoint gives. `Debug` leaves the
/// tokens out.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Microsoft may send a new refresh token with every answer; keep the
    /// latest.
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// An XSTS token for `http://xboxlive.com`, with the account it names.
/// `Debug` leaves the token out.
#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub user_hash: String,
    pub token: String,
    pub xuid: String,
    pub gamertag: String,
    /// Unix seconds.
    pub not_after: i64,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("xuid", &self.xuid)
            .field("gamertag", &self.gamertag)
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// The `Authorization` header value Xbox services take.
    pub fn authorization(&self) -> String {
        format!("XBL3.0 x={};{}", self.user_hash, self.token)
    }

    /// Whether it still has five minutes to go at `now` (Unix seconds).
    pub fn is_valid_at(&self, now: i64) -> bool {
        self.not_after - 300 > now
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XToken {
    token: String,
    #[serde(default)]
    not_after: Option<String>,
    #[serde(default)]
    display_claims: Option<DisplayClaims>,
}

#[derive(Deserialize, Default)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<UserClaims>,
}

#[derive(Deserialize, Default)]
struct UserClaims {
    #[serde(default)]
    uhs: String,
    #[serde(default)]
    xid: Option<String>,
    #[serde(default)]
    gtg: Option<String>,
}

#[derive(Deserialize, Default)]
struct OAuthError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize, Default)]
struct XstsError {
    #[serde(rename = "XErr", default)]
    xerr: Option<serde_json::Value>,
}

/// The XSTS refusals that mean the account can't use Xbox services,
/// as the Minecraft launchers' community documentation of this flow lists
/// them.
fn xsts_refusal(code: u64) -> Option<&'static str> {
    Some(match code {
        2_148_916_233 => {
            "This Microsoft account has no Xbox profile yet; sign in once at xbox.com to create one"
        }
        2_148_916_235 => "Xbox services aren't available in this account's country",
        2_148_916_236 | 2_148_916_237 => {
            "This account has to be verified as an adult's at xbox.com before it can sign in"
        }
        2_148_916_238 => {
            "This is a child's account: an adult has to add it to a Microsoft family first"
        }
        _ => return None,
    })
}

fn unix_seconds(rfc3339: &str) -> Option<i64> {
    time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// An Xbox network client for one Microsoft (Azure) app. Holds no tokens;
/// cheap to clone.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    client_id: String,
    pub(crate) endpoints: Endpoints,
}

impl Client {
    pub fn new(client_id: &str) -> Result<Self> {
        Self::with_endpoints(client_id, Endpoints::default())
    }

    /// A client for other servers (a test server).
    pub fn with_endpoints(client_id: &str, endpoints: Endpoints) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Irori/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .context("Could not set up the connection to the Xbox network")?;
        Ok(Self {
            http,
            client_id: client_id.trim().to_string(),
            endpoints,
        })
    }

    /// The Microsoft sign-in page for `pkce`; it ends on [`REDIRECT_URL`]
    /// with the code.
    pub fn authorize_url(&self, pkce: &Pkce) -> String {
        let base = format!("{}/oauth20_authorize.srf", self.endpoints.login);
        url::Url::parse_with_params(
            &base,
            &[
                ("client_id", self.client_id.as_str()),
                ("response_type", "code"),
                ("redirect_uri", REDIRECT_URL),
                ("scope", SCOPES),
                ("code_challenge", pkce.challenge.as_str()),
                ("code_challenge_method", "S256"),
            ],
        )
        .map(String::from)
        .unwrap_or(base)
    }

    async fn token(&self, form: &[(&str, &str)]) -> Result<Tokens> {
        let mut form = form.to_vec();
        form.push(("client_id", self.client_id.as_str()));
        form.push(("scope", SCOPES));
        let resp = self
            .http
            .post(format!("{}/oauth20_token.srf", self.endpoints.login))
            .form(&form)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach the Microsoft sign-in")?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("The Microsoft sign-in's answer was cut off")?;
        tracing::debug!(status, "microsoft token call");
        match status {
            200..=299 => serde_json::from_slice(&body)
                .context("The Microsoft sign-in sent an unexpected answer"),
            400 | 401 => {
                let e: OAuthError = serde_json::from_slice(&body).unwrap_or_default();
                let why = e.error_description.filter(|d| !d.is_empty()).unwrap_or(
                    if e.error.is_empty() {
                        format!("HTTP status {status}")
                    } else {
                        e.error
                    },
                );
                Err(XboxError::SignInRefused(why).into())
            }
            429 => Err(XboxError::TooManyRequests.into()),
            s => Err(XboxError::Status(s).into()),
        }
    }

    /// Turn the code from the sign-in page into tokens.
    pub async fn redeem_code(&self, code: &str, pkce: &Pkce) -> Result<Tokens> {
        self.token(&[
            ("grant_type", "authorization_code"),
            ("code", code.trim()),
            ("redirect_uri", REDIRECT_URL),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .await
    }

    /// New tokens from a saved refresh token. A refused one fails with
    /// [`XboxError::SignInRefused`].
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        self.token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("redirect_uri", REDIRECT_URL),
        ])
        .await
    }

    async fn x_token(&self, url: String, body: serde_json::Value) -> Result<XToken> {
        let resp = self
            .http
            .post(url)
            .header("x-xbl-contract-version", "1")
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Could not reach the Xbox sign-in")?;
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("The Xbox sign-in's answer was cut off")?;
        tracing::debug!(status, "xbox token call");
        match status {
            200..=299 => {
                serde_json::from_slice(&bytes).context("The Xbox sign-in sent an unexpected answer")
            }
            401 | 403 => {
                let e: XstsError = serde_json::from_slice(&bytes).unwrap_or_default();
                let code = e.xerr.and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                });
                match code.and_then(xsts_refusal) {
                    Some(why) => Err(XboxError::NoXboxAccess(why.into()).into()),
                    None => Err(XboxError::SignInRefused(format!(
                        "the Xbox sign-in answered with HTTP status {status}"
                    ))
                    .into()),
                }
            }
            429 => Err(XboxError::TooManyRequests.into()),
            s => Err(XboxError::Status(s).into()),
        }
    }

    /// The Xbox Live user token and XSTS token for a Microsoft access token.
    pub async fn session(&self, access_token: &str) -> Result<Session> {
        let user = self
            .x_token(
                format!("{}/user/authenticate", self.endpoints.user_auth),
                serde_json::json!({
                    "RelyingParty": "http://auth.xboxlive.com",
                    "TokenType": "JWT",
                    "Properties": {
                        "AuthMethod": "RPS",
                        "SiteName": "user.auth.xboxlive.com",
                        "RpsTicket": format!("d={access_token}"),
                    },
                }),
            )
            .await?;
        let xsts = self
            .x_token(
                format!("{}/xsts/authorize", self.endpoints.xsts),
                serde_json::json!({
                    "RelyingParty": "http://xboxlive.com",
                    "TokenType": "JWT",
                    "Properties": {
                        "UserTokens": [user.token],
                        "SandboxId": "RETAIL",
                    },
                }),
            )
            .await?;
        let claims = xsts
            .display_claims
            .unwrap_or_default()
            .xui
            .into_iter()
            .next()
            .unwrap_or_default();
        let xuid = claims.xid.unwrap_or_default();
        anyhow::ensure!(
            !claims.uhs.is_empty() && !xuid.is_empty(),
            "The Xbox sign-in didn't name the account"
        );
        Ok(Session {
            user_hash: claims.uhs,
            token: xsts.token,
            xuid,
            gamertag: claims.gtg.unwrap_or_default(),
            not_after: xsts
                .not_after
                .as_deref()
                .and_then(unix_seconds)
                .unwrap_or_else(|| now_unix() + 3600),
        })
    }

    async fn data<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        session: &Session,
        language: &str,
        what: &str,
    ) -> Result<T> {
        self.data_with_contract(request, session, language, what, "2")
            .await
    }

    /// [`data`](Self::data) for a service that takes another contract
    /// version.
    pub(crate) async fn data_with_contract<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        session: &Session,
        language: &str,
        what: &str,
        contract: &str,
    ) -> Result<T> {
        let resp = request
            .header("Authorization", session.authorization())
            .header("x-xbl-contract-version", contract)
            .header("Accept-Language", language)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("Could not reach the Xbox network for {what}"))?;
        let status = resp.status().as_u16();
        tracing::debug!(status, what, "xbox data call");
        match status {
            200..=299 => {}
            401 => return Err(XboxError::Unauthorized.into()),
            429 => return Err(XboxError::TooManyRequests.into()),
            s => return Err(XboxError::Status(s).into()),
        }
        let body = resp
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("The Xbox network's answer for {what} was cut off"))?;
        serde_json::from_slice(&body)
            .with_context(|| format!("The Xbox network sent an unexpected answer for {what}"))
    }

    /// Every title the account played, most recent first, with achievement
    /// counts, images, store details and when it was last played.
    pub async fn title_history(&self, session: &Session, language: &str) -> Result<Vec<Title>> {
        let url = format!(
            "{}/users/xuid({})/titles/titlehistory/decoration/achievement,image,detail,productId",
            self.endpoints.titlehub, session.xuid
        );
        let hub: TitleHub = self
            .data(
                self.http.get(url).query(&[("maxItems", "1000")]),
                session,
                language,
                "the title history",
            )
            .await?;
        Ok(hub.titles)
    }

    /// The account's minutes played of a title, when the title keeps them.
    pub async fn minutes_played(&self, session: &Session, title_id: &str) -> Result<Option<u64>> {
        let body = serde_json::json!({
            "arrangebyfield": "xuid",
            "xuids": [session.xuid],
            "groups": [{"name": "Hero", "titleId": title_id}],
            "stats": [{"name": "MinutesPlayed", "titleId": title_id}],
        });
        let answer: serde_json::Value = self
            .data(
                self.http
                    .post(format!("{}/batch", self.endpoints.userstats))
                    .json(&body),
                session,
                "en-US",
                "minutes played",
            )
            .await?;
        Ok(model::minutes_played(&answer))
    }

    /// A title's achievements with the account's progress, every page.
    /// `title_id` may name several titles, comma-separated, as the service
    /// accepts.
    pub async fn achievements(
        &self,
        session: &Session,
        title_id: &str,
        language: &str,
    ) -> Result<Vec<Achievement>> {
        let url = format!(
            "{}/users/xuid({})/achievements",
            self.endpoints.achievements, session.xuid
        );
        let mut all = Vec::new();
        let mut continuation: Option<String> = None;
        // A title has at most a few hundred achievements; the bound only
        // stops a server that keeps sending the same token.
        for _ in 0..20 {
            let mut query = vec![
                ("titleId", title_id.to_string()),
                ("maxItems", "1000".to_string()),
            ];
            if let Some(token) = &continuation {
                query.push(("continuationToken", token.clone()));
            }
            let page: AchievementPage = self
                .data(
                    self.http.get(&url).query(&query),
                    session,
                    language,
                    "achievements",
                )
                .await?;
            all.extend(page.achievements);
            continuation = page
                .paging_info
                .and_then(|p| p.continuation_token)
                .filter(|t| !t.is_empty());
            if continuation.is_none() {
                break;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Req, Resp, TestServer};

    fn form(req: &Req) -> Vec<(String, String)> {
        url::form_urlencoded::parse(&req.body)
            .into_owned()
            .collect()
    }

    fn field<'a>(f: &'a [(String, String)], k: &str) -> Option<&'a str> {
        f.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    fn json(req: &Req) -> serde_json::Value {
        serde_json::from_slice(&req.body).unwrap()
    }

    /// The whole chain, with the shapes the services answer in (redacted).
    fn server() -> TestServer {
        TestServer::start(|req| {
            let path = req.path.split('?').next().unwrap_or_default();
            match (req.method.as_str(), path) {
                ("POST", "/oauth20_token.srf") => {
                    let f = form(req);
                    let ok = match field(&f, "grant_type") {
                        Some("authorization_code") => {
                            field(&f, "code") == Some("good-code")
                                && field(&f, "code_verifier").is_some_and(|v| v.len() == 64)
                        }
                        Some("refresh_token") => field(&f, "refresh_token") == Some("refresh-1"),
                        _ => false,
                    };
                    if !ok || field(&f, "client_id") != Some("client-1") {
                        return Resp {
                            status: 400,
                            body: br#"{"error":"invalid_grant","error_description":"The token has expired."}"#.to_vec(),
                            ..Default::default()
                        };
                    }
                    Resp::json(serde_json::json!({
                        "token_type": "bearer", "expires_in": 3600, "scope": SCOPES,
                        "access_token": "access-1", "refresh_token": "refresh-2", "user_id": "u"
                    }))
                }
                ("POST", "/user/authenticate") => {
                    let b = json(req);
                    assert_eq!(req.header("x-xbl-contract-version"), Some("1"));
                    assert_eq!(b["Properties"]["RpsTicket"], "d=access-1");
                    assert_eq!(b["RelyingParty"], "http://auth.xboxlive.com");
                    Resp::json(serde_json::json!({
                        "IssueInstant": "2026-11-01T00:00:00.0000000Z",
                        "NotAfter": "2026-11-15T00:00:00.0000000Z",
                        "Token": "user-token",
                        "DisplayClaims": {"xui": [{"uhs": "hash-1"}]}
                    }))
                }
                ("POST", "/xsts/authorize") => {
                    let b = json(req);
                    assert_eq!(b["RelyingParty"], "http://xboxlive.com");
                    assert_eq!(b["Properties"]["UserTokens"][0], "user-token");
                    assert_eq!(b["Properties"]["SandboxId"], "RETAIL");
                    Resp::json(serde_json::json!({
                        "IssueInstant": "2026-11-01T00:00:00.0000000Z",
                        "NotAfter": "2099-11-01T16:00:00.1234567Z",
                        "Token": "xsts-token",
                        "DisplayClaims": {"xui": [{"gtg": "Pilot", "xid": "2500000000000001", "uhs": "hash-1"}]}
                    }))
                }
                (
                    "GET",
                    "/users/xuid(2500000000000001)/titles/titlehistory/decoration/achievement,image,detail,productId",
                ) => {
                    assert_eq!(
                        req.header("Authorization"),
                        Some("XBL3.0 x=hash-1;xsts-token")
                    );
                    assert_eq!(req.header("x-xbl-contract-version"), Some("2"));
                    assert_eq!(req.header("Accept-Language"), Some("de-DE"));
                    Resp::json(serde_json::json!({"xuid": "2500000000000001", "titles": [
                        {"titleId": "1700000001", "pfn": "Studio.SkyRacer_abcdefghjkmnp", "name": "Sky Racer",
                         "type": "Game", "devices": ["PC"]}
                    ]}))
                }
                ("POST", "/batch") => {
                    let b = json(req);
                    assert_eq!(b["stats"][0]["name"], "MinutesPlayed");
                    assert_eq!(b["xuids"][0], "2500000000000001");
                    Resp::json(serde_json::json!({"groups": [], "statlistscollection": [
                        {"arrangebyfield": "xuid", "arrangebyfieldid": "2500000000000001", "stats": [
                            {"name": "MinutesPlayed", "type": "Integer", "value": "95",
                             "titleid": b["stats"][0]["titleId"]}]}]}))
                }
                ("GET", "/users/xuid(2500000000000001)/achievements") => {
                    let next = req.path.contains("continuationToken=page2");
                    assert!(req.path.contains("titleId=1700000001"));
                    Resp::json(serde_json::json!({
                        "achievements": [{"id": if next { "2" } else { "1" }, "name": "A",
                            "progressState": "NotStarted"}],
                        "pagingInfo": {"continuationToken": if next { serde_json::Value::Null } else { "page2".into() }}
                    }))
                }
                _ => Resp::status(404),
            }
        })
    }

    #[tokio::test]
    async fn signs_in_and_reads_the_account() {
        let s = server();
        let client = Client::with_endpoints("client-1", Endpoints::all_at(&s.base())).unwrap();
        let pkce = Pkce::new();
        let url = client.authorize_url(&pkce);
        assert!(url.starts_with(&format!("{}/oauth20_authorize.srf?", s.base())));
        let parsed = url::Url::parse(&url).unwrap();
        let q: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
        assert_eq!(field(&q, "client_id"), Some("client-1"));
        assert_eq!(field(&q, "redirect_uri"), Some(REDIRECT_URL));
        assert_eq!(field(&q, "scope"), Some(SCOPES));
        assert_eq!(field(&q, "code_challenge"), Some(pkce.challenge.as_str()));
        assert_eq!(field(&q, "code_challenge_method"), Some("S256"));

        let tokens = client.redeem_code("good-code", &pkce).await.unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-2"));
        assert!(!format!("{tokens:?}").contains("access-1"));
        let session = client.session(&tokens.access_token).await.unwrap();
        assert_eq!(session.xuid, "2500000000000001");
        assert_eq!(session.gamertag, "Pilot");
        assert!(session.is_valid_at(now_unix()));
        assert!(!format!("{session:?}").contains("xsts-token"));

        let titles = client.title_history(&session, "de-DE").await.unwrap();
        assert_eq!(titles.len(), 1);
        assert!(titles[0].is_pc_game());
        let minutes = client.minutes_played(&session, "1700000001").await.unwrap();
        assert_eq!(minutes, Some(95));
        let achievements = client
            .achievements(&session, "1700000001", "en-US")
            .await
            .unwrap();
        let ids: Vec<&str> = achievements.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["1", "2"]);
    }

    #[tokio::test]
    async fn a_refused_refresh_says_sign_in_again() {
        let s = server();
        let client = Client::with_endpoints("client-1", Endpoints::all_at(&s.base())).unwrap();
        assert_eq!(
            client.refresh("refresh-1").await.unwrap().access_token,
            "access-1"
        );
        let refused = client.refresh("revoked").await.unwrap_err();
        assert_eq!(
            refused.downcast_ref::<XboxError>(),
            Some(&XboxError::SignInRefused("The token has expired.".into()))
        );
        assert!(!format!("{refused:#}").contains("revoked"));
    }

    #[tokio::test]
    async fn explains_accounts_without_xbox_access() {
        let s = TestServer::start(|req| {
            match req.path.as_str() {
            "/user/authenticate" => Resp::json(serde_json::json!({
                "Token": "user-token", "DisplayClaims": {"xui": [{"uhs": "h"}]}
            })),
            _ => Resp {
                status: 401,
                body: br#"{"Identity":"0","XErr":2148916238,"Message":"","Redirect":"https://start.ui.xboxlive.com/AddChildToFamily"}"#.to_vec(),
                ..Default::default()
            },
        }
        });
        let client = Client::with_endpoints("client-1", Endpoints::all_at(&s.base())).unwrap();
        let e = client.session("access").await.unwrap_err();
        assert!(matches!(
            e.downcast_ref::<XboxError>(),
            Some(XboxError::NoXboxAccess(why)) if why.contains("child")
        ));
    }

    #[tokio::test]
    async fn an_expired_xsts_token_reads_as_unauthorized() {
        let s = TestServer::start(|_| Resp::status(401));
        let client = Client::with_endpoints("c", Endpoints::all_at(&s.base())).unwrap();
        let session = Session {
            user_hash: "h".into(),
            token: "t".into(),
            xuid: "1".into(),
            gamertag: String::new(),
            not_after: 0,
        };
        assert!(!session.is_valid_at(now_unix()));
        let e = client.title_history(&session, "en-US").await.unwrap_err();
        assert_eq!(
            e.downcast_ref::<XboxError>(),
            Some(&XboxError::Unauthorized)
        );
    }
}
