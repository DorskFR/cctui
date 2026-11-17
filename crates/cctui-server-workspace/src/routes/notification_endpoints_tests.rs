use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_server_core::test_support::with_isolated_database;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    CreateNotificationEndpoint, NotificationEndpoint, create, delete, list, public_config,
    send_test, settings,
};
use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::notify::env_owner_tests;
use crate::notify::ntfy_channel::SEALED_TOKEN;
use crate::notify::prefs::NotificationStage;
use crate::routes::settings::{SettingsPayload, put_settings};
use crate::state::AppState;

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

async fn add(
    state: &AppState,
    who: Uuid,
    channel: &str,
    config: Value,
) -> Result<NotificationEndpoint, AppError> {
    let req = CreateNotificationEndpoint { channel: channel.into(), label: None, config };
    create(State(state.clone()), Extension(caller(who)), Json(req)).await.map(|(_, Json(e))| e)
}

async fn listed(state: &AppState, who: Uuid) -> Vec<NotificationEndpoint> {
    list(State(state.clone()), Extension(caller(who))).await.unwrap().0
}

fn push_subscription(endpoint: &str) -> Value {
    json!({ "endpoint": endpoint, "keys": { "p256dh": "BPk", "auth": "c2Vj" } })
}

#[tokio::test]
async fn endpoints_crud_is_owner_scoped() {
    let Some(state) = state("notif_endpoints_owner_scope").await else { return };
    let (alice, bob) = (user(&state).await, user(&state).await);
    let mine =
        add(&state, alice, "web_push", push_subscription("https://1.1.1.1/push/a")).await.unwrap();
    assert_eq!(mine.channel, "web_push");
    assert_eq!(listed(&state, alice).await.iter().map(|e| e.id).collect::<Vec<_>>(), [mine.id]);
    assert!(listed(&state, bob).await.is_empty(), "bob sees none of alice's endpoints");

    let refused = delete(State(state.clone()), Extension(caller(bob)), Path(mine.id)).await;
    assert_eq!(status(refused), StatusCode::NOT_FOUND);
    let tested = send_test(State(state.clone()), Extension(caller(bob)), Path(mine.id)).await;
    assert_eq!(status(tested), StatusCode::NOT_FOUND);
    assert_eq!(listed(&state, alice).await.len(), 1);

    let gone = delete(State(state.clone()), Extension(caller(alice)), Path(mine.id)).await;
    assert_eq!(gone.unwrap(), StatusCode::NO_CONTENT);
    assert!(listed(&state, alice).await.is_empty());
    drop_users(&state, &[alice, bob]).await;
}

#[tokio::test]
async fn a_browser_subscribing_twice_keeps_one_row() {
    let Some(state) = state("notif_endpoints_push_dedupe").await else { return };
    let u = user(&state).await;
    let wrapped = json!({ "subscription": push_subscription("https://1.1.1.1/push/same") });
    add(&state, u, "web_push", wrapped).await.unwrap();
    let again =
        add(&state, u, "web_push", push_subscription("https://1.1.1.1/push/same")).await.unwrap();
    let all = listed(&state, u).await;
    assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), [again.id]);
    drop_users(&state, &[u]).await;
}

