use super::{Account, session_id_for_token};

use uuid::Uuid;

use crate::state::AppState;

/// Merge a `CctuiAgent` child's per-session dollar budget into `cap` as a
/// `session_usd` limit. A budget on the child always wins over an account-level
/// `session_usd`: it is the tighter, purpose-set ceiling.
pub fn merge_session_budget(
    cap: &crate::soft_limit::SoftLimits,
    budget_usd: Option<f64>,
) -> crate::soft_limit::SoftLimits {
    let Some(budget) = budget_usd.filter(|b| b.is_finite() && *b > 0.0) else {
        return cap.clone();
    };
    let mut merged = cap.clone();
    let entry = merged.limits.entry(crate::soft_limit::KEY_SESSION_USD.to_owned()).or_default();
    entry.cap_usd = Some(budget);
    merged
}

/// The account's soft limits with any per-session `CctuiAgent` budget applied.
/// Skips the token→session lookup entirely while no child budget is live.
pub async fn session_budget_limits(
    state: &AppState,
    acct: &Account,
    session_token: &str,
) -> crate::soft_limit::SoftLimits {
    if state.session_usd_budgets.is_empty() {
        return acct.soft_limits.clone();
    }
    let Some(session_id) = session_id_for_token(state, session_token).await else {
        return acct.soft_limits.clone();
    };
    let budget = state.session_usd_budgets.get(&session_id).map(|b| *b);
    merge_session_budget(&acct.soft_limits, budget)
}

/// Resolve a session token to its `(session_id, account_name)` — used by the
/// soft-limit signalling path to tag the per-session WS event with the
/// human account name (the `Account` struct carries no name). `None` for
/// unknown/revoked tokens.
pub async fn session_and_account_name_for_token(
    state: &AppState,
    session_token: &str,
) -> Option<(String, String)> {
    let hash = crate::auth::sha256_hex(session_token);
    sqlx::query_as::<_, (String, String)>(
        "SELECT t.session_id, a.name \
         FROM session_tokens t \
         JOIN account_providers ap ON ap.id = t.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Record a soft-limit block against a session and broadcast it.
///
/// Idempotent per block episode: the first refused passthrough for a session
/// flips `soft_limit_blocked` and emits [`ServerEvent::SoftLimitReached`]; the
/// worker's repeated Retry-After retries (still blocked) are no-ops, so the WS
/// stream isn't spammed. The webui shows the banner; the matching clear arrives
/// from [`clear_soft_limit_block`] on the next success or an account switch.
pub async fn mark_soft_limit_block(
    state: &AppState,
    session_id: &str,
    account_id: Uuid,
    account_name: &str,
    reason: &str,
    retry_after_secs: i64,
) {
    if session_id.is_empty() {
        return;
    }
    // Persist a durable block on the session row so the classifier drives the
    // session to `Bucket::Blocked` (✋ needs input) and the block survives a
    // resubscribe. The stored reason is an actionable "continue on
    // another account" hint; `list_sessions` reads it. Idempotent (overwrite),
    // and never clobbers the churning daemon `tempo`/`agent_state` signals.
    let needs = format!("switch account: {account_name} rate-limited");
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET soft_limit_reason = $2 WHERE id = $1 AND status != 'archived'",
    )
    .bind(session_id)
    .bind(&needs)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "failed to persist soft-limit block");
    }
    // Only broadcast on the clear→blocked transition.
    if state.soft_limit_blocked.insert(session_id.to_owned(), ()).is_none() {
        state.bus.publish_server(cctui_proto::ws::ServerEvent::SoftLimitReached {
            session_id: session_id.to_owned(),
            account_id,
            account_name: account_name.to_owned(),
            reason: reason.to_owned(),
            retry_after_secs,
        });
    }
}

