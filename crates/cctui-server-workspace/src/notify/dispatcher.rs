use std::time::Duration;

use cctui_proto::backoff::Backoff;
use futures_util::StreamExt;
use sqlx::PgPool;
use uuid::Uuid;

use super::channel::{
    Button, Capabilities, Channels, Endpoint, Press, ProviderRef, Rendered, SendError,
};
use super::store::{Edge, UPDATE_THROTTLE_SECS, priority_for_severity, transition};
use super::tokens;
use crate::config::Config;

const CLAIM_LEASE_SECS: i64 = 300;
const SWEEP_LIMIT: i64 = 50;
const CONCURRENCY: usize = 8;
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_ATTEMPTS: i32 = 11;

const fn retry_schedule() -> Backoff {
    Backoff::new(Duration::from_secs(10), Duration::from_hours(1))
}

const CLAIM_SQL: &str = "\
    WITH claimed AS ( \
        UPDATE notification_deliveries d \
        SET next_attempt_at = now() + ($1 || ' seconds')::interval \
        WHERE d.id IN ( \
            SELECT id FROM notification_deliveries \
            WHERE op IS NOT NULL AND next_attempt_at <= now() \
              AND ($3::uuid IS NULL OR notification_id = $3) \
            ORDER BY next_attempt_at \
            LIMIT $2 \
            FOR UPDATE SKIP LOCKED) \
        RETURNING d.id, d.notification_id, d.channel, d.endpoint_id, d.op, d.state, \
                  d.attempts, d.provider_ref) \
    SELECT c.id, c.notification_id, c.channel, c.endpoint_id, c.op, c.state, c.attempts, \
           c.provider_ref, n.title, n.body, n.severity, n.tags, n.actions, n.session_id, \
           n.repeat_count, \
           e.config AS endpoint_config, e.enabled AS endpoint_enabled \
    FROM claimed c \
    JOIN notifications n ON n.id = c.notification_id \
    LEFT JOIN notification_endpoints e ON e.id = c.endpoint_id";

#[derive(Debug, sqlx::FromRow)]
struct Due {
    id: Uuid,
    notification_id: Uuid,
    channel: String,
    endpoint_id: Option<Uuid>,
    op: String,
    state: String,
    attempts: i32,
    provider_ref: Option<serde_json::Value>,
    title: String,
    body: Option<String>,
    severity: String,
    tags: Option<String>,
    actions: serde_json::Value,
    session_id: Option<String>,
    repeat_count: i32,
    endpoint_config: Option<serde_json::Value>,
    endpoint_enabled: Option<bool>,
}

impl Due {
    fn rendered(&self) -> Rendered {
        Rendered {
            delivery_id: self.id,
            title: self.title.clone(),
            body: self.body.clone(),
            priority: priority_for_severity(&self.severity),
            tags: self.tags.clone(),
            link: None,
            buttons: Vec::new(),
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint { config: self.endpoint_config.clone().unwrap_or_default() }
    }

    fn provider_ref(&self) -> Option<ProviderRef> {
        self.provider_ref.clone().map(ProviderRef)
    }
}

#[derive(Debug)]
enum Outcome {
    Sent(ProviderRef),
    Updated,
    Retracted,
    Cancelled,
    Failed(SendError),
}

async fn claim(pool: &PgPool, scope: Option<Uuid>) -> sqlx::Result<Vec<Due>> {
    sqlx::query_as(CLAIM_SQL)
        .bind(CLAIM_LEASE_SECS.to_string())
        .bind(SWEEP_LIMIT)
        .bind(scope)
        .fetch_all(pool)
        .await
}

pub async fn sweep(pool: &PgPool, config: &Config) {
    run(pool, &Channels::from_config(config), None).await;
}

pub async fn run(pool: &PgPool, channels: &Channels, scope: Option<Uuid>) {
    let rows = match claim(pool, scope).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("notification dispatch claim failed: {e}");
            return;
        }
    };
    futures_util::stream::iter(rows)
        .for_each_concurrent(CONCURRENCY, |row| process(pool, channels, row))
        .await;
}

async fn process(pool: &PgPool, channels: &Channels, row: Due) {
    let outcome = attempt(pool, channels, &row).await;
    if let Err(e) = settle(pool, &row, outcome).await {
        tracing::warn!(delivery_id = %row.id, "notification delivery update failed: {e}");
    }
}

#[must_use]
pub fn deep_link(config: &Config, session_id: Option<&str>) -> Option<String> {
    let base = config.external_url.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(session_id.map_or_else(|| format!("{base}/"), |id| format!("{base}/sessions/{id}")))
}

