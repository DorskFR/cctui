//! Dispatch runs on execution tables with an explicit headless host.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::{DispatchRequest, SpawnCapability};
use cctui_server_core::account_resolve::AccountResolutionContext;
use cctui_server_core::auth::{AuthContext, Scope};
use cctui_server_core::bus::{Bus, NoopTransport};
use cctui_server_core::dispatch::{DispatchContext, DispatchNotification, execute_dispatch};
use cctui_server_core::dispatchers::Registry;
use cctui_server_core::dispatchers::http::HttpDispatcher;
use cctui_server_core::gateway::mint::MintContext;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Execution tables the dispatch path touches, as session-private copies.
const TEMP_TABLES: &[&str] = &[
    "CREATE TEMP TABLE users (LIKE public.users INCLUDING ALL)",
    "CREATE TEMP TABLE user_acls (LIKE public.user_acls INCLUDING ALL)",
    "CREATE TEMP TABLE key_acls (LIKE public.key_acls INCLUDING ALL)",
    "CREATE TEMP TABLE machines (LIKE public.machines INCLUDING ALL)",
    "CREATE TEMP TABLE auth_keys (LIKE public.auth_keys INCLUDING ALL)",
    "CREATE TEMP TABLE accounts (LIKE public.accounts INCLUDING ALL)",
    "CREATE TEMP TABLE account_pools (LIKE public.account_pools INCLUDING ALL)",
    "CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING ALL)",
    "CREATE TEMP TABLE dispatchers (LIKE public.dispatchers INCLUDING ALL)",
    "CREATE TEMP TABLE sessions (LIKE public.sessions INCLUDING ALL)",
    "CREATE TEMP TABLE session_spawn_capabilities (LIKE public.session_spawn_capabilities INCLUDING ALL)",
    "CREATE TEMP TABLE dispatch_policies (LIKE public.dispatch_policies INCLUDING ALL)",
    "CREATE TEMP TABLE dispatch_policy_uses (LIKE public.dispatch_policy_uses INCLUDING ALL)",
    "CREATE TEMP TABLE dispatch_handles (LIKE public.dispatch_handles INCLUDING ALL)",
    "CREATE TEMP TABLE dispatch_records (LIKE public.dispatch_records INCLUDING ALL)",
    "CREATE TEMP TABLE session_tokens (LIKE public.session_tokens INCLUDING ALL)",
    "CREATE TEMP TABLE account_tool_policies (LIKE public.account_tool_policies INCLUDING ALL)",
    "CREATE TEMP TABLE session_hook_policies (LIKE public.session_hook_policies INCLUDING ALL)",
    "CREATE TEMP TABLE machine_launches (LIKE public.machine_launches INCLUDING ALL)",
    "CREATE TEMP TABLE resource_shares (LIKE public.resource_shares INCLUDING ALL)",
];

struct Host {
    pool: PgPool,
    bus: Bus,
    dispatchers: Registry,
    callbacks: Mutex<Vec<&'static str>>,
}

impl AccountResolutionContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn cached_needs_reauth(&self, _: Uuid) -> bool {
        false
    }
    async fn usage_for_soft_limit(&self, _: Uuid) -> Option<Value> {
        None
    }
}
impl MintContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
    fn external_url(&self) -> &'static str {
        "https://automation.example.invalid"
    }
    fn clear_orphan_fingerprint(&self, _: &str) {}
}
impl DispatchContext for Host {
    fn dispatch_bus(&self) -> &Bus {
        &self.bus
    }
    fn dispatchers(&self) -> &Registry {
        &self.dispatchers
    }
    fn notify(&self, _: DispatchNotification) {
        self.callbacks.lock().unwrap().push("notify");
    }
    fn cache_spawn_capability(&self, _: &str, _: SpawnCapability) {
        self.callbacks.lock().unwrap().push("capability");
    }
    async fn spawn_default_capability(&self) -> SpawnCapability {
        SpawnCapability::machine_default()
    }
    async fn register_completion_webhook(&self, _: &AuthContext, _: &DispatchRequest, _: &str) {
        self.callbacks.lock().unwrap().push("webhook");
    }
    async fn remember_dispatch_context(&self, _: Option<Uuid>, _: &DispatchRequest, _: &str) {
        self.callbacks.lock().unwrap().push("context");
    }
}

