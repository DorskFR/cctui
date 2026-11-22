use axum::http::StatusCode;
use uuid::Uuid;

use super::spawn_share_tests::{SPAWNER, caller, seed_machine, seed_user};
use crate::routes::fs::answer_list_dirs;
use crate::state::AppState;

const ENOENT: &str = "cannot resolve /nope: No such file or directory (os error 2)";

fn request(machine: Uuid, cwd: &str) -> cctui_proto::api::SpawnRequest {
    serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(),
        "working_dir": cwd,
        "permission_mode": "yolo",
    }))
    .unwrap()
}

fn lazy_state() -> AppState {
    AppState::for_test(sqlx::PgPool::connect_lazy("postgres://invalid").unwrap())
}

#[tokio::test]
async fn only_a_dir_the_daemon_reports_absent_is_refused() {
    let state = lazy_state();
    let machine = Uuid::new_v4();
    assert!(super::refuse_missing_cwd(&state, machine, "/w").await.is_ok(), "offline");
    assert!(super::refuse_missing_cwd(&state, machine, "  ").await.is_ok(), "no cwd");

    let daemon = answer_list_dirs(&state, machine, Ok(Vec::new()));
    assert!(super::refuse_missing_cwd(&state, machine, "/w").await.is_ok(), "exists");
    daemon.await.unwrap();

    let daemon = answer_list_dirs(&state, machine, Err(ENOENT.into()));
    let (code, body) = super::refuse_missing_cwd(&state, machine, " /nope ").await.unwrap_err();
    daemon.await.unwrap();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(body.0.error, "working directory /nope does not exist on that machine");
}

async fn rows_on(pool: &sqlx::PgPool, sql: &'static str, machine: Uuid) -> i64 {
    sqlx::query_scalar(sql).bind(machine).fetch_one(pool).await.unwrap()
}

const SESSIONS_ON: &str = "SELECT count(*) FROM sessions WHERE machine_uuid = $1";
const LAUNCHES_ON: &str = "SELECT count(*) FROM machine_launches WHERE machine_id = $1";

#[tokio::test]
async fn a_spawn_into_a_missing_dir_is_refused_before_any_row_exists() {
    let name = "a_spawn_into_a_missing_dir_is_refused_before_any_row_exists";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let daemon = answer_list_dirs(&state, machine, Err(ENOENT.into()));

    let refused = super::dispatch_spawn(
        &state,
        &caller(owner, SPAWNER),
        request(machine, "/nope"),
        Vec::new(),
        Vec::new(),
    )
    .await;
    let mut rx = daemon.await.unwrap();
    let (code, body) = refused.expect_err("refused");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(body.0.error.contains("/nope"), "{}", body.0.error);
    assert!(rx.try_recv().is_err(), "nothing but the listing reached the daemon");
    assert_eq!(rows_on(&pool, SESSIONS_ON, machine).await, 0);
    assert_eq!(rows_on(&pool, LAUNCHES_ON, machine).await, 0);
}

#[tokio::test]
async fn an_offline_machine_is_not_refused_for_its_cwd() {
    let name = "an_offline_machine_is_not_refused_for_its_cwd";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let state = AppState::for_test(pool);
    let outcome = super::dispatch_spawn(
        &state,
        &caller(owner, SPAWNER),
        request(machine, "/nope"),
        Vec::new(),
        Vec::new(),
    )
    .await;
    if let Err((_, body)) = outcome {
        assert!(!body.0.error.contains("does not exist"), "{}", body.0.error);
    }
}
