use cctui_proto::adapter::PermissionMode;
use cctui_server_dispatch::dispatch::payload_adapter;

use crate::dispatch_context::{context_record_is_callers, dispatch_context_scope};

async fn seed_user(pool: &sqlx::PgPool) -> uuid::Uuid {
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("dispatch-svc-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    uid
}

#[test]
fn dispatch_notices_map_to_typed_kinds() {
    use cctui_server_dispatch::dispatch::{DispatchEvent, DispatchNotification};
    let owner = uuid::Uuid::new_v4();
    let notice = |event| DispatchNotification {
        event,
        session_id: "s1".into(),
        title: "t".into(),
        message: "m".into(),
        owner: None,
        registered: false,
    };
    let received = crate::dispatch_context::dispatch_notice(owner, notice(DispatchEvent::Received));
    assert_eq!((received.kind, received.user_id), ("dispatch_info", owner));
    assert_eq!(received.collapse_key.as_deref(), Some("dispatch:s1"));
    assert_eq!(received.subject_id.as_deref(), Some("s1"));
    assert_eq!(received.session_id, None, "the worker session is not registered yet");
    assert!(!crate::notify::is_recorded(received.kind));

    let failed = crate::dispatch_context::dispatch_notice(owner, notice(DispatchEvent::Failed));
    assert_eq!((failed.kind, failed.severity), ("dispatch_failed", "critical"));
    assert_eq!(failed.collapse_key.as_deref(), Some("dispatch_failed:s1"), "one per dispatch");
    assert_eq!(failed.tags.as_deref(), Some("rotating_light"));
    assert_eq!(failed.session_id, None, "an unregistered worker opens its failure record");
    assert_eq!(
        (failed.subject_kind.as_deref(), failed.subject_id.as_deref()),
        (Some("dispatch"), Some("s1"))
    );
    assert!(crate::notify::is_recorded(failed.kind));

    let crashed = crate::dispatch_context::dispatch_notice(
        owner,
        DispatchNotification { registered: true, ..notice(DispatchEvent::Failed) },
    );
    assert_eq!(crashed.session_id.as_deref(), Some("s1"), "a registered session opens itself");
}

/// A dispatch naming an existing session id must not re-scope that
/// session's context set to the dispatcher: only the user who recorded the
/// set, or who owns the registered session, may overwrite it.
#[tokio::test]
async fn a_dispatch_only_overwrites_its_own_users_context_set() {
    let Some(url) = crate::routes::gateway::test_db_url("dispatch_context_owner") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (owner, other) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    for uid in [owner, other] {
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("dctx-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
    }
    let key = uuid::Uuid::new_v4().to_string();
    let scope = dispatch_context_scope(owner, &serde_json::json!({}));
    crate::routes::context::remember_intent(&pool, &key, &[], &scope).await;
    let registered = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
         VALUES ($1, 'm1', '/w', $2, 'claude-code')",
    )
    .bind(&registered)
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seed session");

    assert!(context_record_is_callers(&pool, &key, owner).await, "the recording user");
    assert!(!context_record_is_callers(&pool, &key, other).await, "another user's set");
    assert!(context_record_is_callers(&pool, &registered, owner).await, "the session owner");
    assert!(
        !context_record_is_callers(&pool, &registered, other).await,
        "another user's registered session"
    );
    assert!(
        context_record_is_callers(&pool, &uuid::Uuid::new_v4().to_string(), other).await,
        "a fresh id records freely"
    );

    sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
        .bind(&key)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&registered).execute(&pool).await.ok();
    for uid in [owner, other] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}

