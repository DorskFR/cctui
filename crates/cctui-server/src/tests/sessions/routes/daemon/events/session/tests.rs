use super::*;
use cctui_server_sessions::routes::daemon::test_support::{seed_machine, seed_owned_session};

/// A `<synthetic>` model (Claude Code's local API-error messages) never
/// overwrites the recorded model; a real id still does.
#[tokio::test]
async fn session_model_ignores_synthetic_placeholder() {
    let Some(url) = cctui_server_kernel::test_support::db_url("session_model_synthetic") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "session-model-synthetic").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    let model_of = |sid: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT model FROM sessions WHERE id = $1")
                .bind(sid)
                .fetch_one(&pool)
                .await
                .expect("read model")
        }
    };
    let send = |model: &str| {
        super::on_session_event(
            &state,
            mid,
            uid,
            "claude-code",
            AdapterEvent::SessionModel { local_id: sid.clone(), model: model.to_owned() },
        )
    };

    send("deepseek-v4-flash").await.expect("real model");
    assert_eq!(model_of(sid.clone()).await.as_deref(), Some("deepseek-v4-flash"));
    for placeholder in ["<synthetic>", "  ", ""] {
        send(placeholder).await.expect("placeholder model");
        assert_eq!(
            model_of(sid.clone()).await.as_deref(),
            Some("deepseek-v4-flash"),
            "{placeholder:?} must not overwrite the model"
        );
    }

    cctui_server_sessions::routes::daemon::test_support::drop_machines(
        &pool,
        &[sid],
        &[(uid, mid)],
    )
    .await;
}

#[tokio::test]
async fn a_session_end_forgets_its_guard_step() {
    let Some(url) = cctui_server_kernel::test_support::db_url("session_end_guard_step") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "session-end-guard").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    cctui_server_sessions::guard_step::record(
        sid.clone(),
        cctui_proto::guard_step::GuardStepState { step: 2, ..Default::default() },
    );

    super::on_session_event(
        &state,
        mid,
        uid,
        "claude-code",
        AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed },
    )
    .await
    .expect("session end");
    assert_eq!(cctui_server_sessions::guard_step::get(&sid), None);

    cctui_server_sessions::routes::daemon::test_support::drop_machines(
        &pool,
        &[sid],
        &[(uid, mid)],
    )
    .await;
}

/// A guard-less dispatch's daemon ends its session `completed` as the
/// worker shuts down; arriving after a transport mark, it replaces it.
#[tokio::test]
async fn a_completed_end_after_a_transport_mark_upgrades_it() {
    let Some(url) = cctui_server_kernel::test_support::db_url("session_end_after_mark") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "end-after-mark").await;
    let mut sids = Vec::new();
    for mark in ["daemon_lost", "machine_offline"] {
        let sid = seed_owned_session(&pool, uid, mid).await;
        sqlx::query(
            "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = $2, \
             end_detail = 'transport' WHERE id = $1",
        )
        .bind(&sid)
        .bind(mark)
        .execute(&pool)
        .await
        .expect("mark");

        super::on_session_event(
            &state,
            mid,
            uid,
            "claude-code",
            AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed },
        )
        .await
        .expect("session end");

        let (status, end_reason, end_detail): (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status, end_reason, end_detail FROM sessions WHERE id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("read back");
        assert_eq!(
            (status.as_str(), end_reason.as_deref(), end_detail),
            ("ended", Some("completed"), None),
            "{mark} is replaced by the reported completion"
        );
        sids.push(sid);
    }

    cctui_server_sessions::routes::daemon::test_support::drop_machines(&pool, &sids, &[(uid, mid)])
        .await;
}

/// An account named `Astra-<id>` with one openai provider row, for `uid`.
async fn seed_openai_account(pool: &sqlx::PgPool, uid: Uuid) -> (Uuid, Uuid) {
    let acct = Uuid::new_v4();
    let prov = Uuid::new_v4();
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(acct)
        .bind(uid)
        .bind(format!("Astra-{acct}"))
        .execute(pool)
        .await
        .expect("seed account");
    sqlx::query(
        "INSERT INTO account_providers \
             (id, user_id, provider, encrypted_refresh_token, account_id) \
         VALUES ($1, $2, 'openai', 'x', $3)",
    )
    .bind(prov)
    .bind(uid)
    .bind(acct)
    .execute(pool)
    .await
    .expect("seed provider");
    (acct, prov)
}