/// Clear a session's soft-limit block and broadcast the dismissal.
/// Only emits on the blocked→clear transition (no-op if it wasn't blocked).
pub async fn clear_soft_limit_block(state: &AppState, session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    // Drop the durable block on the session row so the classifier stops forcing
    // `Bucket::Blocked` and the session returns to its real signal-derived
    // bucket. Best-effort; clear it whenever set, even if the
    // in-memory dedup entry was already gone (e.g. after a server restart).
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET soft_limit_reason = NULL \
         WHERE id = $1 AND soft_limit_reason IS NOT NULL",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "failed to clear soft-limit block");
    }
    if state.soft_limit_blocked.remove(session_id).is_some() {
        state.bus.publish_server(cctui_proto::ws::ServerEvent::SoftLimitCleared {
            session_id: session_id.into(),
        });
    }
}

/// Record that a session token was just presented at the gateway, so the UI
/// can distinguish an account-bound session whose worker actually routes here
/// from one silently riding ambient creds. Fire-and-forget + self-throttling
/// (skips a write when stamped within the last minute) to stay off the
/// passthrough hot path. `token_fp` is the sha256 hex == `session_tokens.token_hash`.
pub fn note_token_used(state: &AppState, token_fp: &str) {
    let pool = state.pool.clone();
    let hash = token_fp.to_owned();
    tokio::spawn(async move {
        let _ = crate::store::tokens::stamp_last_used(&pool, &hash).await;
    });
}

/// Flag an account as needing reauthentication: the upstream provider
/// rejected its OAuth credentials. Persists `needs_reauth` + the error so the
/// accounts UI can show a "credential rejected — reauthenticate" badge. Gated on
/// the in-memory set so a flapping worker doesn't re-write the row on every 401 —
/// the DB write fires only on the false→true transition.
pub fn flag_account_reauth(state: &AppState, account_id: Uuid, reason: &str) {
    if state.account_reauth.insert(account_id, ()).is_some() {
        return; // already flagged in memory — no redundant write
    }
    let pool = state.pool.clone();
    let reason = reason.to_string();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = true, last_auth_error = $2, last_auth_error_at = now() \
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(reason)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to flag account reauth");
        }
    });
}

/// Clear an account's reauth flag after a successful upstream call.
/// Gated on the in-memory set so the common case (account healthy) costs nothing;
/// the DB write fires only on the true→false transition.
pub fn clear_account_reauth(state: &AppState, account_id: Uuid) {
    if state.account_reauth.remove(&account_id).is_none() {
        return; // not flagged — nothing to clear
    }
    let pool = state.pool.clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = false, last_auth_error = NULL, last_auth_error_at = NULL \
             WHERE id = $1 AND needs_reauth",
        )
        .bind(account_id)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to clear account reauth");
        }
    });
}

/// Resolve the session token (the upstream bearer the worker sent) to its
/// account. Returns `None` for unknown/revoked tokens.
/// Env-tunable thresholds for the orphan-token spam guard. Parsed once.
pub struct OrphanSpamCfg {
    /// Unresolved 401s within `window` before a fingerprint is blocked.
    threshold: u32,
    /// Counting window.
    window: std::time::Duration,
    /// How long a flagged fingerprint stays blocked (DB lookups skipped).
    block: std::time::Duration,
}

pub static ORPHAN_SPAM_CFG: std::sync::LazyLock<OrphanSpamCfg> = std::sync::LazyLock::new(|| {
    fn env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    OrphanSpamCfg {
        threshold: u32::try_from(env_u64("CCTUI_GATEWAY_SPAM_THRESHOLD", 10)).unwrap_or(10),
        window: std::time::Duration::from_secs(env_u64("CCTUI_GATEWAY_SPAM_WINDOW_SECS", 60)),
        block: std::time::Duration::from_secs(env_u64("CCTUI_GATEWAY_SPAM_BLOCK_SECS", 300)),
    }
});

pub type OrphanSpamMap = dashmap::DashMap<String, crate::state::OrphanSpam>;

/// True if this token fingerprint is currently blocked as a spamming orphan.
/// Pure in-memory check — no DB — so blocked orphans cost ~nothing.
pub fn orphan_is_blocked(state: &AppState, token_fp: &str) -> bool {
    orphan_is_blocked_at(&state.gateway_orphan_spam, token_fp, std::time::Instant::now())
}

