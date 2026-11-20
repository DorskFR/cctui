use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::followup::{FollowupReason, FollowupRequest, FollowupState};
use cctui_proto::ws::ServerEvent;
use cctui_server_core::bus::ServerFrame;
use sqlx::PgPool;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use super::{
    BUSY, Begun, NOT_REGISTERED, NewOp, PINNED, begin, followup_name, load, op_by_key, settle,
    start,
};
use crate::auth::AuthContext;
use crate::state::AppState;

struct Fixture {
    state: AppState,
    pool: PgPool,
    user: Uuid,
    machine: Uuid,
    ctx: AuthContext,
    source: String,
}

async fn fixture(name: &str) -> Option<Fixture> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    let owner = crate::presets::store_tests::seed_owner(&pool, name).await;
    sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
        .bind(owner.user)
        .execute(&pool)
        .await
        .unwrap();
    let source = session(&pool, owner.user, owner.machine).await;
    sqlx::query(
        "UPDATE sessions SET session_name = 'triage', effort = 'high', \
         metadata = '{\"name_source\": \"user\"}'::jsonb WHERE id = $1",
    )
    .bind(&source)
    .execute(&pool)
    .await
    .unwrap();
    let ctx = crate::inbox::queued::acting_as(&pool, owner.user).await.unwrap();
    Some(Fixture {
        state: AppState::for_test(pool.clone()),
        pool,
        user: owner.user,
        machine: owner.machine,
        ctx,
        source,
    })
}

async fn session(pool: &PgPool, user: Uuid, machine: Uuid) -> String {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code')",
    )
    .bind(&id)
    .bind(machine.to_string())
    .bind(machine)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn cleanup(f: &Fixture) {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(f.user).execute(&f.pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(f.user).execute(&f.pool).await.ok();
}

fn request(key: &str, instruction: &str) -> FollowupRequest {
    FollowupRequest {
        instruction: instruction.to_owned(),
        reason: FollowupReason::Cold,
        archive_source: true,
        idempotency_key: key.to_owned(),
    }
}

async fn begun(f: &Fixture, reason: FollowupReason, instruction: &str) -> super::Op {
    let new = NewOp {
        source: &f.source,
        owner: Some(f.user),
        key: "k1",
        reason,
        instruction,
        archive_source: true,
        command_id: Uuid::new_v4(),
    };
    match begin(&f.pool, &new).await.unwrap() {
        Begun::Started(op) => op,
        other => panic!("expected a new op, got {other:?}"),
    }
}

async fn launched_to(f: &Fixture, op: &super::Op) -> String {
    let to = session(&f.pool, f.user, f.machine).await;
    sqlx::query("UPDATE session_followup_ops SET spawn_key = $2 WHERE id = $1")
        .bind(op.id)
        .bind(&to)
        .execute(&f.pool)
        .await
        .unwrap();
    to
}

async fn status(pool: &PgPool, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn launches(pool: &PgPool, machine: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM machine_launches WHERE machine_id = $1")
        .bind(machine)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn followup_events(rx: &mut Receiver<ServerFrame>) -> Vec<ServerEvent> {
    let mut out = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        if matches!(
            *frame.event,
            ServerEvent::SessionSuperseded { .. } | ServerEvent::FollowupFailed { .. }
        ) {
            out.push((*frame.event).clone());
        }
    }
    out
}

#[test]
fn a_followup_name_bumps_the_counter_of_a_user_set_name() {
    assert_eq!(followup_name(Some("triage"), true).as_deref(), Some("triage 2"));
    assert_eq!(followup_name(Some("triage 2"), true).as_deref(), Some("triage 3"));
    assert_eq!(followup_name(Some("triage 999"), true).as_deref(), Some("triage 1000"));
    assert_eq!(followup_name(Some("triage 1000"), true).as_deref(), Some("triage 1000 2"));
    assert_eq!(followup_name(Some("v 02"), true).as_deref(), Some("v 02 2"));
    assert_eq!(followup_name(Some("triage"), false), None);
    assert_eq!(followup_name(Some("  "), true), None);
    assert_eq!(followup_name(None, true), None);
}

#[tokio::test]
async fn followup_starts_pending_and_records_the_op() {
    let Some(f) = fixture("followup_op_starts_pending").await else { return };
    let daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));

    let (code, res) =
        start(&f.state, &f.ctx, &f.source, &request("k1", "fix the flaky test")).await.unwrap();

    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!((res.state, res.from.as_str()), (FollowupState::Pending, f.source.as_str()));
    assert!(res.to.is_some(), "claude pre-mints the follow-up's id");
    let op = load(&f.pool, res.followup_id).await.unwrap().unwrap();
    assert_eq!(op.reason, "cold");
    assert_eq!(op.instruction, "fix the flaky test");
    assert!(op.archive_source);
    assert_eq!(op.target_session_id, res.to);
    let recorded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM machine_launches WHERE command_id = $1")
            .bind(op.command_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(recorded, 1, "the spawn is recorded under the op's command id");

    let mut rx = daemon.await.unwrap();
    let Some(cctui_proto::ws::DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let cctui_proto::adapter::AdapterCommand::Spawn { spec, .. } = *command else {
        panic!("expected a spawn")
    };
    assert!(spec.prompt.as_deref().is_some_and(|p| p.contains("fix the flaky test")));
    assert_eq!(spec.name.as_deref(), Some("triage 2"));
    assert_eq!(spec.effort.as_deref(), Some("high"));
    assert_eq!(spec.working_dir.as_deref(), Some("/w"));
    assert_eq!(status(&f.pool, &f.source).await, "active", "nothing archived before registering");
    cleanup(&f).await;
}

#[tokio::test]
async fn same_idempotency_key_returns_the_same_followup() {
    let Some(f) = fixture("followup_op_same_key").await else { return };
    let _daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));

    let (first_code, first) =
        start(&f.state, &f.ctx, &f.source, &request("k1", "go on")).await.unwrap();
    let (again_code, again) =
        start(&f.state, &f.ctx, &f.source, &request("k1", "go on")).await.unwrap();

    assert_eq!((first_code, again_code), (StatusCode::ACCEPTED, StatusCode::OK));
    assert_eq!(again.followup_id, first.followup_id);
    assert_eq!(again.to, first.to);
    assert_eq!(launches(&f.pool, f.machine).await, 1, "one spawn only");
    cleanup(&f).await;
}

