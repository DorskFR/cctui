use super::run;
use crate::events::tests::{rows_for_session, seed_session, seed_user_machine};
use crate::state::AppState;
use cctui_proto::adapter::{AdapterCommand, RemoveInitiator};
use cctui_proto::ws::DaemonFrameDown;
use serde_json::{Value, json};

async fn set_signals(
    pool: &sqlx::PgPool,
    id: &str,
    parent: Option<&str>,
    signals: [Option<&str>; 3],
    metadata: Value,
) {
    sqlx::query(
        "UPDATE sessions SET parent_id = COALESCE($2, parent_id), tempo = $3, \
                agent_state = $4, activity = $5, metadata = $6 WHERE id = $1",
    )
    .bind(id)
    .bind(parent)
    .bind(signals[0])
    .bind(signals[1])
    .bind(signals[2])
    .bind(metadata)
    .execute(pool)
    .await
    .unwrap();
}

async fn status_of(pool: &sqlx::PgPool, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn archived_detail(pool: &sqlx::PgPool, id: &str) -> Value {
    for _ in 0..100 {
        let rows = rows_for_session(pool, id).await;
        if let Some(row) = rows
            .into_iter()
            .find(|r| r.kind == cctui_server_kernel::state_events::kind::SESSION_ARCHIVED)
        {
            return row.detail;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no session.archived event for {id}");
}

const DONE: [Option<&str>; 3] = [Some("idle"), Some("done"), Some("success")];
const WORKING: [Option<&str>; 3] = [Some("active"), Some("working"), None];

#[tokio::test]
async fn sweep_waits_for_a_working_child_then_archives_once_both_are_done() {
    let Some(url) = cctui_server_kernel::test_support::db_url("auto_archive_waits_for_children")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, machine) = seed_user_machine(&pool, "aa-kids").await;
    let parent = seed_session(&pool, uid, machine, Some("orch")).await;
    let child = seed_session(&pool, uid, machine, Some("kid")).await;
    set_signals(&pool, &parent, None, DONE, json!({"auto_archive": true})).await;
    set_signals(&pool, &child, Some(&parent), WORKING, json!({"relation": "subagent"})).await;

    run(&state, Some(uid)).await;
    assert_eq!(status_of(&pool, &parent).await, "active");
    assert_eq!(status_of(&pool, &child).await, "active");

    set_signals(&pool, &child, None, DONE, json!({"relation": "subagent"})).await;
    run(&state, Some(uid)).await;
    assert_eq!(status_of(&pool, &parent).await, "archived");
    assert_eq!(status_of(&pool, &child).await, "archived");
    let detail = archived_detail(&pool, &parent).await;
    assert_eq!(detail["initiator"], "automatic");
    assert_eq!(detail["children"], 1);
    assert_eq!(detail["children_live"], 0);
}

#[tokio::test]
async fn an_automatic_archive_never_removes_a_working_child() {
    let Some(url) = cctui_server_kernel::test_support::db_url("auto_archive_spares_working_child")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, machine) = seed_user_machine(&pool, "aa-spare").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    state.bus.register_daemon(machine, uuid::Uuid::new_v4(), tx);
    let parent = seed_session(&pool, uid, machine, Some("orch")).await;
    let working = seed_session(&pool, uid, machine, Some("busy")).await;
    let finished = seed_session(&pool, uid, machine, Some("idle")).await;
    set_signals(&pool, &parent, None, DONE, json!({})).await;
    set_signals(&pool, &working, Some(&parent), WORKING, json!({"relation": "subagent"})).await;
    set_signals(&pool, &finished, Some(&parent), DONE, json!({"relation": "subagent"})).await;

    cctui_server_sessions::routes::sessions::archive_one(
        &state,
        &parent,
        false,
        RemoveInitiator::Automatic,
        cctui_server_kernel::state_events::Actor::Reaper,
    )
    .await
    .unwrap();

    let mut removed = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        if let DaemonFrameDown::Command { command, .. } = frame
            && let AdapterCommand::Remove { local_id, .. } = *command
        {
            removed.push(local_id);
        }
    }
    assert!(!removed.contains(&working), "a working child got a Remove: {removed:?}");
    assert!(removed.contains(&finished) && removed.contains(&parent), "{removed:?}");
    assert_eq!(status_of(&pool, &working).await, "active");
    assert_eq!(status_of(&pool, &finished).await, "archived");
    assert_eq!(status_of(&pool, &parent).await, "archived");
    let detail = archived_detail(&pool, &parent).await;
    assert_eq!(detail["children"], 1);
    assert_eq!(detail["children_live"], 1);
}
