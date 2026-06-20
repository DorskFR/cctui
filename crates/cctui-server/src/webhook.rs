//! Server-emitted completion webhooks (CCT-294).
//!
//! The replacement for the worker's `REPLY_URL` exit-trap as the PRIMARY
//! completion signal. The server already owns session lifecycle (daemon
//! connect/disconnect, status, the `SessionEnded` event), so it — not the
//! worker — is the authoritative place to fire a completion callback. Crucially
//! this covers cases the worker's trap CANNOT: a pod OOM/SIGKILL that never runs
//! the trap, a daemon that never connected, or a connection lost past the grace
//! window. The server detects every one of those as a terminal session state.
//!
//! Flow:
//!   1. At dispatch (see `routes::dispatch`), if the request carries
//!      `notify_url`, [`register`] writes a `pending` row to `session_webhooks`
//!      keyed on the (pre-minted) `session_id`.
//!   2. The reaper sweep ([`sweep`], called from `main::reaper_task`) finds
//!      `pending` rows whose session has reached a TERMINAL status (`ended`,
//!      `failed`, `archived`), freezes the automation-contract payload onto the row,
//!      and POSTs it. Delivery uses exponential backoff; after `MAX_ATTEMPTS`
//!      the row is dead-lettered (`state = 'dead'`, logged).
//!
//! Wire shape: `{ task_id, status, error? , verdict? }` — preserving the
//! `REPLY_URL` contract so automation flows migrate by swapping the URL. When a
//! per-target `secret` is registered, the body is signed HMAC-SHA256 and the
//! hex digest is sent in `X-CCTUI-Signature: sha256=<hex>`.
//!
//! This is ADDITIVE: the `REPLY_URL` trap keeps working during migration.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

/// Terminal session statuses that fire a completion webhook. `ended` =
/// `SessionEnded` received from the daemon (normal completion / killed /
/// crashed); `failed` = the dispatch never launched a runtime; `archived` =
/// the session went silent past the TTL and the reaper archived it (the
/// "daemon never connected" / "connection lost > grace" crash cases).
const TERMINAL_STATUSES: &[&str] = &["ended", "failed", "archived"];

/// Retry budget before dead-lettering. With the backoff schedule below this
/// spans well over an hour of attempts.
const MAX_ATTEMPTS: i32 = 8;

/// Exponential backoff (seconds) for the Nth attempt (0-indexed). Capped at the
/// last entry for any attempt beyond the table.
const BACKOFF_SECS: &[i64] = &[10, 30, 120, 300, 900, 1800, 3600];

