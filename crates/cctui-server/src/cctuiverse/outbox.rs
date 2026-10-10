use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::client::{self, ClientError};
use super::{CloseReason, Link, LinkKind, LinkState, MAX_TEXT_BYTES, audit, enabled, publish_changed};
use crate::state::AppState;

pub enum Payload {
    Direct { text: String },
    RoomPost { room_name: String, sender_label: String, text: String },
}

impl Payload {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Direct { .. } => "direct",
            Self::RoomPost { .. } => "room_post",
        }
    }

    fn body(&self) -> Value {
        match self {
            Self::Direct { text } => json!({ "text": text }),
            Self::RoomPost { room_name, sender_label, text } => {
                json!({ "room_name": room_name, "sender_label": sender_label, "text": text })
            }
        }
    }

    fn texts(&self) -> Vec<&str> {
        match self {
            Self::Direct { text } => vec![text.as_str()],
            Self::RoomPost { room_name, sender_label, text } => {
                vec![room_name.as_str(), sender_label.as_str(), text.as_str()]
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    Queued,
    AwaitingReview,
    Refused(String),
}

const BACKOFF_SECS: [u64; 5] = [5, 30, 120, 600, 3600];
const GIVE_UP_AFTER: chrono::TimeDelta = chrono::TimeDelta::hours(24);
const SWEEP_BATCH: i64 = 50;

#[must_use]
pub fn backoff(attempts: i32) -> Duration {
    let i = usize::try_from(attempts.max(1) - 1).unwrap_or(0).min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[i])
}

const fn permanent(status: StatusCode) -> bool {
    !(status.is_server_error()
        || matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT))
}

pub async fn send(state: &AppState, link: &Link, payload: Payload) -> SendOutcome {
    send_with_id(state, link, payload, Uuid::new_v4()).await
}

fn refusal(link: &Link, payload: &Payload) -> Option<String> {
    if link.state != LinkState::Active {
        return Some("the cctuiverse link is closed".into());
    }
    if link.expired(Utc::now()) {
        return Some("the cctuiverse link has expired".into());
    }
    match (link.kind, payload) {
        (LinkKind::Session, Payload::Direct { .. }) | (LinkKind::Room, Payload::RoomPost { .. }) => {}
        _ => return Some("this message kind does not fit this link".into()),
    }
    let texts = payload.texts();
    if texts.last().is_none_or(|t| t.trim().is_empty()) {
        return Some("the message is empty".into());
    }
    if texts.iter().any(|t| t.len() > MAX_TEXT_BYTES) {
        return Some(format!("the message exceeds {MAX_TEXT_BYTES} bytes"));
    }
    texts.iter().find_map(|t| crate::envelope_guard::check(t).err())
}

pub(super) async fn send_with_id(
    state: &AppState,
    link: &Link,
    payload: Payload,
    message_id: Uuid,
) -> SendOutcome {
    if !enabled(state) {
        return SendOutcome::Refused("cctuiverse is disabled on this server".into());
    }
    if let Some(why) = refusal(link, &payload) {
        return SendOutcome::Refused(why);
    }
    let review = link.settings.review_outbound;
    let row = match reserve(state, link.id, message_id, &payload, review).await {
        Ok(Reserved::Row(id)) => id,
        Ok(Reserved::Duplicate) => return SendOutcome::Queued,
        Ok(Reserved::Capped) => {
            return SendOutcome::Refused("this link's message cap is reached".into());
        }
        Err(e) => {
            tracing::error!(link = %link.id, "cctuiverse outbox insert failed: {e}");
            return SendOutcome::Refused("database error".into());
        }
    };
    publish_changed(state, link);
    if review {
        return SendOutcome::AwaitingReview;
    }
    attempt(state, link, row).await
}

enum Reserved {
    Row(i64),
    Duplicate,
    Capped,
}