fn requires_login(action: &serde_json::Value) -> bool {
    action.get("requires_login").and_then(serde_json::Value::as_bool) == Some(true)
}

/// An `Open` button to the deep link, then one `Post` button per action that may be
/// answered without logging in, each bound to its own single-use token.
async fn render(pool: &PgPool, config: &Config, caps: Capabilities, row: &Due) -> Rendered {
    let mut n = row.rendered();
    let Some(link) = deep_link(config, row.session_id.as_deref()) else { return n };
    n.link = Some(link.clone());
    let links_only = caps.buttons == 0 && caps.needs_token;
    let max = if links_only { usize::MAX } else { usize::from(caps.buttons) };
    if max == 0 {
        return n;
    }
    if !links_only {
        n.buttons.push(Button { label: "Open".into(), url: link, press: Press::Open });
    }
    if !caps.needs_token {
        return n;
    }
    let lifetime = chrono::Duration::hours(tokens::MAX_LIFETIME_HOURS.into());
    let expires_at = chrono::Utc::now() + lifetime;
    for action in row.actions.as_array().into_iter().flatten() {
        if n.buttons.len() >= max {
            break;
        }
        let Some(id) = action.get("id").and_then(serde_json::Value::as_str) else { continue };
        if requires_login(action) {
            continue;
        }
        let label = action.get("label").and_then(serde_json::Value::as_str).unwrap_or(id);
        match tokens::mint(pool, row.notification_id, Some(row.id), id, &row.channel, expires_at)
            .await
        {
            Ok(Some(token)) if links_only => n.buttons.push(Button {
                label: label.to_owned(),
                url: tokens::confirm_url(config, &token),
                press: Press::Open,
            }),
            Ok(Some(token)) => n.buttons.push(Button {
                label: label.to_owned(),
                url: tokens::action_url(config, &token),
                press: Press::Post,
            }),
            Ok(None) => {}
            Err(e) => tracing::warn!(delivery_id = %row.id, "action token mint failed: {e}"),
        }
    }
    n
}

async fn attempt(pool: &PgPool, channels: &Channels, row: &Due) -> Outcome {
    if row.op == "retract" && row.state != "sent" {
        return Outcome::Cancelled;
    }
    if row.endpoint_id.is_some() && row.endpoint_enabled != Some(true) {
        return match row.op.as_str() {
            "retract" => Outcome::Retracted,
            "send" => Outcome::Cancelled,
            _ => Outcome::Updated,
        };
    }
    let Some(channel) = channels.get(&row.channel) else {
        let reason = format!("channel {} is not configured", row.channel);
        return Outcome::Failed(SendError::Permanent(reason));
    };
    let caps = channel.capabilities();
    let n = match channels.config() {
        Some(config) if row.op != "retract" => render(pool, config, caps, row).await,
        _ => row.rendered(),
    };
    let (to, sent) = (row.endpoint(), row.provider_ref());
    let call = async {
        match (row.op.as_str(), sent) {
            ("send", _) => channel.send(&n, &to).await.map(Outcome::Sent),
            ("update", Some(r)) if caps.update => {
                channel.update(&n, &to, &r).await.map(|()| Outcome::Updated)
            }
            ("update", _) => Ok(Outcome::Updated),
            ("retract", Some(r)) if caps.retract => {
                channel.retract(&to, &r).await.map(|()| Outcome::Retracted)
            }
            ("retract", _) => Ok(Outcome::Retracted),
            (op, _) => Err(SendError::Permanent(format!("unknown op {op}"))),
        }
    };
    match tokio::time::timeout(CALL_TIMEOUT, call).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => Outcome::Failed(e),
        Err(_) => Outcome::Failed(SendError::Retry("timed out".into())),
    }
}

async fn settle(pool: &PgPool, row: &Due, outcome: Outcome) -> sqlx::Result<()> {
    match outcome {
        Outcome::Sent(r) => sent(pool, row, r).await,
        Outcome::Updated => updated(pool, row).await,
        Outcome::Retracted => finish(pool, row, "retracted").await,
        Outcome::Cancelled => finish(pool, row, "cancelled").await,
        Outcome::Failed(SendError::Retry(e)) => retry(pool, row, &e).await,
        Outcome::Failed(SendError::Permanent(e)) => dead(pool, row, &e).await,
        Outcome::Failed(SendError::Gone) => {
            sqlx::query("UPDATE notification_endpoints SET enabled = FALSE WHERE id = $1")
                .bind(row.endpoint_id)
                .execute(pool)
                .await?;
            if row.op == "retract" {
                finish(pool, row, "retracted").await
            } else {
                dead(pool, row, "endpoint gone").await
            }
        }
    }
}

