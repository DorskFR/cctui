use super::*;
use cctui_server_kernel::bus::NoopTransport;

fn test_bus() -> Bus {
    Bus::new(Box::new(NoopTransport))
}

/// The `(state, user_id, launch_at, last_error)` of the next scheduled-launch
/// event for `draft_id`, or `None` if the stream holds none.
#[allow(clippy::type_complexity)]
fn next_launch_event(
    rx: &mut tokio::sync::broadcast::Receiver<cctui_server_kernel::bus::ServerFrame>,
    draft_id: &str,
) -> Option<(ScheduledLaunchState, Option<uuid::Uuid>, Option<DateTime<Utc>>, Option<String>)> {
    while let Ok(frame) = rx.try_recv() {
        if let ServerEvent::ScheduledLaunch { draft_id: id, user_id, state, launch_at, last_error } =
            &*frame.event
            && id == draft_id
        {
            return Some((*state, *user_id, *launch_at, last_error.clone()));
        }
    }
    None
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-24T10:00:00Z").unwrap().with_timezone(&Utc)
}

async fn seed_draft(pool: &sqlx::PgPool, name: &str) -> String {
    seed_owned_draft(pool, name).await.0
}

async fn seed_owned_draft(pool: &sqlx::PgPool, name: &str) -> (String, uuid::Uuid) {
    let uid = uuid::Uuid::new_v4();
    let machine = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{name}-{uid}"))
        .bind(format!("h-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    let sid = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $2, $3, '/w', 'draft', 'claude-code')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    (sid, uid)
}

async fn lane_of(pool: &sqlx::PgPool, draft: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM inbox_lanes WHERE key = $1")
        .bind(lane_key(draft))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn event_of(pool: &sqlx::PgPool, draft: &str) -> (String, i32, Option<String>) {
    sqlx::query_as(
        "SELECT e.status, e.attempts, e.last_error FROM inbox_events e \
         JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE l.key = $1 AND e.source = 'timer' AND e.source_id = $2",
    )
    .bind(lane_key(draft))
    .bind(draft)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn claim(pool: &sqlx::PgPool, draft: &str) -> Vec<BatchEvent> {
    let lane = lane_of(pool, draft).await;
    let filter =
        cctui_server_sessions::inbox::claim::LaneFilter { lane: Some(lane), machine: None };
    let lanes = cctui_server_sessions::inbox::claim::claim_lanes(pool, 1, filter).await.unwrap();
    if lanes.is_empty() {
        return Vec::new();
    }
    let events = cctui_server_sessions::inbox::claim::claim_batch(pool, lane).await.unwrap();
    cctui_server_sessions::inbox::claim::release_lane(pool, lane).await.unwrap();
    events
}

#[tokio::test]
async fn a_timed_draft_launches_with_its_sealed_env() {
    let name = "scheduled_spawn_sealed_env";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    let (_, owner) = seed_owned_draft(&pool, name).await;
    let machine: Uuid = sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1")
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
    let state = AppState::for_test(pool.clone());
    let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(), "working_dir": "/w", "prompt": "digest",
        "permission_mode": "yolo", "no_account": true, "save_draft": true,
        "env": {"FOO": "bar-sealed-value"},
    }))
    .unwrap();
    let draft = cctui_server_sessions::routes::spawn::store_spawn_row(
        &state,
        &req,
        machine,
        &[],
        SessionRowStatus::Draft,
    )
    .await
    .unwrap()
    .to_string();
    let metadata: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM sessions WHERE id = $1")
            .bind(&draft)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!metadata.to_string().contains("bar-sealed-value"), "{metadata}");
    assert_eq!(metadata["draft"]["env_keys"], serde_json::json!(["FOO"]));

    schedule(&pool, &state.bus, &draft, Some(owner), now()).await.unwrap();
    let events = claim(&pool, &draft).await;
    assert_eq!(events.len(), 1);
    let daemon =
        cctui_server_sessions::routes::fs::answer_list_dirs(&state, machine, Ok(Vec::new()));
    let outcome = launch(&state, &draft, &events).await;
    let mut rx = daemon.await.unwrap();
    assert!(matches!(outcome, Outcome::Delivered { .. }), "{outcome:?}");
    let Some(cctui_proto::ws::DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let cctui_proto::adapter::AdapterCommand::Spawn { spec, .. } = *command else {
        panic!("expected a spawn")
    };
    assert_eq!(spec.env.get("FOO").map(String::as_str), Some("bar-sealed-value"));
}

async fn auto_archive_intents(pool: &sqlx::PgPool, spawn_key: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM session_auto_archive WHERE spawn_key = $1")
        .bind(spawn_key)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn launch_scheduled_draft(
    state: &AppState,
    owner: Uuid,
    machine: Uuid,
    auto_archive: bool,
) -> String {
    let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(), "working_dir": "/w", "prompt": "nightly",
        "no_account": true, "save_draft": true, "auto_archive": auto_archive,
    }))
    .unwrap();
    let draft = cctui_server_sessions::routes::spawn::store_spawn_row(
        state,
        &req,
        machine,
        &[],
        SessionRowStatus::Draft,
    )
    .await
    .unwrap()
    .to_string();
    schedule(&state.pool, &state.bus, &draft, Some(owner), now()).await.unwrap();
    let events = claim(&state.pool, &draft).await;
    assert_eq!(events.len(), 1);
    let daemon =
        cctui_server_sessions::routes::fs::answer_list_dirs(state, machine, Ok(Vec::new()));
    let outcome = launch(state, &draft, &events).await;
    drop(daemon.await.unwrap());
    let Outcome::Delivered { session_id, .. } = outcome else {
        panic!("expected a delivered launch, got {outcome:?}")
    };
    session_id
}

