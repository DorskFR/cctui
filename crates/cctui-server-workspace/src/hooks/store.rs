use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use cctui_proto::api::webhooks::{Webhook, WebhookDelivery, WebhookOutcome};

use super::ValidHook;
use crate::inbox::RETENTION_DAYS;

const DELIVERIES_LIMIT: i64 = 100;
const PRUNE_BATCH: i64 = 1000;
/// Refusals are logged up to this many a minute, so a flood cannot fill the log.
const LOGGED_REFUSALS_PER_MINUTE: i64 = 60;
const REFUSAL_RETENTION_HOURS: i32 = 24;

macro_rules! columns {
    () => {
        "id, user_id, preset_id, name, prompt_template, rate_per_minute, max_usd, \
         budget_window_hours, enabled, created_at"
    };
}

#[derive(Debug)]
pub enum WriteError {
    Invalid(String),
    NameTaken,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for WriteError {
    fn from(e: sqlx::Error) -> Self {
        if matches!(&e, sqlx::Error::Database(db) if db.is_unique_violation()) {
            Self::NameTaken
        } else {
            Self::Db(e)
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    user_id: Uuid,
    preset_id: Uuid,
    name: String,
    prompt_template: String,
    rate_per_minute: i32,
    max_usd: Option<f64>,
    budget_window_hours: i32,
    enabled: bool,
    created_at: DateTime<Utc>,
}

impl From<Row> for Webhook {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            user_id: r.user_id,
            preset_id: r.preset_id,
            name: r.name,
            prompt_template: r.prompt_template,
            rate_per_minute: r.rate_per_minute,
            max_usd: r.max_usd,
            budget_window_hours: r.budget_window_hours,
            enabled: r.enabled,
            created_at: r.created_at,
            url: String::new(),
        }
    }
}

/// What `POST /hooks/{id}` needs to judge a delivery.
#[derive(Clone, PartialEq)]
pub struct Receiver {
    pub id: Uuid,
    pub user_id: Uuid,
    pub preset_id: Uuid,
    pub secret: String,
    pub prompt_template: String,
    pub rate_per_minute: i32,
    pub max_usd: Option<f64>,
    pub budget_window_hours: i32,
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("preset_id", &self.preset_id)
            .field("secret", &"<redacted>")
            .field("rate_per_minute", &self.rate_per_minute)
            .field("max_usd", &self.max_usd)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The delivery log row the dispatch is recorded under.
    Granted(i64),
    Duplicate,
    RateLimited,
}

/// The preset must be the owner's and take external sources: a webhook body is
/// external text.
async fn check_preset(pool: &PgPool, owner: Uuid, preset_id: Uuid) -> Result<(), WriteError> {
    let external: Option<bool> =
        sqlx::query_scalar("SELECT external_sources FROM presets WHERE id = $1 AND user_id = $2")
            .bind(preset_id)
            .bind(owner)
            .fetch_optional(pool)
            .await?;
    match external {
        None => Err(WriteError::Invalid("preset_id names no preset of yours".into())),
        Some(false) => Err(WriteError::Invalid(
            "the preset does not accept external sources: set external_sources on it".into(),
        )),
        Some(true) => Ok(()),
    }
}

fn seal(secret: &str) -> String {
    crate::crypto::encrypt(secret, &crate::crypto::vault_key())
}

/// `owner` is [`crate::auth::AuthContext::owner_filter`].
pub async fn list(pool: &PgPool, owner: Option<Uuid>) -> sqlx::Result<Vec<Webhook>> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM preset_hooks WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name, id"
    ))
    .bind(owner)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<Webhook>> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM preset_hooks WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)"
    ))
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

/// Returns the hook with its secret, which is stored sealed and not shown again.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    hook: &ValidHook,
) -> Result<(Webhook, String), WriteError> {
    check_preset(pool, user_id, hook.preset_id).await?;
    let secret = super::new_secret();
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO preset_hooks \
           (user_id, preset_id, name, sealed_secret, prompt_template, rate_per_minute, max_usd, \
            budget_window_hours, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING ",
        columns!()
    ))
    .bind(user_id)
    .bind(hook.preset_id)
    .bind(&hook.name)
    .bind(seal(&secret))
    .bind(&hook.prompt_template)
    .bind(hook.rate_per_minute)
    .bind(hook.max_usd)
    .bind(hook.budget_window_hours)
    .bind(hook.enabled)
    .fetch_one(pool)
    .await?;
    Ok((row.into(), secret))
}