struct Fixture {
    host: Host,
    ctx: AuthContext,
    requests: tokio::sync::mpsc::UnboundedReceiver<(HeaderMap, Value)>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(status: StatusCode, response: Value) -> Option<Self> {
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.trim().is_empty());
        let Some(url) = url else {
            assert!(std::env::var_os("CI").is_none(), "dispatch contract needs a test database");
            eprintln!("skipping dispatch contract: no test database");
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
        for &ddl in TEMP_TABLES {
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
        let uid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, name, key_hash) VALUES ($1, 'automation', 'fixture-user')",
        )
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        let (tx, requests) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/dispatch",
            axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                tx.send((headers, body)).unwrap();
                let response = response.clone();
                async move { (status, axum::Json(response)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/dispatch", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        cctui_server_core::client::install_crypto_provider();
        let dispatcher = HttpDispatcher::new(
            "executor",
            endpoint,
            Some("fixture-token".into()),
            reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap(),
        );
        Some(Self {
            host: Host {
                pool,
                bus: Bus::new(Box::new(NoopTransport)),
                dispatchers: Registry::new().with(Arc::new(dispatcher)),
                callbacks: Mutex::new(Vec::new()),
            },
            ctx: AuthContext {
                user_id: uid,
                key_id: Uuid::new_v4(),
                machine_id: None,
                scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
            },
            requests,
            server,
        })
    }
    fn request(payload: Value) -> DispatchRequest {
        DispatchRequest {
            dispatcher: "executor".into(),
            session_id: None,
            timeout: Some(1),
            reply_url: None,
            notify_url: None,
            notify_secret: None,
            payload,
            account: None,
            provider: None,
            accounts: Vec::new(),
        }
    }
}

#[tokio::test]
async fn dispatch_mints_scoped_worker_identity_and_records_the_forward_without_workspace_tables() {
    let Some(mut f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    let request =
        Fixture::request(json!({"prompt_file":"review", "env":{"FEATURE_FLAG":"fixture"}}));
    let (status, reply) = execute_dispatch(&f.host, &f.ctx, request).await.unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);
    let (headers, forwarded) = f.requests.try_recv().unwrap();
    assert_eq!(headers["authorization"], "Bearer fixture-token");
    assert_eq!(forwarded["session_id"], reply.session_id);
    assert_eq!(forwarded["timeout_minutes"], 1);
    let token = forwarded["payload"]["cctui_machine_key"].as_str().unwrap();
    let (session, ttl): (String, f64) = sqlx::query_as(
        "SELECT session_id, EXTRACT(EPOCH FROM (expires_at - now()))::float8 FROM auth_keys WHERE key_hash = $1 AND kind = 'ephemeral'")
        .bind(cctui_server_core::auth::sha256_hex(token)).fetch_one(&f.host.pool).await.unwrap();
    assert_eq!(session, reply.session_id);
    assert!((1800.0..=1860.0).contains(&ttl));
    let grants: Vec<String> = sqlx::query_scalar("SELECT scope FROM key_acls ORDER BY scope")
        .fetch_all(&f.host.pool)
        .await
        .unwrap();
    assert_eq!(grants, ["read"]);
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM dispatch_records WHERE session_id = $1")
            .bind(&reply.session_id)
            .fetch_one(&f.host.pool)
            .await
            .unwrap();
    assert_eq!(outcome, "forwarded");
    let handle: String =
        sqlx::query_scalar("SELECT handle FROM dispatch_handles WHERE session_id = $1")
            .bind(&reply.session_id)
            .fetch_one(&f.host.pool)
            .await
            .unwrap();
    assert_eq!(handle, "job/review");
    assert_eq!(
        *f.host.callbacks.lock().unwrap(),
        ["notify", "webhook", "capability", "context", "notify"]
    );
}

#[tokio::test]
async fn dispatch_rejects_scope_and_reserved_environment_before_minting_a_worker_key() {
    let Some(mut f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    f.ctx.scopes.remove(&Scope::Dispatch);
    let error = execute_dispatch(&f.host, &f.ctx, Fixture::request(json!({}))).await.unwrap_err();
    assert_eq!(error.0, StatusCode::FORBIDDEN);
    f.ctx.scopes.insert(Scope::Dispatch);
    let error = execute_dispatch(
        &f.host,
        &f.ctx,
        Fixture::request(json!({"env":{"TASK_VALUE":"spoofed"}})),
    )
    .await
    .unwrap_err();
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    let keys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_keys").fetch_one(&f.host.pool).await.unwrap();
    assert_eq!(keys, 0);
    assert!(f.requests.try_recv().is_err());
}

#[tokio::test]
async fn refusal_and_deduplication_refund_policy_capacity_and_keep_children_disabled() {
    for (status, response, expected) in [
        (StatusCode::BAD_REQUEST, json!({"error":"invalid fixture"}), StatusCode::BAD_REQUEST),
        (
            StatusCode::OK,
            json!({"handle":"job/existing", "status":"deduplicated"}),
            StatusCode::ACCEPTED,
        ),
    ] {
        let Some(mut f) = Fixture::new(status, response).await else { return };
        cctui_server_core::store::dispatch_policies::upsert(
            &f.host.pool,
            f.ctx.user_id,
            &cctui_proto::api::admin::DispatchPolicyRules {
                daily_cap: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let mut request = Fixture::request(json!({"spawn_capability":{"max_children":10}}));
            request.session_id = Some("logical-review".into());
            let outcome = execute_dispatch(&f.host, &f.ctx, request).await;
            assert_eq!(outcome.as_ref().map_or_else(|e| e.0, |ok| ok.0), expected);
            let (_, delivered) = f.requests.try_recv().unwrap();
            assert!(delivered["payload"].get("spawn_capability").is_none());
            let session = delivered["session_id"].as_str().unwrap();
            let capability =
                cctui_server_core::store::spawn_capabilities::get(&f.host.pool, session)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(capability.max_children, Some(0));
            let uses = cctui_server_core::store::dispatch_policies::dispatches_today(
                &f.host.pool,
                f.ctx.user_id,
            )
            .await
            .unwrap();
            assert_eq!(uses, 0);
            let record: Option<String> =
                sqlx::query_scalar("SELECT outcome FROM dispatch_records WHERE session_id = $1")
                    .bind(session)
                    .fetch_optional(&f.host.pool)
                    .await
                    .unwrap();
            if status == StatusCode::BAD_REQUEST {
                assert_eq!(record.as_deref(), Some("refused"));
            } else {
                assert!(record.is_none());
            }
        }
    }
}

#[tokio::test]
async fn concurrent_dispatches_cannot_forward_past_the_daily_cap() {
    let Some(mut f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    cctui_server_core::store::dispatch_policies::upsert(
        &f.host.pool,
        f.ctx.user_id,
        &cctui_proto::api::admin::DispatchPolicyRules { daily_cap: Some(1), ..Default::default() },
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        execute_dispatch(&f.host, &f.ctx, Fixture::request(json!({}))),
        execute_dispatch(&f.host, &f.ctx, Fixture::request(json!({}))),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let rejected = if let Err(e) = a { e } else { b.unwrap_err() };
    assert_eq!(rejected.0, StatusCode::FORBIDDEN, "{rejected:?}");
    assert!(f.requests.try_recv().is_ok());
    assert!(f.requests.try_recv().is_err());
    let machines: i64 = sqlx::query_scalar("SELECT count(*) FROM machines WHERE kind = 'dispatch'")
        .fetch_one(&f.host.pool)
        .await
        .unwrap();
    assert_eq!(machines, 1);
}

#[tokio::test]
async fn repeated_dispatch_preserves_the_machine_key_and_repairs_a_legacy_missing_key() {
    let Some(f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    let mut first = None;
    for _ in 0..2 {
        let _ = execute_dispatch(&f.host, &f.ctx, Fixture::request(json!({}))).await.unwrap();
        let machine: (Uuid, String, String) = sqlx::query_as(
            "SELECT id, dispatch_key, key_hash FROM machines WHERE kind = 'dispatch'",
        )
        .fetch_one(&f.host.pool)
        .await
        .unwrap();
        if let Some(prior) = &first {
            assert_eq!(prior, &machine);
        } else {
            first = Some(machine);
        }
    }
    sqlx::query("UPDATE machines SET dispatch_key = NULL").execute(&f.host.pool).await.unwrap();
    let _ = execute_dispatch(&f.host, &f.ctx, Fixture::request(json!({}))).await.unwrap();
    let repaired: (Uuid, String, String) =
        sqlx::query_as("SELECT id, dispatch_key, key_hash FROM machines WHERE kind = 'dispatch'")
            .fetch_one(&f.host.pool)
            .await
            .unwrap();
    assert_eq!(repaired.0, first.unwrap().0);
    assert_eq!(repaired.2, cctui_server_core::auth::sha256_hex(&repaired.1));
}

#[tokio::test]
async fn a_dispatched_worker_payload_carries_the_effective_hook_policy() {
    use cctui_proto::hook_policy::{from_dispatch_payload, no_direct_harness_rule};
    let Some(mut f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    let request = Fixture::request(json!({
        "prompt_file": "review",
        "hook_policy": {"rules": []},
        "hook_rules": [{"event": "PreToolUse", "tool": "shell", "command": "audit"}],
    }));
    let (status, reply) = execute_dispatch(&f.host, &f.ctx, request).await.unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, forwarded) = f.requests.try_recv().unwrap();
    assert!(forwarded["payload"].get("hook_rules").is_none());
    let policy = from_dispatch_payload(&forwarded["payload"]).expect("the worker gets a policy");
    assert_eq!(policy.rules[0], no_direct_harness_rule());
    assert_eq!(policy.rules[1].command, "audit");
    let recorded =
        cctui_server_core::hook_policy::for_session(&f.host.pool, &reply.session_id).await.unwrap();
    assert_eq!(recorded.rules, policy.rules);

    let count = |sql: &'static str| {
        let pool = f.host.pool.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.unwrap() }
    };
    let keys = count("SELECT count(*) FROM auth_keys").await;
    let policies = count("SELECT count(*) FROM session_hook_policies").await;
    let refused = execute_dispatch(
        &f.host,
        &f.ctx,
        Fixture::request(json!({"prompt_file": "review", "hook_rules": [{
            "event": "PreToolUse", "tool": "shell",
            "command": no_direct_harness_rule().command, "timeout_secs": 1,
        }]})),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.0, StatusCode::BAD_REQUEST);
    assert!(refused.1.error.contains("add-only"), "{}", refused.1.error);
    assert_eq!(count("SELECT count(*) FROM auth_keys").await, keys, "no worker key minted");
    assert_eq!(count("SELECT count(*) FROM session_hook_policies").await, policies);
    assert!(f.requests.try_recv().is_err(), "nothing was forwarded");
}

#[tokio::test]
async fn weakening_an_account_hook_rule_is_refused_before_any_worker_credential_is_minted() {
    let Some(mut f) = Fixture::new(StatusCode::OK, json!({"handle":"job/review"})).await else {
        return;
    };
    let (account, provider) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'reviewer')")
        .bind(account)
        .bind(f.ctx.user_id)
        .execute(&f.host.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO account_providers \
         (id, user_id, account_id, provider, auth_scheme, encrypted_access_token, base_url) \
         VALUES ($1, $2, $3, 'anthropic-compatible', 'api_key', $4, 'https://upstream.invalid')",
    )
    .bind(provider)
    .bind(f.ctx.user_id)
    .bind(account)
    .bind(cctui_crypto::encrypt("upstream-fixture", &[42; 32]))
    .execute(&f.host.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO account_tool_policies (account_id, hooks) VALUES ($1, $2)")
        .bind(account)
        .bind(json!([{"event": "PreToolUse", "tool": "shell", "command": "account-audit"}]))
        .execute(&f.host.pool)
        .await
        .unwrap();
    let mut request = Fixture::request(json!({"prompt_file": "review", "hook_rules": [{
        "event": "PreToolUse", "tool": "shell", "command": "account-audit", "timeout_secs": 1,
    }]}));
    request.account = Some("reviewer".into());
    let refused = execute_dispatch(&f.host, &f.ctx, request).await.unwrap_err();
    assert_eq!(refused.0, StatusCode::BAD_REQUEST);
    assert!(refused.1.error.contains("add-only"), "{}", refused.1.error);
    let count = |sql: &'static str| {
        let pool = f.host.pool.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.unwrap() }
    };
    assert_eq!(count("SELECT count(*) FROM auth_keys").await, 0, "no worker key minted");
    assert_eq!(count("SELECT count(*) FROM session_tokens").await, 0, "no gateway token minted");
    assert_eq!(count("SELECT count(*) FROM session_hook_policies").await, 0);
    assert!(f.requests.try_recv().is_err(), "nothing was forwarded");

    let mut request = Fixture::request(json!({"prompt_file": "review", "hook_rules": [{
        "event": "PreToolUse", "tool": "shell", "command": "account-audit",
    }]}));
    request.account = Some("reviewer".into());
    let (status, _) = execute_dispatch(&f.host, &f.ctx, request).await.unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(count("SELECT count(*) FROM session_tokens").await, 1);
}

#[tokio::test]
async fn an_enrolled_dispatchers_reported_exposure_lands_on_the_dispatch_record() {
    use cctui_proto::api::{DispatchExposure, HumanGate};
    use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
    let declared = DispatchExposure {
        untrusted_input: true,
        sensitive_data: true,
        outward: true,
        human_gate: Some(HumanGate::DraftPrOnly),
    };
    for reported in [Some(declared), None] {
        let Some(f) = Fixture::new(StatusCode::OK, json!({})).await else { return };
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO dispatchers (id, user_id, name, kind) \
             VALUES ($1, $2, 'enrolled', 'kubernetes')",
        )
        .bind(id)
        .bind(f.ctx.user_id)
        .execute(&f.host.pool)
        .await
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DispatcherFrameDown>(8);
        f.host.bus.register_dispatcher(id, tx);
        let bus = f.host.bus.clone();
        let fake = tokio::spawn(async move {
            let Some(DispatcherFrameDown::Dispatch { request_id, spec }) = rx.recv().await else {
                panic!("expected a Dispatch frame");
            };
            assert!(bus.resolve_dispatcher_reply(
                request_id,
                DispatcherFrameUp::DispatchResult {
                    request_id,
                    session_id: spec.session_id,
                    handle: "jobs/worker".into(),
                    namespace: Some("cctui".into()),
                    status: Some("dispatched".into()),
                    queue_position: None,
                    exposure: reported,
                    error: None,
                },
            ));
        });
        let mut request = Fixture::request(json!({"prompt_file": "review"}));
        request.dispatcher = "enrolled".into();
        let (status, reply) = execute_dispatch(&f.host, &f.ctx, request).await.unwrap();
        fake.await.unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(reply.exposure, reported);
        let recorded: Option<Value> =
            sqlx::query_scalar("SELECT exposure FROM dispatch_records WHERE session_id = $1")
                .bind(&reply.session_id)
                .fetch_one(&f.host.pool)
                .await
                .unwrap();
        let recorded = recorded.map(|v| serde_json::from_value::<DispatchExposure>(v).unwrap());
        assert_eq!(recorded, reported);
    }
}

/// How the fake enrolled dispatcher meets a dedup check.
#[derive(Clone, Copy, Debug)]
enum Probe {
    /// A live worker holds the dedup key.
    Live,
    /// The worker that held the key is terminal: the dispatch redispatches.
    Terminal,
    /// The check is never answered.
    Silent,
    /// The dispatcher's current connection never announced the check.
    Unannounced,
}

/// Enroll a fake dispatcher named `enrolled` that meets dedup checks per
/// `probe` and answers a dispatch `redispatched`; returns the frames it saw.
async fn enrolled_dispatcher(f: &Fixture, probe: Probe) -> Arc<Mutex<Vec<&'static str>>> {
    use cctui_proto::ws::{DISPATCHER_FEATURE_DEDUP_CHECK, DispatcherFrameDown, DispatcherFrameUp};
    use cctui_server_core::dispatchers::enrolled::{clear_features, record_features};
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO dispatchers (id, user_id, name, kind) \
         VALUES ($1, $2, 'enrolled', 'kubernetes')",
    )
    .bind(id)
    .bind(f.ctx.user_id)
    .execute(&f.host.pool)
    .await
    .unwrap();
    let pool = &f.host.pool;
    let announced = [DISPATCHER_FEATURE_DEDUP_CHECK.to_owned()];
    record_features(pool, id, &announced).await.unwrap();
    if matches!(probe, Probe::Unannounced) {
        clear_features(pool, id).await.unwrap();
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<DispatcherFrameDown>(8);
    f.host.bus.register_dispatcher(id, tx);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (bus, log) = (f.host.bus.clone(), seen.clone());
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            match frame {
                DispatcherFrameDown::DedupCheck { request_id, .. } => {
                    log.lock().unwrap().push("dedup_check");
                    let handle = match probe {
                        Probe::Silent => continue,
                        Probe::Live => Some("jobs/worker/first-session".to_owned()),
                        Probe::Terminal | Probe::Unannounced => None,
                    };
                    let reply = DispatcherFrameUp::DedupCheckResult {
                        request_id,
                        handle,
                        namespace: Some("cctui".into()),
                        queue_position: None,
                        exposure: None,
                        error: None,
                    };
                    assert!(bus.resolve_dispatcher_reply(request_id, reply));
                }
                DispatcherFrameDown::Dispatch { request_id, spec } => {
                    log.lock().unwrap().push("dispatch");
                    let reply = DispatcherFrameUp::DispatchResult {
                        request_id,
                        handle: format!("jobs/worker/{}", spec.session_id),
                        session_id: spec.session_id,
                        namespace: Some("cctui".into()),
                        status: Some("redispatched".into()),
                        queue_position: None,
                        exposure: None,
                        error: None,
                    };
                    assert!(bus.resolve_dispatcher_reply(request_id, reply));
                }
                _ => {}
            }
        }
    });
    seen
}

