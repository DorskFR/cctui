//! The core journey against a real server process started with the core feature file.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cctui_proto::adapter::{AdapterCommand, AdapterEvent, EndReason, SessionMeta};
use cctui_proto::api::routes::ROUTES;
use cctui_proto::features::{Feature, FeatureSet};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const SERVER_PROCESS_ENV: &str = "CCTUI_CORE_JOURNEY_SERVER";
const CORE_FEATURES_FILE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/features/core.toml");
const ADMIN_TOKEN: &str = "journey-bootstrap-admin";
const OPERATOR: &str = "journey-operator";
const ACCOUNT: &str = "journey-claude";
const DISPATCHER: &str = "journey-dispatcher";
const ALIAS: (&str, &str) = ("fable", "claude-fable-5-2[1m]");
const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const WAIT: Duration = Duration::from_secs(15);

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The journey re-runs this binary with only this test selected to get a server process.
#[test]
fn journey_server_process() {
    if std::env::var_os(SERVER_PROCESS_ENV).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    if let Err(err) = runtime.block_on(cctui_server_workspace::run()) {
        panic!("server exited: {err:#}");
    }
}

#[test]
fn the_kubernetes_example_mounts_the_core_feature_file() {
    assert_eq!(
        include_str!("../../../deploy/core/core.toml"),
        include_str!("../../../deploy/features/core.toml"),
        "deploy/core/core.toml must be a copy of deploy/features/core.toml",
    );
    let kustomization = include_str!("../../../deploy/core/kustomization.yaml");
    assert!(kustomization.contains("core.toml"), "the ConfigMap must be generated from core.toml");
    let deployment = include_str!("../../../deploy/core/server.yaml");
    assert!(deployment.contains("value: /etc/cctui/features/core.toml"));
    assert!(!deployment.contains("PersistentVolumeClaim"));
}

#[tokio::test]
async fn core_journey() {
    let Some(url) = cctui_server_core::test_support::db_url("core_journey") else { return };
    cctui_server_core::test_support::with_isolated_database("core_journey", |pool| async move {
        let database: String =
            sqlx::query_scalar("SELECT current_database()").fetch_one(&pool).await.unwrap();
        let server = Server::start(&with_database(&url, &database)).await;
        journey(&server, &pool).await;
        server.stop().await;
    })
    .await;
}

async fn journey(server: &Server, pool: &sqlx::PgPool) {
    the_server_reports_the_core_feature_set(server).await;
    the_schema_has_no_search_objects(pool).await;
    let admin = token_login_sets_the_cookie(server, ADMIN_TOKEN).await;
    let operator = create_a_user_and_a_key(server, &admin).await;
    let (account, provider) = add_an_account(server, &operator).await;
    set_a_model_alias(server, &operator, &account, &provider).await;
    let mut dispatcher = enroll_a_dispatcher(server, &operator).await;
    let (session, machine_key) = dispatch_a_session(server, &operator, &mut dispatcher).await;
    let mut worker = connect_the_worker(server, &machine_key).await;
    stream_a_transcript(&mut worker, &session).await;
    the_session_list_and_conversation_return_it(server, &operator, &session).await;
    its_events_carry_no_search_text(pool, &session).await;
    kill_the_session(server, &operator, &mut worker, &session).await;
    logout_clears_the_cookie(server, &operator).await;
    every_route_outside_core_answers_404_or_405(server).await;
}

struct Operator {
    id: String,
    key: String,
    cookie: String,
}