/// `Ok(None)` when the caller sees no hook with that id. The secret is kept.
pub async fn update(
    pool: &PgPool,
    owner: Option<Uuid>,
    id: Uuid,
    hook: &ValidHook,
) -> Result<Option<Webhook>, WriteError> {
    let Some(user_id): Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM preset_hooks WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    check_preset(pool, user_id, hook.preset_id).await?;
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE preset_hooks SET preset_id = $2, name = $3, prompt_template = $4, \
           rate_per_minute = $5, max_usd = $6, budget_window_hours = $7, enabled = $8 \
         WHERE id = $1 RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(hook.preset_id)
    .bind(&hook.name)
    .bind(&hook.prompt_template)
    .bind(hook.rate_per_minute)
    .bind(hook.max_usd)
    .bind(hook.budget_window_hours)
    .bind(hook.enabled)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

/// Replaces the secret; deliveries signed with the old one are refused.
pub async fn rotate(
    pool: &PgPool,
    owner: Option<Uuid>,
    id: Uuid,
) -> sqlx::Result<Option<(Webhook, String)>> {
    let secret = super::new_secret();
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE preset_hooks SET sealed_secret = $3 \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(owner)
    .bind(seal(&secret))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| (row.into(), secret)))
}

pub async fn delete(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "DELETE FROM preset_hooks WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// `None` for an unknown or disabled hook, one whose secret no longer opens,
/// or one whose preset is paused or no longer takes external text guarded.
pub async fn receiver(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Receiver>> {
    let row: Option<ReceiverRow> = sqlx::query_as(
        "SELECT h.user_id, h.preset_id, h.sealed_secret, h.prompt_template, h.rate_per_minute, \
                h.max_usd, h.budget_window_hours \
         FROM preset_hooks h JOIN presets p ON p.id = h.preset_id AND p.user_id = h.user_id \
         WHERE h.id = $1 AND h.enabled AND p.enabled AND p.external_sources \
           AND (p.machine_id IS NULL OR COALESCE(p.permission_mode IN ('ask', 'auto'), false))",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| {
        let secret = crate::crypto::decrypt(&r.sealed_secret, &crate::crypto::vault_key())?;
        Some(Receiver {
            id,
            user_id: r.user_id,
            preset_id: r.preset_id,
            secret,
            prompt_template: r.prompt_template,
            rate_per_minute: r.rate_per_minute,
            max_usd: r.max_usd,
            budget_window_hours: r.budget_window_hours,
        })
    }))
}

/// The sessions this hook's dispatched deliveries reached within its budget
/// window.
pub async fn budget_sessions(pool: &PgPool, hook: &Receiver) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT e.delivered_session_id FROM preset_hook_deliveries d \
         JOIN inbox_events e ON e.id = d.event_id \
         WHERE d.hook_id = $1 AND d.outcome = 'dispatched' \
           AND d.received_at > now() - make_interval(hours => $2) \
           AND e.delivered_session_id IS NOT NULL",
    )
    .bind(hook.id)
    .bind(hook.budget_window_hours)
    .fetch_all(pool)
    .await
}

#[derive(sqlx::FromRow)]
struct ReceiverRow {
    user_id: Uuid,
    preset_id: Uuid,
    sealed_secret: String,
    prompt_template: String,
    rate_per_minute: i32,
    max_usd: Option<f64>,
    budget_window_hours: i32,
}

async fn lock(tx: &mut sqlx::PgConnection, hook_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(hook_id)
        .execute(tx)
        .await
        .map(|_| ())
}

