use cctui_proto::models::MachineLiveness;
use cctui_server_dispatch::machine_liveness::apply;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::state::AppState;

/// One `machine.offline` per dead-window crossing, none on later sweeps.
#[tokio::test]
async fn an_offline_machine_logs_one_offline_row_and_one_end_per_session() {
    let Some(url) = crate::routes::gateway::test_db_url("liveness_offline_events") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "lv-off").await;
    let a = crate::events::tests::seed_session(&pool, uid, machine, Some("a")).await;
    let b = crate::events::tests::seed_session(&pool, uid, machine, Some("b")).await;
    sqlx::query("UPDATE machines SET last_seen_at = now() - interval '2 hours' WHERE id = $1")
        .bind(machine)
        .execute(&pool)
        .await
        .unwrap();
    state.machine_liveness.insert(machine, MachineLiveness::Online);
    let row: (Uuid, DateTime<Utc>) =
        sqlx::query_as("SELECT id, last_seen_at FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_one(&pool)
            .await
            .unwrap();
    // Only this machine: the shared test DB holds thousands of other
    // tests' stale machines, which a full sweep would all take offline.
    for _ in 0..3 {
        apply(&state, vec![row]).await;
    }
    let offline = events_once_recorded(&pool, machine).await;
    assert_eq!(offline.len(), 1, "one machine.offline, not one per sweep or per session");
    assert_eq!(offline[0].0["ended_sessions"], 2);
    for sid in [&a, &b] {
        let rows = crate::events::tests::rows_for_session(&pool, sid).await;
        let ended: Vec<_> = rows.iter().filter(|r| r.kind == "session.ended").collect();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].detail["end_reason"], "machine_offline");
    }
}

/// The sweep records its events on detached tasks; wait, bounded, for the
/// machine's `machine.offline` plus one `session.ended` per session.
async fn events_once_recorded(pool: &sqlx::PgPool, machine: Uuid) -> Vec<(serde_json::Value,)> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let offline: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT detail FROM events WHERE machine_id = $1 AND kind = 'machine.offline'",
        )
        .bind(machine)
        .fetch_all(pool)
        .await
        .unwrap();
        let ended: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE machine_id = $1 AND kind = 'session.ended'",
        )
        .bind(machine)
        .fetch_one(pool)
        .await
        .unwrap();
        if (!offline.is_empty() && ended >= 2) || tokio::time::Instant::now() >= deadline {
            return offline;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
