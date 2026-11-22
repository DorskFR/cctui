use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::adapter::AdapterCommand;
use cctui_proto::ws::DaemonFrameDown;
use tokio::sync::mpsc::Receiver;
use uuid::Uuid;

use super::{ClaimedRow, claim_one, deliver_due, settle};
use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};

const READ_ONLY: &[Scope] = &[Scope::Read];
const DISPATCH: &[Scope] = &[Scope::Read, Scope::Dispatch];

fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: scopes.iter().copied().collect(),
    }
}

async fn state(name: &str) -> Option<AppState> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn seed_owner(pool: &sqlx::PgPool, ceiling: &[Scope]) -> (Uuid, Uuid) {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("reply-gate-{uid}"))
        .bind(format!("hrg-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    set_ceiling(pool, uid, ceiling).await;
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mkrg-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    (uid, machine)
}

async fn set_ceiling(pool: &sqlx::PgPool, uid: Uuid, scopes: &[Scope]) {
    let mut conn = pool.acquire().await.unwrap();
    cctui_server_kernel::store::acls::set_user_ceiling(&mut conn, uid, scopes).await.unwrap();
}

async fn seed_session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid, tempo: &str) -> String {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id, tempo) VALUES ($1, $2, $3, $4, '/w', 'inactive', 'claude-code', $5)",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .bind(tempo)
    .execute(pool)
    .await
    .unwrap();
    sid
}

fn daemon(state: &AppState, machine: Uuid) -> Receiver<DaemonFrameDown> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    state.bus.register_daemon(machine, Uuid::new_v4(), tx);
    rx
}

fn replied(rx: &mut Receiver<DaemonFrameDown>) -> bool {
    matches!(
        rx.try_recv(),
        Ok(DaemonFrameDown::Command { command, .. })
            if matches!(*command, AdapterCommand::Reply { .. })
    )
}

async fn reply(state: &AppState, ctx: AuthContext, sid: &str) -> Result<(), StatusCode> {
    cctui_server_sessions::routes::sessions::send_message(
        State(state.clone()),
        Extension(ctx),
        Path(sid.to_owned()),
        Json(serde_json::from_value(serde_json::json!({ "content": "hi" })).unwrap()),
    )
    .await
    .map(|_| ())
    .map_err(|e| e.into_parts().0)
}

async fn queue(state: &AppState, sid: &str, user: Uuid) -> Uuid {
    let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (_, Json(resp)) = cctui_server_sessions::routes::scheduled_messages::schedule(
        state,
        &caller(user, DISPATCH),
        sid,
        "later",
        &at,
    )
    .await
    .unwrap();
    resp.command_id
}