#[tokio::test]
async fn a_second_key_while_pending_is_refused_with_409() {
    let Some(f) = fixture("followup_op_second_key").await else { return };
    let _daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));

    let (_, first) = start(&f.state, &f.ctx, &f.source, &request("k1", "go on")).await.unwrap();
    let (code, busy) = start(&f.state, &f.ctx, &f.source, &request("k2", "else")).await.unwrap();

    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(busy.followup_id, first.followup_id, "the in-flight op answers");
    assert_eq!(busy.state, FollowupState::Pending);
    assert_eq!(launches(&f.pool, f.machine).await, 1);
    assert!(op_by_key(&f.pool, &f.source, "k2").await.unwrap().is_none());
    cleanup(&f).await;
}

#[tokio::test]
async fn registration_swaps_archives_and_emits_superseded() {
    let Some(f) = fixture("followup_op_swaps").await else { return };
    let op = begun(&f, FollowupReason::Cold, "go on").await;
    let to = launched_to(&f, &op).await;
    let mut rx = f.state.bus.subscribe_server();

    assert!(!settle(&f.state, op.id).await.unwrap(), "nothing to swap before registration");
    assert_eq!(status(&f.pool, &f.source).await, "active");
    crate::followup::claim_intent(&f.pool, &to, Some(&to)).await;
    let registered = load(&f.pool, op.id).await.unwrap().unwrap();
    assert_eq!(registered.state(), FollowupState::Registered);
    assert_eq!(registered.target_session_id.as_deref(), Some(to.as_str()));
    assert_eq!(status(&f.pool, &f.source).await, "active", "registration alone archives nothing");

    assert!(settle(&f.state, op.id).await.unwrap());
    assert!(!settle(&f.state, op.id).await.unwrap(), "settled once");

    assert_eq!(status(&f.pool, &f.source).await, "archived");
    let events = followup_events(&mut rx);
    assert_eq!(events.len(), 1, "{events:?}");
    let ServerEvent::SessionSuperseded {
        followup_id, from, to: got, archived, archive_error, ..
    } = &events[0]
    else {
        panic!("expected session_superseded, got {events:?}")
    };
    assert_eq!(
        (*followup_id, from.as_str(), got.as_str()),
        (op.id, f.source.as_str(), to.as_str())
    );
    assert!(*archived && archive_error.is_none());
    cleanup(&f).await;
}