pub fn orphan_is_blocked_at(map: &OrphanSpamMap, token_fp: &str, now: std::time::Instant) -> bool {
    let Some(entry) = map.get(token_fp) else { return false };
    matches!(entry.blocked_until, Some(until) if until > now)
}

/// Drop a token fingerprint from the in-memory orphan-spam state.
///
/// Called after a successful rebind/mint that reuses an existing token string:
/// the fingerprint may have been blocked while the binding was broken (an
/// unresolvable token 401s its way past the threshold), and since a rebind
/// repoints the SAME token string, the block would otherwise keep dropping a
/// NOW-VALID token's requests for the remainder of the block window (up to
/// 300s). Clearing re-enables the DB lookup immediately. Idempotent.
pub fn clear_orphan_fingerprint(map: &OrphanSpamMap, token_fp: &str) {
    map.remove(token_fp);
}

/// Clear the orphan-spam block for every live token of `session_id`.
///
/// The explicit account-switch path (`sessions::switch_account`) rebinds token
/// rows by session id without the token plaintext in hand;
/// `session_tokens.token_hash` IS the fingerprint the spam guard keys on (both
/// are the sha256 hex of the token string), so clearing by stored hash needs no
/// token material. Best-effort: a failed lookup just leaves the block to
/// expire on its own.
pub async fn clear_orphan_block_for_session(state: &AppState, session_id: &str) {
    let hashes: Vec<String> =
        crate::store::tokens::token_hashes_by_session(&state.pool, session_id)
            .await
            .unwrap_or_default();
    for hash in &hashes {
        clear_orphan_fingerprint(&state.gateway_orphan_spam, hash);
    }
}

/// Record an unresolvable-token 401 and, once a fingerprint crosses the spam
/// threshold within the window, flag it as a blocked orphan and log LOUDLY.
pub fn note_orphan_401(state: &AppState, token_fp: &str) {
    let cfg = &*ORPHAN_SPAM_CFG;
    let fp_short: String = token_fp.chars().take(12).collect();
    let (count, newly_blocked) = bump_orphan_401(
        &state.gateway_orphan_spam,
        token_fp,
        std::time::Instant::now(),
        cfg.threshold,
        cfg.window,
        cfg.block,
    );

    if newly_blocked {
        tracing::error!(
            stage = "session-token",
            token_fp = %fp_short,
            count,
            block_secs = cfg.block.as_secs(),
            "🔴 GATEWAY ORPHAN SPAM: unresolvable session token exceeded {} 401s in {}s — \
             blocking fingerprint for {}s; subsequent requests dropped before any DB lookup. \
             A zombie worker lost its session→account binding; resume or kill it.",
            cfg.threshold,
            cfg.window.as_secs(),
            cfg.block.as_secs(),
        );
    } else {
        tracing::warn!(
            stage = "session-token",
            token_fp = %fp_short,
            count,
            "gateway 401: session token not resolvable (orphan worker retrying)"
        );
    }
}

/// Pure sliding-window counter. Returns `(count_in_window, newly_blocked)` where
/// `newly_blocked` is true only on the transition that flags the fingerprint.
pub fn bump_orphan_401(
    map: &OrphanSpamMap,
    token_fp: &str,
    now: std::time::Instant,
    threshold: u32,
    window: std::time::Duration,
    block: std::time::Duration,
) -> (u32, bool) {
    let mut entry = map.entry(token_fp.to_string()).or_insert_with(|| crate::state::OrphanSpam {
        count: 0,
        window_start: now,
        blocked_until: None,
    });

    // Roll the window over once it elapses (also clears an expired block).
    if now.duration_since(entry.window_start) > window {
        entry.count = 0;
        entry.window_start = now;
        entry.blocked_until = None;
    }
    entry.count += 1;
    let count = entry.count;

    let newly_blocked = count >= threshold && entry.blocked_until.is_none();
    if newly_blocked {
        entry.blocked_until = Some(now + block);
    }
    drop(entry);
    (count, newly_blocked)
}