enum Auth<'a> {
    Bearer(&'a str),
    Cookie(&'a str),
}

struct Server {
    child: tokio::process::Child,
    base: String,
    http: reqwest::Client,
    log: Arc<Mutex<Vec<String>>>,
    _skills: tempfile::TempDir,
}

impl Server {
    async fn start(database_url: &str) -> Self {
        cctui_server_core::client::install_crypto_provider();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let skills = tempfile::tempdir().unwrap();
        let vault_key = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "journey_server_process", "--nocapture", "--test-threads=1"])
            .env(SERVER_PROCESS_ENV, "1")
            .env("DATABASE_URL", database_url)
            .env("CCTUI_FEATURES_FILE", CORE_FEATURES_FILE)
            .env("CCTUI_ADMIN_TOKENS", ADMIN_TOKEN)
            .env("CCTUI_VAULT_KEY", vault_key)
            .env("CCTUI_EXTERNAL_URL", &base)
            .env("CCTUI_HOST", "127.0.0.1")
            .env("CCTUI_PORT", port.to_string())
            .env("CCTUI_SKILLS_PATH", skills.path())
            .env("CCTUI_UPDATE_CHECK", "0")
            .env("CCTUI_PROVIDER_STATUS", "0")
            .env("RUST_LOG", "info,sqlx=warn")
            .env("NO_COLOR", "1")
            .env_remove("TEST_DATABASE_URL")
            .env_remove("CCTUI_POD_IP")
            .env_remove("CCTUI_PLUGINS_DIR")
            .env_remove("CCTUI_DISPATCHERS")
            .env_remove("CCTUI_HTTP_DISPATCHERS")
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        collect_lines(child.stdout.take().unwrap(), &log);
        collect_lines(child.stderr.take().unwrap(), &log);
        let http = reqwest::Client::builder().no_proxy().timeout(WAIT).build().unwrap();
        let mut server = Self { child, base, http, log, _skills: skills };
        server.wait_ready().await;
        server
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + BOOT_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("server exited before it was ready ({status}):\n{}", self.log_text());
            }
            let ready = self.http.get(format!("{}/readyz", self.base)).send().await;
            if ready.is_ok_and(|r| r.status() == StatusCode::OK) {
                return;
            }
            assert!(Instant::now() < deadline, "server never became ready:\n{}", self.log_text());
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }

    fn log_text(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    fn ws_url(&self, path: &str) -> String {
        format!("{}{path}", self.base.replace("http://", "ws://"))
    }

    fn request(&self, method: Method, path: &str, auth: &Auth<'_>) -> reqwest::RequestBuilder {
        let request = self.http.request(method, format!("{}{path}", self.base));
        match auth {
            Auth::Bearer(token) => request.bearer_auth(token),
            Auth::Cookie(cookie) => request.header("cookie", *cookie).header("origin", &self.base),
        }
    }

    async fn json(&self, method: Method, path: &str, auth: &Auth<'_>, body: Option<Value>) -> Value {
        let mut request = self.request(method.clone(), path, auth);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert!(status.is_success(), "{method} {path}: {status} {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} {path}: {e}: {text}"))
    }

    async fn get(&self, path: &str, auth: &Auth<'_>) -> Value {
        self.json(Method::GET, path, auth, None).await
    }

    async fn post(&self, path: &str, auth: &Auth<'_>, body: Value) -> Value {
        self.json(Method::POST, path, auth, Some(body)).await
    }
}

fn collect_lines(stream: impl AsyncRead + Unpin + Send + 'static, log: &Arc<Mutex<Vec<String>>>) {
    let log = Arc::clone(log);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[server] {line}");
            log.lock().unwrap().push(line);
        }
    });
}

fn with_database(url: &str, database: &str) -> String {
    let (base, query) = url.split_once('?').map_or((url, None), |(b, q)| (b, Some(q)));
    let authority = base.find("://").map_or(0, |i| i + 3);
    let root = base[authority..].find('/').map_or(base, |i| &base[..authority + i]);
    query.map_or_else(|| format!("{root}/{database}"), |q| format!("{root}/{database}?{q}"))
}

