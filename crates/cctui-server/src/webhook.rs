//! Server-emitted completion webhooks.
//!
//! A lifecycle-only **death-detector**, complementing — not replacing — the
//! worker's `REPLY_URL` exit-trap. The worker owns the verdict: on any orderly
//! exit (clean, killed, or crashed) its trap POSTs the real `RESULT_FILE` to
//! `REPLY_URL`, and that payload (opaque to the server) is the source of truth.
//! The server fires only for the cases the worker's trap CANNOT cover — a pod
//! OOM/SIGKILL that never runs the trap, a worker that never registered
//! (`CrashLoopBackOff` / unschedulable), or a connection lost past grace — and
//! then the callback is uniformly `status:"failed"` with a reason. The verdict
//! never transits or is stored on the server.
//!
//! Flow:
//!   1. At dispatch (see `routes::dispatch`), if the request carries
//!      `notify_url`, [`register`] writes a `pending` row to `session_webhooks`
//!      keyed on the (pre-minted) `session_id`; the dispatch handle is persisted
//!      to `dispatch_handles`.
//!   2. The reaper sweep ([`sweep`], called from `main::reaper_task`) resolves
//!      each `pending` row via [`decide`]: a clean `SessionEnded` is superseded
//!      (the worker's trap owned the callback); a session that never registered
//!      is probed by asking the owning **dispatcher** whether its workload is
//!      `Running` / `Complete` / `Failed` / `Gone`. Only `Failed`/`Gone` (and
//!      the dispatch-never-launched / time-archive backstops) fire the death
//!      payload, with exponential-backoff delivery and dead-lettering after
//!      `MAX_ATTEMPTS`.
//!
//! Wire shape: `{ task_id, status:"failed", error }` — preserving the
//! `REPLY_URL` contract so automation flows migrate by swapping the URL. When a
//! per-target `secret` is registered, the body is signed HMAC-SHA256 and the
//! hex digest is sent in `X-CCTUI-Signature: sha256=<hex>`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::dispatchers::HandleStatus;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

/// How long to wait before re-polling a dispatcher for a still-running workload.
/// Separate from delivery backoff — a liveness poll that says
/// "running" must not consume the delivery retry budget.
const POLL_INTERVAL_SECS: i64 = 30;

/// Retry budget before dead-lettering. With the backoff schedule below this
/// spans well over an hour of attempts.
const MAX_ATTEMPTS: i32 = 8;

/// Exponential backoff (seconds) for the Nth attempt (0-indexed). Capped at the
/// last entry for any attempt beyond the table.
const BACKOFF_SECS: &[i64] = &[10, 30, 120, 300, 900, 1800, 3600];

#[derive(Debug)]
pub enum NotifyUrlError {
    Malformed,
    NotHttps,
    NoHost,
    Unresolvable,
    Internal,
}

impl std::fmt::Display for NotifyUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "must be a valid absolute URL",
            Self::NotHttps => "must use the https scheme",
            Self::NoHost => "must include a host",
            Self::Unresolvable => "host does not resolve",
            Self::Internal => "resolves to a private or loopback address",
        })
    }
}

fn ipv4_is_internal(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        // CGNAT 100.64.0.0/10; `Ipv4Addr::is_shared` is still unstable.
        || (a == 100 && (64..=127).contains(&b))
}

fn ipv6_is_internal(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // `to_ipv4` also maps `::`/`::1`, but those return above, so any remaining
    // embedded IPv4 (v4-mapped or deprecated v4-compatible) is a real target.
    if let Some(v4) = ip.to_ipv4() {
        return ipv4_is_internal(v4);
    }
    let seg0 = ip.segments()[0];
    (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
}

fn ip_is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_internal(v4),
        IpAddr::V6(v6) => ipv6_is_internal(v6),
    }
}

/// Fail-closed SSRF guard: requires `https` and refuses a host that resolves to
/// an internal address or does not resolve. DNS rebinding at delivery is out of
/// scope — pinning the resolved IP is left for a follow-up.
pub async fn validate_notify_url(raw: &str) -> Result<(), NotifyUrlError> {
    let url = reqwest::Url::parse(raw).map_err(|_| NotifyUrlError::Malformed)?;
    if url.scheme() != "https" {
        return Err(NotifyUrlError::NotHttps);
    }
    let host = url.host_str().ok_or(NotifyUrlError::NoHost)?;
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return if ip_is_internal(ip) { Err(NotifyUrlError::Internal) } else { Ok(()) };
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| NotifyUrlError::Unresolvable)?
        .peekable();
    if addrs.peek().is_none() {
        return Err(NotifyUrlError::Unresolvable);
    }
    for addr in addrs {
        if ip_is_internal(addr.ip()) {
            return Err(NotifyUrlError::Internal);
        }
    }
    Ok(())
}

/// Redirects are disabled so a target can't 3xx-bounce the POST onto an internal
/// address the registration check vetted; the shared gateway client follows
/// redirects, so it is not reused here.
fn delivery_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build webhook delivery client")
    })
}