/// A notice closed while the send was in flight gets its retraction queued; one that
/// collapsed meanwhile gets a throttled update carrying the newer text.
const SENT_SQL: &str = "\
    UPDATE notification_deliveries d SET \
        op = CASE WHEN d.state = 'cancelled' THEN 'retract' \
                  WHEN d.op = 'send' AND n.repeat_count > $3 THEN 'update' \
                  WHEN d.op = 'send' THEN NULL ELSE d.op END, \
        next_attempt_at = CASE \
            WHEN d.state <> 'cancelled' AND d.op = 'send' AND n.repeat_count > $3 \
            THEN now() + make_interval(secs => $4) ELSE now() END, \
        state = 'sent', provider_ref = $2, sent_at = now(), attempts = 0, last_error = NULL \
    FROM notifications n \
    WHERE d.id = $1 AND n.id = d.notification_id";

/// `sent_at` tracks the last push, which the collapse throttle counts from.
const UPDATED_SQL: &str = "\
    UPDATE notification_deliveries d SET \
        op = CASE WHEN n.repeat_count > $2 THEN 'update' END, \
        next_attempt_at = now() + make_interval(secs => $3), \
        sent_at = now(), attempts = 0, last_error = NULL \
    FROM notifications n \
    WHERE d.id = $1 AND d.op = 'update' AND n.id = d.notification_id";

async fn sent(pool: &PgPool, row: &Due, r: ProviderRef) -> sqlx::Result<()> {
    sqlx::query(SENT_SQL)
        .bind(row.id)
        .bind(r.0)
        .bind(row.repeat_count)
        .bind(UPDATE_THROTTLE_SECS)
        .execute(pool)
        .await?;
    if let Some(endpoint) = row.endpoint_id {
        sqlx::query("UPDATE notification_endpoints SET last_ok_at = now() WHERE id = $1")
            .bind(endpoint)
            .execute(pool)
            .await?;
    }
    transition(pool, row.notification_id, Edge::Deliver).await?;
    Ok(())
}

async fn updated(pool: &PgPool, row: &Due) -> sqlx::Result<()> {
    sqlx::query(UPDATED_SQL)
        .bind(row.id)
        .bind(row.repeat_count)
        .bind(UPDATE_THROTTLE_SECS)
        .execute(pool)
        .await?;
    Ok(())
}

async fn finish(pool: &PgPool, row: &Due, state: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET op = NULL, state = $3, \
           attempts = 0, last_error = NULL \
         WHERE id = $1 AND op = $2",
    )
    .bind(row.id)
    .bind(&row.op)
    .bind(state)
    .execute(pool)
    .await?;
    Ok(())
}

async fn retry(pool: &PgPool, row: &Due, err: &str) -> sqlx::Result<()> {
    let next = row.attempts + 1;
    if next >= MAX_ATTEMPTS {
        return dead(pool, row, err).await;
    }
    let delay = retry_schedule().delay_secs_for(u32::try_from(row.attempts).unwrap_or(u32::MAX));
    sqlx::query(
        "UPDATE notification_deliveries SET attempts = $3, last_error = $4, \
           next_attempt_at = now() + ($5 || ' seconds')::interval \
         WHERE id = $1 AND op = $2",
    )
    .bind(row.id)
    .bind(&row.op)
    .bind(next)
    .bind(err)
    .bind(delay.to_string())
    .execute(pool)
    .await?;
    tracing::warn!(
        delivery_id = %row.id,
        attempt = next,
        retry_in_secs = delay,
        "notification delivery failed, will retry: {err}"
    );
    Ok(())
}

/// A failed update leaves a sent message in place, so it stays retractable.
async fn dead(pool: &PgPool, row: &Due, err: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET \
           state = CASE WHEN op = 'update' THEN state ELSE 'dead' END, \
           op = NULL, attempts = attempts + 1, last_error = $3 \
         WHERE id = $1 AND op = $2",
    )
    .bind(row.id)
    .bind(&row.op)
    .bind(err)
    .execute(pool)
    .await?;
    tracing::error!(
        delivery_id = %row.id,
        channel = %row.channel,
        "notification delivery dead-lettered: {err}"
    );
    Ok(())
}