pub(super) async fn queue_state(pool: &sqlx::PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT CASE status WHEN 'pending' THEN 'scheduled' WHEN 'claimed' THEN 'sending' \
                WHEN 'delivered' THEN 'sent' ELSE status END, last_error \
         FROM inbox_events WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn due(state: &AppState, row: ClaimedRow) {
    let (turn_id, attempts) = (row.turn_id, row.attempts);
    let outcome = deliver_due(state, row).await;
    let _ = settle(&state.pool, turn_id, attempts, outcome).await.unwrap();
}

#[tokio::test]
async fn a_reply_that_wakes_the_worker_needs_dispatch_a_live_one_does_not() {
    let name = "a_reply_that_wakes_the_worker_needs_dispatch_a_live_one_does_not";
    let Some(state) = state(name).await else { return };
    let pool = state.pool.clone();
    let (owner, machine) = seed_owner(&pool, DISPATCH).await;
    let mut rx = daemon(&state, machine);
    let asleep = seed_session(&pool, owner, machine, "hibernated").await;
    let live = seed_session(&pool, owner, machine, "idle").await;

    let refused = reply(&state, caller(owner, READ_ONLY), &asleep).await;
    assert_eq!(refused, Err(StatusCode::FORBIDDEN));
    assert!(rx.try_recv().is_err(), "a refused reply never reaches the daemon");

    let scheduled = queue(&state, &asleep, owner).await;
    let send_now = cctui_server_sessions::routes::scheduled_messages::send_now(
        State(state.clone()),
        Extension(caller(owner, READ_ONLY)),
        Path((asleep.clone(), scheduled)),
    )
    .await
    .map_err(|e| e.into_parts().0);
    assert_eq!(send_now, Err(StatusCode::FORBIDDEN));
    assert!(rx.try_recv().is_err());
    assert_eq!(queue_state(&pool, scheduled).await.0, "scheduled", "left for its owner");

    assert_eq!(reply(&state, caller(owner, DISPATCH), &asleep).await, Ok(()));
    assert!(replied(&mut rx), "a dispatch key wakes the worker");

    assert_eq!(reply(&state, caller(owner, READ_ONLY), &live).await, Ok(()));
    assert!(replied(&mut rx), "typing into a live session is unchanged");
}

fn lazy_state(features: cctui_proto::features::FeatureSet) -> AppState {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    let mut state = AppState::for_test(pool);
    state.config.features = features;
    state
}

async fn send_at(state: &AppState, sid: &str, at: &str) -> cctui_server_kernel::error::AppError {
    let body = serde_json::json!({ "content": "later", "deliver_at": at });
    let sent = cctui_server_sessions::routes::sessions::send_message(
        State(state.clone()),
        Extension(caller(Uuid::new_v4(), DISPATCH)),
        Path(sid.to_owned()),
        Json(serde_json::from_value(body).unwrap()),
    )
    .await;
    let Err(err) = sent else { panic!("a scheduled send must be refused here") };
    err
}

#[tokio::test]
async fn a_scheduled_send_is_rejected_when_scheduling_is_off() {
    use cctui_proto::features::{Feature, FeatureSet};
    let state = lazy_state(FeatureSet::all().without(Feature::Scheduling));
    let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let err = send_at(&state, "s1", &at).await;
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert_eq!(err.message(), cctui_server_sessions::routes::sessions::SCHEDULING_OFF);
}

#[tokio::test]
async fn a_scheduled_send_takes_the_queue_path_when_scheduling_is_on() {
    let state = lazy_state(cctui_proto::features::FeatureSet::all());
    let err = send_at(&state, "s1", "tomorrow").await;
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert_ne!(err.message(), cctui_server_sessions::routes::sessions::SCHEDULING_OFF);
}

#[tokio::test]
async fn with_scheduling_off_a_send_goes_out_now_and_queues_nothing() {
    use cctui_proto::features::{Feature, FeatureSet};
    let name = "with_scheduling_off_a_send_goes_out_now_and_queues_nothing";
    let Some(mut state) = state(name).await else { return };
    state.config.features = FeatureSet::all().without(Feature::Scheduling);
    let pool = state.pool.clone();
    let (owner, machine) = seed_owner(&pool, DISPATCH).await;
    let mut rx = daemon(&state, machine);
    let live = seed_session(&pool, owner, machine, "idle").await;

    assert_eq!(reply(&state, caller(owner, DISPATCH), &live).await, Ok(()));
    assert!(replied(&mut rx), "a plain send is dispatched immediately");

    let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let err = send_at(&state, &live, &at).await;
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(rx.try_recv().is_err(), "a rejected schedule is not sent now either");
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE l.key = 'session:' || $1::text",
    )
    .bind(&live)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn a_scheduled_redacted_span_is_stored_sealed_and_delivered_real() {
    let name = "a_scheduled_redacted_span_is_stored_sealed_and_delivered_real";
    let Some(state) = state(name).await else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = state.pool.clone();
    let (owner, machine) = seed_owner(&pool, DISPATCH).await;
    let mut rx = daemon(&state, machine);
    let live = seed_session(&pool, owner, machine, "idle").await;
    let content = "log in with <redacted>hunter2-xyz</redacted> please";
    let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();

    let (_, Json(resp)) = cctui_server_sessions::routes::scheduled_messages::schedule(
        &state,
        &caller(owner, DISPATCH),
        &live,
        content,
        &at,
    )
    .await
    .unwrap();
    let id = resp.command_id;
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(e)::text FROM inbox_events e WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!row.contains("hunter2-xyz"), "plaintext stored: {row}");
    assert!(row.contains("log in with [REDACTED:user] please"), "{row}");

    let Json(listed) = cctui_server_sessions::routes::scheduled_messages::list(
        State(state.clone()),
        Path(live.clone()),
    )
    .await
    .unwrap();
    let shown = listed.iter().find(|m| m.id == id).expect("listed");
    assert_eq!(shown.body, "log in with [REDACTED:user] please");

    let row = claim_one(&pool, &live, id).await.unwrap().expect("claimed");
    due(&state, row).await;
    assert_eq!(queue_state(&pool, id).await.0, "sent");
    match rx.try_recv() {
        Ok(DaemonFrameDown::Command { command, .. }) => match *command {
            AdapterCommand::Reply { text, .. } => assert_eq!(text, content),
            other => panic!("expected a reply, got {other:?}"),
        },
        other => panic!("nothing delivered: {other:?}"),
    }
}

#[tokio::test]
async fn a_due_wake_is_refused_once_its_scheduler_lost_dispatch() {
    let name = "a_due_wake_is_refused_once_its_scheduler_lost_dispatch";
    let Some(state) = state(name).await else { return };
    let pool = state.pool.clone();
    let (owner, machine) = seed_owner(&pool, DISPATCH).await;
    let mut rx = daemon(&state, machine);
    let asleep = seed_session(&pool, owner, machine, "hibernated").await;
    let live = seed_session(&pool, owner, machine, "idle").await;
    let to_asleep = queue(&state, &asleep, owner).await;
    let to_live = queue(&state, &live, owner).await;
    set_ceiling(&pool, owner, READ_ONLY).await;

    let row = claim_one(&pool, &asleep, to_asleep).await.unwrap().expect("claimed");
    due(&state, row).await;
    let (st, err) = queue_state(&pool, to_asleep).await;
    assert_eq!(st, "dead");
    assert!(err.is_some_and(|e| e.contains("dispatch")), "the refusal is recorded");
    assert!(rx.try_recv().is_err(), "the worker is not woken");

    let row = claim_one(&pool, &live, to_live).await.unwrap().expect("claimed");
    due(&state, row).await;
    assert_eq!(queue_state(&pool, to_live).await.0, "sent", "a live worker still gets it");
    assert!(replied(&mut rx));

    set_ceiling(&pool, owner, DISPATCH).await;
    let again = queue(&state, &asleep, owner).await;
    let row = claim_one(&pool, &asleep, again).await.unwrap().expect("claimed");
    due(&state, row).await;
    assert_eq!(queue_state(&pool, again).await.0, "sent");
    assert!(replied(&mut rx), "with dispatch the due message wakes the worker");
}
