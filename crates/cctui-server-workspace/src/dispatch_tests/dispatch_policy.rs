use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::admin::{ContextPackGrant, DispatchPolicy, DispatchPolicyRules};
use cctui_proto::api::{ApiError, DispatchRequest};
use uuid::Uuid;

use crate::state::AppState;

async fn enforce(
    state: &AppState,
    user_id: Uuid,
    req: &mut DispatchRequest,
) -> Result<Option<DispatchPolicy>, (StatusCode, Json<ApiError>)> {
    cctui_server_core::dispatch_policy::enforce(&state.pool, user_id, req).await
}

fn req(dispatcher: &str, payload: serde_json::Value) -> DispatchRequest {
    DispatchRequest {
        dispatcher: dispatcher.into(),
        session_id: None,
        timeout: None,
        reply_url: None,
        notify_url: None,
        notify_secret: None,
        payload,
        account: None,
        provider: None,
        accounts: Vec::new(),
    }
}

fn pack(name: &str, url: &str, git_ref: Option<&str>, subdir: Option<&str>) -> ContextPackGrant {
    ContextPackGrant {
        name: name.into(),
        url: url.into(),
        git_ref: git_ref.map(str::to_owned),
        subdir: subdir.map(str::to_owned),
    }
}

async fn seed_user(pool: &sqlx::PgPool, tag: &str) -> uuid::Uuid {
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{tag}-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    uid
}

async fn seed_dispatcher(pool: &sqlx::PgPool, owner: uuid::Uuid, name: &str) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO dispatchers (id, user_id, name, kind, key_hash, key_preview) \
         VALUES ($1, $2, $3, 'kubernetes', $4, 'dk…')",
    )
    .bind(id)
    .bind(owner)
    .bind(name)
    .bind(format!("dk-{id}"))
    .execute(pool)
    .await
    .expect("seed dispatcher");
    id
}

