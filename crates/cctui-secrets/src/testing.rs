//! In-process doubles of every secret engine, for tests.
//!
//! One key is served by all four kinds at once, so a test can loop over
//! [`KINDS`] and rotate or break the key in one call.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use cctui_aws::{AmbientAws, AwsCredentials, FixedSource};

use crate::{AwsSecretsManager, Engines, K8sClient, RefResolver, VaultClient};

pub const KINDS: [&str; 4] = ["file", "k8s", "bao", "aws-sm"];

const SA_TOKEN: &str = "fixture-sa-token";
const VAULT_TOKEN: &str = "fixture-vault-token";
const NAMESPACE: &str = "fixture-ns";
const K8S_SECRET: &str = "fixture";
const KV_PATH: &str = "fixture";

/// `None` makes every engine fail the key as a backend error.
type Values = Arc<Mutex<HashMap<String, Option<String>>>>;

pub struct Fixture {
    values: Values,
    dir: tempfile::TempDir,
    engines: Arc<Engines>,
}

impl Fixture {
    pub async fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let values: Values = Arc::default();
        let dir = tempfile::tempdir().expect("fixture tempdir");
        let token = dir.path().join("sa-token");
        std::fs::write(&token, SA_TOKEN).expect("fixture sa token");
        let k8s = serve(k8s_router(values.clone())).await;
        let bao = serve(bao_router(values.clone())).await;
        let aws = serve(aws_router(values.clone())).await;
        let engines = Engines::new()
            .with_k8s(Some(
                K8sClient::with_base(k8s, token.clone(), NAMESPACE.to_owned()).expect("k8s client"),
            ))
            .with_vault(Some(
                VaultClient::new(bao, "fixture".to_owned(), token).expect("vault client"),
            ))
            .with_aws(aws_manager(aws));
        std::fs::create_dir(dir.path().join("files")).expect("fixture files dir");
        Self { values, dir, engines: Arc::new(engines) }
    }

    #[must_use]
    pub fn resolver(&self) -> Arc<dyn RefResolver> {
        self.engines.clone()
    }

    #[must_use]
    pub fn engines(&self) -> &Engines {
        &self.engines
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.path().join("files").join(key)
    }

    /// Serves `value` for `key` from every kind. `key` must be a plain
    /// `[A-Za-z0-9_-]` name.
    pub fn set(&self, key: &str, value: &str) {
        let path = self.file(key);
        if path.is_dir() {
            std::fs::remove_dir(&path).expect("unbreak fixture file");
        }
        std::fs::write(&path, format!("{value}\n")).expect("write fixture file");
        self.lock().insert(key.to_owned(), Some(value.to_owned()));
    }

    /// Makes every kind fail `key` as a backend error (an outage, not a
    /// deletion). `k8s` and `bao` serve all keys from one secret, so they fail
    /// every key until it is `set` again.
    pub fn break_key(&self, key: &str) {
        let path = self.file(key);
        let _ = std::fs::remove_file(&path);
        std::fs::create_dir_all(&path).expect("break fixture file");
        self.lock().insert(key.to_owned(), None);
    }

    /// The ref of `kind` (one of [`KINDS`]) that reads `key`.
    #[must_use]
    pub fn reference(&self, kind: &str, key: &str) -> String {
        match kind {
            "file" => format!("file:{}", self.file(key).display()),
            "k8s" => format!("k8s:{NAMESPACE}/{K8S_SECRET}#{key}"),
            "bao" => format!("bao:kv/data/{KV_PATH}#{key}"),
            "aws-sm" => format!("aws-sm:{key}"),
            other => panic!("unknown fixture kind {other:?}"),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<String>>> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn snapshot(values: &Values) -> HashMap<String, Option<String>> {
    values.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn broken(values: &HashMap<String, Option<String>>) -> bool {
    values.values().any(Option::is_none)
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("fixture server") });
    format!("http://{addr}")
}

fn bearer(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

fn k8s_router(values: Values) -> Router {
    Router::new().route(
        "/api/v1/namespaces/{ns}/secrets/{name}",
        get(move |Path((ns, name)): Path<(String, String)>, headers: HeaderMap| {
            let values = snapshot(&values);
            async move {
                if bearer(&headers, "authorization") != Some(format!("Bearer {SA_TOKEN}")) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if ns != NAMESPACE || name != K8S_SECRET {
                    return StatusCode::NOT_FOUND.into_response();
                }
                if broken(&values) {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                let data: serde_json::Map<String, serde_json::Value> = values
                    .into_iter()
                    .filter_map(|(k, v)| {
                        v.map(|v| (k, base64::engine::general_purpose::STANDARD.encode(v).into()))
                    })
                    .collect();
                Json(serde_json::json!({ "data": data })).into_response()
            }
        }),
    )
}

fn bao_router(values: Values) -> Router {
    Router::new()
        .route(
            "/v1/auth/kubernetes/login",
            post(|Json(body): Json<serde_json::Value>| async move {
                if body["jwt"] == SA_TOKEN {
                    Json(serde_json::json!({ "auth": { "client_token": VAULT_TOKEN } }))
                        .into_response()
                } else {
                    StatusCode::FORBIDDEN.into_response()
                }
            }),
        )
        .route(
            &format!("/v1/kv/data/{KV_PATH}"),
            get(move |headers: HeaderMap| {
                let values = snapshot(&values);
                async move {
                    if bearer(&headers, "x-vault-token").as_deref() != Some(VAULT_TOKEN) {
                        return StatusCode::FORBIDDEN.into_response();
                    }
                    if broken(&values) {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    let data: serde_json::Map<String, serde_json::Value> =
                        values.into_iter().filter_map(|(k, v)| v.map(|v| (k, v.into()))).collect();
                    Json(serde_json::json!({ "data": { "data": data } })).into_response()
                }
            }),
        )
}

fn aws_error(code: &str) -> Response {
    let body = serde_json::json!({ "__type": code, "Message": code }).to_string();
    (StatusCode::BAD_REQUEST, [("content-type", "application/x-amz-json-1.1")], body)
        .into_response()
}

fn aws_router(values: Values) -> Router {
    Router::new().route(
        "/",
        post(move |body: String| {
            let values = snapshot(&values);
            async move {
                let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let id = req["SecretId"].as_str().unwrap_or_default().to_owned();
                match values.get(&id) {
                    Some(Some(value)) => {
                        let out = serde_json::json!({ "Name": id, "SecretString": value });
                        ([("content-type", "application/x-amz-json-1.1")], out.to_string())
                            .into_response()
                    }
                    Some(None) => aws_error("InternalServiceError"),
                    None => aws_error("ResourceNotFoundException"),
                }
            }
        }),
    )
}

fn aws_manager(endpoint: String) -> AwsSecretsManager {
    let clock: cctui_aws::Clock = Arc::new(SystemTime::now);
    let ambient = AmbientAws::new(
        Box::new(FixedSource(AwsCredentials::new("AKIAFIXTURE", "fixture-secret"))),
        clock.clone(),
    );
    let env = move |k: &str| match k {
        "AWS_REGION" => Some("us-east-1".to_owned()),
        "AWS_ENDPOINT_URL" => Some(endpoint.clone()),
        _ => None,
    };
    AwsSecretsManager::new(ambient, Arc::new(env), clock).expect("aws-sm client")
}

/// Polls `check` until it holds or `timeout` passes.
pub async fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}

/// Everything logged on this thread while the capture lives, as plain text.
pub struct LogCapture {
    buf: Arc<Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Captures logs of every level on the current thread; use it with a
/// current-thread runtime so spawned tasks log into it too.
#[must_use]
pub fn capture_logs() -> LogCapture {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(CaptureWriter(buf.clone()))
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    LogCapture { buf, _guard: tracing::subscriber::set_default(subscriber) }
}

impl LogCapture {
    #[must_use]
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().unwrap_or_else(PoisonError::into_inner))
            .into_owned()
    }
}