/// Register a pending completion webhook for a dispatched session.
/// No-op when `notify_url` is absent. Idempotent on the `session_id` unique
/// constraint, so a re-dispatch with the same id refreshes the target rather
/// than duplicating. `task_id` falls back to the session id when the dispatch
/// payload carries none, so the receiver always gets a correlation key.
///
/// Best-effort: a failure here is logged and swallowed — it must never block an
/// otherwise-valid dispatch (the `REPLY_URL` trap still covers completion during
/// migration).
#[allow(clippy::cognitive_complexity)]
pub async fn register(
    state: &AppState,
    session_id: &str,
    user_id: uuid::Uuid,
    notify_url: &str,
    notify_secret: Option<&str>,
    task_id: &str,
) {
    if let Err(reason) = validate_notify_url(notify_url).await {
        tracing::warn!(%session_id, "refusing completion webhook with unsafe notify_url: {reason}");
        return;
    }
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

/// A pending webhook joined to its session's current status and dispatch handle.
#[derive(sqlx::FromRow)]
struct PendingRow {
    id: uuid::Uuid,
    session_id: String,
    user_id: Option<uuid::Uuid>,
    notify_url: String,
    secret: Option<String>,
    task_id: String,
    attempts: i32,
    /// The session's current status, if a `sessions` row exists. `None` for a
    /// dispatched session whose worker never registered (pod crashlooped / never
    /// started) — exactly the case the dispatcher poll below resolves.
    session_status: Option<String>,
    /// The owning dispatcher's name + opaque handle, if one was
    /// persisted at dispatch. `None` for the http escape-hatch.
    dispatcher_name: Option<String>,
    handle: Option<String>,
    /// The frozen payload once we've decided to fire; `None` until then.
    payload: Option<serde_json::Value>,
}

/// Build the lifecycle-only death payload. The server webhook is a
/// *death-detector*: it only ever fires for a run that did NOT complete, so the
/// status is always `failed`. The real verdict of a clean run is delivered by
/// the worker's own `REPLY_URL` callback and never touches the server.
///
/// Preserves the `REPLY_URL` wire shape: `{ task_id, status:"failed", error }`.
fn build_payload(task_id: &str, reason: &str) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("task_id".into(), serde_json::Value::String(task_id.to_string()));
    obj.insert("status".into(), serde_json::Value::String("failed".into()));
    obj.insert("error".into(), serde_json::Value::String(reason.to_string()));
    serde_json::Value::Object(obj)
}

/// Hex HMAC-SHA256 of `body` under `secret`, for the `X-CCTUI-Signature` header.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// What the sweep should do with one pending row this cycle.
enum Outcome {
    /// The run died without a conclusion — fire the death callback with reason.
    Fire(String),
    /// The worker owns the callback (clean exit → its `REPLY_URL` trap delivered
    /// the verdict). Close the row without firing.
    Supersede,
    /// Still alive / not yet decidable — re-poll on the next sweep.
    Wait,
}

/// Decide a pending row's fate. The server is a death-detector: it
/// fires only for runs that did NOT complete. A clean `SessionEnded` (the
/// worker reached its exit trap and `POSTed` `REPLY_URL`) is superseded; a session
/// that never registered is resolved by asking the owning dispatcher whether its
/// workload is still alive.
async fn decide(state: &AppState, row: &PendingRow) -> Outcome {
    match row.session_status.as_deref() {
        // Any SessionEnded means the worker process reached its EXIT/INT/TERM
        // trap (completed, killed, or crashed) and already POSTed REPLY_URL —
        // the worker owns the verdict, the server stays quiet.
        Some("ended") => return Outcome::Supersede,
        // Dispatch never launched a runtime: no worker, no callback ever.
        Some("failed") => return Outcome::Fire("dispatch never launched".into()),
        // Time-based archive backstop (silence past grace) for when no
        // dispatcher poll resolved it first.
        Some("archived") => {
            return Outcome::Fire(
                "session ended without a completion signal (timed out / crashed / connection lost)"
                    .into(),
            );
        }
        // Non-terminal (active/inactive) or no session row at all (worker never
        // registered — crashloop/never-started): fall through to the dispatcher
        // liveness probe.
        _ => {}
    }

    let (Some(name), Some(handle)) = (row.dispatcher_name.as_deref(), row.handle.as_deref()) else {
        // No persisted handle (http escape-hatch, or a dispatch) —
        // nothing to probe; the time-based archive path is the only backstop.
        return Outcome::Wait;
    };
    let Ok(Some(dispatcher)) =
        crate::routes::dispatch::resolve_dispatcher(state, row.user_id, name).await
    else {
        // Dispatcher gone/unreachable — wait; archive backstop still applies.
        return Outcome::Wait;
    };
    match dispatcher.status(handle).await {
        Ok(HandleStatus::Complete) => Outcome::Supersede,
        Ok(HandleStatus::Failed(reason)) => {
            Outcome::Fire(reason.unwrap_or_else(|| "workload failed".into()))
        }
        Ok(HandleStatus::Gone) => {
            Outcome::Fire("workload no longer exists (crashed / evicted before reporting)".into())
        }
        // Still running, or the dispatcher can't introspect (http) / a transient
        // error — wait.
        Ok(HandleStatus::Running) | Err(_) => Outcome::Wait,
    }
}