#[tokio::test]
async fn with_context_off_a_dispatch_records_only_its_owner() {
    use crate::routes::context::ownership::test_support::assert_no_content;
    use crate::routes::context::ownership::{self};
    use cctui_proto::features::{Feature, FeatureSet};
    use cctui_server_dispatch::dispatch::DispatchContext as _;
    let Some(url) = crate::routes::gateway::test_db_url("dispatch_context_off") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let mut state = crate::state::AppState::for_test(pool.clone());
    state.config.features = FeatureSet::all().without(Feature::Context);
    let (owner, other) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let request = cctui_proto::api::DispatchRequest {
        dispatcher: "worker".into(),
        session_id: None,
        timeout: None,
        reply_url: None,
        notify_url: None,
        notify_secret: None,
        payload: serde_json::json!({"adapter_id": "codex", "working_dir": "/w"}),
        account: None,
        provider: None,
        accounts: Vec::new(),
    };
    let session = uuid::Uuid::new_v4().to_string();

    state.remember_dispatch_context(Some(owner), &request, &session).await;
    assert_eq!(ownership::owner_of(&pool, &session).await.expect("lookup"), Some(owner));
    let scope = ownership::launch_scope(&pool, &session).await.expect("recorded");
    assert!(scope.dispatched);
    assert_eq!(scope.coords.adapter.as_deref(), Some("codex"));
    assert_no_content(&pool, &session).await;
    assert!(context_record_is_callers(&pool, &session, owner).await);
    assert!(!context_record_is_callers(&pool, &session, other).await);

    state.remember_dispatch_context(Some(other), &request, &session).await;
    assert_eq!(
        ownership::owner_of(&pool, &session).await.expect("lookup"),
        Some(owner),
        "another user's dispatch leaves the ownership record alone"
    );

    sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
        .bind(&session)
        .execute(&pool)
        .await
        .ok();
}

#[test]
fn a_dispatched_worker_resolves_rules_by_adapter_and_cwd_and_saves_for_approval() {
    let uid = uuid::Uuid::new_v4();
    let scope = dispatch_context_scope(
        uid,
        &serde_json::json!({"adapter_id": "codex", "working_dir": " /w/repo "}),
    );
    assert!(scope.dispatched);
    assert_eq!(scope.user_id, uid);
    assert_eq!(scope.coords.adapter.as_deref(), Some("codex"));
    assert_eq!(scope.coords.working_dir.as_deref(), Some("/w/repo"));
    assert!(scope.coords.machine_id.is_none(), "a worker pod is no enrolled machine");

    let bare = dispatch_context_scope(uid, &serde_json::json!({"cwd": "/srv"}));
    assert_eq!(bare.coords.adapter.as_deref(), Some("claude-code"));
    assert_eq!(bare.coords.working_dir.as_deref(), Some("/srv"));
}

/// Records every dispatch it receives and refuses the first.
struct RefuseFirst {
    calls: std::sync::Mutex<Vec<Call>>,
    refuse: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl cctui_server_dispatch::dispatchers::Dispatcher for RefuseFirst {
    fn id(&self) -> &'static str {
        "refuse-first"
    }

    async fn dispatch(
        &self,
        spec: &cctui_server_dispatch::dispatchers::DispatchSpec<'_>,
    ) -> Result<
        cctui_server_dispatch::dispatchers::DispatchHandle,
        cctui_server_dispatch::dispatchers::DispatchError,
    > {
        self.calls.lock().unwrap().push((
            spec.session_id.to_owned(),
            spec.reply_url.map(str::to_owned),
            spec.payload.clone(),
        ));
        if self.refuse.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(cctui_server_dispatch::dispatchers::DispatchError::Backend(
                "quota exceeded".into(),
            ));
        }
        Ok(cctui_server_dispatch::dispatchers::DispatchHandle {
            handle: format!("jobs/{}", spec.session_id),
            namespace: None,
            status: None,
            queue_position: None,
            exposure: None,
        })
    }
}

fn user_ctx(uid: uuid::Uuid) -> crate::auth::AuthContext {
    crate::auth::AuthContext {
        user_id: uid,
        key_id: uid,
        machine_id: None,
        scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
    }
}

