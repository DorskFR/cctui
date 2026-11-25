use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_server_dispatch::launch::LaunchError;
use uuid::Uuid;

use super::spawn_share_tests::{SPAWNER, caller, seed_machine, seed_user};
use super::{QueueTarget, SpawnFailure, queues_on};
use crate::state::AppState;
use cctui_server_kernel::bus::BusError;
use cctui_server_sessions::routes::fs::answer_list_dirs;
use cctui_server_sessions::store::sessions::SessionRowStatus;

fn request(machine: Uuid, cwd: &str) -> cctui_proto::api::SpawnRequest {
    serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(),
        "working_dir": cwd,
        "permission_mode": "yolo",
        "prompt": "hello",
    }))
    .unwrap()
}

#[test]
fn only_a_spawn_that_reached_no_daemon_queues() {
    assert!(queues_on(&LaunchError::Delivery(BusError::NoDaemon(Uuid::nil()))));
    assert!(queues_on(&LaunchError::Delivery(BusError::Closed)));
    for sent_or_refused in [
        LaunchError::Delivery(BusError::Timeout),
        LaunchError::Delivery(BusError::Disconnected),
        LaunchError::Refused("bad cwd".into()),
        LaunchError::Policy("hook policy".into()),
        LaunchError::Storage(sqlx::Error::RowNotFound),
    ] {
        assert!(!queues_on(&sent_or_refused), "{sent_or_refused}");
    }
}

async fn queued_event(pool: &sqlx::PgPool, session_id: &str) -> (String, String, bool) {
    sqlx::query_as(
        "SELECT l.key, e.status, e.expires_at > now() + interval '23 hours' \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id WHERE l.session_id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn status_of(pool: &sqlx::PgPool, session_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_delivery_error_queues_while_a_refusal_returns_the_error() {
    let name = "a_delivery_error_queues_while_a_refusal_returns_the_error";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let ctx = caller(owner, SPAWNER);

    let undelivered =
        super::try_dispatch_spawn(&state, &ctx, request(machine, "/w"), Vec::new(), Vec::new())
            .await;
    let Err(SpawnFailure { response: (code, _), queue }) = undelivered else {
        panic!("no daemon is connected");
    };
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    let target = queue.expect("nothing was sent, so the spawn may queue");
    assert_eq!(target, QueueTarget { machine_uuid: machine, owner });

    let (code, Json(queued)) =
        super::queue_spawn(&state, &ctx, target, &request(machine, "/w"), &[]).await.unwrap();
    assert_eq!((code, queued.status.as_str()), (StatusCode::ACCEPTED, "queued"));
    let id = queued.command_id.to_string();
    assert_eq!(status_of(&pool, &id).await.as_deref(), Some("queued"));
    assert_eq!(queued_event(&pool, &id).await, (format!("session:{id}"), "pending".into(), true));
    let (stored, _, _) =
        super::stored_spawn(&state, &id, SessionRowStatus::Queued).await.expect("stored");
    assert_eq!((stored.working_dir.as_str(), stored.prompt.as_deref()), ("/w", Some("hello")));
    assert!(super::stored_spawn(&state, &id, SessionRowStatus::Draft).await.is_err());

    let daemon = answer_list_dirs(
        &state,
        machine,
        Err("cannot resolve /nope: No such file or directory (os error 2)".into()),
    );
    let refused =
        super::try_dispatch_spawn(&state, &ctx, request(machine, "/nope"), Vec::new(), Vec::new())
            .await;
    daemon.await.unwrap();
    let failure = refused.expect_err("a missing cwd is refused");
    assert_eq!(failure.response.0, StatusCode::BAD_REQUEST);
    assert!(failure.queue.is_none(), "a refusal never queues");
}

#[tokio::test]
async fn discarding_a_queued_spawn_cancels_its_events() {
    let name = "discarding_a_queued_spawn_cancels_its_events";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let ctx = caller(owner, SPAWNER);
    let target = QueueTarget { machine_uuid: machine, owner };
    let (_, Json(queued)) =
        super::queue_spawn(&state, &ctx, target, &request(machine, "/w"), &[]).await.unwrap();
    let id = queued.command_id.to_string();

    let code = super::discard_draft(State(state.clone()), Path(id.clone())).await.unwrap();
    assert_eq!(code, StatusCode::NO_CONTENT);
    assert_eq!(status_of(&pool, &id).await, None);
    let status: String = sqlx::query_scalar(
        "SELECT e.status FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE l.key = $1",
    )
    .bind(format!("session:{id}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "cancelled");
}