#[tokio::test]
async fn a_scheduled_draft_keeps_its_auto_archive_intent() {
    let name = "scheduled_spawn_auto_archive";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    let (_, owner) = seed_owned_draft(&pool, name).await;
    let machine: Uuid = sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1")
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
    let state = AppState::for_test(pool.clone());

    let armed = launch_scheduled_draft(&state, owner, machine, true).await;
    assert_eq!(auto_archive_intents(&pool, &armed).await, 1, "a ticked draft arms it");
    let plain = launch_scheduled_draft(&state, owner, machine, false).await;
    assert_eq!(auto_archive_intents(&pool, &plain).await, 0, "an unticked draft does not");

    sqlx::query("DELETE FROM session_auto_archive WHERE spawn_key = $1")
        .bind(&armed)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn discarding_a_draft_takes_its_schedule_with_it() {
    let name = "scheduled_spawn_cascade";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let draft = seed_draft(&pool, name).await;
    schedule(&pool, &test_bus(), &draft, None, now()).await.unwrap();
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&draft).execute(&pool).await.unwrap();
    assert!(pending_for(&pool, std::slice::from_ref(&draft)).await.unwrap().is_empty());

    let state = AppState::for_test(pool.clone());
    let events = claim(&pool, &draft).await;
    assert_eq!(events.len(), 1);
    assert_eq!(launch(&state, &draft, &events).await, Outcome::Skip);
    let (status, _, err) = event_of(&pool, &draft).await;
    assert_eq!((status.as_str(), err.as_deref()), ("cancelled", Some(GONE)), "not dead");
}

#[tokio::test]
async fn every_transition_pushes_a_scheduled_launch_event() {
    let name = "scheduled_spawn_events";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let bus = &state.bus;
    let mut rx = bus.subscribe_server();
    let (draft, owner) = seed_owned_draft(&pool, name).await;
    let at = Utc::now() + chrono::Duration::hours(3);

    schedule(&pool, bus, &draft, Some(owner), at).await.unwrap();
    let (st, user, launch_at, err) = next_launch_event(&mut rx, &draft).expect("scheduled");
    assert_eq!((st, user, err), (ScheduledLaunchState::Scheduled, Some(owner), None));
    assert_eq!(launch_at, Some(at), "the schedule rides along so no refetch is needed");

    assert!(cancel(&pool, bus, &draft).await.unwrap());
    let (st, user, ..) = next_launch_event(&mut rx, &draft).expect("cancelled");
    assert_eq!((st, user), (ScheduledLaunchState::Cancelled, Some(owner)));
    assert!(
        next_launch_event(&mut rx, &draft).is_none(),
        "cancelling an unscheduled draft is silent"
    );
    assert!(!cancel(&pool, bus, &draft).await.unwrap());
    assert!(next_launch_event(&mut rx, &draft).is_none());

    schedule(&pool, bus, &draft, Some(owner), at).await.unwrap();
    next_launch_event(&mut rx, &draft).expect("rescheduled");
    let lane = lane_of(&pool, &draft).await;

    for (status, error, expected) in [
        ("pending", Some("machine offline"), ScheduledLaunchState::Failed),
        ("dead", Some("still offline"), ScheduledLaunchState::Dead),
        ("delivered", None, ScheduledLaunchState::Launched),
    ] {
        sqlx::query(
            "UPDATE inbox_events SET status = $3, last_error = $4 \
             WHERE lane_id = $1 AND source = 'timer' AND source_id = $2",
        )
        .bind(lane)
        .bind(&draft)
        .bind(status)
        .bind(error)
        .execute(&pool)
        .await
        .unwrap();
        announce_settled(&state, lane, &draft).await.unwrap();
        let (st, user, _, err) = next_launch_event(&mut rx, &draft).expect(status);
        assert_eq!((st, user, err.as_deref()), (expected, Some(owner), error));
    }

    sqlx::query("UPDATE inbox_events SET status = 'pending', last_error = NULL WHERE lane_id = $1")
        .bind(lane)
        .execute(&pool)
        .await
        .unwrap();
    announce_settled(&state, lane, &draft).await.unwrap();
    assert!(next_launch_event(&mut rx, &draft).is_none(), "a deferred launch is silent");
}