#[tokio::test]
async fn an_ntfy_token_is_sealed_and_never_returned() {
    crate::crypto::install_vault_key(vec![0x5a; 32]);
    let Some(state) = state("notif_endpoints_ntfy_token").await else { return };
    let u = user(&state).await;
    let config = json!({ "url": "https://1.1.1.1/", "topic": "mine", "token": "tk_secret" });
    let created = add(&state, u, "ntfy", config).await.unwrap();
    assert_eq!(
        created.config,
        json!({ "url": "https://1.1.1.1", "topic": "mine", "has_token": true })
    );
    let stored: Value =
        sqlx::query_scalar("SELECT config FROM notification_endpoints WHERE id = $1")
            .bind(created.id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let sealed = stored[SEALED_TOKEN].as_str().unwrap();
    assert_ne!(sealed, "tk_secret");
    assert_eq!(
        crate::crypto::decrypt(sealed, &crate::crypto::vault_key()).as_deref(),
        Some("tk_secret")
    );
    assert!(stored.get("token").is_none());
    drop_users(&state, &[u]).await;
}

#[tokio::test]
async fn bad_endpoint_configs_are_refused() {
    let Some(state) = state("notif_endpoints_refused").await else { return };
    let u = user(&state).await;
    let cases = [
        ("ntfy", json!({ "url": "http://1.1.1.1", "topic": "t" })),
        ("ntfy", json!({ "url": "https://10.0.0.5", "topic": "t" })),
        ("ntfy", json!({ "url": "https://1.1.1.1", "topic": "a/b" })),
        ("ntfy", json!({ "topic": "t" })),
        ("web_push", json!({ "endpoint": "https://1.1.1.1/p" })),
        ("web_push", push_subscription("https://127.0.0.1/p")),
        ("email", json!({ "address": "me@example.com" })),
        ("signal", json!({})),
        ("ntfy", json!("https://1.1.1.1")),
    ];
    for (channel, config) in cases {
        let refused = add(&state, u, channel, config.clone()).await;
        assert_eq!(status(refused), StatusCode::BAD_REQUEST, "{channel} {config}");
    }
    assert!(listed(&state, u).await.is_empty());
    drop_users(&state, &[u]).await;
}

#[test]
fn public_configs_hide_secrets() {
    let ntfy = json!({ "url": "https://n", "topic": "t", SEALED_TOKEN: "sealed" });
    assert_eq!(
        public_config("ntfy", &ntfy),
        json!({ "url": "https://n", "topic": "t", "has_token": true })
    );
    let push = push_subscription("https://push/x");
    assert_eq!(public_config("web_push", &push), json!({ "endpoint": "https://push/x" }));
    assert_eq!(public_config("ntfy", &json!({ "source": "env" })), json!({ "source": "env" }));
}

async fn raw_endpoint(state: &AppState, who: Uuid, channel: &str, config: Value) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO notification_endpoints (user_id, channel, config) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(who)
    .bind(channel)
    .bind(config)
    .fetch_one(&state.pool)
    .await
    .unwrap()
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
        let stale = env_owner_tests::endpoint(&state.pool, other, json!({ "source": "env" })).await;

        let mine = listed(&state, owner).await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].config, json!({ "source": "env" }));
        let kept = delete(State(state.clone()), Extension(caller(owner)), Path(mine[0].id)).await;
        assert_eq!(status(kept), StatusCode::CONFLICT);

        assert!(listed(&state, newcomer).await.is_empty());
        assert_eq!(env_rows(&state, newcomer).await, 0, "never seeded for a non-owner");
        assert!(listed(&state, other).await.is_empty(), "a stale one stays hidden");
        let hidden = send_test(State(state.clone()), Extension(caller(other)), Path(stale)).await;
        assert_eq!(status(hidden), StatusCode::NOT_FOUND);
        let hidden = delete(State(state.clone()), Extension(caller(other)), Path(stale)).await;
        assert_eq!(status(hidden), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn send_test_creates_one_delivery_to_that_endpoint_only() {
    let Some(state) = state("notif_endpoints_send_test").await else { return };
    let u = user(&state).await;
    let closed = json!({ "url": "http://127.0.0.1:9", "topic": "closed" });
    let target = raw_endpoint(&state, u, "ntfy", closed.clone()).await;
    let other = raw_endpoint(&state, u, "ntfy", closed).await;

    let (code, Json(sent)) =
        send_test(State(state.clone()), Extension(caller(u)), Path(target)).await.unwrap();
    assert_eq!(code, StatusCode::ACCEPTED);
    let rows: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT d.id, d.endpoint_id, n.kind FROM notification_deliveries d \
         JOIN notifications n ON n.id = d.notification_id WHERE n.user_id = $1",
    )
    .bind(u)
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(rows, [(sent.delivery_id, Some(target), "test".to_owned())]);
    let others: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notification_deliveries WHERE endpoint_id = $1")
            .bind(other)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(others, 0);
    drop_users(&state, &[u]).await;
}

#[tokio::test]
async fn a_disabled_endpoint_is_not_tested() {
    let Some(state) = state("notif_endpoints_test_disabled").await else { return };
    let u = user(&state).await;
    let id =
        raw_endpoint(&state, u, "ntfy", json!({ "url": "http://127.0.0.1:9", "topic": "t" })).await;
    sqlx::query("UPDATE notification_endpoints SET enabled = FALSE WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    let refused = send_test(State(state.clone()), Extension(caller(u)), Path(id)).await;
    assert_eq!(status(refused), StatusCode::CONFLICT);
    drop_users(&state, &[u]).await;
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
