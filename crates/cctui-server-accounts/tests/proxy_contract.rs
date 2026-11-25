//! The gateway enforces execution policy without tracing, voice or workspace storage.
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use cctui_server_accounts::account_resolve::AccountResolutionContext;
use cctui_server_accounts::gateway::Family;
use cctui_server_accounts::gateway::credentials::{Account, CredentialContext};
use cctui_server_accounts::gateway::failover::FailoverContext;
use cctui_server_accounts::gateway::limits::{LimitContext, OrphanSpamMap};
use cctui_server_accounts::gateway::mint::MintContext;
use cctui_server_accounts::gateway::proxy::{
    ProxyContext, ResponseTee, TraceCapture, TraceSink, passthrough,
};
use cctui_server_accounts::gateway::ratelimit::RateWindow;
use cctui_server_accounts::gateway::usage::{CachedUsage, UsageContext};
use cctui_server_accounts::provider_status::ProviderStatusCache;
use cctui_server_kernel::bus::{Bus, NoopTransport};
use dashmap::DashMap;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
struct Host(Arc<Resources>);
struct Resources {
    pool: PgPool,
    client: reqwest::Client,
    bus: Bus,
    locks: DashMap<Uuid, Arc<Mutex<()>>>,
    reauth: DashMap<Uuid, ()>,
    usage: DashMap<Uuid, CachedUsage>,
    budgets: DashMap<String, f64>,
    orphans: OrphanSpamMap,
    windows: Arc<DashMap<Uuid, RateWindow>>,
    status: ProviderStatusCache,
    notices: AtomicUsize,
    refuse_upstream: bool,
    tracer: std::sync::Mutex<Option<Arc<dyn TraceSink>>>,
    scrub: std::sync::atomic::AtomicBool,
}
impl CredentialContext for Host {
    fn pool(&self) -> &PgPool {
        &self.0.pool
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.0.client
    }
    fn account_locks(&self) -> &DashMap<Uuid, Arc<Mutex<()>>> {
        &self.0.locks
    }
    fn account_reauth(&self) -> &DashMap<Uuid, ()> {
        &self.0.reauth
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
}
impl AccountResolutionContext for Host {
    fn pool(&self) -> &PgPool {
        &self.0.pool
    }
    fn cached_needs_reauth(&self, id: Uuid) -> bool {
        self.0.reauth.contains_key(&id)
    }
    async fn usage_for_soft_limit(&self, _: Uuid) -> Option<Value> {
        None
    }
}
impl MintContext for Host {
    fn pool(&self) -> &PgPool {
        &self.0.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
    fn external_url(&self) -> &'static str {
        "https://automation.example.invalid"
    }
    fn clear_orphan_fingerprint(&self, fp: &str) {
        self.0.orphans.remove(fp);
    }
}
impl UsageContext for Host {
    fn usage_cache(&self) -> &DashMap<Uuid, CachedUsage> {
        &self.0.usage
    }
    fn probe_client(&self) -> &reqwest::Client {
        &self.0.client
    }
    fn probe_url_permitted(&self, _: &str) -> Result<(), String> {
        Err("no probes".into())
    }
    async fn usage_updated(&self, _: Uuid, _: String, _: Option<&Value>) {}
}
impl LimitContext for Host {
    fn limit_bus(&self) -> &Bus {
        &self.0.bus
    }
    fn session_usd_budgets(&self) -> &DashMap<String, f64> {
        &self.0.budgets
    }
    fn orphan_spam(&self) -> &OrphanSpamMap {
        &self.0.orphans
    }
}
impl FailoverContext for Host {
    fn provider_status(&self) -> &ProviderStatusCache {
        &self.0.status
    }
    async fn clear_failover_signals(&self, sid: &str) {
        cctui_server_accounts::gateway::limits::clear_orphan_block_for_session(self, sid).await;
        cctui_server_accounts::gateway::limits::clear_soft_limit_block(self, sid).await;
    }
}
impl ProxyContext for Host {
    fn rate_windows(&self) -> &Arc<DashMap<Uuid, RateWindow>> {
        &self.0.windows
    }
    fn upstream_url_permitted(
        &self,
        _: &str,
    ) -> Result<(), cctui_server_kernel::outbound::OutboundUrlError> {
        if self.0.refuse_upstream {
            Err(cctui_server_kernel::outbound::OutboundUrlError::Internal)
        } else {
            Ok(())
        }
    }
    fn upstream_client(&self) -> &reqwest::Client {
        &self.0.client
    }
    fn trace_sink(&self) -> Option<Arc<dyn TraceSink>> {
        self.0.tracer.lock().unwrap().clone()
    }
    fn any_voice_watch(&self) -> bool {
        false
    }
    fn watched_session(&self, _: Option<String>) -> Option<String> {
        None
    }
    fn voice_from_request(&self, _: String, _: &Value) -> Option<Box<dyn ResponseTee>> {
        panic!("no voice observer")
    }
    async fn deliver_usage_notice(&self, _: &Account, _: &str) {
        self.0.notices.fetch_add(1, Ordering::SeqCst);
    }
    async fn outbound_scrub(
        &self,
        _: &str,
    ) -> Result<Option<Arc<cctui_crypto::redact::CompiledPatterns>>, sqlx::Error> {
        Ok(self
            .0
            .scrub
            .load(Ordering::SeqCst)
            .then(|| Arc::new(cctui_crypto::redact::compile(true, &[], b"contract-key"))))
    }
}
struct Fixture {
    host: Host,
    provider: Uuid,
    account: Uuid,
    token: String,
    calls: Arc<AtomicUsize>,
    received: tokio::sync::mpsc::UnboundedReceiver<(HeaderMap, Bytes)>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    // Keep the execution-only schema and deterministic upstream fixture together.
    #[allow(clippy::too_many_lines)]
    async fn new(
        status: StatusCode,
        response: &'static str,
        refuse_upstream: bool,
    ) -> Option<Self> {
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.trim().is_empty());
        let Some(url) = url else {
            assert!(std::env::var_os("CI").is_none(), "proxy contract requires a database");
            eprintln!("skipping proxy contract: no database");
            return None;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for ddl in [
            "CREATE TEMP TABLE users (LIKE public.users INCLUDING ALL)",
            "CREATE TEMP TABLE machines (LIKE public.machines INCLUDING ALL)",
            "CREATE TEMP TABLE accounts (LIKE public.accounts INCLUDING ALL)",
            "CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING ALL)",
            "CREATE TEMP TABLE session_tokens (LIKE public.session_tokens INCLUDING ALL)",
            "CREATE TEMP TABLE sessions (LIKE public.sessions INCLUDING ALL)",
            "CREATE TEMP TABLE resource_shares (LIKE public.resource_shares INCLUDING ALL)",
            "CREATE TEMP TABLE account_tool_policies (LIKE public.account_tool_policies INCLUDING ALL)",
            "CREATE TEMP TABLE gateway_blocks (LIKE public.gateway_blocks INCLUDING ALL)",
            "CREATE TEMP TABLE events (LIKE public.events INCLUDING ALL)",
            "CREATE TEMP TABLE session_token_usage (LIKE public.session_token_usage INCLUDING ALL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        for table in ["stream_events", "session_context", "session_attachments"] {
            assert!(
                sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass($1)::text")
                    .bind(table)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, received) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().fallback(axum::routing::post({
            let calls = calls.clone();
            move |headers: HeaderMap, body: Bytes| {
                calls.fetch_add(1, Ordering::SeqCst);
                tx.send((headers, body)).unwrap();
                async move {
                    (
                        status,
                        [("content-type", "application/json"), ("retry-after", "17")],
                        response,
                    )
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        cctui_server_kernel::client::install_crypto_provider();
        let host = Host(Arc::new(Resources {
            pool,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            bus: Bus::new(Box::new(NoopTransport)),
            locks: DashMap::new(),
            reauth: DashMap::new(),
            usage: DashMap::new(),
            budgets: DashMap::new(),
            orphans: DashMap::new(),
            windows: Arc::new(DashMap::new()),
            status: ProviderStatusCache::default(),
            notices: AtomicUsize::new(0),
            refuse_upstream,
            tracer: std::sync::Mutex::new(None),
            scrub: std::sync::atomic::AtomicBool::new(false),
        }));
        let provider = Uuid::new_v4();
        let account = Uuid::new_v4();
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id,name,key_hash) VALUES ($1,'automation',$2)")
            .bind(uid)
            .bind(uid.to_string())
            .execute(&host.0.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO accounts (id,user_id,name) VALUES ($1,$2,'automation')")
            .bind(account)
            .bind(uid)
            .execute(&host.0.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO account_providers (id,user_id,account_id,provider,auth_scheme,encrypted_access_token,base_url) VALUES ($1,$2,$3,'anthropic-compatible','api_key',$4,$5)")
            .bind(provider).bind(uid).bind(account).bind(cctui_crypto::encrypt("upstream-fixture", &[42;32])).bind(upstream).execute(&host.0.pool).await.unwrap();
        sqlx::query("INSERT INTO sessions (id,machine_id,user_id,working_dir,status) VALUES ('run','fixture',$1,'/work','active')").bind(uid).execute(&host.0.pool).await.unwrap();
        let token = format!("cctui_s_{}", Uuid::new_v4());
        sqlx::query("INSERT INTO session_tokens (token_hash,session_id,account_id,user_id) VALUES ($1,'run',$2,$3)")
            .bind(cctui_server_kernel::auth::sha256_hex(&token)).bind(provider).bind(uid).execute(&host.0.pool).await.unwrap();
        Some(Self { host, provider, account, token, calls, received, server })
    }
    async fn request(&self, path: &str, token: &str) -> Response {
        self.request_as(path, token, Family::Anthropic, "/gateway/anthropic").await
    }
    async fn send_body(&self, body: &'static str, encoding: Option<&'static str>) -> Response {
        let mut headers = vec![("content-type", "application/json")];
        headers.extend(encoding.map(|e| ("content-encoding", e)));
        self.send("/v1/messages", Bytes::from_static(body.as_bytes()), &headers).await
    }
    async fn send(&self, path: &str, body: Bytes, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/gateway/anthropic{path}"))
            .header("authorization", format!("Bearer {}", self.token))
            .header("x-api-key", "worker-placeholder");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(Body::from(body)).unwrap();
        tokio::time::timeout(
            Duration::from_secs(4),
            passthrough(
                self.host.clone(),
                request,
                Family::Anthropic,
                "/gateway/anthropic",
                "https://unused.invalid",
            ),
        )
        .await
        .expect("bounded proxy")
        .unwrap_or_else(|status| Response::builder().status(status).body(Body::empty()).unwrap())
    }
    async fn request_as(&self, path: &str, token: &str, family: Family, prefix: &str) -> Response {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("x-api-key", "worker-placeholder")
            .body(Body::from("{\"model\": \"fixture\",  \"messages\":[]}"))
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(4),
            passthrough(self.host.clone(), request, family, prefix, "https://unused.invalid"),
        )
        .await
        .expect("bounded proxy")
        .unwrap_or_else(|status| Response::builder().status(status).body(Body::empty()).unwrap())
    }
}

#[tokio::test]
async fn headless_proxy_preserves_bytes_credentials_status_and_rate_limit() {
    let Some(mut f) = Fixture::new(StatusCode::OK, "{\"content\": []}", false).await else {
        return;
    };
    sqlx::query("UPDATE account_providers SET rate_limits_json = '{\"rpm\":1}' WHERE id=$1")
        .bind(f.provider)
        .execute(&f.host.0.pool)
        .await
        .unwrap();
    let reply = f.request("/gateway/anthropic/v1/messages", &f.token).await;
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()["retry-after"], "17");
    assert_eq!(axum::body::to_bytes(reply.into_body(), 4096).await.unwrap(), "{\"content\": []}");
    let (headers, body) = f.received.recv().await.unwrap();
    assert_eq!(headers["x-api-key"], "upstream-fixture");
    assert!(!headers.contains_key("authorization"));
    assert_eq!(body, "{\"model\": \"fixture\",  \"messages\":[]}");
    assert_eq!(f.host.0.notices.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.request("/gateway/anthropic/v1/messages", &f.token).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn headless_proxy_refuses_paths_and_host_denial_before_network_io() {
    let Some(f) = Fixture::new(StatusCode::OK, "{}", true).await else { return };
    assert_eq!(
        f.request("/gateway/anthropic/admin/secrets", &f.token).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("/gateway/anthropic/v1/messages", &f.token).await.status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn headless_proxy_keeps_tool_enforcement_without_workspace_observers() {
    let Some(f)=Fixture::new(StatusCode::OK,r#"{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"cat /secret/acmecorp"}}]}"#,false).await else {return};
    sqlx::query(
        "INSERT INTO account_tool_policies (account_id,terms) VALUES ($1,ARRAY['acmecorp'])",
    )
    .bind(f.account)
    .execute(&f.host.0.pool)
    .await
    .unwrap();
    let mut events = f.host.0.bus.subscribe_server();
    let reply = f.request("/gateway/anthropic/v1/messages", &f.token).await;
    assert_eq!(reply.status(), StatusCode::OK);
    let body = axum::body::to_bytes(reply.into_body(), 8192).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains("cat /secret/acmecorp"), "tool input escaped enforcement: {body}");
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("tool block must be reported after persistence")
        .unwrap();
    let blocks: i64 = sqlx::query_scalar("SELECT count(*) FROM gateway_blocks")
        .fetch_one(&f.host.0.pool)
        .await
        .unwrap();
    assert_eq!(blocks, 1);
}
#[tokio::test]
async fn headless_proxy_distinguishes_orphan_provider_and_policy_failures() {
    let Some(f) = Fixture::new(StatusCode::UNAUTHORIZED, "{}", false).await else { return };
    let reply = f.request("/gateway/anthropic/v1/messages", "cctui_s_unknown").await;
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(reply.headers()["x-cctui-auth-stage"], "session-token");
    let reply = f.request("/gateway/anthropic/v1/messages", &f.token).await;
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(reply.headers()["x-cctui-auth-stage"], "provider-oauth");
    assert_eq!(f.host.0.notices.load(Ordering::SeqCst), 0);
    let Some(f) = Fixture::new(StatusCode::OK, "{}", false).await else { return };
    sqlx::query("DROP TABLE account_tool_policies").execute(&f.host.0.pool).await.unwrap();
    assert_eq!(
        f.request("/gateway/anthropic/v1/messages", &f.token).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

const SCRUB_SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

#[tokio::test]
async fn outbound_scrub_masks_readable_bodies_and_refuses_unreadable_ones() {
    let Some(mut f) = Fixture::new(StatusCode::OK, "{\"content\": []}", false).await else {
        return;
    };
    f.host.0.scrub.store(true, Ordering::SeqCst);
    let reply = f
        .send_body(
            r#"{"model":"fixture","messages":[{"role":"user","content":"GH=ghp_abcdefghijklmnopqrstuvwxyz0123456789"}]}"#,
            None,
        )
        .await;
    assert_eq!(reply.status(), StatusCode::OK);
    let (_, body) = f.received.recv().await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains(SCRUB_SECRET), "{body}");
    assert!(body.contains("[REDACTED:github_token"), "{body}");

    let reply = f.send_body("{\"messages\":[]}", Some("br")).await;
    assert_eq!(reply.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let text = axum::body::to_bytes(reply.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&text).contains("outbound secret scrub"), "{text:?}");

    let reply = f.send_body("{\"messages\":[", None).await;
    assert_eq!(reply.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}

fn gzip(raw: &[u8]) -> Bytes {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(raw).unwrap();
    Bytes::from(enc.finish().unwrap())
}

#[tokio::test]
async fn outbound_scrub_decodes_gzip_and_zstd_model_calls() {
    let Some(mut f) = Fixture::new(StatusCode::OK, "{\"content\": []}", false).await else {
        return;
    };
    f.host.0.scrub.store(true, Ordering::SeqCst);
    let raw = format!(
        r#"{{"model":"fixture","messages":[{{"role":"user","content":"GH={SCRUB_SECRET}"}}]}}"#
    );
    let zstd_body = Bytes::from(zstd::encode_all(raw.as_bytes(), 3).unwrap());
    for (encoding, body) in [("gzip", gzip(raw.as_bytes())), ("zstd", zstd_body)] {
        let headers = [("content-type", "application/json"), ("content-encoding", encoding)];
        let reply = f.send("/v1/messages", body, &headers).await;
        assert_eq!(reply.status(), StatusCode::OK, "{encoding}");
        let (headers, body) = f.received.recv().await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains(SCRUB_SECRET), "{text}");
        assert!(text.contains("[REDACTED:github_token"), "{text}");
        assert!(!headers.contains_key("content-encoding"), "{encoding}: {headers:?}");
        assert_eq!(headers["content-length"], body.len().to_string());
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn outbound_scrub_refuses_a_body_that_decodes_past_the_cap() {
    let Some(f) = Fixture::new(StatusCode::OK, "{}", false).await else { return };
    f.host.0.scrub.store(true, Ordering::SeqCst);
    let bomb = gzip(&vec![b' '; 33 * 1024 * 1024]);
    let headers = [("content-type", "application/json"), ("content-encoding", "gzip")];
    let reply = f.send("/v1/messages", bomb, &headers).await;
    assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn outbound_scrub_forwards_non_model_paths_untouched() {
    let Some(mut f) = Fixture::new(StatusCode::OK, "{}", false).await else { return };
    f.host.0.scrub.store(true, Ordering::SeqCst);
    let multipart = format!(
        "--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nGH={SCRUB_SECRET}\r\n--b--\r\n"
    );
    let content_type = "multipart/form-data; boundary=b";
    let reply = f
        .send("/v1/complete", Bytes::from(multipart.clone()), &[("content-type", content_type)])
        .await;
    assert_eq!(reply.status(), StatusCode::OK);
    let (headers, body) = f.received.recv().await.unwrap();
    assert_eq!(body, multipart.as_bytes());
    assert_eq!(headers["content-type"], content_type);

    let packed = gzip(br#"{"prompt":"hello"}"#);
    let sent = [("content-type", "application/json"), ("content-encoding", "gzip")];
    assert_eq!(f.send("/v1/complete", packed.clone(), &sent).await.status(), StatusCode::OK);
    let (headers, body) = f.received.recv().await.unwrap();
    assert_eq!(body, packed);
    assert_eq!(headers["content-encoding"], "gzip");
    assert_eq!(headers["content-type"], "application/json");
}

#[tokio::test]
async fn outbound_scrub_off_forwards_unreadable_bodies_unchanged() {
    let Some(mut f) = Fixture::new(StatusCode::OK, "{}", false).await else { return };
    assert_eq!(f.send_body("{\"messages\":[", Some("gzip")).await.status(), StatusCode::OK);
    let (headers, body) = f.received.recv().await.unwrap();
    assert_eq!(body, "{\"messages\":[");
    assert_eq!(headers["content-encoding"], "gzip");
}

// Keep Axum's Send requirement part of the shared-host contract.
async fn handler(State(host): State<Host>, request: Request) -> Result<Response, StatusCode> {
    passthrough(host, request, Family::Anthropic, "/gateway/anthropic", "https://unused.invalid")
        .await
}

#[test]
fn headless_handler_satisfies_axum_send_contract() {
    let _: axum::Router<Host> =
        axum::Router::new().route("/gateway/anthropic/{*path}", axum::routing::any(handler));
}

struct MeteredTrace {
    pool: PgPool,
    captures: tokio::sync::mpsc::UnboundedSender<TraceCapture>,
    samples: AtomicUsize,
}
impl TraceSink for MeteredTrace {
    fn should_sample(&self) -> bool {
        self.samples.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn report(
        &self,
        capture: TraceCapture,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let usage: (i64,i64) = sqlx::query_as("SELECT input_tokens, output_tokens FROM session_token_usage WHERE session_id='run'")
                .fetch_one(&self.pool).await.expect("execution metering must precede optional tracing");
            assert_eq!(usage, (7, 3));
            self.captures.send(capture).unwrap();
        })
    }
}
#[tokio::test]
async fn optional_trace_runs_after_metering_and_tpm_accounting() {
    const RAW: &str = r#"{"id":"fixture-message","usage":{"prompt_tokens":7,"completion_tokens":3},"choices":[]}"#;
    let Some(f) = Fixture::new(StatusCode::OK, RAW, false).await else { return };
    sqlx::query("UPDATE account_providers SET provider='fireworks', auth_scheme='bearer', rate_limits_json='{\"tpm\":10}' WHERE id=$1")
        .bind(f.provider).execute(&f.host.0.pool).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let trace = Arc::new(MeteredTrace {
        pool: f.host.0.pool.clone(),
        captures: tx,
        samples: AtomicUsize::new(0),
    });
    *f.host.0.tracer.lock().unwrap() = Some(trace.clone());
    let reply = f
        .request_as(
            "/gateway/fireworks/chat/completions",
            &f.token,
            Family::Fireworks,
            "/gateway/fireworks",
        )
        .await;
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(axum::body::to_bytes(reply.into_body(), 8192).await.unwrap(), RAW);
    let capture = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("trace reporting")
        .unwrap();
    assert_eq!(capture.session_id.as_deref(), Some("run"));
    assert_eq!(capture.model.as_deref(), Some("fixture"));
    assert_eq!(capture.request.unwrap()["model"], "fixture");
    assert_eq!(capture.response, RAW.as_bytes());
    assert_eq!(trace.samples.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.request_as(
            "/gateway/fireworks/chat/completions",
            &f.token,
            Family::Fireworks,
            "/gateway/fireworks"
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
}
