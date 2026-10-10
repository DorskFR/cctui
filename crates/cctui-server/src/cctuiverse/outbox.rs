use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use futures_util::StreamExt;
use uuid::Uuid;

use super::client::{self, ClientError};
use super::{
    CloseReason, Link, LinkKind, LinkState, MAX_TEXT_BYTES, audit, enabled, publish_changed,
};
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
/// 50 rows, 8 at a time, at most 15 s each: a pass ends well inside the 3 min lease.
const SWEEP_CONCURRENCY: usize = 8;

#[must_use]
pub fn backoff(attempts: i32) -> Duration {
    let i = usize::try_from(attempts.max(1) - 1).unwrap_or(0).min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[i])
}

fn permanent(status: StatusCode) -> bool {
    !(status.is_server_error()
        || matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::REQUEST_TIMEOUT))
}

pub async fn send(state: &AppState, link: &Link, payload: Payload) -> SendOutcome {
    send_with_id(state, link, payload, Uuid::new_v4(), true).await
}

/// Like [`send`], but never waits on the peer: the attempt runs on its own task.
pub async fn enqueue(state: &AppState, link: &Link, payload: Payload) -> SendOutcome {
    send_with_id(state, link, payload, Uuid::new_v4(), false).await
}

fn refusal(link: &Link, payload: &Payload) -> Option<String> {
    if link.state != LinkState::Active {
        return Some("the cctuiverse link is closed".into());
    }
    if link.expired(Utc::now()) {
        return Some("the cctuiverse link has expired".into());
    }
    match (link.kind, payload) {
        (LinkKind::Session, Payload::Direct { .. })
        | (LinkKind::Room, Payload::RoomPost { .. }) => {}
        _ => return Some("this message kind does not fit this link".into()),
    }
    let texts = payload.texts();
    if texts.last().is_none_or(|t| t.trim().is_empty()) {
        return Some("the message is empty".into());
    }
    if texts.iter().any(|t| t.len() > MAX_TEXT_BYTES) {
        return Some(format!("the message exceeds {MAX_TEXT_BYTES} bytes"));
    }
    texts.iter().find_map(|t| crate::envelope_guard::check_remote(t).err())
}

pub(super) async fn send_with_id(
    state: &AppState,
    link: &Link,
    payload: Payload,
    message_id: Uuid,
    inline: bool,
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
    if inline {
        return attempt(state, link, row).await;
    }
    let (state, link) = (state.clone(), link.clone());
    tokio::spawn(async move { attempt(&state, &link, row).await });
    SendOutcome::Queued
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
             (link_id, message_id, direction, kind, body, status, next_attempt_at, \
              first_queued_at) \
         VALUES ($1, $2, 'out', $3, $4, $5, \
                 CASE WHEN $5 = 'queued' THEN now() + interval '1 minute' END, \
                 CASE WHEN $5 = 'queued' THEN now() END) \
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
        "SELECT message_id, kind, body, attempts, COALESCE(first_queued_at, created_at) \
         FROM cctuiverse_messages \
         WHERE id = $1 AND link_id = $2 AND direction = 'out' AND status = 'queued'",
    )
    .bind(row)
    .bind(link.id)
    .fetch_optional(&state.pool)
    .await;
    let Ok(Some((message_id, kind, body, attempts, queued_at))) = loaded else {
        return SendOutcome::Refused("no such queued message".into());
    };
    if kind == "close" {
        return attempt_close(state, link, row, attempts, queued_at).await;
    }
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
                 WHERE id = $1 AND status = 'queued'",
            )
            .bind(row)
            .execute(&state.pool)
            .await;
            let _ = sqlx::query(
                "UPDATE cctuiverse_links SET peer_404_count = 0, peer_404_since = NULL \
                 WHERE id = $1 AND peer_404_count > 0",
            )
            .bind(link.id)
            .execute(&state.pool)
            .await;
            SendOutcome::Delivered
        }
        Ok((status, body)) if permanent(status) => {
            let why = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.chars().take(200).collect()))
                .unwrap_or_else(|| format!("peer answered {status}"));
            let outcome = fail(state, link, row, &why).await;
            if status == StatusCode::NOT_FOUND
                && peer_gone(state, link.id).await
                && let Err(e) = super::close(state, link, CloseReason::Gone).await
            {
                tracing::warn!(link = %link.id, "cctuiverse close after peer 404s failed: {e}");
            }
            outcome
        }
        Ok((status, _)) => {
            retry(state, link, row, attempts, queued_at, &format!("peer answered {status}")).await
        }
        Err(ClientError::Url(e)) => fail(state, link, row, &e).await,
        Err(e) => retry(state, link, row, attempts, queued_at, &e.to_string()).await,
    }
}