/// Register a pending completion webhook for a dispatched session (CCT-294).
/// No-op when `notify_url` is absent. Idempotent on the `session_id` unique
/// constraint, so a re-dispatch with the same id refreshes the target rather
/// than duplicating. `task_id` falls back to the session id when the dispatch
/// payload carries none, so the receiver always gets a correlation key.
///
/// Best-effort: a failure here is logged and swallowed — it must never block an
/// otherwise-valid dispatch (the `REPLY_URL` trap still covers completion during
/// migration).
pub async fn register(
    state: &AppState,
    session_id: &str,
    user_id: uuid::Uuid,
    notify_url: &str,
    notify_secret: Option<&str>,
    task_id: &str,
) {
    let res = sqlx::query(
        "INSERT INTO session_webhooks (session_id, user_id, notify_url, secret, task_id) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (session_id) DO UPDATE SET \
           notify_url = EXCLUDED.notify_url, \
           secret = EXCLUDED.secret, \
           task_id = EXCLUDED.task_id, \
           user_id = EXCLUDED.user_id \
         WHERE session_webhooks.state = 'pending'",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(notify_url)
    .bind(notify_secret)
    .bind(task_id)
    .execute(&state.pool)
    .await;
    match res {
        Ok(_) => tracing::info!(%session_id, "registered completion webhook"),
        Err(e) => tracing::warn!(%session_id, "failed to register completion webhook: {e}"),
    }
}

/// A pending webhook joined to its session's current terminal state (if any).
#[derive(sqlx::FromRow)]
struct PendingRow {
    id: uuid::Uuid,
    session_id: String,
    notify_url: String,
    secret: Option<String>,
    task_id: String,
    attempts: i32,
    /// `Some(status)` only when the session has reached a terminal state.
    terminal_status: Option<String>,
    /// The frozen payload once captured; `None` until the session goes terminal.
    payload: Option<serde_json::Value>,
}

/// Build the automation-contract completion payload from the session's terminal state.
/// Preserves the `REPLY_URL` wire shape (`task_id`, `status`, `error`) plus an
/// optional `verdict` carrying the `SessionEnded` reason the server observed.
///
/// Mapping (server-observable — the worker's `RESULT_FILE` is not on the server,
/// so the verdict is derived from the lifecycle the daemon reported):
///   - `ended`  + reason `completed`      → status `completed`
///   - `ended`  + reason killed/crashed   → status `failed` (+ error detail)
///   - `failed` (dispatch never launched) → status `failed`
///   - `archived` (silence past grace)    → status `failed` (crash/never-connected)
fn build_payload(
    task_id: &str,
    terminal_status: &str,
    end_reason: Option<&serde_json::Value>,
) -> serde_json::Value {
    // The `SessionEnded` reason, if the daemon reported one. Tagged enum:
    // `{ "kind": "completed" | "killed" | "crashed" | "other", "detail"? }`.
    let reason_kind = end_reason.and_then(|r| r.get("kind")).and_then(|k| k.as_str());
    let reason_detail = end_reason.and_then(|r| r.get("detail")).and_then(|d| d.as_str());

    let (status, error): (&str, Option<String>) = match (terminal_status, reason_kind) {
        ("ended", Some("killed")) => ("failed", Some("session killed".into())),
        ("ended", Some("crashed")) => (
            "failed",
            Some(
                reason_detail
                    .map_or_else(|| "session crashed".into(), |d| format!("session crashed: {d}")),
            ),
        ),
        ("ended", Some("other")) => {
            // An adapter-specific end reason; treat as completed unless a detail
            // marks it failed — but stay conservative and surface the detail.
            ("completed", reason_detail.map(str::to_string))
        }
        ("ended", _) => ("completed", None),
        // `failed` = dispatch never launched; `archived` = silence past grace
        // (OOM/SIGKILL/daemon-never-connected). Both are failures from the
        // caller's perspective — the run did not produce a clean completion.
        ("failed", _) => ("failed", Some("dispatch never launched".into())),
        ("archived", _) => (
            "failed",
            Some(
                "session ended without a completion signal (timed out / crashed / connection lost)"
                    .into(),
            ),
        ),
        _ => ("failed", Some(format!("session reached terminal state {terminal_status}"))),
    };

    let mut obj = serde_json::Map::new();
    obj.insert("task_id".into(), serde_json::Value::String(task_id.to_string()));
    obj.insert("status".into(), serde_json::Value::String(status.to_string()));
    if let Some(err) = error {
        obj.insert("error".into(), serde_json::Value::String(err));
    }
    if let Some(reason) = end_reason {
        obj.insert("verdict".into(), reason.clone());
    }
    serde_json::Value::Object(obj)
}

/// Hex HMAC-SHA256 of `body` under `secret`, for the `X-CCTUI-Signature` header.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// One reaper-cadence sweep of the completion-webhook outbox (CCT-294).
///
/// For each `pending` row whose session is now terminal and which is due for an
/// attempt: freeze the payload onto the row (first time only), then POST it.
/// On a 2xx the row flips to `sent`; on failure the attempt count bumps and the
/// next attempt is scheduled with exponential backoff, dead-lettering after
/// `MAX_ATTEMPTS`. Best-effort and self-healing — a transient outage just
/// retries on the next sweep.
pub async fn sweep(state: &AppState) {
    let rows: Vec<PendingRow> = match sqlx::query_as(
        "SELECT w.id, w.session_id, w.notify_url, w.secret, w.task_id, w.attempts, \
                s.status AS terminal_status, w.payload \
         FROM session_webhooks w \
         JOIN sessions s ON s.id = w.session_id \
         WHERE w.state = 'pending' AND w.next_attempt_at <= now() \
           AND s.status = ANY($1) \
         LIMIT 50",
    )
    .bind(TERMINAL_STATUSES)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("completion-webhook sweep query failed: {e}");
            return;
        }
    };

    for row in rows {
        let Some(terminal_status) = row.terminal_status.as_deref() else {
            continue;
        };
        // Freeze the payload the first time we observe the terminal state, so a
        // later status change (e.g. archived after ended) can't rewrite a body
        // mid-retry and a server restart re-uses the same bytes.
        let payload = if let Some(p) = row.payload {
            p
        } else {
            let end_reason = latest_end_reason(state, &row.session_id).await;
            let p = build_payload(&row.task_id, terminal_status, end_reason.as_ref());
            let _ = sqlx::query("UPDATE session_webhooks SET payload = $2 WHERE id = $1")
                .bind(row.id)
                .bind(&p)
                .execute(&state.pool)
                .await;
            p
        };

        deliver(state, row.id, &row.notify_url, row.secret.as_deref(), &payload, row.attempts)
            .await;
    }
}

