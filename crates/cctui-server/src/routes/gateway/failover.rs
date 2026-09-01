//! Gateway account failover — when the bound account runs out of allocation,
//! rebind the session to the sibling account with the most headroom instead of
//! letting it die against a window that resets hours later.
//!
//! Deliberately NOT an in-gateway replay: request bodies stream through
//! unbuffered on the hot path, so the refused request cannot be re-sent by the
//! server. Instead the gateway repoints the session's token row (the same
//! statement the explicit switch-account endpoint uses — the token string the
//! worker holds never changes) and answers 429 `Retry-After: 1`. Every
//! supported harness retries a 429, and the retry resolves to the new account.
//!
//! Two callers in `passthrough`:
//!
//!   * the soft-limit gate, instead of refusing with the account's own reset
//!     horizon;
//!   * an upstream 429, which today is mirrored verbatim and strands the
//!     session until its window resets.
//!
//! Guard rails: the election reuses [`crate::account_pick::pick_account`], so a
//! sibling is only chosen when it has measured headroom (or is unreadable —
//! fail open, as everywhere else); a per-session cooldown keeps a burst-429
//! (RPM, not quota) from ping-ponging a session between accounts; and
//! `CCTUI_GATEWAY_FAILOVER=0` turns the whole thing off, restoring the old
//! mirror-it-verbatim behaviour.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::state::AppState;

/// Minimum spacing between two failovers of the same session. A quota 429
/// happens once per exhausted window; anything more frequent is a burst limit
/// the account will recover from on its own, where hopping accounts only
/// churns bindings (and prompt caches) for nothing.
const FAILOVER_COOLDOWN: Duration = Duration::from_mins(1);

/// Sessions that failed over recently, keyed by session id. Process-wide like
/// the orphan-spam config: the map guards wall-clock spacing, which no test
/// state isolation needs (tests inject their own map).
static RECENT_FAILOVERS: LazyLock<dashmap::DashMap<String, Instant>> =
    LazyLock::new(dashmap::DashMap::new);

/// `CCTUI_GATEWAY_FAILOVER=0|false|off` restores the pre-failover behaviour.
fn failover_enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| !std::env::var("CCTUI_GATEWAY_FAILOVER").is_ok_and(|v| flag_disables(&v)));
    *ENABLED
}

/// Whether an env-flag value spells "off". Unset/anything else means on.
pub fn flag_disables(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no")
}

/// True when `session_id` failed over within `cooldown` of `now`.
pub fn cooldown_active(
    map: &dashmap::DashMap<String, Instant>,
    session_id: &str,
    now: Instant,
    cooldown: Duration,
) -> bool {
    map.get(session_id).is_some_and(|at| now.duration_since(*at) < cooldown)
}

/// Stamp `session_id` as having just failed over.
pub fn note_failover(map: &dashmap::DashMap<String, Instant>, session_id: &str, now: Instant) {
    map.insert(session_id.to_owned(), now);
}

/// The sibling credential a failing session should rebind to.
pub struct FailoverTarget {
    pub session_id: String,
    pub provider_id: Uuid,
    pub account_name: String,
    pub headroom_pct: Option<f64>,
}