/// The receiver's uniform 404 also covers clock skew, a redeploy or a peer that
/// briefly disabled cctuiverse, so one 404 only fails its message. The link is
/// treated as gone after this many in a row, the first at least this long ago.
const GONE_AFTER_404S: i32 = 5;
const GONE_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

async fn peer_gone(state: &AppState, link_id: Uuid) -> bool {
    let counted: Option<(i32, DateTime<Utc>)> = sqlx::query_as(
        "UPDATE cctuiverse_links SET peer_404_count = peer_404_count + 1, \
             peer_404_since = COALESCE(peer_404_since, now()) \
         WHERE id = $1 RETURNING peer_404_count, peer_404_since",
    )
    .bind(link_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    counted.is_some_and(|(n, since)| gone(n, Utc::now() - since))
}

fn gone(count: i32, span: chrono::TimeDelta) -> bool {
    count >= GONE_AFTER_404S && span >= GONE_AFTER
}

/// Queue the close notice for a just-closed link; the sweep sends it within
/// seconds. It is retried like any message, 404s included, since a joiner's
/// withdrawal can reach the inviter before the inviter committed; the link's
/// key is wiped once it settles. Not attempted inline: `attempt` can close a
/// link, which lands here, so spawning `attempt` would make the futures recursive.
pub async fn enqueue_close(state: &AppState, link: &Link) {
    let queued = sqlx::query(
        "INSERT INTO cctuiverse_messages \
             (link_id, message_id, direction, kind, body, status, next_attempt_at, \
              first_queued_at) \
         VALUES ($1, $2, 'out', 'close', '{}'::jsonb, 'queued', now(), now())",
    )
    .bind(link.id)
    .bind(Uuid::new_v4())
    .execute(&state.pool)
    .await;
    if let Err(e) = queued {
        tracing::warn!(link = %link.id, "cctuiverse close notice not queued: {e}");
        wipe_key(state, link.id).await;
    }
}

const CLOSE_GIVE_UP_AFTER: chrono::TimeDelta = chrono::TimeDelta::hours(1);

async fn wipe_key(state: &AppState, link_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE cctuiverse_links SET encrypted_private_key = NULL \
         WHERE id = $1 AND state = 'closed'",
    )
    .bind(link_id)
    .execute(&state.pool)
    .await;
}

async fn settle_close(state: &AppState, link: &Link, row: i64, status: &str, why: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET status = $2, attempts = attempts + 1, \
             next_attempt_at = NULL, last_error = $3, \
             delivered_at = CASE WHEN $2 = 'delivered' THEN now() END \
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(row)
    .bind(status)
    .bind(why)
    .execute(&state.pool)
    .await;
    wipe_key(state, link.id).await;
}