/// DB-gated: a codex spawn binds its gateway token to the dispatch key
/// (codex mints its own thread id), and the thread registers under that
/// thread id echoing the key as `spawn_key`. The token, and the spawn's
/// attachments, must land on the thread id: every account lookup of the
/// session list (`account_name`, switch-account, revocation at end) reads
/// `session_tokens` by the registered id.
#[tokio::test]
async fn codex_start_rebinds_the_dispatch_key_onto_the_thread_id() {
    let Some(url) = cctui_server_kernel::test_support::db_url("codex_start_rebinds") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "codex-rebind").await;
    let (acct, prov) = seed_openai_account(&pool, uid).await;
    let dispatch_key = Uuid::new_v4().to_string();
    let thread_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(format!("th-{dispatch_key}"))
    .bind(&dispatch_key)
    .bind(prov)
    .execute(&pool)
    .await
    .expect("seed token under the dispatch key");
    sqlx::query(
        "INSERT INTO session_attachments (session_id, name, hash, size) \
         VALUES ($1, 'brief.md', 'h', 1)",
    )
    .bind(&dispatch_key)
    .execute(&pool)
    .await
    .expect("seed bootstrap attachment");

    super::on_session_event(
        &state,
        mid,
        uid,
        "codex",
        AdapterEvent::SessionStarted {
            local_id: thread_id.clone(),
            meta: cctui_proto::adapter::SessionMeta {
                working_dir: Some("/w".into()),
                parent_local_id: None,
                extra: json!({ "source": "codex-app-server", "spawn_key": dispatch_key }),
            },
        },
    )
    .await
    .expect("session started");

    let name: Option<String> = sqlx::query_scalar(
        "SELECT a.name FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = $1 AND st.revoked_at IS NULL",
    )
    .bind(&thread_id)
    .fetch_optional(&pool)
    .await
    .expect("account lookup");
    assert_eq!(name, Some(format!("Astra-{acct}")), "the thread id must resolve its account");
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM session_tokens WHERE session_id = $1) \
              + (SELECT count(*) FROM session_attachments WHERE session_id = $1)",
    )
    .bind(&dispatch_key)
    .fetch_one(&pool)
    .await
    .expect("count leftovers");
    assert_eq!(left, 0, "nothing may stay behind on the dispatch key");
    let attachments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_attachments WHERE session_id = $1")
            .bind(&thread_id)
            .fetch_one(&pool)
            .await
            .expect("count attachments");
    assert_eq!(attachments, 1, "the bootstrap attachment follows the session");

    sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
        .bind(&thread_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM account_providers WHERE id = $1").bind(prov).execute(&pool).await.ok();
    sqlx::query("DELETE FROM accounts WHERE id = $1").bind(acct).execute(&pool).await.ok();
    cctui_server_sessions::routes::daemon::test_support::drop_machines(
        &pool,
        &[thread_id],
        &[(uid, mid)],
    )
    .await;
}

async fn announce(
    state: &AppState,
    (uid, mid): (Uuid, Uuid),
    sid: &str,
    parent: Option<&str>,
    relation: serde_json::Value,
) {
    let meta = cctui_proto::adapter::SessionMeta {
        working_dir: Some("/w".to_owned()),
        parent_local_id: parent.map(str::to_owned),
        extra: json!({ "short": "s", "relation": relation }),
    };
    super::on_session_event(
        state,
        mid,
        uid,
        "claude-code",
        AdapterEvent::SessionStarted { local_id: sid.to_owned(), meta },
    )
    .await
    .expect("session started");
}

