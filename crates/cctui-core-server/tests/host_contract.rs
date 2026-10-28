//! Exercise the assembled automation host over HTTP and the daemon WebSocket.
use cctui_core_server::host::Host;
use cctui_proto::adapter::{AdapterEvent, SessionMeta};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use cctui_server_core::auth::AuthConfig;
use cctui_server_core::bus::{Bus, NoopTransport};
use cctui_server_core::state::CoreState;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Fixture {
    host: Host,
    pool: sqlx::PgPool,
    base: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Option<Self> {
        Self::with_dispatchers(cctui_server_core::dispatchers::Registry::new()).await
    }
    async fn with_dispatchers(
        dispatchers: cctui_server_core::dispatchers::Registry,
    ) -> Option<Self> {
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|v| !v.trim().is_empty());
        let Some(url) = url else {
            assert!(std::env::var_os("CI").is_none(), "host contract needs a database");
            return None;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path=pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for ddl in [
            "CREATE TEMP TABLE users (LIKE public.users INCLUDING ALL)",
            "CREATE TEMP TABLE auth_keys (LIKE public.auth_keys INCLUDING ALL)",
            "CREATE TEMP TABLE user_tokens (LIKE public.user_tokens INCLUDING ALL)",
            "CREATE TEMP TABLE user_acls (LIKE public.user_acls INCLUDING ALL)",
            "CREATE TEMP TABLE key_acls (LIKE public.key_acls INCLUDING ALL)",
            "CREATE TEMP TABLE machines (LIKE public.machines INCLUDING ALL)",
            "CREATE TEMP TABLE events (LIKE public.events INCLUDING ALL)",
            "CREATE TEMP TABLE sessions (LIKE public.sessions INCLUDING ALL)",
            "CREATE TEMP TABLE session_tokens (LIKE public.session_tokens INCLUDING ALL)",
            "CREATE TEMP TABLE session_token_usage (LIKE public.session_token_usage INCLUDING ALL)",
            "CREATE TEMP TABLE session_spawn_capabilities (LIKE public.session_spawn_capabilities INCLUDING ALL)",
            "CREATE TEMP TABLE session_spawner_intents (LIKE public.session_spawner_intents INCLUDING ALL)",
            "CREATE TEMP TABLE adapters_enabled (LIKE public.adapters_enabled INCLUDING ALL)",
            "CREATE TEMP TABLE user_settings (LIKE public.user_settings INCLUDING ALL)",
            "CREATE TEMP TABLE accounts (LIKE public.accounts INCLUDING ALL)",
            "CREATE TEMP TABLE session_usage_totals (LIKE public.session_usage_totals INCLUDING ALL)",
            "CREATE TEMP TABLE instance_settings (LIKE public.instance_settings INCLUDING ALL)",
            "CREATE TEMP TABLE resource_shares (LIKE public.resource_shares INCLUDING ALL)",
            "CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING ALL)",
            "CREATE TEMP TABLE account_pools (LIKE public.account_pools INCLUDING ALL)",
            "CREATE TEMP TABLE dispatchers (LIKE public.dispatchers INCLUDING ALL)",
            "CREATE TEMP TABLE dispatch_policies (LIKE public.dispatch_policies INCLUDING ALL)",
            "CREATE TEMP TABLE dispatch_policy_uses (LIKE public.dispatch_policy_uses INCLUDING ALL)",
            "CREATE TEMP TABLE dispatch_handles (LIKE public.dispatch_handles INCLUDING ALL)",
            "CREATE TEMP TABLE session_webhooks (LIKE public.session_webhooks INCLUDING ALL)",
            "CREATE TEMP TABLE dispatch_records (LIKE public.dispatch_records INCLUDING ALL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        cctui_server_core::client::install_crypto_provider();
        let auth = AuthConfig::new(vec!["fixture-admin".into()], pool.clone());
        auth.seed_admin().await;
        let host = Host::new(
            CoreState::new(
                pool.clone(),
                auth,
                Bus::new(Box::new(NoopTransport)),
                cctui_server_core::client::build_http_client(),
                Arc::new(dispatchers),
            ),
            vec![42; 32],
            "https://automation.example.invalid".into(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router_host = host.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, cctui_core_server::router::router(router_host)).await.unwrap();
        });
        Some(Self {
            host,
            pool,
            base,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            task,
        })
    }
    async fn daemon(
        &self,
        key: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("{}/api/v1/daemon/ws", self.base.replace("http://", "ws://"))
            .into_client_request()
            .unwrap();
        request.headers_mut().insert("authorization", format!("Bearer {key}").parse().unwrap());
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
    async fn wait_for_spawner(&self, session: &str, user: &Value, expected_name: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let row: Option<(Option<uuid::Uuid>, Option<String>)> =
                    sqlx::query_as("SELECT spawned_by,session_name FROM sessions WHERE id=$1")
                        .bind(session)
                        .fetch_optional(&self.pool)
                        .await
                        .unwrap();
                if let Some((Some(spawner), Some(name))) = row {
                    assert_eq!(spawner.to_string(), user["id"].as_str().unwrap());
                    assert_eq!(name, expected_name);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn assert_completion_outbox(&self, session: &str, user_id: &str) {
        let row: (String, String, String, String) = sqlx::query_as(
            "SELECT state,task_id,notify_url,secret FROM session_webhooks WHERE session_id=$1",
        )
        .bind(session)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "pending".into(),
                "review-fixture".into(),
                "https://1.1.1.1/hook".into(),
                "fixture-signing-secret".into()
            )
        );
        // An unregistered HTTP worker cannot be probed; polling must not spend delivery retries.
        cctui_server_core::webhook::sweep(&self.host).await;
        let row: (String, i32, bool) = sqlx::query_as(
        "SELECT state,attempts,next_attempt_at > now() FROM session_webhooks WHERE session_id=$1",
    )
    .bind(session)
    .fetch_one(&self.pool)
    .await
    .unwrap();
        assert_eq!(row, ("pending".into(), 0, true));
        sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,user_id,adapter_id,status) VALUES ($1,'executor','/tmp',$2,'codex','ended')")
        .bind(session).bind(uuid::Uuid::parse_str(user_id).unwrap()).execute(&self.pool).await.unwrap();
        sqlx::query("UPDATE session_webhooks SET next_attempt_at=now() WHERE session_id=$1")
            .bind(session)
            .execute(&self.pool)
            .await
            .unwrap();
        cctui_server_core::webhook::sweep(&self.host).await;
        let state: String =
            sqlx::query_scalar("SELECT state FROM session_webhooks WHERE session_id=$1")
                .bind(session)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert_eq!(state, "superseded");
    }
    async fn wait_for_dispatcher(&self, key: &str, name: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let rows: Value = self
                    .client
                    .get(format!("{}/api/v1/dispatchers", self.base))
                    .bearer_auth(key)
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(rows[0]["name"], name);
                assert!(rows[0].get("dispatcher_key").is_none());
                if rows[0]["connected"] == true {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn post(&self, path: &str, token: &str, body: Value) -> Value {
        self.client
            .post(format!("{}{path}", self.base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn unattended_human_input_stops_the_worker_and_records_a_terminal_failure() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let machine = f
        .post("/api/v1/enroll", user["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let mut req = format!("{}/api/v1/daemon/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", machine["machine_key"].as_str().unwrap()).parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    for kind in ["permission", "question", "plan"] {
        let session = uuid::Uuid::new_v4().to_string();
        let prompt = match kind {
            "question" => AdapterEvent::AskQuestion {
                local_id: session.clone(),
                question: "choose an option".into(),
                questions: None,
                preamble: None,
            },
            "plan" => AdapterEvent::PlanRequest {
                local_id: session.clone(),
                plan: "review changes".into(),
                preamble: None,
            },
            _ => AdapterEvent::PermissionRequest {
                local_id: session.clone(),
                request_id: "fixture-request".into(),
                tool: "shell".into(),
                input: Value::Null,
                options: Vec::new(),
            },
        };
        for event in [
            AdapterEvent::SessionStarted {
                local_id: session.clone(),
                meta: SessionMeta::default(),
            },
            prompt,
        ] {
            socket
                .send(Message::Text(
                    serde_json::to_string(&DaemonFrameUp::Event {
                        adapter_id: "codex".into(),
                        event,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        }
        let command = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let frame = socket.next().await.unwrap().unwrap();
                if let Message::Text(text) = frame
                    && let Ok(DaemonFrameDown::Command { command, .. }) =
                        serde_json::from_str(&text)
                {
                    break command;
                }
            }
        })
        .await
        .expect("unattended work must not wait indefinitely for human input");
        assert!(
            matches!(*command,cctui_proto::adapter::AdapterCommand::Kill {ref local_id,..} if local_id==&session)
        );
        let row: (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status,end_reason,end_detail FROM sessions WHERE id=$1")
                .bind(&session)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(row.0, "ended");
        assert_eq!(row.1.as_deref(), Some("other"));
        assert!(row.2.unwrap().contains("human input"));
    }
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn owned_result_survives_replay_and_foreign_credentials_cannot_read_or_cancel_it() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let foreign =
        f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other-automation"})).await;
    let machine = f
        .post("/api/v1/enroll", user["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let mut req = format!("{}/api/v1/daemon/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", machine["machine_key"].as_str().unwrap()).parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    let text = json!({"type":"text","content":"review complete","role":"Assistant","message_id":"fixture-message"});
    let frames = vec![
        AdapterEvent::SessionStarted { local_id: session.clone(), meta: SessionMeta::default() },
        AdapterEvent::Message { local_id: session.clone(), payload: text.clone(), turn_id: None },
        AdapterEvent::Message { local_id: session.clone(), payload: text, turn_id: None },
        AdapterEvent::SessionEnded {
            local_id: session.clone(),
            reason: cctui_proto::adapter::EndReason::Completed,
        },
    ]
    .into_iter()
    .map(|event| DaemonFrameUp::Event { adapter_id: "opencode".into(), event })
    .collect();
    socket
        .send(Message::Text(
            serde_json::to_string(&DaemonFrameUp::Batch { frames }).unwrap().into(),
        ))
        .await
        .unwrap();
    let url = format!("{}/api/v1/sessions/{session}/result", f.base);
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response =
                f.client.get(&url).bearer_auth(user["key"].as_str().unwrap()).send().await.unwrap();
            if response.status().is_success() {
                let value: Value = response.json().await.unwrap();
                if value["done"] == true {
                    break value;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["final_assistant_text"], "review complete");
    assert_eq!(result["end_reason"], "completed");
    assert_eq!(result["final_assistant_text_truncated"], false);
    let denied =
        f.client.get(&url).bearer_auth(foreign["key"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
    let denied = f
        .client
        .post(format!("{}/api/v1/sessions/{session}/kill", f.base))
        .bearer_auth(foreign["key"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
    let status: String = sqlx::query_scalar("SELECT end_reason FROM sessions WHERE id=$1")
        .bind(&session)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(status, "completed");
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn cancellation_requires_dispatch_scope_and_stops_an_owned_running_worker() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let primary = user["key"].as_str().unwrap();
    let key = f
        .post(
            &format!("/api/v1/users/{}/keys", user["id"].as_str().unwrap()),
            primary,
            json!({"label":"reader","scopes":["read"]}),
        )
        .await;
    let machine = f.post("/api/v1/enroll", primary, json!({"hostname":"executor"})).await;
    let mut req = format!("{}/api/v1/daemon/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", machine["machine_key"].as_str().unwrap()).parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    socket
        .send(Message::Text(
            serde_json::to_string(&DaemonFrameUp::SessionRegistered {
                adapter_id: "codex".into(),
                local_id: session.clone(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let result_url = format!("{}/api/v1/sessions/{session}/result", f.base);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if f.client
                .get(&result_url)
                .bearer_auth(primary)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let url = format!("{}/api/v1/sessions/{session}/kill", f.base);
    let denied =
        f.client.post(&url).bearer_auth(key["key"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    let accepted = f.client.post(&url).bearer_auth(primary).send().await.unwrap();
    assert_eq!(accepted.status(), reqwest::StatusCode::NO_CONTENT);
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            if let Message::Text(text)=socket.next().await.unwrap().unwrap() && let Ok(DaemonFrameDown::Command {command,..})=serde_json::from_str(&text) {
                assert!(matches!(*command,cctui_proto::adapter::AdapterCommand::Kill {ref local_id,..} if local_id==&session));
                break;
            }
        }
    }).await.unwrap();
    let result: Value =
        f.client.get(&result_url).bearer_auth(primary).send().await.unwrap().json().await.unwrap();
    assert_eq!(result["done"], true);
    assert_eq!(result["end_reason"], "killed");
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn dispatch_through_the_host_delivers_an_authenticated_job_and_a_usable_worker_identity() {
    cctui_server_core::client::install_crypto_provider();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let executor = axum::Router::new().route(
        "/dispatch",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                tx.send((headers, body)).unwrap();
                async { axum::Json(json!({"handle":"job/fixture","status":"dispatched"})) }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/dispatch", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, executor).await.unwrap();
    });
    let dispatcher = cctui_server_core::dispatchers::http::HttpDispatcher::new(
        "fixture",
        endpoint,
        Some("executor-token".into()),
        reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap(),
    );
    let registry = cctui_server_core::dispatchers::Registry::new().with(Arc::new(dispatcher));
    let Some(f) = Fixture::with_dispatchers(registry).await else {
        task.abort();
        return;
    };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let reply=f.post("/api/v1/sessions/dispatch",user["key"].as_str().unwrap(),json!({"dispatcher":"fixture","timeout":1,"notify_url":"https://1.1.1.1/hook","notify_secret":"fixture-signing-secret","payload":{"task_id":"review-fixture","prompt_file":"review","env":{"FEATURE_FLAG":"fixture"}}})).await;
    let (headers, job) =
        tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    assert_eq!(headers["authorization"], "Bearer executor-token");
    assert_eq!(job["session_id"], reply["session_id"]);
    assert_eq!(job["payload"]["env"]["FEATURE_FLAG"], "fixture");
    let machine_key = job["payload"]["cctui_machine_key"].as_str().unwrap();
    assert!(machine_key.starts_with("cctui_m_"));
    let identity = f.post("/api/v1/daemon/auth", "", json!({"machine_key":machine_key})).await;
    assert_eq!(identity["user_id"], user["id"]);
    assert_eq!(reply["handle"], "job/fixture");
    let session = reply["session_id"].as_str().unwrap();
    let pending: cctui_proto::api::SessionResult = f
        .client
        .get(format!("{}/api/v1/sessions/{session}/result", f.base))
        .bearer_auth(user["key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!pending.done);
    assert!(!pending.queued);
    f.assert_completion_outbox(session, user["id"].as_str().unwrap()).await;
    let rejected = f
        .client
        .post(format!("{}/api/v1/sessions/dispatch", f.base))
        .bearer_auth(user["key"].as_str().unwrap())
        .json(&json!({"dispatcher":"fixture","notify_url":"https://127.0.0.1/hook","payload":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(rx.try_recv().is_err(), "unsafe callbacks must be refused before launch");
    task.abort();
}

#[tokio::test]
async fn daemon_launch_pulls_require_a_machine_key_and_withhold_foreign_session_credentials() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let primary = user["key"].as_str().unwrap();
    let machine = f.post("/api/v1/enroll", primary, json!({"hostname":"executor"})).await;
    let other = f.post("/api/v1/enroll", primary, json!({"hostname":"other-executor"})).await;
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,machine_uuid,user_id,adapter_id,status) VALUES ($1,'executor','/tmp',$2,$3,'codex','active')")
        .bind(&session).bind(uuid::Uuid::parse_str(machine["machine_id"].as_str().unwrap()).unwrap())
        .bind(uuid::Uuid::parse_str(user["id"].as_str().unwrap()).unwrap()).execute(&f.pool).await.unwrap();
    let url = format!("{}/api/v1/daemon/sessions/{session}/gateway-env", f.base);
    let response = f
        .client
        .get(&url)
        .bearer_auth(machine["machine_key"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["account_bound"], false);
    assert!(reply["spawn_capability"].is_object());
    let response = f
        .client
        .get(&url)
        .bearer_auth(other["machine_key"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["spawn_capability"], Value::Null);
    assert_eq!(reply["env"], json!({}));
    assert_eq!(
        f.client.get(&url).bearer_auth(primary).send().await.unwrap().status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let context: Value = f
        .client
        .get(format!("{}/api/v1/daemon/sessions/{session}/context", f.base))
        .bearer_auth(machine["machine_key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(context["context"], json!([]));
}

#[tokio::test]
async fn bound_daemon_launch_restores_credentials_and_token_validity_without_exposing_provider_secrets()
 {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let uid = uuid::Uuid::parse_str(user["id"].as_str().unwrap()).unwrap();
    let machine = f
        .post("/api/v1/enroll", user["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let mid = uuid::Uuid::parse_str(machine["machine_id"].as_str().unwrap()).unwrap();
    let account: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (user_id,name) VALUES ($1,'fixture') RETURNING id",
    )
    .bind(uid)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let secret = cctui_crypto::encrypt("fixture-provider-secret", &[42; 32]);
    let provider: uuid::Uuid = sqlx::query_scalar("INSERT INTO account_providers (user_id,account_id,provider,encrypted_access_token,auth_scheme) VALUES ($1,$2,'openai',$3,'bearer') RETURNING id").bind(uid).bind(account).bind(secret).fetch_one(&f.pool).await.unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,machine_uuid,user_id,adapter_id,status,account_id,end_reason,ended_at) VALUES ($1,'executor','/tmp',$2,$3,'codex','ended',$4,'daemon_lost',now())").bind(&session).bind(mid).bind(uid).bind(provider.to_string()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO instance_settings (key,value) VALUES ('spawn_defaults',$1)")
        .bind(json!({"max_children":2,"max_depth":1}))
        .execute(&f.pool)
        .await
        .unwrap();
    let base = format!("{}/api/v1/daemon/sessions/{session}", f.base);
    let key = machine["machine_key"].as_str().unwrap();
    let reply: Value = f
        .client
        .get(format!("{base}/gateway-env"))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reply["account_bound"], true);
    assert_eq!(reply["settings"]["service_tier"], "default");
    assert_eq!(reply["spawn_capability"]["max_children"], 2);
    assert_eq!(reply["spawn_capability"]["max_depth"], 1);
    assert!(!reply.to_string().contains("fixture-provider-secret"));
    let token = reply["env"]["OPENAI_API_KEY"].as_str().unwrap();
    let hash = cctui_server_core::auth::sha256_hex(token);
    let validity: Value = f
        .client
        .get(format!("{base}/token-valid?hash={hash}"))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(validity["valid"], true);
    let status: String = sqlx::query_scalar("SELECT status FROM sessions WHERE id=$1")
        .bind(&session)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    let other = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other"})).await;
    let foreign = f
        .post(
            "/api/v1/enroll",
            other["key"].as_str().unwrap(),
            json!({"hostname":"other-executor"}),
        )
        .await;
    assert_eq!(
        f.client
            .get(format!("{base}/token-valid?hash={hash}"))
            .bearer_auth(foreign["machine_key"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE session_tokens SET revoked_at=now() WHERE session_id=$1")
        .bind(&session)
        .execute(&f.pool)
        .await
        .unwrap();
    let validity: Value = f
        .client
        .get(format!("{base}/token-valid?hash={hash}"))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(validity["valid"], false);
}

#[tokio::test]
async fn account_administration_provisions_credentials_and_keeps_them_private_and_owner_scoped() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let key = user["key"].as_str().unwrap();
    let account = f
        .post(
            "/api/v1/accounts",
            key,
            json!({"name":"fixture","provider":"openai","refresh_token":"fixture-refresh"}),
        )
        .await;
    assert_eq!(account["name"], "fixture");
    assert_eq!(account["providers"][0]["provider"], "openai");
    assert!(!account.to_string().contains("fixture-refresh"));
    let id = account["id"].as_str().unwrap();
    let stored: String = sqlx::query_scalar(
        "SELECT encrypted_refresh_token FROM account_providers WHERE account_id=$1",
    )
    .bind(uuid::Uuid::parse_str(id).unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_ne!(stored, "fixture-refresh");
    assert_eq!(cctui_crypto::decrypt(&stored, &[42; 32]).as_deref(), Some("fixture-refresh"));
    let other = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other"})).await;
    assert_eq!(
        f.client
            .get(format!("{}/api/v1/accounts/{id}", f.base))
            .bearer_auth(other["key"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let machine = f.post("/api/v1/enroll", key, json!({"hostname":"executor"})).await;
    assert_eq!(
        f.client
            .get(format!("{}/api/v1/accounts", f.base))
            .bearer_auth(machine["machine_key"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn oauth_login_state_is_owner_bound_and_consumed_once() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let other = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other"})).await;
    let login = f
        .post(
            "/api/v1/accounts/oauth/start",
            owner["key"].as_str().unwrap(),
            json!({"provider":"openai"}),
        )
        .await;
    assert!(login["authorize_url"].as_str().unwrap().contains("code_challenge="));
    let url = format!("{}/api/v1/accounts/oauth/finish", f.base);
    let finish = json!({"nonce":login["nonce"],"code":"fixture-code"});
    let refused: Value = f
        .client
        .post(&url)
        .bearer_auth(other["key"].as_str().unwrap())
        .json(&finish)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refused["error"], "unknown or expired login");
    let consumed: Value = f
        .client
        .post(&url)
        .bearer_auth(owner["key"].as_str().unwrap())
        .json(&finish)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(consumed["error"], "name required");
    let repeated: Value = f
        .client
        .post(&url)
        .bearer_auth(owner["key"].as_str().unwrap())
        .json(&finish)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repeated["error"], "unknown or expired login");
}

#[tokio::test]
async fn machine_spawn_refuses_read_only_keys_and_unsupported_workspace_inputs() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let primary = user["key"].as_str().unwrap();
    let reader = f
        .post(
            &format!("/api/v1/users/{}/keys", user["id"].as_str().unwrap()),
            primary,
            json!({"label":"reader","scopes":["read"]}),
        )
        .await;
    let machine = f.post("/api/v1/enroll", primary, json!({"hostname":"executor"})).await;
    let body = json!({"machine_id":machine["machine_id"],"working_dir":"/work/acme","prompt":"review","no_account":true});
    let url = format!("{}/api/v1/sessions/spawn", f.base);
    let response = f
        .client
        .post(&url)
        .bearer_auth(reader["key"].as_str().unwrap())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    for field in [
        "save_draft",
        "auto_archive",
        "context",
        "profile_id",
        "attachment_names",
        "label_ids",
        "parent_session_id",
    ] {
        let mut invalid = body.clone();
        invalid[field] = match field {
            "save_draft" | "auto_archive" => json!(true),
            "context" => json!({"items":["fixture"]}),
            "profile_id" => json!(uuid::Uuid::new_v4()),
            "attachment_names" | "label_ids" => json!(["fixture"]),
            _ => json!("fixture"),
        };
        let response =
            f.client.post(&url).bearer_auth(primary).json(&invalid).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST, "{field}");
    }
}

#[tokio::test]
async fn machine_spawn_delivers_bound_credentials_rebinds_identity_and_caps_child_permissions() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let key = user["key"].as_str().unwrap();
    f.post(
        "/api/v1/accounts",
        key,
        json!({"name":"fixture","provider":"openai","refresh_token":"fixture-refresh"}),
    )
    .await;
    let machine = f.post("/api/v1/enroll", key, json!({"hostname":"executor"})).await;
    let mut request = format!("{}/api/v1/daemon/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", machine["machine_key"].as_str().unwrap()).parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // Consume reconciliation before issuing the request so target presence is established.
    tokio::time::timeout(Duration::from_secs(3), socket.next()).await.unwrap().unwrap().unwrap();
    let client = f.client.clone();
    let url = format!("{}/api/v1/sessions/spawn", f.base);
    let token = key.to_owned();
    let body = json!({"machine_id":machine["machine_id"],"working_dir":"/work/acme","adapter_id":"codex","prompt":"review","name":"review-run","account":"fixture","permission_mode":"ask","env":{"TASK_FLAG":"fixture"},"spawn_capability":{"adapters":["codex"],"max_children":2,"max_permission_mode":"yolo"}});
    let pending = tokio::spawn(async move {
        client.post(url).bearer_auth(token).json(&body).send().await.unwrap()
    });
    let command = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Ok(Message::Text(text))) = socket.next().await
                && let Ok(DaemonFrameDown::Command { command, .. }) = serde_json::from_str(&text)
            {
                break command;
            }
        }
    })
    .await
    .unwrap();
    let cctui_proto::adapter::AdapterCommand::Spawn { spec, command_id, session_id } = *command
    else {
        panic!("expected spawn")
    };
    assert_eq!(spec.prompt.as_deref(), Some("review"));
    assert_eq!(spec.env.get("TASK_FLAG").map(String::as_str), Some("fixture"));
    assert!(spec.env.contains_key("OPENAI_API_KEY"));
    assert!(!serde_json::to_string(&spec).unwrap().contains("fixture-refresh"));
    let spawn_key = session_id.unwrap().to_string();
    let command_id = command_id.unwrap();
    let grant = cctui_server_core::store::spawn_capabilities::get(&f.pool, &spawn_key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.max_children, Some(2));
    assert_eq!(grant.max_permission_mode, Some(cctui_proto::adapter::PermissionMode::Ask));
    let session = uuid::Uuid::new_v4().to_string();
    for event in [
        AdapterEvent::CommandResult { command_id, ok: true, error: None },
        AdapterEvent::SessionStarted {
            local_id: session.clone(),
            meta: SessionMeta { extra: json!({"spawn_key":spawn_key}), ..Default::default() },
        },
    ] {
        socket
            .send(Message::Text(
                serde_json::to_string(&DaemonFrameUp::Event { adapter_id: "codex".into(), event })
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    let response = pending.await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["command_id"], command_id.to_string());
    f.wait_for_spawner(&session, &user, "review-run").await;
    assert!(
        cctui_server_core::store::spawn_capabilities::get(&f.pool, &session)
            .await
            .unwrap()
            .is_some()
    );
    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM session_tokens WHERE session_id=$1 AND revoked_at IS NULL)",
    )
    .bind(&session)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(bound);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn machine_spawn_enforces_target_shares_environment_and_reported_harness_health() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let other = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other"})).await;
    let key = owner["key"].as_str().unwrap();
    let foreign = other["key"].as_str().unwrap();
    let machine = f.post("/api/v1/enroll", key, json!({"hostname":"executor"})).await;
    let machine_id = uuid::Uuid::parse_str(machine["machine_id"].as_str().unwrap()).unwrap();
    let url = format!("{}/api/v1/sessions/spawn", f.base);
    let body = json!({"machine_id":machine_id,"working_dir":"/work/acme","no_account":true});
    for (token, status) in [(key, 503), (foreign, 403)] {
        let response = f.client.post(&url).bearer_auth(token).json(&body).send().await.unwrap();
        assert_eq!(response.status().as_u16(), status);
    }
    for (field, value) in [("env", json!({"bad-key":"fixture"})), ("adapter_id", json!("unknown"))]
    {
        let mut invalid = body.clone();
        invalid[field] = value;
        let response = f.client.post(&url).bearer_auth(key).json(&invalid).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    }
    let grantee = uuid::Uuid::parse_str(other["id"].as_str().unwrap()).unwrap();
    sqlx::query("INSERT INTO resource_shares(resource_type,resource_id,grantee_id,action) VALUES('machine',$1,$2,'use')").bind(machine_id).bind(grantee).execute(&f.pool).await.unwrap();
    let response = f.client.post(&url).bearer_auth(foreign).json(&body).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    sqlx::query("UPDATE resource_shares SET action='spawn' WHERE resource_id=$1")
        .bind(machine_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let response = f.client.post(&url).bearer_auth(foreign).json(&body).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    sqlx::query("UPDATE resource_shares SET revoked_at=now() WHERE resource_id=$1")
        .bind(machine_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let response = f.client.post(&url).bearer_auth(foreign).json(&body).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    sqlx::query("UPDATE machines SET harness_report=$2 WHERE id=$1")
        .bind(machine_id)
        .bind(json!({"binaries":{"claude-code":{"bin":"claude","config_key":"bin","path":null}}}))
        .execute(&f.pool)
        .await
        .unwrap();
    let response = f.client.post(&url).bearer_auth(key).json(&body).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
}

#[tokio::test]
async fn daemon_registration_cannot_claim_a_foreign_launch_intent_or_its_credentials() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let other = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"other"})).await;
    let account = f
        .post(
            "/api/v1/accounts",
            other["key"].as_str().unwrap(),
            json!({"name":"fixture","provider":"openai","refresh_token":"fixture-refresh"}),
        )
        .await;
    let foreign = uuid::Uuid::parse_str(other["id"].as_str().unwrap()).unwrap();
    let provider = uuid::Uuid::parse_str(account["providers"][0]["id"].as_str().unwrap()).unwrap();
    let spawn_key = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO session_tokens(token_hash,account_id,user_id,session_id) VALUES($1,$2,$3,$4)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(provider)
    .bind(foreign)
    .bind(&spawn_key)
    .execute(&f.pool)
    .await
    .unwrap();
    cctui_server_core::machine_policy::remember_spawner(
        &f.pool,
        &spawn_key,
        foreign,
        Some("private-name"),
    )
    .await;
    let machine = f
        .post("/api/v1/enroll", owner["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let mut socket = f.daemon(machine["machine_key"].as_str().unwrap()).await;
    let session = uuid::Uuid::new_v4().to_string();
    for event in [
        AdapterEvent::SessionStarted {
            local_id: session.clone(),
            meta: SessionMeta { extra: json!({"spawn_key":spawn_key}), ..Default::default() },
        },
        AdapterEvent::SessionEnded {
            local_id: session.clone(),
            reason: cctui_proto::adapter::EndReason::Completed,
        },
    ] {
        socket
            .send(Message::Text(
                serde_json::to_string(&DaemonFrameUp::Event { adapter_id: "codex".into(), event })
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let row: Option<(Option<uuid::Uuid>, Option<String>, String)> =
                sqlx::query_as("SELECT spawned_by,session_name,status FROM sessions WHERE id=$1")
                    .bind(&session)
                    .fetch_optional(&f.pool)
                    .await
                    .unwrap();
            if let Some((spawner, name, status)) = row
                && status == "ended"
            {
                assert_eq!(spawner, None);
                assert_eq!(name, None);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM session_spawner_intents WHERE spawn_key=$1)",
    )
    .bind(&spawn_key)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(retained);
    let token_session: String =
        sqlx::query_scalar("SELECT session_id FROM session_tokens WHERE account_id=$1")
            .bind(provider)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(token_session, spawn_key);
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn daemon_checkpoints_are_durable_monotonic_and_acknowledged_with_idempotent_usage() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let machine = f
        .post("/api/v1/enroll", owner["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let key = machine["machine_key"].as_str().unwrap();
    let mut socket = f.daemon(key).await;
    let session = uuid::Uuid::new_v4().to_string();
    socket
        .send(Message::Text(
            json!({"type":"heartbeat","sent_at":"2026-01-01T00:00:00Z","mark_acks":true})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let usage = AdapterEvent::TokenUsage {
        local_id: session.clone(),
        message_id: "fixture-usage".into(),
        input_tokens: 101,
        output_tokens: 17,
        cache_read_tokens: 5,
        cache_creation_tokens: 4,
    };
    for event in [
        AdapterEvent::SessionStarted { local_id: session.clone(), meta: SessionMeta::default() },
        usage.clone(),
        usage,
        AdapterEvent::TranscriptMark { local_id: session.clone(), offset: 100 },
        AdapterEvent::TranscriptMark { local_id: session.clone(), offset: 40 },
    ] {
        socket
            .send(Message::Text(
                serde_json::to_string(&DaemonFrameUp::Event { adapter_id: "codex".into(), event })
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut acks = Vec::new();
        while acks.len() < 2 {
            if let Some(Ok(Message::Text(text))) = socket.next().await
                && let Ok(DaemonFrameDown::TranscriptAck { session_marks, .. }) =
                    serde_json::from_str(&text)
            {
                acks.extend(session_marks);
            }
        }
        assert_eq!(acks, vec![(session.clone(), 100), (session.clone(), 40)]);
    })
    .await
    .expect("durably accepted checkpoints must be acknowledged");
    let offset: i64 = sqlx::query_scalar("SELECT transcript_offset FROM sessions WHERE id=$1")
        .bind(&session)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(offset, 100);
    let usage:(i64,i64,i64,i64,i64)=sqlx::query_as("SELECT count(*)::bigint,sum(input_tokens)::bigint,sum(output_tokens)::bigint,sum(cache_read_tokens)::bigint,sum(cache_creation_tokens)::bigint FROM session_token_usage WHERE session_id=$1").bind(&session).fetch_one(&f.pool).await.unwrap();
    assert_eq!(usage, (1, 101, 17, 5, 4));
    socket.close(None).await.unwrap();
    let mut resumed = f.daemon(key).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Ok(Message::Text(text))) = resumed.next().await
                && let Ok(DaemonFrameDown::ResumeMarks { session_marks, .. }) =
                    serde_json::from_str(&text)
            {
                assert_eq!(session_marks, vec![(session.clone(), 100)]);
                break;
            }
        }
    })
    .await
    .expect("reconnect must resume at the stored checkpoint");
    resumed.close(None).await.unwrap();
}

#[tokio::test]
async fn an_older_daemon_reconnecting_after_a_newer_one_receives_no_checkpoint_ack() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let machine = f
        .post("/api/v1/enroll", owner["key"].as_str().unwrap(), json!({"hostname":"executor"}))
        .await;
    let key = machine["machine_key"].as_str().unwrap();
    let mut modern = f.daemon(key).await;
    modern
        .send(Message::Text(
            json!({"type":"heartbeat","sent_at":"2026-01-01T00:00:00Z","mark_acks":true})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    for event in [
        AdapterEvent::SessionStarted { local_id: session.clone(), meta: SessionMeta::default() },
        AdapterEvent::TranscriptMark { local_id: session.clone(), offset: 50 },
    ] {
        modern
            .send(Message::Text(
                serde_json::to_string(&DaemonFrameUp::Event { adapter_id: "codex".into(), event })
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Ok(Message::Text(text))) = modern.next().await
                && matches!(
                    serde_json::from_str::<DaemonFrameDown>(&text),
                    Ok(DaemonFrameDown::TranscriptAck { .. })
                )
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    modern.close(None).await.unwrap();
    let mut older = f.daemon(key).await;
    older
        .send(Message::Text(
            json!({"type":"heartbeat","sent_at":"2026-01-01T00:00:00Z"}).to_string().into(),
        ))
        .await
        .unwrap();
    older
        .send(Message::Text(
            serde_json::to_string(&DaemonFrameUp::Event {
                adapter_id: "codex".into(),
                event: AdapterEvent::TranscriptMark { local_id: session.clone(), offset: 100 },
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mark: i64 =
                sqlx::query_scalar("SELECT transcript_offset FROM sessions WHERE id=$1")
                    .bind(&session)
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            if mark == 100 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let received = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            if let Some(Ok(Message::Text(text))) = older.next().await
                && matches!(
                    serde_json::from_str::<DaemonFrameDown>(&text),
                    Ok(DaemonFrameDown::TranscriptAck { .. })
                )
            {
                break;
            }
        }
    })
    .await;
    assert!(received.is_err(), "never send an unknown frame to an older daemon");
    older.close(None).await.unwrap();
}

#[tokio::test]
async fn reaper_ended_oom_keeps_the_server_failure_callback() {
    let Some(f) = Fixture::new().await else { return };
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,adapter_id,status,end_reason,end_detail) VALUES ($1,'executor','/tmp','codex','ended','oom_killed','OOMKilled')")
        .bind(&session).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO session_webhooks (session_id,notify_url,task_id) VALUES ($1,'https://127.0.0.1:1/hook','fixture-task')")
        .bind(&session).execute(&f.pool).await.unwrap();
    cctui_server_core::webhook::sweep(&f.host).await;
    let row: (String, i32, Option<Value>) =
        sqlx::query_as("SELECT state,attempts,payload FROM session_webhooks WHERE session_id=$1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        (
            "pending".into(),
            1,
            Some(json!({
                "task_id":"fixture-task", "status":"failed", "error":"OOMKilled"
            }))
        )
    );
}

#[tokio::test]
async fn completion_runtime_freezes_failure_payloads_and_preserves_the_retry_budget() {
    let Some(f) = Fixture::new().await else { return };
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,adapter_id,status) VALUES ($1,'executor','/tmp','codex','failed')")
        .bind(&session).execute(&f.pool).await.unwrap();
    // An unsafe historical target fails immediately through the guarded client;
    // this exercises delivery failure without contacting an external receiver.
    sqlx::query("INSERT INTO session_webhooks (session_id,notify_url,task_id) VALUES ($1,'https://127.0.0.1:1/hook','fixture-task')")
        .bind(&session).execute(&f.pool).await.unwrap();
    let runtime = cctui_core_server::background::BackgroundTasks::start(f.host.clone());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let attempts: i32 =
                sqlx::query_scalar("SELECT attempts FROM session_webhooks WHERE session_id=$1")
                    .bind(&session)
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            if attempts == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(runtime);
    let row: (String, Value, bool) = sqlx::query_as(
        "SELECT state,payload,next_attempt_at > now() FROM session_webhooks WHERE session_id=$1",
    )
    .bind(&session)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let expected =
        json!({"task_id":"fixture-task","status":"failed","error":"dispatch never launched"});
    assert_eq!(row, ("pending".into(), expected.clone(), true));
    // A later lifecycle update must not rewrite a payload already being retried.
    sqlx::query("UPDATE sessions SET status='ended' WHERE id=$1")
        .bind(&session)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE session_webhooks SET attempts=10,next_attempt_at=now() WHERE session_id=$1",
    )
    .bind(&session)
    .execute(&f.pool)
    .await
    .unwrap();
    cctui_server_core::webhook::sweep(&f.host).await;
    let row: (String, i32, Value) =
        sqlx::query_as("SELECT state,attempts,payload FROM session_webhooks WHERE session_id=$1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(row, ("dead".into(), 11, expected));
}

#[tokio::test]
async fn enrolled_dispatcher_delivers_correlated_work_and_revocation_blocks_reconnection() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let key = user["key"].as_str().unwrap();
    let enrolled = f
        .post(
            "/api/v1/dispatcher/enroll",
            key,
            json!({"name":"enrolled-executor","kind":"kubernetes"}),
        )
        .await;
    let dispatcher_key = enrolled["dispatcher_key"].as_str().unwrap();
    let auth = f.post("/api/v1/dispatcher/auth", "", json!({"machine_key":dispatcher_key})).await;
    assert_eq!(auth["machine_id"], enrolled["dispatcher_id"]);
    assert_eq!(auth["user_id"], user["id"]);
    let mut request = format!("{}/api/v1/dispatcher/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {dispatcher_key}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({"type":"hello","kind":"kubernetes","version":"fixture"}).to_string().into(),
        ))
        .await
        .unwrap();
    f.wait_for_dispatcher(key, "enrolled-executor").await;
    let dispatch = f.client.post(format!("{}/api/v1/sessions/dispatch", f.base)).bearer_auth(key)
        .json(&json!({"dispatcher":"enrolled-executor","timeout":1,"payload":{"prompt_file":"review"}}));
    let job = tokio::spawn(async move {
        dispatch.send().await.unwrap().error_for_status().unwrap().json::<Value>().await.unwrap()
    });
    let frame: Value = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["type"] == "dispatch" {
                    break frame;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(
        frame["spec"]["payload"]["cctui_machine_key"].as_str().unwrap().starts_with("cctui_m_")
    );
    socket
        .send(Message::Text(
            json!({"type":"dispatch_result","request_id":frame["request_id"],
        "session_id":frame["spec"]["session_id"],"handle":"job/fixture","status":"dispatched"})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), job).await.unwrap().unwrap();
    assert_eq!(reply["handle"], "job/fixture");
    assert_eq!(reply["session_id"], frame["spec"]["session_id"]);
    let deleted = f
        .client
        .delete(format!(
            "{}/api/v1/dispatchers/{}",
            f.base,
            enrolled["dispatcher_id"].as_str().unwrap()
        ))
        .bearer_auth(key)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);
    let rejected = f
        .client
        .post(format!("{}/api/v1/dispatcher/auth", f.base))
        .json(&json!({"machine_key":dispatcher_key}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
    let mut request = format!("{}/api/v1/dispatcher/ws", f.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {dispatcher_key}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
}

#[tokio::test]
async fn dispatcher_management_enforces_scope_ownership_and_owner_bound_default_accounts() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"owner"})).await;
    let foreign = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"foreign"})).await;
    let key = owner["key"].as_str().unwrap();
    let reader = f
        .post(
            &format!("/api/v1/users/{}/keys", owner["id"].as_str().unwrap()),
            key,
            json!({"label":"reader","scopes":["read"]}),
        )
        .await;
    let refused = f
        .client
        .post(format!("{}/api/v1/dispatcher/enroll", f.base))
        .bearer_auth(reader["key"].as_str().unwrap())
        .json(&json!({"name":"denied"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::FORBIDDEN);
    let enrolled = f.post("/api/v1/dispatcher/enroll", key, json!({"name":"owned-executor"})).await;
    let url =
        format!("{}/api/v1/dispatchers/{}", f.base, enrolled["dispatcher_id"].as_str().unwrap());
    let hidden: Value = f
        .client
        .get(format!("{}/api/v1/dispatchers", f.base))
        .bearer_auth(foreign["key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hidden, json!([]));
    assert_eq!(
        f.client
            .delete(&url)
            .bearer_auth(foreign["key"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.client
            .patch(&url)
            .bearer_auth(reader["key"].as_str().unwrap())
            .json(&json!({"name":"denied"}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let uid = uuid::Uuid::parse_str(owner["id"].as_str().unwrap()).unwrap();
    sqlx::query("INSERT INTO accounts (user_id,name) VALUES ($1,'owner-default')")
        .bind(uid)
        .execute(&f.pool)
        .await
        .unwrap();
    let renamed: Value = f
        .client
        .patch(&url)
        .bearer_auth("fixture-admin")
        .json(&json!({"name":"renamed-executor","default_account":"owner-default"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["name"], "renamed-executor");
    assert_eq!(renamed["default_account"], "owner-default");
    assert!(renamed.get("dispatcher_key").is_none());
}

#[tokio::test]
async fn unregistered_run_results_report_queue_position_failure_and_ownership() {
    let Some(f) = Fixture::new().await else { return };
    let owner = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"owner"})).await;
    let foreign = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"foreign"})).await;
    let uid = uuid::Uuid::parse_str(owner["id"].as_str().unwrap()).unwrap();
    let first = uuid::Uuid::new_v4().to_string();
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO dispatch_records (session_id,user_id,dispatcher_name,profile,outcome,queued,created_at) VALUES ($1,$3,'executor','review','forwarded',true,'2026-01-01T00:00:00Z'),($2,$3,'executor','review','forwarded',true,'2026-01-01T00:00:01Z')")
        .bind(first).bind(&session).bind(uid).execute(&f.pool).await.unwrap();
    let url = format!("{}/api/v1/sessions/{session}/result", f.base);
    let result: cctui_proto::api::SessionResult = f
        .client
        .get(&url)
        .bearer_auth(owner["key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!result.done);
    assert!(result.queued);
    assert_eq!(result.queue_position, Some(2));
    let denied =
        f.client.get(&url).bearer_auth(foreign["key"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::NOT_FOUND);
    cctui_server_core::store::dispatch_records::set_queued(&f.pool, &session, false).await.unwrap();
    let result: cctui_proto::api::SessionResult = f
        .client
        .get(&url)
        .bearer_auth(owner["key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!result.done);
    assert!(!result.queued);
    assert!(result.queue_position.is_none());
    cctui_server_core::store::dispatch_records::mark_failed(
        &f.pool,
        &session,
        "oom_killed",
        "OOMKilled",
    )
    .await
    .unwrap();
    let result: cctui_proto::api::SessionResult = f
        .client
        .get(&url)
        .bearer_auth(owner["key"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(result.done);
    assert_eq!(result.end_reason.as_deref(), Some("oom_killed"));
    assert!(!result.queued);
    assert!(result.queue_position.is_none());
}

#[tokio::test]
async fn workload_runtime_records_a_dispatcher_reported_oom_as_a_terminal_owned_result() {
    let Some(f) = Fixture::new().await else { return };
    let user = f.post("/api/v1/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    let key = user["key"].as_str().unwrap();
    let uid = uuid::Uuid::parse_str(user["id"].as_str().unwrap()).unwrap();
    let executor = f
        .post(
            "/api/v1/dispatcher/enroll",
            key,
            json!({"name":"runtime-executor","kind":"kubernetes"}),
        )
        .await;
    let dispatcher = uuid::Uuid::parse_str(executor["dispatcher_id"].as_str().unwrap()).unwrap();
    let machine = f.post("/api/v1/enroll", key, json!({"hostname":"runtime-worker"})).await;
    let mid = uuid::Uuid::parse_str(machine["machine_id"].as_str().unwrap()).unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sessions (id,machine_id,working_dir,machine_uuid,user_id,adapter_id,status) VALUES ($1,'runtime-worker','/tmp',$2,$3,'codex','active')")
        .bind(&session).bind(mid).bind(uid).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO dispatch_handles (session_id,dispatcher_name,handle) VALUES ($1,'runtime-executor','job/fixture')")
        .bind(&session).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO dispatch_records (session_id,user_id,dispatcher_name,outcome) VALUES ($1,$2,'runtime-executor','forwarded')")
        .bind(&session).bind(uid).execute(&f.pool).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    f.host.bus.register_dispatcher(dispatcher, tx);
    let bus = f.host.bus.clone();
    let responder = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let cctui_proto::ws::DispatcherFrameDown::Status { request_id, handle } = frame else {
                panic!("expected workload status probe")
            };
            bus.resolve_dispatcher_reply(
                request_id,
                cctui_proto::ws::DispatcherFrameUp::StatusResult {
                    request_id,
                    handle,
                    state: Some("failed".into()),
                    error: Some("OOMKilled".into()),
                    worker_exit: Some(cctui_proto::api::WorkerExit {
                        exit_code: Some(137),
                        reason: Some("OOMKilled".into()),
                        ..Default::default()
                    }),
                },
            );
        }
    });
    let runtime = cctui_core_server::background::BackgroundTasks::start(f.host.clone());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let reason: Option<String> =
                sqlx::query_scalar("SELECT end_reason FROM sessions WHERE id=$1")
                    .bind(&session)
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            if reason.as_deref() == Some("oom_killed") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let result: cctui_proto::api::SessionResult = f
        .client
        .get(format!("{}/api/v1/sessions/{session}/result", f.base))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(result.done);
    assert_eq!(result.end_reason.as_deref(), Some("oom_killed"));
    let exit: Value =
        sqlx::query_scalar("SELECT worker_exit FROM dispatch_records WHERE session_id=$1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(exit["exit_code"], 137);
    assert_eq!(exit["reason"], "OOMKilled");
    drop(runtime);
    responder.abort();
}