async fn connect(test_name: &str) -> Option<sqlx::PgPool> {
    let url = crate::routes::gateway::test_db_url(test_name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

/// Without a policy a user resolves only its own dispatchers; a policy
/// naming another owner's dispatcher lets the name resolve, and the admin
/// view is unchanged either way.
#[tokio::test]
async fn a_policy_grants_another_owners_dispatcher_by_name() {
    let Some(pool) = connect("a_policy_grants_another_owners_dispatcher_by_name").await else {
        return;
    };
    let state = crate::state::AppState::for_test(pool.clone());
    let owner = seed_user(&pool, "owner").await;
    let service = seed_user(&pool, "service").await;
    let name = format!("kube-{}", uuid::Uuid::new_v4());
    let dispatcher_id = seed_dispatcher(&pool, owner, &name).await;
    let resolve = |uid: Option<uuid::Uuid>| {
        cctui_server_dispatch::dispatch::resolve_dispatcher(&state, uid, &name)
    };

    assert!(resolve(Some(owner)).await.unwrap().is_some(), "the owner resolves it");
    assert!(resolve(Some(service)).await.unwrap().is_none(), "no policy: not visible");
    assert!(resolve(None).await.unwrap().is_some(), "admin resolves across owners");

    let rules = DispatchPolicyRules { dispatchers: vec![name.clone()], ..Default::default() };
    cctui_server_dispatch::store::dispatch_policies::upsert(&pool, service, &rules).await.unwrap();
    assert!(resolve(Some(service)).await.unwrap().is_some(), "granted by policy");

    let other = DispatchPolicyRules { dispatchers: vec!["elsewhere".into()], ..Default::default() };
    cctui_server_dispatch::store::dispatch_policies::upsert(&pool, service, &other).await.unwrap();
    assert!(
        resolve(Some(service)).await.unwrap().is_none(),
        "a policy naming other dispatchers grants nothing"
    );

    sqlx::query("DELETE FROM dispatchers WHERE id = $1")
        .bind(dispatcher_id)
        .execute(&pool)
        .await
        .ok();
    for uid in [owner, service] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}

/// The store round-trips every rule, `enforce` is a no-op without a row and
/// refuses on the DB-backed caps once the row exists.
#[tokio::test]
async fn enforce_is_inert_without_a_policy_and_applies_the_caps_with_one() {
    let Some(pool) =
        connect("enforce_is_inert_without_a_policy_and_applies_the_caps_with_one").await
    else {
        return;
    };
    let state = crate::state::AppState::for_test(pool.clone());
    let uid = seed_user(&pool, "svc").await;
    let mut r = req("anything", serde_json::json!({"model": "whatever"}));
    assert!(enforce(&state, uid, &mut r).await.unwrap().is_none());
    assert_eq!(r.timeout, None);
    let mut unbound = req(
        "anything",
        serde_json::json!({"model": "opus", "codex_model": "gpt-5.6-sol", "env": {
            "FIREWORKS_BASE_URL": "https://x", "WORKER_NET_ALLOW": "*",
            "ANTHROPIC_MODEL": "opus", "CONTEXT_PACK_REF": "dev"
        }}),
    );
    let sent = unbound.payload.clone();
    assert!(enforce(&state, uid, &mut unbound).await.unwrap().is_none());
    assert_eq!(unbound.payload, sent, "no policy: the payload goes through untouched");

    let rules = DispatchPolicyRules {
        dispatchers: vec!["anything".into()],
        context_packs: vec![pack("p", "https://git/p.git", Some("main"), None)],
        max_timeout_minutes: Some(15),
        daily_cap: Some(1),
        max_concurrent: Some(1),
        ..Default::default()
    };
    cctui_server_dispatch::store::dispatch_policies::upsert(&pool, uid, &rules).await.unwrap();
    let stored =
        cctui_server_dispatch::store::dispatch_policies::get(&pool, uid).await.unwrap().unwrap();
    assert_eq!(stored.rules, rules);
    assert_eq!(stored.user_id, uid);

    let applied = enforce(&state, uid, &mut r).await.unwrap().expect("policy applied");
    assert_eq!(applied.rules.daily_cap, Some(1));
    assert_eq!(r.timeout, Some(15), "the ceiling fills in a missing timeout");

    cctui_server_dispatch::store::dispatch_policies::record_use(&pool, uid, &format!("s-1-{uid}"))
        .await
        .unwrap();
    let (code, body) = enforce(&state, uid, &mut r).await.unwrap_err();
    assert_eq!(code, axum::http::StatusCode::FORBIDDEN);
    assert!(body.error.contains("(daily_cap)"), "{}", body.error);

    let relaxed = DispatchPolicyRules { daily_cap: None, ..rules };
    cctui_server_dispatch::store::dispatch_policies::upsert(&pool, uid, &relaxed).await.unwrap();
    let live_sid = format!("s-live-{uid}");
    for sid in [live_sid.clone(), format!("s-refused-{uid}")] {
        sqlx::query(
            "INSERT INTO auth_keys (user_id, key_hash, kind, session_id, expires_at) \
             VALUES ($1, $2, 'ephemeral', $3, now() + interval '1 hour')",
        )
        .bind(uid)
        .bind(format!("eph-{sid}"))
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert_eq!(
        cctui_server_dispatch::store::dispatch_policies::live_dispatches(&pool, uid).await.unwrap(),
        0,
        "a key whose forward was refused holds no slot"
    );
    cctui_server_dispatch::store::dispatch_policies::record_use(&pool, uid, &live_sid)
        .await
        .unwrap();
    assert_eq!(
        cctui_server_dispatch::store::dispatch_policies::live_dispatches(&pool, uid).await.unwrap(),
        1
    );
    let (code, body) = enforce(&state, uid, &mut r).await.unwrap_err();
    assert_eq!(code, axum::http::StatusCode::FORBIDDEN);
    assert!(body.error.contains("(max_concurrent)"), "{}", body.error);

    assert!(cctui_server_dispatch::store::dispatch_policies::delete(&pool, uid).await.unwrap());
    assert!(enforce(&state, uid, &mut r).await.unwrap().is_none());

    sqlx::query("DELETE FROM auth_keys WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

/// A policy-bound service user reads only the sessions it dispatched: the
/// session read guard is unchanged by the policy.
#[tokio::test]
async fn a_service_user_reads_only_its_own_sessions() {
    let Some(pool) = connect("a_service_user_reads_only_its_own_sessions").await else {
        return;
    };
    let service = seed_user(&pool, "svc").await;
    let other = seed_user(&pool, "other").await;
    let rules = DispatchPolicyRules { dispatchers: vec!["kube".into()], ..Default::default() };
    cctui_server_dispatch::store::dispatch_policies::upsert(&pool, service, &rules).await.unwrap();
    let mut sids = Vec::new();
    for uid in [service, other] {
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash, kind) VALUES ($1, $2, 'dispatch', $3, 'dispatch')")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        sids.push((sid, machine));
    }
    let ctx = crate::auth::AuthContext {
        user_id: service,
        key_id: uuid::Uuid::new_v4(),
        machine_id: None,
        scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
    };
    assert_eq!(crate::authz::authorize_session_read(&ctx, &sids[0].0, &pool).await, Ok(()));
    assert_eq!(
        crate::authz::authorize_session_read(&ctx, &sids[1].0, &pool).await,
        Err(axum::http::StatusCode::FORBIDDEN)
    );

    for (sid, machine) in &sids {
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(machine).execute(&pool).await.ok();
    }
    for uid in [service, other] {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