/// A dispatch the dispatcher refused is listed with its stored fields; a
/// re-dispatch sends them as a new dispatch without the callback, once,
/// and only for its own user.
#[tokio::test]
async fn a_refused_dispatch_is_listed_and_re_dispatched_with_its_stored_fields() {
    use axum::extract::{Path, State};
    use axum::{Extension, Json};
    let Some(url) = crate::routes::gateway::test_db_url("dispatch_record_redispatch") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let stub = std::sync::Arc::new(RefuseFirst {
        calls: std::sync::Mutex::default(),
        refuse: std::sync::atomic::AtomicBool::new(true),
    });
    let mut state = crate::state::AppState::for_test(pool.clone());
    state.dispatchers =
        std::sync::Arc::new(cctui_server_dispatch::dispatchers::Registry::new().with(stub.clone()));
    let (uid, other) = (seed_user(&pool).await, seed_user(&pool).await);
    let req: cctui_proto::api::DispatchRequest = serde_json::from_value(serde_json::json!({
        "dispatcher": "refuse-first",
        "session_id": "triage-PROJ-9",
        "timeout": 20,
        "reply_url": "https://n8n/reply/CAPABILITY",
        "payload": {"profile": "tonbo", "repo": "cctui", "model": "opus", "prompt": "fix it"},
    }))
    .unwrap();
    let (code, _) = cctui_server_dispatch::routes::dispatch::dispatch(
        State(state.clone()),
        Extension(user_ctx(uid)),
        Json(req),
    )
    .await
    .unwrap_err();
    assert_eq!(code, axum::http::StatusCode::BAD_GATEWAY);

    let failed = cctui_server_dispatch::store::dispatch_records::list_failed(&pool, Some(uid))
        .await
        .unwrap();
    assert_eq!(failed.len(), 1);
    let original = failed[0].session_id.clone();
    assert_eq!(failed[0].reason, cctui_proto::api::FailedDispatchReason::Refused);
    assert!(failed[0].detail.as_deref().unwrap_or_default().contains("quota exceeded"));
    assert_eq!(failed[0].name.as_deref(), Some("triage-PROJ-9"));
    assert_eq!(failed[0].prompt_head.as_deref(), Some("fix it"));
    assert_eq!(failed[0].rerun_blocked, None);

    let (code, _) = cctui_server_dispatch::routes::dispatch::redispatch(
        State(state.clone()),
        Extension(user_ctx(other)),
        Path(original.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(code, axum::http::StatusCode::NOT_FOUND, "another user's record");

    let (code, Json(resp)) = cctui_server_dispatch::routes::dispatch::redispatch(
        State(state.clone()),
        Extension(user_ctx(uid)),
        Path(original.clone()),
    )
    .await
    .unwrap();
    assert_eq!(code, axum::http::StatusCode::ACCEPTED);
    assert_ne!(resp.session_id, original, "a new dispatch");
    let calls = stub.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    let (sid, reply_url, payload) = &calls[1];
    assert_eq!(sid, &resp.session_id);
    assert_eq!(*reply_url, None);
    for (key, want) in [
        ("profile", "tonbo"),
        ("repo", "cctui"),
        ("model", "opus"),
        ("prompt", "fix it"),
        ("name", "triage-PROJ-9"),
    ] {
        assert_eq!(payload[key], want, "{key}");
    }
    assert!(
        cctui_server_dispatch::store::dispatch_records::list_failed(&pool, Some(uid))
            .await
            .unwrap()
            .is_empty(),
        "a re-run record leaves the list"
    );
    let (code, _) = cctui_server_dispatch::routes::dispatch::redispatch(
        State(state.clone()),
        Extension(user_ctx(uid)),
        Path(original),
    )
    .await
    .unwrap_err();
    assert_eq!(code, axum::http::StatusCode::CONFLICT, "re-run once");

    for u in [uid, other] {
        for sql in
            ["DELETE FROM auth_keys WHERE user_id = $1", "DELETE FROM machines WHERE user_id = $1"]
        {
            sqlx::query(sql).bind(u).execute(&pool).await.ok();
        }
        sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
    }
}

/// A policy-bound worker gets a capability that refuses every child,
/// whatever it declared; without a policy the declared or default
/// capability is kept as is.
#[tokio::test]
async fn a_policy_bound_dispatch_grants_no_children() {
    use cctui_server_core::child_policy::{Denied, Usage, authorize};
    let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_no_children") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let state = crate::state::AppState::for_test(pool.clone());
    let declared = serde_json::json!({
        "spawn_capability": {"adapters": ["claude-code", "codex"], "max_children": 5}
    });
    let persisted = |sid: String, payload: serde_json::Value, bound: bool| {
        let state = state.clone();
        async move {
            let mut payload = payload;
            cctui_server_dispatch::dispatch::persist_spawn_capability(
                &state,
                &sid,
                &mut payload,
                bound,
            )
            .await
            .expect("persisted");
            assert!(payload.get("spawn_capability").is_none(), "never forwarded");
            let cached = state.spawn_capabilities.get(&sid).map(|c| c.value().clone()).unwrap();
            let stored = crate::store::spawn_capabilities::get(&state.pool, &sid).await.unwrap();
            assert_eq!(stored.as_ref(), Some(&cached));
            sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
                .bind(&sid)
                .execute(&state.pool)
                .await
                .ok();
            cached
        }
    };
    let sid = || format!("dispatch-cap-{}", uuid::Uuid::new_v4());

    for payload in [declared.clone(), serde_json::json!({})] {
        let cap = persisted(sid(), payload, true).await;
        assert_eq!(cap.max_children, Some(0));
        let child = cctui_proto::api::SpawnChildRequest {
            adapter: "claude-code".into(),
            prompt: "go".into(),
            ..Default::default()
        };
        assert_eq!(
            authorize(Some(&cap), &child, &Usage::default()),
            Err(Denied::TooManyChildren { max: 0 })
        );
    }

    let cap = persisted(sid(), declared, false).await;
    assert_eq!(cap.adapters, ["claude-code", "codex"]);
    assert_eq!(cap.max_children, Some(5));
    assert_eq!(cap.max_permission_mode, Some(PermissionMode::Yolo));

    let default = crate::routes::server_settings::spawn_default_capability(&state).await;
    let cap = persisted(sid(), serde_json::json!({}), false).await;
    assert_eq!(
        cap,
        cctui_proto::api::SpawnCapability {
            max_permission_mode: Some(PermissionMode::Yolo),
            ..default
        }
    );
}

/// A policy-bound dispatch whose capability cannot be stored is refused
/// before it is forwarded; without a policy it still goes out.
#[tokio::test]
async fn a_policy_bound_dispatch_is_refused_when_its_capability_is_not_stored() {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    pool.close().await;
    let state = crate::state::AppState::for_test(pool);
    let payload = serde_json::json!({"spawn_capability": {"adapters": ["claude-code"]}});

    let sid = format!("dispatch-cap-{}", uuid::Uuid::new_v4());
    let (code, _) = cctui_server_dispatch::dispatch::persist_spawn_capability(
        &state,
        &sid,
        &mut payload.clone(),
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(code, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(state.spawn_capabilities.get(&sid).is_none());

    let sid = format!("dispatch-cap-{}", uuid::Uuid::new_v4());
    let mut unbound = payload;
    cctui_server_dispatch::dispatch::persist_spawn_capability(&state, &sid, &mut unbound, false)
        .await
        .expect("no policy: a persist failure does not block the dispatch");
    assert!(state.spawn_capabilities.get(&sid).is_some());
}

/// A fake dispatcher that acknowledges every `Cancel` and records its handle.
fn cancelling_dispatcher(
    state: &crate::state::AppState,
    id: uuid::Uuid,
) -> (tokio::task::JoinHandle<()>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
    let (tx, mut rx) = tokio::sync::mpsc::channel::<DispatcherFrameDown>(8);
    state.bus.register_dispatcher(id, tx);
    let bus = state.bus.clone();
    let cancelled = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = cancelled.clone();
    let task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let DispatcherFrameDown::Cancel { request_id, handle } = frame else {
                panic!("expected Cancel, got {frame:?}");
            };
            seen.lock().unwrap().push(handle.clone());
            assert!(bus.resolve_dispatcher_reply(
                request_id,
                DispatcherFrameUp::CancelResult { request_id, handle, ok: true, error: None },
            ));
        }
    });
    (task, cancelled)
}

#[tokio::test]
async fn killing_a_dispatched_session_with_no_live_worker_deletes_its_job() {
    use axum::extract::{Path, State};
    let Some(url) = crate::routes::gateway::test_db_url("dispatch_kill_pending") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let state = crate::state::AppState::for_test(pool.clone());
    let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "kill-pending").await;
    let name = format!("k8s-{uid}");
    let dispatcher: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO dispatchers (user_id, name, kind, key_hash, key_preview) \
         VALUES ($1, $2, 'kubernetes', $3, 'dk-test') RETURNING id",
    )
    .bind(uid)
    .bind(&name)
    .bind(format!("dk-{uid}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let pending = crate::events::tests::seed_session(&pool, uid, machine, Some("pending")).await;
    let handle = format!("jobs/cctui-worker-pending/{pending}");
    sqlx::query(
        "INSERT INTO dispatch_handles (session_id, dispatcher_name, handle) \
         VALUES ($1, $2, $3)",
    )
    .bind(&pending)
    .bind(&name)
    .bind(&handle)
    .execute(&pool)
    .await
    .unwrap();
    let (fake, cancelled) = cancelling_dispatcher(&state, dispatcher);

    let ctx = axum::Extension(crate::auth::AuthContext {
        user_id: uid,
        key_id: uid,
        machine_id: None,
        scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
    });
    let code =
        crate::routes::sessions::kill_session(State(state.clone()), ctx, Path(pending.clone()))
            .await
            .expect("kill");
    assert_eq!(code, axum::http::StatusCode::NO_CONTENT);
    assert_eq!(*cancelled.lock().unwrap(), [handle], "the dispatcher deleted the Job");
    let gone: bool = sqlx::query_scalar(
        "SELECT gone_at IS NOT NULL FROM dispatch_handles WHERE session_id = $1",
    )
    .bind(&pending)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(gone, "the deleted handle leaves the liveness sweep");
    assert!(
        !cctui_server_dispatch::routes::dispatch::cancel_dispatched_workload(&state, &pending)
            .await,
        "a handle already gone is not cancelled twice"
    );
    assert_eq!(cancelled.lock().unwrap().len(), 1);
    fake.abort();

    sqlx::query("DELETE FROM dispatch_handles WHERE session_id = $1")
        .bind(&pending)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&pending).execute(&pool).await.ok();
    sqlx::query("DELETE FROM dispatchers WHERE id = $1").bind(dispatcher).execute(&pool).await.ok();
}

/// The daemon reads `payload.adapter`; the server used to read only
/// `adapter_id`, so a codex dispatch could be routed to an anthropic
/// account. Both spellings are read, `adapter` first.
#[test]
fn the_payload_adapter_prefers_adapter_over_adapter_id() {
    let cases = [
        (serde_json::json!({"adapter": "codex"}), Some("codex")),
        (serde_json::json!({"adapter_id": "codex"}), Some("codex")),
        (serde_json::json!({"adapter": "opencode", "adapter_id": "codex"}), Some("opencode")),
        (serde_json::json!({"adapter": "", "adapter_id": "codex"}), Some("codex")),
        (serde_json::json!({"adapter": " codex "}), Some("codex")),
        (serde_json::json!({"adapter": 7}), None),
        (serde_json::json!({}), None),
    ];
    for (payload, want) in cases {
        assert_eq!(payload_adapter(&payload), want, "payload {payload}");
    }
    let scope =
        dispatch_context_scope(uuid::Uuid::new_v4(), &serde_json::json!({"adapter": "codex"}));
    assert_eq!(scope.coords.adapter.as_deref(), Some("codex"));
}

type Call = (String, Option<String>, serde_json::Value);