async fn reserve(
    state: &AppState,
    link_id: Uuid,
    message_id: Uuid,
    payload: &Payload,
    review: bool,
) -> Result<Reserved, sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    let counted: Option<i32> = sqlx::query_scalar(
        "UPDATE cctuiverse_links SET sent_count = sent_count + 1 \
         WHERE id = $1 AND state = 'active' \
           AND ((settings->>'max_messages') IS NULL \
                OR sent_count < (settings->>'max_messages')::int) \
         RETURNING sent_count",
    )
    .bind(link_id)
    .fetch_optional(&mut *tx)
    .await?;
    if counted.is_none() {
        return Ok(Reserved::Capped);
    }
    let row: Option<i64> = sqlx::query_scalar(
        "INSERT INTO cctuiverse_messages \
             (link_id, message_id, direction, kind, body, status, next_attempt_at) \
         VALUES ($1, $2, 'out', $3, $4, $5, CASE WHEN $5 = 'queued' THEN now() END) \
         ON CONFLICT (link_id, direction, message_id) DO NOTHING RETURNING id",
    )
    .bind(link_id)
    .bind(message_id)
    .bind(payload.kind())
    .bind(payload.body())
    .bind(if review { "review" } else { "queued" })
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Ok(Reserved::Duplicate) };
    tx.commit().await?;
    Ok(Reserved::Row(row))
}

type OutRow = (Uuid, String, Value, i32, DateTime<Utc>);

pub async fn attempt(state: &AppState, link: &Link, row: i64) -> SendOutcome {
    let loaded: Result<Option<OutRow>, _> = sqlx::query_as(
        "SELECT message_id, kind, body, attempts, created_at FROM cctuiverse_messages \
         WHERE id = $1 AND link_id = $2 AND direction = 'out' AND status = 'queued'",
    )
    .bind(row)
    .bind(link.id)
    .fetch_optional(&state.pool)
    .await;
    let Ok(Some((message_id, kind, body, attempts, created_at))) = loaded else {
        return SendOutcome::Refused("no such queued message".into());
    };
    let (Some(seed), Some(url), Some(peer_id)) =
        (link.seed(), link.peer_url.as_deref(), link.peer_link_id)
    else {
        return fail(state, link, row, "the link has no key").await;
    };
    if !link.usable() {
        return fail(state, link, row, "the link is no longer active").await;
    }
    let mut wire = body;
    wire["message_id"] = json!(message_id);
    wire["kind"] = json!(kind);
    let route = format!("/cctuiverse/v1/links/{peer_id}/messages");
    match client::post_signed(state, &seed, link.id, url, &route, &wire).await {
        Ok((status, _)) if status.is_success() => {
            let _ = sqlx::query(
                "UPDATE cctuiverse_messages SET status = 'delivered', delivered_at = now(), \
                     attempts = attempts + 1, next_attempt_at = NULL, last_error = NULL \
                 WHERE id = $1",
            )
            .bind(row)
            .execute(&state.pool)
            .await;
            SendOutcome::Delivered
        }
        Ok((status, body)) if permanent(status) => {
            let why = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.chars().take(200).collect()))
                .unwrap_or_else(|| format!("peer answered {status}"));
            fail(state, link, row, &why).await
        }
        Ok((status, _)) => {
            retry(state, link, row, attempts, created_at, &format!("peer answered {status}")).await
        }
        Err(ClientError::Url(e)) => fail(state, link, row, &e).await,
        Err(e) => retry(state, link, row, attempts, created_at, &e.to_string()).await,
    }
}

async fn fail(state: &AppState, link: &Link, row: i64, why: &str) -> SendOutcome {
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET status = 'failed', attempts = attempts + 1, \
             next_attempt_at = NULL, last_error = $2 WHERE id = $1",
    )
    .bind(row)
    .bind(why)
    .execute(&state.pool)
    .await;
    if let Some(sid) = link.session_id.as_deref() {
        audit(&state.pool, sid, &format!("message to {} not delivered: {why}", link.peer_name()))
            .await;
    }
    SendOutcome::Refused(why.to_owned())
}

