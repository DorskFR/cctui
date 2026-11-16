use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub use cctui_proto::api::notifications::{
    CreateNotificationEndpoint, NotificationEndpoint, NotificationSettings, NotificationTestSent,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::notify::ntfy_channel::{self, SEALED_TOKEN, user_topic_url};
use crate::notify::prefs::{self, channel};
use crate::notify::{Channels, Routing, dispatcher, store};
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;

const MAX_LABEL_CHARS: usize = 80;
const MAX_TOKEN_CHARS: usize = 512;
const MAX_ADDRESS_CHARS: usize = 254;
pub const TOKEN_NEEDS_KEY: &str =
    "an ntfy token can only be stored when the server has a vault key (CCTUI_VAULT_KEY)";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    channel: String,
    label: Option<String>,
    config: Value,
    enabled: bool,
    created_at: DateTime<Utc>,
    last_ok_at: Option<DateTime<Utc>>,
}

macro_rules! columns {
    () => {
        "id, channel, label, config, enabled, created_at, last_ok_at"
    };
}

fn bad_request(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no notification endpoint with that id")
}

fn is_env(config: &Value) -> bool {
    config.get("source").and_then(Value::as_str) == Some("env")
}

#[must_use]
pub fn public_config(channel_id: &str, config: &Value) -> Value {
    if is_env(config) {
        return json!({ "source": "env" });
    }
    match channel_id {
        channel::NTFY => json!({
            "url": config.get("url"),
            "topic": config.get("topic"),
            "has_token": config.get(SEALED_TOKEN).is_some(),
        }),
        channel::WEB_PUSH => json!({ "endpoint": config.get("endpoint") }),
        _ => config.clone(),
    }
}

impl From<Row> for NotificationEndpoint {
    fn from(row: Row) -> Self {
        Self {
            config: public_config(&row.channel, &row.config),
            id: row.id,
            channel: row.channel,
            label: row.label,
            enabled: row.enabled,
            created_at: row.created_at,
            last_ok_at: row.last_ok_at,
        }
    }
}

fn str_field<'a>(config: &'a Value, key: &str) -> Option<&'a str> {
    config.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

async fn ntfy_config(config: &Value) -> Result<Value, String> {
    let url = str_field(config, "url").ok_or("ntfy needs a server url")?.trim_end_matches('/');
    let topic = str_field(config, "topic").ok_or("ntfy needs a topic")?;
    let mut stored = Map::new();
    stored.insert("url".into(), Value::String(url.to_owned()));
    stored.insert("topic".into(), Value::String(topic.to_owned()));
    let mut stored = Value::Object(stored);
    if user_topic_url(&stored).is_none() {
        return Err("ntfy topics are letters, digits, '_' and '-', at most 64".into());
    }
    let allow = crate::outbound::upstream_allowlist();
    crate::outbound::validate_outbound_url(url, &allow)
        .await
        .map_err(|e| format!("ntfy url refused: {e}"))?;
    let Some(token) = str_field(config, "token") else { return Ok(stored) };
    if token.chars().count() > MAX_TOKEN_CHARS {
        return Err(format!("the ntfy token must be at most {MAX_TOKEN_CHARS} characters"));
    }
    let key = crate::crypto::vault_key();
    if key.is_empty() {
        return Err(TOKEN_NEEDS_KEY.into());
    }
    stored[SEALED_TOKEN] = Value::String(crate::crypto::encrypt(token, &key));
    Ok(stored)
}

async fn web_push_config(config: &Value) -> Result<Value, String> {
    let subscription = config.get("subscription").unwrap_or(config);
    let endpoint =
        str_field(subscription, "endpoint").ok_or("a push subscription needs an endpoint")?;
    let keys = subscription.get("keys");
    let key = |name: &str| keys.and_then(|k| str_field(k, name)).map(str::to_owned);
    let (Some(p256dh), Some(auth)) = (key("p256dh"), key("auth")) else {
        return Err("a push subscription needs keys.p256dh and keys.auth".into());
    };
    crate::outbound::validate_outbound_url(endpoint, &[])
        .await
        .map_err(|e| format!("push endpoint refused: {e}"))?;
    Ok(json!({ "endpoint": endpoint, "keys": { "p256dh": p256dh, "auth": auth } }))
}