/// Serialised per hook, so two replicas cannot both pass the rate limit or
/// both take the same delivery. The sender's id is not signed, so a delivery
/// is also a replay when its signed body was already dispatched.
pub async fn claim(
    pool: &PgPool,
    hook: &Receiver,
    delivery_id: &str,
    body_sha256: &str,
) -> sqlx::Result<Claim> {
    let mut tx = pool.begin().await?;
    lock(&mut tx, hook.id).await?;
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM preset_hook_deliveries \
                        WHERE hook_id = $1 AND outcome = 'dispatched' \
                          AND (delivery_id = $2 OR body_sha256 = $3))",
    )
    .bind(hook.id)
    .bind(delivery_id)
    .bind(body_sha256)
    .fetch_one(&mut *tx)
    .await?;
    if seen {
        return Ok(Claim::Duplicate);
    }
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM preset_hook_deliveries \
         WHERE hook_id = $1 AND outcome = 'dispatched' \
           AND received_at > now() - interval '1 minute'",
    )
    .bind(hook.id)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= i64::from(hook.rate_per_minute) {
        return Ok(Claim::RateLimited);
    }
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO preset_hook_deliveries (hook_id, delivery_id, body_sha256, outcome) \
         VALUES ($1, $2, $3, 'dispatched') RETURNING id",
    )
    .bind(hook.id)
    .bind(delivery_id)
    .bind(body_sha256)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Claim::Granted(row))
}

pub async fn set_event(pool: &PgPool, row: i64, event_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE preset_hook_deliveries SET event_id = $2 WHERE id = $1")
        .bind(row)
        .bind(event_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Frees a delivery id whose event could not be queued, so a retry is accepted.
pub async fn unclaim(pool: &PgPool, row: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM preset_hook_deliveries WHERE id = $1")
        .bind(row)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn log_refusal(
    pool: &PgPool,
    hook_id: Uuid,
    delivery_id: Option<&str>,
    outcome: WebhookOutcome,
    detail: &str,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    lock(&mut tx, hook_id).await?;
    sqlx::query(
        "INSERT INTO preset_hook_deliveries (hook_id, delivery_id, outcome, detail) \
         SELECT $1, $2, $3, $4 \
         WHERE (SELECT count(*) FROM preset_hook_deliveries \
                WHERE hook_id = $1 AND outcome <> 'dispatched' \
                  AND received_at > now() - interval '1 minute') < $5",
    )
    .bind(hook_id)
    .bind(delivery_id)
    .bind(outcome.as_str())
    .bind(detail)
    .bind(LOGGED_REFUSALS_PER_MINUTE)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    id: i64,
    delivery_id: Option<String>,
    outcome: String,
    detail: Option<String>,
    event_id: Option<Uuid>,
    session_id: Option<String>,
    received_at: DateTime<Utc>,
}

/// Newest first; `None` when the caller sees no hook with that id.
pub async fn deliveries(
    pool: &PgPool,
    owner: Option<Uuid>,
    hook_id: Uuid,
) -> sqlx::Result<Option<Vec<WebhookDelivery>>> {
    if get(pool, owner, hook_id).await?.is_none() {
        return Ok(None);
    }
    let rows: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT d.id, d.delivery_id, d.outcome, COALESCE(d.detail, e.last_error) AS detail, \
                d.event_id, e.delivered_session_id AS session_id, d.received_at \
         FROM preset_hook_deliveries d LEFT JOIN inbox_events e ON e.id = d.event_id \
         WHERE d.hook_id = $1 ORDER BY d.id DESC LIMIT $2",
    )
    .bind(hook_id)
    .bind(DELIVERIES_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(Some(
        rows.into_iter()
            .map(|r| WebhookDelivery {
                id: r.id,
                delivery_id: r.delivery_id,
                outcome: WebhookOutcome::parse(&r.outcome).unwrap_or(WebhookOutcome::Rejected),
                detail: r.detail,
                event_id: r.event_id,
                session_id: r.session_id,
                received_at: r.received_at,
            })
            .collect(),
    ))
}

/// A dispatched delivery is kept for the replay window, a refusal for a day.
pub async fn prune(pool: &PgPool) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM preset_hook_deliveries WHERE id IN ( \
             SELECT id FROM preset_hook_deliveries \
             WHERE received_at < now() - make_interval(days => $1) \
                OR (outcome <> 'dispatched' \
                    AND received_at < now() - make_interval(hours => $2)) \
             LIMIT $3)",
    )
    .bind(RETENTION_DAYS)
    .bind(REFUSAL_RETENTION_HOURS)
    .bind(PRUNE_BATCH)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
