use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    CreateNotificationEndpoint, NotificationEndpoint, create, delete, list, public_config,
    send_test,
};
use crate::notify::ntfy_channel::SEALED_TOKEN;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_kernel::error::AppError;
use cctui_server_kernel::test_support::TestHost;

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

async fn state(name: &str) -> Option<TestHost> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(TestHost::new(pool))
}

async fn user(state: &TestHost) -> Uuid {
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

async fn drop_users(state: &TestHost, ids: &[Uuid]) {
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
    state: &TestHost,
    who: Uuid,
    channel: &str,
    config: Value,
) -> Result<NotificationEndpoint, AppError> {
    let req = CreateNotificationEndpoint { channel: channel.into(), label: None, config };
    create(State(state.clone()), Extension(caller(who)), Json(req)).await.map(|(_, Json(e))| e)
}

async fn listed(state: &TestHost, who: Uuid) -> Vec<NotificationEndpoint> {
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
    cctui_server_kernel::crypto::install_vault_key(vec![0x5a; 32]);
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
        cctui_server_kernel::crypto::decrypt(sealed, &cctui_server_kernel::crypto::vault_key())
            .as_deref(),
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

async fn raw_endpoint(state: &TestHost, who: Uuid, channel: &str, config: Value) -> Uuid {
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