fn logical_request() -> DispatchRequest {
    let mut request = Fixture::request(json!({"prompt_file": "review"}));
    request.dispatcher = "enrolled".into();
    request.session_id = Some("logical-review".into());
    request
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn a_dispatch_onto_a_live_worker_is_deduplicated_before_any_key_token_or_record() {
    let Some(f) = Fixture::new(StatusCode::OK, json!({})).await else { return };
    let seen = enrolled_dispatcher(&f, Probe::Live).await;
    let (status, reply) = execute_dispatch(&f.host, &f.ctx, logical_request()).await.unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(reply.status, "deduplicated");
    assert_eq!(reply.handle, "jobs/worker/first-session");
    assert_eq!(reply.namespace.as_deref(), Some("cctui"));
    assert_eq!(*seen.lock().unwrap(), ["dedup_check"], "no Dispatch frame is sent");
    for sql in [
        "SELECT count(*) FROM machines",
        "SELECT count(*) FROM auth_keys",
        "SELECT count(*) FROM session_tokens",
        "SELECT count(*) FROM dispatch_records",
        "SELECT count(*) FROM dispatch_handles",
        "SELECT count(*) FROM session_spawn_capabilities",
    ] {
        assert_eq!(count(&f.host.pool, sql).await, 0, "{sql}");
    }
    assert_eq!(*f.host.callbacks.lock().unwrap(), ["notify", "notify"], "no webhook, no context");
}

#[tokio::test]
async fn a_terminal_unanswered_or_unannounced_check_takes_the_full_dispatch_path() {
    for (probe, frames) in [
        (Probe::Terminal, &["dedup_check", "dispatch"][..]),
        (Probe::Silent, &["dedup_check", "dispatch"][..]),
        (Probe::Unannounced, &["dispatch"][..]),
    ] {
        let Some(f) = Fixture::new(StatusCode::OK, json!({})).await else { return };
        let seen = enrolled_dispatcher(&f, probe).await;
        let started = std::time::Instant::now();
        let (status, reply) = execute_dispatch(&f.host, &f.ctx, logical_request()).await.unwrap();
        assert_eq!(status, StatusCode::ACCEPTED, "{probe:?}");
        assert_eq!(reply.status, "redispatched", "{probe:?}");
        assert_eq!(*seen.lock().unwrap(), frames, "{probe:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "{probe:?}: the check is bounded");
        assert_eq!(
            count(&f.host.pool, "SELECT count(*) FROM auth_keys WHERE kind = 'ephemeral'").await,
            1,
            "{probe:?}: the worker key is minted"
        );
        let outcome: String =
            sqlx::query_scalar("SELECT outcome FROM dispatch_records WHERE session_id = $1")
                .bind(&reply.session_id)
                .fetch_one(&f.host.pool)
                .await
                .unwrap();
        assert_eq!(outcome, "forwarded", "{probe:?}");
    }
}