fn email_config(config: &Value) -> Result<Value, String> {
    let address = str_field(config, "address").ok_or("email needs an address")?;
    if address.chars().count() > MAX_ADDRESS_CHARS {
        return Err(format!("the address must be at most {MAX_ADDRESS_CHARS} characters"));
    }
    address.parse::<lettre::Address>().map_err(|e| format!("not an email address: {e}"))?;
    Ok(json!({ "address": address }))
}

pub async fn validate(
    state: &AppState,
    channel_id: &str,
    config: &Value,
) -> Result<Value, String> {
    if !config.is_object() {
        return Err("config must be an object".into());
    }
    match channel_id {
        channel::NTFY => ntfy_config(config).await,
        channel::WEB_PUSH => web_push_config(config).await,
        channel::EMAIL if state.config.smtp_url.is_none() || state.config.smtp_from.is_none() => {
            Err("email is not configured on this server".into())
        }
        channel::EMAIL => email_config(config),
        other => Err(format!("unknown channel '{other}': ntfy, web_push or email")),
    }
}

fn label(raw: Option<&str>) -> Result<Option<String>, String> {
    let label = raw.map(str::trim).filter(|l| !l.is_empty());
    if label.is_some_and(|l| l.chars().count() > MAX_LABEL_CHARS) {
        return Err(format!("label must be at most {MAX_LABEL_CHARS} characters"));
    }
    Ok(label.map(str::to_owned))
}

async fn seed_env(state: &AppState, user_id: Uuid) -> ApiResult<()> {
    if !Routing::from_config(&state.config).ntfy {
        return Ok(());
    }
    let mut conn = state.pool.acquire().await?;
    store::seed_env_endpoint(&mut conn, user_id, ntfy_channel::ID).await?;
    Ok(())
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<Vec<NotificationEndpoint>>> {
    seed_env(&state, ctx.user_id).await?;
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM notification_endpoints WHERE user_id = $1 ORDER BY created_at, id"
    ))
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(NotificationEndpoint::from).collect()))
}

/// A browser subscribing again replaces its earlier row rather than doubling every push.
const DROP_SAME_PUSH_SQL: &str = "\
    DELETE FROM notification_endpoints \
    WHERE user_id = $1 AND channel = 'web_push' AND config ->> 'endpoint' = $2";

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateNotificationEndpoint>,
) -> ApiResult<(StatusCode, Json<NotificationEndpoint>)> {
    let channel_id = req.channel.trim();
    let config = validate(&state, channel_id, &req.config).await.map_err(bad_request)?;
    let label = label(req.label.as_deref()).map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    if channel_id == channel::WEB_PUSH {
        sqlx::query(DROP_SAME_PUSH_SQL)
            .bind(ctx.user_id)
            .bind(config.get("endpoint").and_then(Value::as_str))
            .execute(&mut *tx)
            .await?;
    }
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO notification_endpoints (user_id, channel, label, config) \
         VALUES ($1, $2, $3, $4) RETURNING ",
        columns!()
    ))
    .bind(ctx.user_id)
    .bind(channel_id)
    .bind(label)
    .bind(config)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn owned(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<Row> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM notification_endpoints WHERE id = $1 AND user_id = $2"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(not_found)
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let row = owned(&state, ctx.user_id, id).await?;
    if is_env(&row.config) {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "the server's ntfy endpoint comes from CCTUI_NTFY_URL: turn the ntfy channel off \
             in the notification settings instead",
        ));
    }
    sqlx::query("DELETE FROM notification_endpoints WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(ctx.user_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_test(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<NotificationTestSent>)> {
    let row = owned(&state, ctx.user_id, id).await?;
    if !row.enabled {
        return Err(AppError::new(StatusCode::CONFLICT, "this endpoint is disabled"));
    }
    let (record, delivery_id) =
        store::emit_test(&state.pool, ctx.user_id, &row.channel, row.id).await?;
    let pool = state.pool.clone();
    let channels = Channels::from_config(&state.config);
    tokio::spawn(async move { dispatcher::run(&pool, &channels, Some(record.id)).await });
    Ok((StatusCode::ACCEPTED, Json(NotificationTestSent { delivery_id })))
}

pub async fn settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<NotificationSettings>> {
    let mut conn = state.pool.acquire().await?;
    let stored = prefs::load(&mut conn, ctx.user_id).await?;
    Ok(Json(prefs::resolved(&stored)))
}

#[cfg(test)]
#[path = "notification_endpoints_tests.rs"]
mod tests;