async fn attempt_close(
    state: &AppState,
    link: &Link,
    row: i64,
    attempts: i32,
    queued_at: DateTime<Utc>,
) -> SendOutcome {
    let (Some(seed), Some(url), Some(peer_id)) =
        (link.seed(), link.peer_url.as_deref(), link.peer_link_id)
    else {
        settle_close(state, link, row, "failed", Some("the link has no key")).await;
        return SendOutcome::Refused("the link has no key".into());
    };
    let route = format!("/cctuiverse/v1/links/{peer_id}/close");
    let why = match client::post_signed(state, &seed, link.id, url, &route, &json!({})).await {
        Ok((status, _)) if status.is_success() => {
            settle_close(state, link, row, "delivered", None).await;
            return SendOutcome::Delivered;
        }
        Ok((status, _)) if permanent(status) && status != StatusCode::NOT_FOUND => {
            let why = format!("peer answered {status}");
            settle_close(state, link, row, "failed", Some(&why)).await;
            return SendOutcome::Refused(why);
        }
        Ok((status, _)) => format!("peer answered {status}"),
        Err(e) => e.to_string(),
    };
    if Utc::now() - queued_at > CLOSE_GIVE_UP_AFTER {
        settle_close(state, link, row, "failed", Some(&why)).await;
        return SendOutcome::Refused(why);
    }
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET attempts = attempts + 1, last_error = $2, \
             next_attempt_at = now() + make_interval(secs => $3) \
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(row)
    .bind(&why)
    .bind(backoff(attempts + 1).as_secs_f64())
    .execute(&state.pool)
    .await;
    SendOutcome::Queued
}

async fn fail(state: &AppState, link: &Link, row: i64, why: &str) -> SendOutcome {
    let changed = sqlx::query(
        "UPDATE cctuiverse_messages SET status = 'failed', attempts = attempts + 1, \
             next_attempt_at = NULL, last_error = $2 WHERE id = $1 AND status = 'queued'",
    )
    .bind(row)
    .bind(why)
    .execute(&state.pool)
    .await
    .is_ok_and(|r| r.rows_affected() == 1);
    if changed && let Some(sid) = link.session_id.as_deref() {
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
    queued_at: DateTime<Utc>,
    why: &str,
) -> SendOutcome {
    if Utc::now() - queued_at > GIVE_UP_AFTER {
        return fail(state, link, row, &format!("gave up after 24 h ({why})")).await;
    }
    let delay = backoff(attempts + 1);
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET attempts = attempts + 1, last_error = $2, \
             next_attempt_at = now() + make_interval(secs => $3) \
         WHERE id = $1 AND status = 'queued'",
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
    let preambles: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cctuiverse_links WHERE preamble_pending AND state = 'active' LIMIT 50",
    )
    .fetch_all(&state.pool)
    .await?;
    for id in preambles {
        super::flush_preamble(state, id).await;
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

    sqlx::query(
        "UPDATE cctuiverse_links l SET encrypted_private_key = NULL \
         WHERE l.state = 'closed' AND l.encrypted_private_key IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM cctuiverse_messages m \
                           WHERE m.link_id = l.id AND m.kind = 'close' AND m.status = 'queued')",
    )
    .execute(&state.pool)
    .await?;

    let due: Vec<(i64, Uuid)> = sqlx::query_as(
        "UPDATE cctuiverse_messages SET next_attempt_at = now() + interval '3 minutes' \
         WHERE id IN (SELECT id FROM cctuiverse_messages \
                      WHERE direction = 'out' AND status = 'queued' AND next_attempt_at <= now() \
                      ORDER BY next_attempt_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, link_id",
    )
    .bind(SWEEP_BATCH)
    .fetch_all(&state.pool)
    .await?;
    futures_util::stream::iter(due)
        .for_each_concurrent(SWEEP_CONCURRENCY, |(row, link_id)| async move {
            match super::load(&state.pool, link_id).await {
                Ok(Some(link)) => {
                    attempt(state, &link, row).await;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(link = %link_id, "cctuiverse sweep load: {e}"),
            }
        })
        .await;
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
    fn a_link_is_gone_only_after_repeated_404s_over_ten_minutes() {
        let min = chrono::TimeDelta::minutes;
        assert!(!gone(1, min(60)));
        assert!(!gone(4, min(60)));
        assert!(!gone(5, min(9)));
        assert!(gone(5, min(10)));
        assert!(gone(9, min(30)));
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