async fn relation_of(pool: &sqlx::PgPool, sid: &str) -> (Option<String>, Option<String>) {
    sqlx::query_as("SELECT parent_id, metadata->>'relation' FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .expect("read relation")
}

async fn followup_intent(pool: &sqlx::PgPool, sid: &str, parent: &str) {
    sqlx::query(
        "INSERT INTO session_followup_intents (spawn_key, parent_session_id) VALUES ($1, $2)",
    )
    .bind(sid)
    .bind(parent)
    .execute(pool)
    .await
    .expect("seed follow-up intent");
}

/// A daemon calls every session it discovers a root and re-announces it on
/// each reconnect, before or after the follow-up claim, with `"root"` or
/// (codex) `null`. Only a genuine root ends up a root.
#[tokio::test]
async fn a_root_announcement_never_takes_a_born_relation_back() {
    let Some(url) = cctui_server_kernel::test_support::db_url("sticky_relation") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let owner = seed_machine(&pool, "sticky-relation").await;
    let parent = seed_owned_session(&pool, owner.0, owner.1).await;
    let id = || Uuid::new_v4().to_string();
    let linked = |relation: &str| (Some(parent.clone()), Some(relation.to_owned()));

    let root_first = id();
    followup_intent(&pool, &root_first, &parent).await;
    announce(&state, owner, &root_first, None, json!("root")).await;
    assert_eq!(relation_of(&pool, &root_first).await, linked("followup"));

    let claim_first = id();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, adapter_id) \
         VALUES ($1, $2, $3, '/w', 'active', $4, 'claude-code')",
    )
    .bind(&claim_first)
    .bind(owner.1.to_string())
    .bind(owner.1)
    .bind(owner.0)
    .execute(&pool)
    .await
    .expect("seed claimed session");
    followup_intent(&pool, &claim_first, &parent).await;
    cctui_server_sessions::followup::claim_intent(&pool, &claim_first, None).await;
    announce(&state, owner, &claim_first, None, json!("root")).await;
    assert_eq!(relation_of(&pool, &claim_first).await, linked("followup"));

    let fork = id();
    announce(&state, owner, &fork, Some(parent.as_str()), json!("fork")).await;
    let late_fork = id();
    announce(&state, owner, &late_fork, None, json!("root")).await;
    announce(&state, owner, &late_fork, Some(parent.as_str()), json!("fork")).await;
    let child = id();
    announce(&state, owner, &child, Some(parent.as_str()), json!("subagent")).await;

    for sid in [&root_first, &claim_first, &fork, &late_fork, &child] {
        announce(&state, owner, sid, None, json!("root")).await;
        announce(&state, owner, sid, None, serde_json::Value::Null).await;
    }
    for sid in [&root_first, &claim_first] {
        assert_eq!(relation_of(&pool, sid).await, linked("followup"), "{sid} re-announced");
    }
    assert_eq!(relation_of(&pool, &fork).await, linked("fork"));
    assert_eq!(relation_of(&pool, &late_fork).await, linked("fork"));
    assert_eq!(relation_of(&pool, &child).await, linked("subagent"));

    let genuine = id();
    announce(&state, owner, &genuine, None, json!("root")).await;
    announce(&state, owner, &genuine, None, json!("root")).await;
    assert_eq!(relation_of(&pool, &genuine).await, (None, Some("root".to_owned())));

    let all = [root_first, claim_first, fork, late_fork, child, genuine, parent];
    cctui_server_sessions::routes::daemon::test_support::drop_machines(&pool, &all, &[owner]).await;
}

async fn followup_intent_count(pool: &sqlx::PgPool, sid: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM session_followup_intents WHERE spawn_key = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .expect("count follow-up intents")
}

/// Registration claims a follow-up intent only with `automation` on.
#[tokio::test]
async fn registration_claims_followups_only_with_automation_on() {
    use cctui_proto::features::{Feature, FeatureSet};
    let Some(url) = cctui_server_kernel::test_support::db_url("followup_claim_gated") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let mut off = AppState::for_test(pool.clone());
    off.config.features = FeatureSet::all().without(Feature::Automation);
    let on = AppState::for_test(pool.clone());
    let owner = seed_machine(&pool, "followup-gated").await;
    let parent = seed_owned_session(&pool, owner.0, owner.1).await;

    let unclaimed = Uuid::new_v4().to_string();
    followup_intent(&pool, &unclaimed, &parent).await;
    announce(&off, owner, &unclaimed, None, json!("root")).await;
    assert_eq!(relation_of(&pool, &unclaimed).await, (None, Some("root".to_owned())));
    assert_eq!(followup_intent_count(&pool, &unclaimed).await, 1, "the intent is not read");

    let claimed = Uuid::new_v4().to_string();
    followup_intent(&pool, &claimed, &parent).await;
    announce(&on, owner, &claimed, None, json!("root")).await;
    assert_eq!(
        relation_of(&pool, &claimed).await,
        (Some(parent.clone()), Some("followup".to_owned()))
    );
    assert_eq!(followup_intent_count(&pool, &claimed).await, 0);

    sqlx::query("DELETE FROM session_followup_intents WHERE spawn_key = $1")
        .bind(&unclaimed)
        .execute(&pool)
        .await
        .ok();
    let all = [unclaimed, claimed, parent];
    cctui_server_sessions::routes::daemon::test_support::drop_machines(&pool, &all, &[owner]).await;
}