/// Elect a sibling account for the session behind `session_token`, excluding
/// the exhausted credential. `None` when failover is off, on cooldown, when the
/// user has no sibling in the family, or when every sibling is out too — the
/// caller then falls back to the old behaviour (mirror / refuse).
///
/// The election is the spawn-time `auto_account` ranking on the same usage
/// cache the soft-limit gate reads, so gateway and spawn can never disagree
/// about who has room. Redirect chains are not followed here: a failover binds
/// the credential it scored, exactly like the explicit switch-account endpoint.
pub async fn pick_failover_target(
    state: &AppState,
    session_token: &str,
    exclude_provider: Uuid,
    model: Option<&str>,
) -> Option<FailoverTarget> {
    if !failover_enabled() {
        return None;
    }
    let hash = crate::auth::sha256_hex(session_token);
    let bound: Option<(String, Uuid, String)> = sqlx::query_as(
        "SELECT t.session_id, ap.user_id, ap.family \
         FROM session_tokens t JOIN account_providers ap ON ap.id = t.account_id \
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (session_id, user_id, family) = bound?;
    if cooldown_active(&RECENT_FAILOVERS, &session_id, Instant::now(), FAILOVER_COOLDOWN) {
        return None;
    }

    // Same candidate set as an `auto_account` spawn (owned or shared-in), minus
    // the credential that just ran dry.
    let rows: Vec<(String, Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT a.name, ap.id, ap.soft_limits_json \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.family = $2 AND ap.id != $3 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         ORDER BY a.name",
    )
    .bind(user_id)
    .bind(&family)
    .bind(exclude_provider)
    .fetch_all(&state.pool)
    .await
    .ok()?;
    if rows.is_empty() {
        return None;
    }

    let usages = futures_util::future::join_all(
        rows.iter().map(|(_, provider_id, _)| super::usage_for_soft_limit(state, *provider_id)),
    )
    .await;
    let candidates: Vec<crate::account_pick::Candidate> = rows
        .iter()
        .zip(&usages)
        .map(|((name, _, soft_limits_json), usage)| crate::account_pick::Candidate {
            name: name.clone(),
            windows: usage
                .as_ref()
                .map(crate::soft_limit::normalize_usage_windows)
                .unwrap_or_default(),
            limits: crate::soft_limit::SoftLimits::from_json(soft_limits_json.as_ref()),
            usage_known: usage.is_some(),
        })
        .collect();

    match crate::account_pick::pick_account(&candidates, model, chrono::Utc::now()) {
        crate::account_pick::Pick::Chosen { name, headroom_pct } => {
            let provider_id = rows.iter().find(|(n, _, _)| *n == name).map(|(_, id, _)| *id)?;
            Some(FailoverTarget { session_id, provider_id, account_name: name, headroom_pct })
        }
        // Everyone else is out too (or there is nobody): let the original
        // refusal stand, with its honest reset horizon.
        crate::account_pick::Pick::Exhausted(_) | crate::account_pick::Pick::None => None,
    }
}

/// Repoint the session's live token row from `from_provider` to the elected
/// target — the switch-account statement, minus the HTTP layer. Returns whether
/// the retry can be expected to land on a different account: a concurrent
/// request may have rebound first (0 rows), which is just as good.
pub async fn rebind_session(
    state: &AppState,
    target: &FailoverTarget,
    from_provider: Uuid,
) -> bool {
    let updated = sqlx::query(
        "UPDATE session_tokens SET account_id = $3 \
         WHERE session_id = $1 AND revoked_at IS NULL AND account_id = $2",
    )
    .bind(&target.session_id)
    .bind(from_provider)
    .bind(target.provider_id)
    .execute(&state.pool)
    .await;
    let rebound = match updated {
        Ok(res) => res.rows_affected() > 0,
        Err(e) => {
            tracing::warn!(session_id = %target.session_id, error = %e, "failover rebind failed");
            return false;
        }
    };
    if rebound {
        note_failover(&RECENT_FAILOVERS, &target.session_id, Instant::now());
        // The token string is unchanged — clear any orphan-spam block on its
        // fingerprint, and dismiss the per-chat soft-limit banner.
        super::clear_orphan_block_for_session(state, &target.session_id).await;
        super::clear_soft_limit_block(state, &target.session_id).await;
        tracing::warn!(
            session_id = %target.session_id,
            from = %from_provider,
            to = %target.provider_id,
            account = %target.account_name,
            headroom_pct = target.headroom_pct,
            "gateway failover: rebound session to the account with the most allocation left"
        );
    }
    // 0 rows = a concurrent failover won the race; the retry still lands on
    // the fresh binding, so the caller should answer retry-shortly either way.
    true
}

/// The response that sends the worker back around: 429 with an immediate
/// `Retry-After`, in the provider's native error envelope so the CLI renders
/// the message. The harness's own 429 backoff performs the "replay".
pub fn failover_retry_response(account_name: &str, is_anthropic: bool) -> axum::response::Response {
    use axum::response::IntoResponse;
    let message = format!(
        "cctui gateway: the bound account is out of allocation — session failed over to \
         account '{account_name}'. Retry now; the request will be served by the new account."
    );
    let body = if is_anthropic {
        serde_json::json!({
            "type": "error",
            "error": { "type": "rate_limit_error", "message": message },
        })
    } else {
        serde_json::json!({
            "error": { "message": message, "type": "rate_limit_error" },
        })
    };
    axum::response::Response::builder()
        .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
        .header(http::header::RETRY_AFTER, "1")
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-cctui-failover", account_name)
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| axum::http::StatusCode::TOO_MANY_REQUESTS.into_response())
}
