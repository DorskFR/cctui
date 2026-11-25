use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_server_kernel::test_support::with_isolated_database;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{NotificationEndpoint, delete, list, send_test, settings};
use crate::host_tests::notify_tests::notify::env_owner_tests;
use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_kernel::error::AppError;
use cctui_server_notify::notify::prefs::NotificationStage;
use cctui_server_sessions::routes::settings::{SettingsPayload, put_settings};

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

async fn state(name: &str) -> Option<AppState> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn user(state: &AppState) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("notif-endpoints-{id}"))
        .bind(format!("notif-endpoints-key-{id}"))
        .execute(&state.pool)
        .await
        .unwrap();
    id
}

async fn drop_users(state: &AppState, ids: &[Uuid]) {
    sqlx::query("DELETE FROM user_settings WHERE user_id = ANY($1)")
        .bind(ids)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(ids)
        .execute(&state.pool)
        .await
        .unwrap();
}

fn status<T>(r: Result<T, AppError>) -> StatusCode {
    match r {
        Ok(_) => StatusCode::OK,
        Err(e) => e.status(),
    }
}

async fn listed(state: &AppState, who: Uuid) -> Vec<NotificationEndpoint> {
    list(State(state.clone()), Extension(caller(who))).await.unwrap().0
}

async fn server_ntfy(pool: sqlx::PgPool) -> AppState {
    env_owner_tests::provisioned(&pool).await;
    let mut state = AppState::for_test(pool);
    state.config.ntfy_url = Some("https://ntfy.example/server".into());
    state.config.ntfy_token = Some(cctui_secrets::Secret::fixed("CCTUI_NTFY_TOKEN", "tok"));
    state
}

async fn env_rows(state: &AppState, who: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notification_endpoints \
         WHERE user_id = $1 AND config ->> 'source' = 'env'",
    )
    .bind(who)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_server_endpoint_is_offered_to_the_instance_owner_only() {
    with_isolated_database("notif_endpoints_env_owner", |pool| async move {
        let state = server_ntfy(pool).await;
        let owner = env_owner_tests::user(&state.pool, 3).await;
        let other = env_owner_tests::user(&state.pool, 2).await;
        let newcomer = env_owner_tests::user(&state.pool, 1).await;
        let env = env_owner_tests::endpoint(&state.pool, other, json!({ "source": "env" })).await;

        let mine = listed(&state, owner).await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].config, json!({ "source": "env" }));
        let kept = delete(State(state.clone()), Extension(caller(owner)), Path(mine[0].id)).await;
        assert_eq!(status(kept), StatusCode::CONFLICT);

        assert!(listed(&state, newcomer).await.is_empty());
        assert_eq!(env_rows(&state, newcomer).await, 0, "never seeded for a non-owner");
        assert!(listed(&state, other).await.is_empty(), "a leftover one stays hidden");
        let hidden = send_test(State(state.clone()), Extension(caller(other)), Path(env)).await;
        assert_eq!(status(hidden), StatusCode::NOT_FOUND);
        let hidden = delete(State(state.clone()), Extension(caller(other)), Path(env)).await;
        assert_eq!(status(hidden), StatusCode::NOT_FOUND);
    })
    .await;
}

async fn put(state: &AppState, who: Uuid, data: Value) -> Result<SettingsPayload, AppError> {
    let body = SettingsPayload { version: 1, data, revision: None };
    put_settings(State(state.clone()), Extension(caller(who)), Json(body)).await.map(|j| j.0)
}

#[tokio::test]
async fn settings_put_normalizes_notifications_and_the_route_resolves_them() {
    let Some(state) = state("notif_settings_roundtrip").await else { return };
    let u = user(&state).await;
    let data = json!({
        "display": { "notifySound": false },
        "notifications": {
            "channels": { "ntfy": { "enabled": false }, "fax": { "enabled": true } },
            "routes": { "approval": { "web_push": { "mode": "now" } } },
            "quiet_hours": { "start": "22:00", "end": "07:00", "tz": "Europe/Paris" },
        },
    });
    let saved = put(&state, u, data).await.unwrap();
    let block = &saved.data["notifications"];
    assert_eq!(block["sound"], json!(false), "display.notifySound migrates");
    assert!(block["channels"].get("fax").is_none());

    let resolved = settings(State(state.clone()), Extension(caller(u))).await.unwrap().0;
    assert!(!resolved.sound);
    assert!(!resolved.channels["ntfy"].enabled);
    assert_eq!(resolved.routes["approval"]["web_push"], NotificationStage::Now);
    assert_eq!(resolved.routes["approval"]["ntfy"], NotificationStage::After { minutes: 5 });
    assert_eq!(resolved.quiet_hours.unwrap().tz, "Europe/Paris");
    drop_users(&state, &[u]).await;
}

#[tokio::test]
async fn settings_put_refuses_an_unknown_time_zone() {
    let Some(state) = state("notif_settings_bad_tz").await else { return };
    let u = user(&state).await;
    let data = json!({ "notifications": {
        "quiet_hours": { "start": "22:00", "end": "07:00", "tz": "Mars/Olympus" } } });
    let refused = put(&state, u, data).await;
    assert_eq!(status(refused), StatusCode::BAD_REQUEST);
    drop_users(&state, &[u]).await;
}