async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    T: Send,
    F: FnMut() -> Fut + Send,
    Fut: std::future::Future<Output = Option<T>> + Send,
{
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = probe().await {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn next_text<T: Send>(
    ws: &mut Ws,
    what: &str,
    mut pick: impl FnMut(&str) -> Option<T> + Send,
) -> T {
    let read = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Some(found) = pick(&text) {
                        return found;
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => panic!("{what}: {err}"),
                None => panic!("{what}: socket closed"),
            }
        }
    };
    tokio::time::timeout(WAIT, read).await.unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn connect(url: String, bearer: &str) -> Ws {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert("authorization", format!("Bearer {bearer}").parse().unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn send_json(ws: &mut Ws, frame: &impl serde::Serialize) {
    ws.send(Message::Text(serde_json::to_string(frame).unwrap().into())).await.unwrap();
}

fn switchable_names() -> Vec<String> {
    let mut names: Vec<String> = Feature::ALL.iter().map(|f| f.name().to_owned()).collect();
    names.sort();
    names
}

fn sorted_strings(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out
}

async fn the_server_reports_the_core_feature_set(server: &Server) {
    let summary = format!("features: {}", FeatureSet::none());
    let response = server.http.get(format!("{}/readyz", server.base)).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), format!("ok\n{summary}\n"));
    let summary = &summary;
    eventually("the boot summary line", || async move {
        server.log_text().lines().any(|line| line.contains(summary)).then_some(())
    })
    .await;
    let capabilities = server.get("/api/v1/capabilities", &Auth::Bearer(ADMIN_TOKEN)).await;
    let switchable = switchable_names();
    assert_eq!(sorted_strings(&capabilities["switchable"]), switchable);
    let enabled = sorted_strings(&capabilities["features"]);
    assert!(enabled.iter().all(|f| !switchable.contains(f)), "core enables {enabled:?}");
}

async fn token_login_sets_the_cookie(server: &Server, token: &str) -> String {
    let refused = server
        .http
        .post(format!("{}/api/v1/auth/login", server.base))
        .json(&json!({ "token": "not-a-token" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let response = server
        .http
        .post(format!("{}/api/v1/auth/login", server.base))
        .json(&json!({ "token": token }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let set_cookie = response.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    assert_eq!(cookie, format!("cctui_auth={token}"));
    let me = server.get("/api/v1/me", &Auth::Cookie(&cookie)).await;
    assert!(me["user_id"].is_string(), "{me}");
    cookie
}

async fn create_a_user_and_a_key(server: &Server, admin_cookie: &str) -> Operator {
    let admin = Auth::Cookie(admin_cookie);
    let user = server.post("/api/v1/admin/users", &admin, json!({ "name": OPERATOR })).await;
    let id = user["id"].as_str().unwrap().to_owned();
    let minted = server
        .post(
            &format!("/api/v1/users/{id}/keys"),
            &admin,
            json!({ "label": "journey", "scopes": ["read", "enroll", "dispatch"] }),
        )
        .await;
    assert_eq!(sorted_strings(&minted["scopes"]), ["dispatch", "enroll", "read"]);
    let key = minted["key"].as_str().unwrap().to_owned();
    let cookie = token_login_sets_the_cookie(server, &key).await;
    let me = server.get("/api/v1/me", &Auth::Cookie(&cookie)).await;
    assert_eq!(me["user_id"], json!(id));
    assert_eq!(me["user_name"], json!(OPERATOR));
    Operator { id, key, cookie }
}

async fn add_an_account(server: &Server, operator: &Operator) -> (String, String) {
    let expires_at = chrono::Utc::now().timestamp() + 24 * 3600;
    let account = server
        .post(
            "/api/v1/accounts",
            &Auth::Cookie(&operator.cookie),
            json!({
                "name": ACCOUNT,
                "provider": "anthropic",
                "refresh_token": "journey-refresh-token",
                "access_token": "journey-access-token",
                "expires_at": expires_at,
            }),
        )
        .await;
    assert_eq!(account["user_id"], json!(operator.id));
    let provider = &account["providers"][0];
    assert_eq!(provider["provider"], "anthropic");
    let text = account.to_string();
    assert!(!text.contains("journey-refresh-token") && !text.contains("journey-access-token"));
    (account["id"].as_str().unwrap().to_owned(), provider["id"].as_str().unwrap().to_owned())
}

async fn set_a_model_alias(server: &Server, operator: &Operator, account: &str, provider: &str) {
    let aliases = Value::Object(std::iter::once((ALIAS.0.to_owned(), json!(ALIAS.1))).collect());
    let updated = server
        .json(
            Method::PATCH,
            &format!("/api/v1/accounts/{account}/providers/{provider}"),
            &Auth::Cookie(&operator.cookie),
            Some(json!({ "model_aliases": aliases })),
        )
        .await;
    assert_eq!(updated["model_aliases"], aliases);
}

async fn enroll_a_dispatcher(server: &Server, operator: &Operator) -> Ws {
    let cookie = &Auth::Cookie(&operator.cookie);
    let enrolled = server
        .post("/api/v1/dispatcher/enroll", cookie, json!({ "name": DISPATCHER, "kind": "kubernetes" }))
        .await;
    let key = enrolled["dispatcher_key"].as_str().unwrap();
    let mut socket = connect(server.ws_url("/api/v1/dispatcher/ws"), key).await;
    send_json(&mut socket, &json!({ "type": "hello", "kind": "kubernetes", "version": "journey" }))
        .await;
    eventually("the dispatcher to show connected", || async move {
        let rows = server.get("/api/v1/dispatchers", cookie).await;
        let row = rows.as_array()?.iter().find(|r| r["name"] == DISPATCHER)?.clone();
        assert!(row.get("dispatcher_key").is_none(), "the key is shown once, at enrollment");
        (row["connected"] == true).then_some(())
    })
    .await;
    socket
}

async fn dispatch_a_session(
    server: &Server,
    operator: &Operator,
    dispatcher: &mut Ws,
) -> (String, String) {
    let request = server
        .request(Method::POST, "/api/v1/sessions/dispatch", &Auth::Bearer(&operator.key))
        .json(&json!({
            "dispatcher": DISPATCHER,
            "account": ACCOUNT,
            "timeout": 5,
            "payload": { "model": ALIAS.0, "prompt": "say hello" },
        }));
    let reply = tokio::spawn(async move { request.send().await.unwrap() });
    let frame: Value = next_text(dispatcher, "the dispatch frame", |text| {
        serde_json::from_str::<Value>(text).ok().filter(|f| f["type"] == "dispatch")
    })
    .await;
    let spec = &frame["spec"];
    let session = spec["session_id"].as_str().unwrap().to_owned();
    assert_eq!(spec["payload"]["model"], ALIAS.1, "the alias resolves at launch");
    assert_eq!(
        spec["payload"]["env"]["ANTHROPIC_BASE_URL"],
        json!(format!("{}/gateway/anthropic", server.base)),
    );
    let machine_key = spec["payload"]["cctui_machine_key"].as_str().unwrap().to_owned();
    assert!(machine_key.starts_with("cctui_m_"), "{machine_key}");
    send_json(
        dispatcher,
        &json!({
            "type": "dispatch_result",
            "request_id": frame["request_id"],
            "session_id": session,
            "handle": "job/journey",
            "status": "dispatched",
        }),
    )
    .await;
    let response = tokio::time::timeout(WAIT, reply).await.unwrap().unwrap();
    assert!(response.status().is_success(), "dispatch answered {}", response.status());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["session_id"], json!(session));
    assert_eq!(body["handle"], "job/journey");
    (session, machine_key)
}

async fn connect_the_worker(server: &Server, machine_key: &str) -> Ws {
    let mut worker = connect(server.ws_url("/api/v1/daemon/ws"), machine_key).await;
    let mut off = next_text(&mut worker, "the features frame", |text| {
        match serde_json::from_str::<DaemonFrameDown>(text) {
            Ok(DaemonFrameDown::Features { off }) => Some(off),
            _ => None,
        }
    })
    .await;
    off.sort();
    assert_eq!(off, switchable_names());
    worker
}

async fn stream_a_transcript(worker: &mut Ws, session: &str) {
    let events = [
        AdapterEvent::SessionStarted {
            local_id: session.to_owned(),
            meta: SessionMeta { working_dir: Some("/workspace".into()), ..SessionMeta::default() },
        },
        AdapterEvent::Message {
            local_id: session.to_owned(),
            payload: json!({ "role": "user", "text": "say hello" }),
            turn_id: None,
        },
        AdapterEvent::Message {
            local_id: session.to_owned(),
            payload: json!({ "role": "assistant", "text": "journey reply", "message_id": "m1" }),
            turn_id: None,
        },
    ];
    for event in events {
        send_json(worker, &DaemonFrameUp::Event { adapter_id: "claude-code".into(), event }).await;
    }
}

async fn the_session_list_and_conversation_return_it(
    server: &Server,
    operator: &Operator,
    session: &str,
) {
    let cookie = &Auth::Cookie(&operator.cookie);
    eventually("the session in the list", || async move {
        let list = server.get("/api/v1/sessions", cookie).await;
        list["sessions"].as_array()?.iter().any(|s| s["id"] == session).then_some(())
    })
    .await;
    eventually("the transcript in the conversation", || async move {
        let rows = server.get(&format!("/api/v1/sessions/{session}/conversation"), cookie).await;
        let contents: Vec<&str> =
            rows.as_array()?.iter().filter_map(|r| r["content"].as_str()).collect();
        (contents.contains(&"▷ User: say hello") && contents.contains(&"journey reply"))
            .then_some(())
    })
    .await;
}

async fn the_schema_has_no_search_objects(pool: &sqlx::PgPool) {
    let extensions: Vec<String> = sqlx::query_scalar(
        "SELECT extname::text FROM pg_extension WHERE extname IN ('pg_trgm', 'btree_gin')",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(extensions.is_empty(), "search extensions in a core database: {extensions:?}");
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class WHERE relname IN \
         ('idx_search_chunks_session_body_trgm', 'idx_stream_events_session_search_trgm')",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(indexes.is_empty(), "trigram indexes in a core database: {indexes:?}");
}

async fn its_events_carry_no_search_text(pool: &sqlx::PgPool, session: &str) {
    let texts: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT search_text FROM stream_events \
         WHERE session_id = $1 AND event_type IN ('message', 'tool_use')",
    )
    .bind(session)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(!texts.is_empty(), "the transcript is stored");
    assert!(texts.iter().all(|t| t.as_deref() == Some("")), "{texts:?}");
}

async fn kill_the_session(server: &Server, operator: &Operator, worker: &mut Ws, session: &str) {
    let cookie = &Auth::Cookie(&operator.cookie);
    let killed = server
        .request(Method::POST, &format!("/api/v1/sessions/{session}/kill"), cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(killed.status(), StatusCode::NO_CONTENT);
    next_text(worker, "the kill command", |text| match serde_json::from_str::<DaemonFrameDown>(text) {
        Ok(DaemonFrameDown::Command { command, .. }) => {
            matches!(*command, AdapterCommand::Kill { ref local_id, .. } if local_id == session)
                .then_some(())
        }
        _ => None,
    })
    .await;
    let ended = AdapterEvent::SessionEnded { local_id: session.to_owned(), reason: EndReason::Killed };
    send_json(worker, &DaemonFrameUp::Event { adapter_id: "claude-code".into(), event: ended })
        .await;
    eventually("the session to stop", || async move {
        let row = server.get(&format!("/api/v1/sessions/{session}"), cookie).await;
        (row["status"] != "active").then_some(())
    })
    .await;
}

async fn logout_clears_the_cookie(server: &Server, operator: &Operator) {
    let response = server
        .request(Method::POST, "/api/v1/auth/logout", &Auth::Cookie(&operator.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cleared = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cleared.starts_with("cctui_auth=;"), "{cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

const OFF_OUTER_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/v1/ws/voice"),
    ("POST", "/api/v1/auth/passkey/login/start"),
    ("POST", "/api/v1/auth/passkey/login/finish"),
    ("POST", "/api/v1/auth/device/start"),
    ("POST", "/api/v1/auth/device/poll"),
    ("POST", "/api/v1/daemon/sessions/s1/spawn-child"),
    ("POST", "/api/v1/daemon/sessions/s1/message-child"),
    ("POST", "/api/v1/daemon/sessions/s1/archive-child"),
    ("POST", "/api/v1/daemon/sessions/s1/child-completion"),
    ("POST", "/api/v1/daemon/sessions/s1/room"),
    ("GET", "/api/v1/daemon/sessions/s1/peers"),
    ("POST", "/api/v1/daemon/sessions/s1/message-peer"),
    ("GET", "/api/v1/daemon/sessions/s1/peer-conversation"),
    ("POST", "/api/v1/daemon/sessions/s1/voice-notes"),
    ("POST", "/api/v1/daemon/sessions/s1/context"),
    ("GET", "/api/v1/daemon/version"),
    ("POST", "/api/v1/daemon/update-hook/r1"),
    ("POST", "/api/v1/triggers/github"),
    ("GET", "/plugins/p1/index.js"),
    ("POST", "/cctuiverse/v1/join"),
    ("POST", "/cctuiverse/v1/links/l1/messages"),
    ("GET", "/internal/preview/p1/"),
    ("GET", "/internal/preview/p1/index.html"),
];

async fn every_route_outside_core_answers_404_or_405(server: &Server) {
    let mut requests: Vec<(Method, String)> = ROUTES
        .iter()
        .filter(|route| route.feature.is_some())
        .map(|route| {
            let params: Vec<(&str, &str)> = route.params().into_iter().map(|p| (p, "j1")).collect();
            (Method::from_bytes(route.method.as_str().as_bytes()).unwrap(), route.url(&params))
        })
        .collect();
    requests.extend(
        OFF_OUTER_ROUTES
            .iter()
            .map(|(method, path)| (Method::from_bytes(method.as_bytes()).unwrap(), (*path).to_owned())),
    );
    let mut mounted = Vec::new();
    for (method, path) in requests {
        let status = server
            .request(method.clone(), &path, &Auth::Bearer(ADMIN_TOKEN))
            .send()
            .await
            .unwrap()
            .status();
        if status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED {
            mounted.push(format!("{method} {path} -> {status}"));
        }
    }
    assert!(mounted.is_empty(), "routes of off features still answer:\n{}", mounted.join("\n"));
}
