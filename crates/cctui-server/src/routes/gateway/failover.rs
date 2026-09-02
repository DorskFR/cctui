//! Gateway account failover — when the bound account runs out of allocation,
//! rebind the session to the account an explicit redirect rule names instead
//! of letting it die against a window that resets hours later.
//!
//! Off by default: only `CCTUI_GATEWAY_FAILOVER=1|true|on|yes` enables it. Even
//! then a live session is only ever moved where the user said it may go — an
//! unexpired `account_redirects` rule for the exhausted account, the same rule
//! that moves launches ([`super::mint`]). There is no implicit balancing: no
//! sibling is ever elected by headroom, and a session with no matching rule
//! stays put and sees the honest refusal.
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
//!   * an upstream 429, which is otherwise mirrored verbatim and strands the
//!     session until its window resets.
//!
//! Like a launch, the redirect target is applied regardless of its own usage.
//! A per-session cooldown keeps a burst-429 (RPM, not quota) from ping-ponging
//! a session between accounts.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::state::AppState;
use crate::store::account_redirects::AccountRedirect;

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

/// Opt-in: `CCTUI_GATEWAY_FAILOVER=1|true|on|yes`. Unset or anything else
/// leaves failover off.
fn failover_enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var("CCTUI_GATEWAY_FAILOVER").is_ok_and(|v| flag_enables(&v)));
    *ENABLED
}