#[tokio::test]
async fn registration_writes_no_label_or_context_content_with_both_off() {
    use cctui_proto::features::{Feature, FeatureSet};
    let Some(url) = cctui_server_kernel::test_support::db_url("start_organising_context_off")
    else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let mut off = AppState::for_test(pool.clone());
    off.config.features = FeatureSet::all().without(Feature::Organising).without(Feature::Context);
    let (uid, mid) = seed_machine(&pool, "start-gated").await;
    let sid = Uuid::new_v4().to_string();
    let fork_key = format!("fork-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO session_label_intents (spawn_key, label_ids) VALUES ($1, ARRAY[$2]::uuid[])",
    )
    .bind(&sid)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("seed label intent");
    sqlx::query(
        "INSERT INTO session_context (spawn_key, item_ids, scope, rules, memories) \
         VALUES ($1, ARRAY[$2]::uuid[], $3, '[{\"name\":\"r\"}]', '[{\"name\":\"m\"}]')",
    )
    .bind(&fork_key)
    .bind(Uuid::new_v4())
    .bind(sqlx::types::Json(json!({ "user_id": uid.to_string(), "dispatched": false })))
    .execute(&pool)
    .await
    .expect("seed the parent's context set");

    let meta = cctui_proto::adapter::SessionMeta {
        working_dir: Some("/w".to_owned()),
        parent_local_id: None,
        extra: json!({ "short": "s", "context_key": fork_key }),
    };
    super::on_session_event(
        &off,
        mid,
        uid,
        "claude-code",
        AdapterEvent::SessionStarted { local_id: sid.clone(), meta },
    )
    .await
    .expect("session started");

    let intents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_label_intents WHERE spawn_key = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .expect("count label intents");
    assert_eq!(intents, 1, "the label intent is not claimed with organising off");
    assert_eq!(
        cctui_server_sessions::routes::context::ownership::owner_of(&pool, &sid)
            .await
            .expect("owner"),
        Some(uid),
        "the launch owner follows the session"
    );
    cctui_server_sessions::routes::context::ownership::test_support::assert_no_content(&pool, &sid)
        .await;

    for sql in [
        "DELETE FROM session_label_intents WHERE spawn_key = $1 OR spawn_key = $2",
        "DELETE FROM session_context WHERE spawn_key = $1 OR spawn_key = $2",
    ] {
        sqlx::query(sql).bind(&sid).bind(&fork_key).execute(&pool).await.ok();
    }
    cctui_server_sessions::routes::daemon::test_support::drop_machines(
        &pool,
        &[sid],
        &[(uid, mid)],
    )
    .await;
}

/// A daemon reconnecting mid-dispatch re-announces a session whose turn
/// already ended: the upsert keeps it live without reopening anything, the
/// re-sent Status lands, and a completed end survives a later re-announce.
#[tokio::test]
async fn a_reannounce_of_a_finished_turn_keeps_its_state_and_its_end() {
    let Some(url) = cctui_server_kernel::test_support::db_url("reannounce_finished_turn") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, mid) = seed_machine(&pool, "reannounce-finished").await;
    let sid = seed_owned_session(&pool, uid, mid).await;
    let status = |tempo: &str, agent_state: &str| AdapterEvent::Status {
        local_id: sid.clone(),
        tempo: Some(tempo.to_owned()),
        state: Some(agent_state.to_owned()),
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: Vec::new(),
    };
    let reannounce = || AdapterEvent::SessionStarted {
        local_id: sid.clone(),
        meta: cctui_proto::adapter::SessionMeta {
            working_dir: Some("/w".into()),
            ..Default::default()
        },
    };
    let row = || {
        let pool = pool.clone();
        let sid = sid.clone();
        async move {
            sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
                "SELECT status, agent_state, tempo, end_reason FROM sessions WHERE id = $1",
            )
            .bind(sid)
            .fetch_one(&pool)
            .await
            .expect("read session")
        }
    };
    let send = |event| super::on_session_event(&state, mid, uid, "claude-code", event);

    super::super::status::on_status_event(&state, status("idle", "working"))
        .await
        .expect("stale status");
    send(reannounce()).await.expect("re-announce");
    let (st, agent_state, tempo, _) = row().await;
    assert_eq!(st, "active");
    assert_eq!((agent_state.as_deref(), tempo.as_deref()), (Some("working"), Some("idle")));

    super::super::status::on_status_event(&state, status("idle", "done"))
        .await
        .expect("re-sent status");
    assert_eq!(row().await.1.as_deref(), Some("done"), "the edge's Status reconciles");

    send(AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed })
        .await
        .expect("end");
    send(reannounce()).await.expect("late re-announce");
    let (st, _, _, end_reason) = row().await;
    assert_eq!(st, "ended", "a completed end is not revived by a re-announce");
    assert_eq!(end_reason.as_deref(), Some("completed"));

    cctui_server_sessions::routes::daemon::test_support::drop_machines(
        &pool,
        &[sid],
        &[(uid, mid)],
    )
    .await;
}