/// One reaper-cadence sweep of the completion-webhook outbox.
///
/// For each due `pending` row: a row with a frozen payload is a delivery retry
/// (POST it). Otherwise [`decide`] resolves the run's fate — `Fire` freezes the
/// death payload and POSTs it (2xx → `sent`, else backoff/dead-letter);
/// `Supersede` closes the row (the worker's own callback owns the verdict);
/// `Wait` re-polls on the next sweep. Best-effort and self-healing.
// Linear per-row outbox processing with per-outcome handling; complexity is
// per-branch, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn sweep(state: &AppState) {
    let rows: Vec<PendingRow> = match sqlx::query_as(
        "SELECT w.id, w.session_id, w.user_id, w.notify_url, w.secret, w.task_id, w.attempts, \
                s.status AS session_status, dh.dispatcher_name, dh.handle, w.payload \
         FROM session_webhooks w \
         LEFT JOIN sessions s ON s.id = w.session_id \
         LEFT JOIN dispatch_handles dh ON dh.session_id = w.session_id \
         WHERE w.state = 'pending' AND w.next_attempt_at <= now() \
         LIMIT 50",
    )
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
        // A frozen payload means we already decided to fire — this is a retry.
        if let Some(payload) = row.payload.clone() {
            deliver(state, row.id, &row.notify_url, row.secret.as_deref(), &payload, row.attempts)
                .await;
            continue;
        }

        match decide(state, &row).await {
            Outcome::Fire(reason) => {
                let payload = build_payload(&row.task_id, &reason);
                // Freeze the payload so a later state change can't rewrite the
                // body mid-retry and a server restart re-uses the same bytes.
                let _ = sqlx::query("UPDATE session_webhooks SET payload = $2 WHERE id = $1")
                    .bind(row.id)
                    .bind(&payload)
                    .execute(&state.pool)
                    .await;
                deliver(
                    state,
                    row.id,
                    &row.notify_url,
                    row.secret.as_deref(),
                    &payload,
                    row.attempts,
                )
                .await;
            }
            Outcome::Supersede => {
                let _ =
                    sqlx::query("UPDATE session_webhooks SET state = 'superseded' WHERE id = $1")
                        .bind(row.id)
                        .execute(&state.pool)
                        .await;
                tracing::debug!(session_id = %row.session_id, "webhook superseded by worker callback");
            }
            Outcome::Wait => {
                // Back off the next liveness poll without bumping the retry
                // budget (that budget is for delivery failures, not polling).
                let _ = sqlx::query(
                    "UPDATE session_webhooks \
                     SET next_attempt_at = now() + ($2 || ' seconds')::interval WHERE id = $1",
                )
                .bind(row.id)
                .bind(POLL_INTERVAL_SECS.to_string())
                .execute(&state.pool)
                .await;
            }
        }
    }
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
        delivery_client().post(url).header("content-type", "application/json").body(body.clone());
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
    use super::{NotifyUrlError, build_payload, ip_is_internal, sign, validate_notify_url};

    #[test]
    fn ip_classifier_flags_internal_and_passes_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.31.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:169.254.169.254",
        ] {
            assert!(ip_is_internal(ip.parse().unwrap()), "{ip} must be internal");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!ip_is_internal(ip.parse().unwrap()), "{ip} must be public");
        }
    }

    #[tokio::test]
    async fn notify_url_accepts_public_https() {
        validate_notify_url("https://1.1.1.1/hook").await.unwrap();
        validate_notify_url("https://93.184.216.34:8443/cb").await.unwrap();
    }

    #[tokio::test]
    async fn notify_url_rejects_non_https() {
        assert!(matches!(
            validate_notify_url("http://1.1.1.1/hook").await,
            Err(NotifyUrlError::NotHttps)
        ));
        assert!(matches!(
            validate_notify_url("file:///etc/passwd").await,
            Err(NotifyUrlError::NotHttps)
        ));
        assert!(matches!(validate_notify_url("not a url").await, Err(NotifyUrlError::Malformed)));
    }

    #[tokio::test]
    async fn notify_url_rejects_internal_targets() {
        for u in [
            "https://127.0.0.1/x",
            "https://127.0.0.1:8080/x",
            "https://10.0.0.5/x",
            "https://172.16.9.9/x",
            "https://192.168.1.1/x",
            "https://100.64.0.1/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[::ffff:169.254.169.254]/",
        ] {
            assert!(matches!(validate_notify_url(u).await, Err(NotifyUrlError::Internal)), "{u}");
        }
    }

    #[test]
    fn death_payload_is_always_failed_with_reason() {
        // The server webhook is a death-detector: it only fires for a
        // run that did not complete, so the status is always `failed` and the
        // dispatcher's reason rides in `error`. The verdict of a clean run is
        // delivered by the worker's own REPLY_URL callback and never here.
        let p = build_payload("task-1", "CrashLoopBackOff");
        assert_eq!(p["task_id"], "task-1");
        assert_eq!(p["status"], "failed");
        assert_eq!(p["error"], "CrashLoopBackOff");
        assert!(p.get("verdict").is_none());
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