#[tokio::test]
async fn registration_writes_the_marker_metadata() {
    let Some(f) = fixture("followup_op_marker").await else { return };
    let op = begun(&f, FollowupReason::Manual, "keep going").await;
    let to = session(&f.pool, f.user, f.machine).await;
    let spawn_key = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO machine_launches (command_id, session_id, machine_id, user_id, \
             acting_user_id, adapter_id, working_dir, state) \
         VALUES ($1, $2, $3, $4, $4, 'codex', '/w', 'delivered')",
    )
    .bind(op.command_id)
    .bind(&spawn_key)
    .bind(f.machine)
    .bind(f.user)
    .execute(&f.pool)
    .await
    .unwrap();

    crate::followup::claim_intent(&f.pool, &to, Some(&spawn_key)).await;

    let meta: serde_json::Value = sqlx::query_scalar("SELECT metadata FROM sessions WHERE id = $1")
        .bind(&to)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(meta["followup_reason"], "manual");
    assert_eq!(meta["followup_instruction"], "keep going");
    assert_eq!(meta["followup_id"], op.id.to_string());
    let registered = load(&f.pool, op.id).await.unwrap().unwrap();
    assert_eq!(
        registered.target_session_id.as_deref(),
        Some(to.as_str()),
        "a registration that beats the spawn key resolves through the launch row"
    );
    cleanup(&f).await;
}

#[tokio::test]
async fn archive_failure_still_keeps_the_followup() {
    let Some(f) = fixture("followup_op_archive_failure").await else { return };
    sqlx::query("UPDATE sessions SET pinned = true WHERE id = $1")
        .bind(&f.source)
        .execute(&f.pool)
        .await
        .unwrap();
    let op = begun(&f, FollowupReason::Cold, "go on").await;
    let to = launched_to(&f, &op).await;
    let mut rx = f.state.bus.subscribe_server();

    crate::followup::claim_intent(&f.pool, &to, Some(&to)).await;
    assert!(settle(&f.state, op.id).await.unwrap());

    let settled = load(&f.pool, op.id).await.unwrap().unwrap();
    assert_eq!(settled.state(), FollowupState::Registered, "the follow-up stands");
    assert_eq!(settled.archive_error.as_deref(), Some(PINNED));
    assert!(!settled.archived);
    assert_eq!(status(&f.pool, &f.source).await, "active");
    let events = followup_events(&mut rx);
    assert!(
        matches!(
            events.as_slice(),
            [ServerEvent::SessionSuperseded { archived: false, archive_error: Some(e), .. }]
                if e == PINNED
        ),
        "{events:?}"
    );
    cleanup(&f).await;
}

#[tokio::test]
async fn spawn_failure_keeps_the_source_and_reports_failed() {
    let Some(f) = fixture("followup_op_spawn_failure").await else { return };
    let mut rx = f.state.bus.subscribe_server();

    let sent = start(&f.state, &f.ctx, &f.source, &request("k1", "fix it")).await;

    assert!(sent.is_err(), "no daemon: the spawn is refused at once");
    let op = op_by_key(&f.pool, &f.source, "k1").await.unwrap().unwrap();
    assert_eq!(op.state(), FollowupState::Failed);
    assert!(op.error.is_some());
    assert_eq!(status(&f.pool, &f.source).await, "active", "nothing archived");
    let events = followup_events(&mut rx);
    assert!(
        matches!(
            events.as_slice(),
            [ServerEvent::FollowupFailed { from, instruction, .. }]
                if *from == f.source && instruction == "fix it"
        ),
        "{events:?}"
    );
    cleanup(&f).await;
}

#[tokio::test]
async fn stale_pending_followup_fails_on_sweep() {
    let Some(f) = fixture("followup_op_stale_sweep").await else { return };
    let op = begun(&f, FollowupReason::Auto, "go on").await;
    sqlx::query(
        "UPDATE session_followup_ops SET created_at = now() - interval '1 hour' WHERE id = $1",
    )
    .bind(op.id)
    .execute(&f.pool)
    .await
    .unwrap();
    let mut rx = f.state.bus.subscribe_server();

    crate::followup::sweep(&f.state).await;

    let failed = load(&f.pool, op.id).await.unwrap().unwrap();
    assert_eq!(failed.state(), FollowupState::Failed);
    assert_eq!(failed.error.as_deref(), Some(NOT_REGISTERED));
    let events = followup_events(&mut rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ServerEvent::FollowupFailed { followup_id, .. } if *followup_id == op.id
        )),
        "{events:?}"
    );
    assert_eq!(status(&f.pool, &f.source).await, "active");
    cleanup(&f).await;
}

#[tokio::test]
async fn plain_send_during_a_pending_followup_is_409() {
    let Some(f) = fixture("followup_op_plain_send").await else { return };
    begun(&f, FollowupReason::Cold, "go on").await;

    let sent = crate::routes::sessions::send_message(
        State(f.state.clone()),
        Extension(f.ctx.clone()),
        Path(f.source.clone()),
        Json(serde_json::from_value(serde_json::json!({ "content": "hi" })).unwrap()),
    )
    .await;

    let Err(err) = sent else { panic!("a send during a pending follow-up must be refused") };
    assert_eq!(err.status(), StatusCode::CONFLICT);
    assert_eq!(err.message(), BUSY);
    cleanup(&f).await;
}