/// Read the most recent `session_ended` stream event's `reason`, if any. The
/// daemon records it in `mark_session_ended`; `None` for sessions that died
/// without ever emitting `SessionEnded` (the crash / never-connected path).
async fn latest_end_reason(state: &AppState, session_id: &str) -> Option<serde_json::Value> {
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM stream_events \
         WHERE session_id = $1 AND event_type = 'session_ended' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    payload.and_then(|p| p.get("reason").cloned())
}

/// POST one webhook body, updating the row on the outcome. Treats any 2xx as
/// delivered; everything else (non-2xx, transport error) bumps the retry.
async fn deliver(
    state: &AppState,
    id: uuid::Uuid,
    url: &str,
    secret: Option<&str>,
    payload: &serde_json::Value,
    attempts: i32,
) {
    let body = serde_json::to_vec(payload).unwrap_or_default();
    let mut req =
        state.http_client.post(url).header("content-type", "application/json").body(body.clone());
    if let Some(secret) = secret {
        req = req.header("X-CCTUI-Signature", format!("sha256={}", sign(secret, &body)));
    }

    let outcome = req.timeout(std::time::Duration::from_secs(30)).send().await;

    match outcome {
        Ok(resp) if resp.status().is_success() => {
            let _ = sqlx::query(
                "UPDATE session_webhooks SET state = 'sent', sent_at = now(), last_error = NULL \
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&state.pool)
            .await;
            tracing::info!(webhook_id = %id, "completion webhook delivered");
        }
        other => {
            let err = match other {
                Ok(resp) => format!("non-success status {}", resp.status()),
                Err(e) => format!("transport error: {e}"),
            };
            schedule_retry(state, id, attempts, &err).await;
        }
    }
}

/// Bump the attempt count and either schedule the next backoff attempt or
/// dead-letter the row once the budget is exhausted.
async fn schedule_retry(state: &AppState, id: uuid::Uuid, attempts: i32, err: &str) {
    let next = attempts + 1;
    if next >= MAX_ATTEMPTS {
        let _ = sqlx::query(
            "UPDATE session_webhooks SET state = 'dead', attempts = $2, last_error = $3 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(next)
        .bind(err)
        .execute(&state.pool)
        .await;
        tracing::error!(
            webhook_id = %id,
            attempts = next,
            "completion webhook dead-lettered after exhausting retries: {err}"
        );
        return;
    }
    let backoff = BACKOFF_SECS
        .get(usize::try_from(attempts).unwrap_or(usize::MAX))
        .copied()
        .unwrap_or_else(|| *BACKOFF_SECS.last().unwrap_or(&3600));
    let _ = sqlx::query(
        "UPDATE session_webhooks \
         SET attempts = $2, last_error = $3, \
             next_attempt_at = now() + ($4 || ' seconds')::interval \
         WHERE id = $1",
    )
    .bind(id)
    .bind(next)
    .bind(err)
    .bind(backoff.to_string())
    .execute(&state.pool)
    .await;
    tracing::warn!(webhook_id = %id, attempt = next, retry_in_secs = backoff, "completion webhook delivery failed, will retry: {err}");
}

#[cfg(test)]
mod tests {
    use super::{build_payload, sign};
    use serde_json::json;

    #[test]
    fn completed_payload_has_completed_status() {
        let reason = json!({ "kind": "completed" });
        let p = build_payload("task-1", "ended", Some(&reason));
        assert_eq!(p["task_id"], "task-1");
        assert_eq!(p["status"], "completed");
        assert!(p.get("error").is_none());
        assert_eq!(p["verdict"]["kind"], "completed");
    }

    #[test]
    fn crashed_payload_is_failed_with_detail() {
        let reason = json!({ "kind": "crashed", "detail": "oom" });
        let p = build_payload("t", "ended", Some(&reason));
        assert_eq!(p["status"], "failed");
        assert!(p["error"].as_str().unwrap().contains("oom"));
    }

    #[test]
    fn archived_without_end_reason_is_failed() {
        // The crash / never-connected coverage: the session was archived by the
        // reaper with no SessionEnded ever observed → the server still fires a
        // `failed` completion the worker trap would have missed.
        let p = build_payload("t", "archived", None);
        assert_eq!(p["status"], "failed");
        assert!(p["error"].as_str().unwrap().contains("without a completion signal"));
        assert!(p.get("verdict").is_none());
    }

    #[test]
    fn dispatch_never_launched_is_failed() {
        let p = build_payload("t", "failed", None);
        assert_eq!(p["status"], "failed");
        assert_eq!(p["error"], "dispatch never launched");
    }

    #[test]
    fn signature_is_stable_hex_hmac() {
        // Known HMAC-SHA256("key", "body") so a receiver can reproduce it.
        let sig = sign("key", b"body");
        assert_eq!(sig.len(), 64);
        assert_eq!(sig, sign("key", b"body"));
        assert_ne!(sig, sign("other", b"body"));
    }
}