async fn retry(
    state: &AppState,
    link: &Link,
    row: i64,
    attempts: i32,
    created_at: DateTime<Utc>,
    why: &str,
) -> SendOutcome {
    if Utc::now() - created_at > GIVE_UP_AFTER {
        return fail(state, link, row, &format!("gave up after 24 h ({why})")).await;
    }
    let delay = backoff(attempts + 1);
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET attempts = attempts + 1, last_error = $2, \
             next_attempt_at = now() + make_interval(secs => $3) WHERE id = $1",
    )
    .bind(row)
    .bind(why)
    .bind(delay.as_secs_f64())
    .execute(&state.pool)
    .await;
    SendOutcome::Queued
}

pub async fn sweep(state: &AppState) {
    if !enabled(state) {
        return;
    }
    if let Err(e) = sweep_inner(state).await {
        tracing::warn!("cctuiverse sweep: {e}");
    }
}

async fn sweep_inner(state: &AppState) -> Result<(), sqlx::Error> {
    let due: Vec<(i64, Uuid)> = sqlx::query_as(
        "UPDATE cctuiverse_messages SET next_attempt_at = now() + interval '1 minute' \
         WHERE id IN (SELECT id FROM cctuiverse_messages \
                      WHERE direction = 'out' AND status = 'queued' AND next_attempt_at <= now() \
                      ORDER BY next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, link_id",
    )
    .bind(SWEEP_BATCH)
    .fetch_all(&state.pool)
    .await?;
    for (row, link_id) in due {
        if let Some(link) = super::load(&state.pool, link_id).await? {
            attempt(state, &link, row).await;
        }
    }

    sqlx::query("DELETE FROM cctuiverse_nonces WHERE seen_at < now() - interval '5 minutes'")
        .execute(&state.pool)
        .await?;

    let ending: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT l.id, (l.state = 'active' AND (l.settings->>'expires_at') IS NOT NULL \
                       AND (l.settings->>'expires_at')::timestamptz <= now()) AS expired \
           FROM cctuiverse_links l \
           LEFT JOIN sessions s ON s.id = l.session_id \
           LEFT JOIN rooms r ON r.id = l.room_id \
          WHERE l.state <> 'closed' \
            AND ((l.state = 'active' AND (l.settings->>'expires_at') IS NOT NULL \
                  AND (l.settings->>'expires_at')::timestamptz <= now()) \
                 OR s.status = 'archived' OR r.archived_at IS NOT NULL \
                 OR (l.state = 'pending' AND l.invite_expires_at < now()) \
                 OR (l.state = 'pending' AND l.role = 'joiner' \
                     AND l.created_at < now() - interval '10 minutes')) \
          LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;
    for (id, expired) in ending {
        if let Some(link) = super::load(&state.pool, id).await? {
            let reason = if expired { CloseReason::Expired } else { CloseReason::Archived };
            if let Err(e) = super::close(state, &link, reason).await {
                tracing::warn!(link = %id, "cctuiverse close failed: {e}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_climbs_then_caps_at_an_hour() {
        let secs: Vec<u64> = (0..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [5, 5, 30, 120, 600, 3600, 3600, 3600, 3600]);
    }

    #[test]
    fn only_transient_statuses_are_retried() {
        for s in [500, 502, 503, 504, 429, 408] {
            assert!(!permanent(StatusCode::from_u16(s).unwrap()), "{s}");
        }
        for s in [400, 401, 403, 404, 409, 413, 422] {
            assert!(permanent(StatusCode::from_u16(s).unwrap()), "{s}");
        }
    }

    #[test]
    fn payload_bodies_match_the_wire_shape() {
        let d = Payload::Direct { text: "hi".into() };
        assert_eq!(d.kind(), "direct");
        assert_eq!(d.body(), json!({ "text": "hi" }));
        let r = Payload::RoomPost {
            room_name: "ops".into(),
            sender_label: "alice".into(),
            text: "hi".into(),
        };
        assert_eq!(r.kind(), "room_post");
        assert_eq!(r.body(), json!({ "room_name": "ops", "sender_label": "alice", "text": "hi" }));
    }
}