/// Whether an env-flag value spells "on". Unset/anything else means off.
pub fn flag_enables(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes")
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

/// The credential a failing session should rebind to.
pub struct FailoverTarget {
    pub session_id: String,
    pub provider_id: Uuid,
    pub account_name: String,
}

/// The account an explicit rule sends `from_account` to for `model`. A rule
/// matching the exact model beats a catch-all (`match_model` NULL); with no
/// model known (the soft-limit gate) only a catch-all applies. Rules that flip
/// the model rather than the account never move a session.
pub fn explicit_target(
    rules: &[AccountRedirect],
    from_account: Uuid,
    family: &str,
    model: Option<&str>,
) -> Option<Uuid> {
    let candidates = || {
        rules.iter().filter(|r| {
            r.from_account == from_account && r.family == family && r.to_account.is_some()
        })
    };
    model
        .and_then(|m| candidates().find(|r| r.match_model.as_deref() == Some(m)))
        .or_else(|| candidates().find(|r| r.match_model.is_none()))
        .and_then(|r| r.to_account)
}

/// The credential an explicit redirect rule names for the session behind
/// `session_token`, or `None` when failover is off, on cooldown, no unexpired
/// rule moves the exhausted account, or the rule's target has no credential
/// in the family — the caller then mirrors / refuses as before.
///
/// Never picks by headroom: a session goes only where its user configured a
/// redirect, exactly as a launch would. The rule's target is applied
/// regardless of its own usage, mirroring launch.
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
    let bound: Option<(String, Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT t.session_id, ap.user_id, ap.account_id, ap.family \
         FROM session_tokens t JOIN account_providers ap ON ap.id = t.account_id \
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (session_id, user_id, from_account, family) = bound?;
    if cooldown_active(&RECENT_FAILOVERS, &session_id, Instant::now(), FAILOVER_COOLDOWN) {
        return None;
    }

    let rules = crate::store::account_redirects::live_for_account(
        &state.pool,
        user_id,
        from_account,
        &family,
    )
    .await
    .ok()?;
    let to_account = explicit_target(&rules, from_account, &family, model)?;

    let (provider_id, account_name): (Uuid, String) = sqlx::query_as(
        "SELECT ap.id, a.name \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.account_id = $1 AND ap.family = $2 AND ap.id != $3",
    )
    .bind(to_account)
    .bind(&family)
    .bind(exclude_provider)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()?;
    Some(FailoverTarget { session_id, provider_id, account_name })
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
            "gateway failover: rebound session to its explicit redirect target"
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
        "cctui gateway: the bound account is out of allocation — session moved to \
         account '{account_name}' per its configured redirect. Retry now; the request \
         will be served by that account."
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn rule(
        from: Uuid,
        to: Option<Uuid>,
        family: &str,
        matches: Option<&str>,
        to_model: Option<&str>,
    ) -> AccountRedirect {
        AccountRedirect {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            from_account: from,
            to_account: to,
            family: family.to_owned(),
            match_model: matches.map(str::to_owned),
            to_model: to_model.map(str::to_owned),
            expires_at: None,
            reason: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn flag_only_enables_on_explicit_on_values() {
        for on in ["1", "true", "on", "yes", " True ", "ON"] {
            assert!(flag_enables(on), "{on:?} must enable");
        }
        for off in ["0", "false", "off", "no", "", "anything"] {
            assert!(!flag_enables(off), "{off:?} must not enable");
        }
    }

    #[test]
    fn disabled_by_default_even_with_a_sibling() {
        // The env is unset in tests: the process-wide gate must read as off.
        assert!(!failover_enabled(), "failover must be opt-in");
    }

    #[test]
    fn no_rule_means_no_target() {
        let a = Uuid::new_v4();
        assert_eq!(explicit_target(&[], a, "anthropic", Some("opus")), None);
        assert_eq!(explicit_target(&[], a, "anthropic", None), None);
    }

    #[test]
    fn explicit_rule_wins_over_any_richer_sibling() {
        let (a, x, y) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        // Y is a sibling with plenty of headroom but no rule points at it: it
        // is never a candidate. Only the configured target X is.
        let rules = [rule(a, Some(x), "anthropic", None, None)];
        assert_eq!(explicit_target(&rules, a, "anthropic", Some("opus")), Some(x));
        assert_eq!(explicit_target(&rules, a, "anthropic", None), Some(x));
        assert_ne!(explicit_target(&rules, a, "anthropic", None), Some(y));
    }

    #[test]
    fn exact_model_rule_beats_catch_all() {
        let (a, x, z) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rules = [
            rule(a, Some(x), "anthropic", None, None),
            rule(a, Some(z), "anthropic", Some("fable"), None),
        ];
        assert_eq!(explicit_target(&rules, a, "anthropic", Some("fable")), Some(z));
        assert_eq!(explicit_target(&rules, a, "anthropic", Some("opus")), Some(x));
    }

    #[test]
    fn unknown_model_only_matches_a_catch_all() {
        let (a, z) = (Uuid::new_v4(), Uuid::new_v4());
        let rules = [rule(a, Some(z), "anthropic", Some("fable"), None)];
        assert_eq!(explicit_target(&rules, a, "anthropic", None), None);
        assert_eq!(explicit_target(&rules, a, "anthropic", Some("opus")), None);
    }

    #[test]
    fn family_and_source_are_scoped() {
        let (a, x) = (Uuid::new_v4(), Uuid::new_v4());
        let rules = [rule(a, Some(x), "anthropic", None, None)];
        assert_eq!(explicit_target(&rules, a, "openai", None), None);
        assert_eq!(explicit_target(&rules, Uuid::new_v4(), "anthropic", None), None);
    }

    #[test]
    fn model_flip_rules_never_move_a_session() {
        let a = Uuid::new_v4();
        let rules = [rule(a, None, "anthropic", None, Some("sonnet"))];
        assert_eq!(explicit_target(&rules, a, "anthropic", Some("opus")), None);
    }

    #[test]
    fn cooldown_spaces_rebinds_and_expires() {
        let map = dashmap::DashMap::new();
        let t0 = Instant::now();
        assert!(!cooldown_active(&map, "s1", t0, Duration::from_mins(1)));
        note_failover(&map, "s1", t0);
        assert!(cooldown_active(&map, "s1", t0 + Duration::from_secs(59), Duration::from_mins(1)));
        assert!(!cooldown_active(&map, "s1", t0 + Duration::from_secs(61), Duration::from_mins(1)));
        assert!(!cooldown_active(&map, "s2", t0, Duration::from_mins(1)));
    }
}
