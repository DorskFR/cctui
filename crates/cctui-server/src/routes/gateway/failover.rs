//! Gateway account failover: move a session whose bound account is out of
//! allocation to another account it may run on.
//!
//! Only two sources elect a target: the session's pool (with `failover` armed,
//! elected by the pool's strategy) or an explicit `account_redirects` rule
//! under `CCTUI_GATEWAY_FAILOVER=1`. A session with neither stays put and sees
//! the refusal. Every move is recorded in `session_account_rebinds`.
//!
//! Under `CCTUI_GATEWAY_OUTAGE_FAILOVER=1` a pool election also reads the
//! provider-status cache: a member whose native family reports a major or
//! critical incident ranks behind every healthy sibling, and a move the
//! incident steered is recorded as such rather than as the pool's own doing.
//!
//! Request bodies stream unbuffered, so the refused request is not replayed:
//! the session's token row is repointed and the gateway answers 429
//! `Retry-After: 1`, which every supported harness retries. Callers are the
//! soft-limit gate and an upstream 429 in `passthrough`; a per-session cooldown
//! keeps a burst-429 from ping-ponging a session between accounts.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::state::AppState;
use crate::store::account_pools::AccountPool;
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

/// Opt-in: `CCTUI_GATEWAY_OUTAGE_FAILOVER=1|true|on|yes`. Off, the
/// provider-status cache is never consulted and a pool ranks as it always has.
fn outage_failover_enabled() -> bool {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        std::env::var("CCTUI_GATEWAY_OUTAGE_FAILOVER").is_ok_and(|v| flag_enables(&v))
    });
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

/// Why a session was moved. Recorded verbatim on the audit row and used to
/// word the retry the worker sees, so "my pool balanced this" is never
/// confused with "a rule I wrote last Tuesday moved this".
pub const REASON_POOL: &str = "pool";
pub const REASON_REDIRECT: &str = "redirect";
/// Prefix of the reason recorded when an upstream incident steered the pool's
/// pick away from a member it would otherwise have elected; the family whose
/// status page reported it follows the colon.
pub const REASON_OUTAGE_PREFIX: &str = "outage:";

/// The reason recorded for a move steered around an incident in `family`.
#[must_use]
pub fn outage_reason(family: &str) -> String {
    format!("{REASON_OUTAGE_PREFIX}{family}")
}

/// The family an incident-steered reason names, if the reason is one.
#[must_use]
pub fn outage_family(reason: &str) -> Option<&str> {
    reason.strip_prefix(REASON_OUTAGE_PREFIX).filter(|f| !f.is_empty())
}

/// The upstream incident a pool member is exposed to, when it is one routing
/// should steer around. A member served by a compatible endpoint (`base_url`
/// set) is not covered by the vendor's status page and reads as healthy.
#[must_use]
pub fn member_outage(
    cache: &crate::provider_status::ProviderStatusCache,
    provider: &str,
    base_url: Option<&str>,
) -> Option<&'static str> {
    if base_url.is_some_and(|u| !u.trim().is_empty()) {
        return None;
    }
    let family = crate::provider_status::family_of_provider(provider)?;
    cache.routing_outage(family).map(|_| family)
}

/// The credential a failing session should rebind to.
pub struct FailoverTarget {
    pub session_id: String,
    pub provider_id: Uuid,
    pub account_name: String,
    /// The account being left, for the audit row.
    pub from_account_name: String,
    /// The pool that authorised the move, when one did.
    pub pool_id: Option<Uuid>,
    /// [`REASON_POOL`], [`REASON_REDIRECT`], or an [`outage_reason`].
    pub reason: String,
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

/// `(session, account owner, session user, account, family, account name, pool)`.
type BoundToken = (String, Uuid, Option<Uuid>, Uuid, String, String, Option<Uuid>);

/// The credential the session behind `session_token` may move to, or `None`
/// when nothing authorises a move — the caller then mirrors / refuses as
/// before.
///
/// Order of authority: the session's own pool first (the standing policy),
/// then an explicit redirect rule (the dated override). Never an implicit
/// election over everything the user can reach.
pub async fn pick_failover_target(
    state: &AppState,
    session_token: &str,
    exclude_provider: Uuid,
    model: Option<&str>,
) -> Option<FailoverTarget> {
    let hash = crate::auth::sha256_hex(session_token);
    // `user_id` is the bound account's OWNER — whose pool and whose redirect
    // rules authorise a move. `session_user` is who the session belongs to, and
    // every elected target is re-checked against them: on a shared account the
    // owner's pool holds accounts the grantee was never given.
    let bound: Option<BoundToken> = sqlx::query_as(
        "SELECT t.session_id, ap.user_id, COALESCE(s.user_id, t.user_id), \
                    ap.account_id, ap.family, a.name, t.pool_id \
             FROM session_tokens t \
             JOIN account_providers ap ON ap.id = t.account_id \
             JOIN accounts a ON a.id = ap.account_id \
             LEFT JOIN sessions s ON s.id = t.session_id \
             WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (session_id, user_id, session_user, from_account, family, from_account_name, pool_id) =
        bound?;
    let session_user = session_user.unwrap_or(user_id);
    if cooldown_active(&RECENT_FAILOVERS, &session_id, Instant::now(), FAILOVER_COOLDOWN) {
        return None;
    }

    // 1. The pool the session was launched into, when its owner armed failover.
    if let Some(pool_id) = pool_id
        && let Ok(Some(pool)) = crate::store::account_pools::get(&state.pool, pool_id, None).await
        && in_pool_failover_armed(Some(&pool))
        && let Some(target) = pick_within_pool(
            state,
            &pool,
            user_id,
            session_user,
            &family,
            exclude_provider,
            model,
            &session_id,
            &from_account_name,
        )
        .await
    {
        return Some(target);
    }

    // 2. The explicit, opt-in redirect path — unchanged, and still gated on the
    // env flag so an operator who wants no mid-session movement at all keeps
    // getting none from this direction.
    if !failover_enabled() {
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
         WHERE ap.account_id = $1 AND ap.family = $2 AND ap.id != $3 \
           AND (a.user_id = $4 OR EXISTS ( \
               SELECT 1 FROM resource_shares rs \
                WHERE rs.resource_type = 'account' AND rs.resource_id = a.id \
                  AND rs.grantee_id = $4 AND rs.revoked_at IS NULL))",
    )
    .bind(to_account)
    .bind(&family)
    .bind(exclude_provider)
    .bind(session_user)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()?;
    Some(FailoverTarget {
        session_id,
        provider_id,
        account_name,
        from_account_name,
        pool_id: None,
        reason: REASON_REDIRECT.to_owned(),
    })
}

/// Elect a member of `pool` for a session whose bound credential just refused.
///
/// The pool's own strategy decides (`headroom` ranks, `ordered` walks the
/// ladder), over the members that are still usable *and* still measurable as
/// having room. Unlike a launch, a member with no readable usage is not
/// elected here: at launch an unreadable account is a degraded guess with
/// nothing at stake, whereas here the current account is already refusing and
/// moving to another unknown would burn the cooldown for nothing.
///
/// The excluded credential is the one that just failed — never a candidate for
/// its own replacement.
#[allow(clippy::too_many_arguments)]
async fn pick_within_pool(
    state: &AppState,
    pool: &AccountPool,
    user_id: Uuid,
    session_user: Uuid,
    family: &str,
    exclude_provider: Uuid,
    model: Option<&str>,
    session_id: &str,
    from_account_name: &str,
) -> Option<FailoverTarget> {
    let members =
        crate::store::account_pools::usable_members(&state.pool, pool.id, user_id, family)
            .await
            .ok()?;
    // A sibling whose catalog does not list the request's model would only
    // trade a 429 for a 404.
    let fam = super::Family::from_label(family)?;
    let mut members: Vec<_> = members
        .into_iter()
        .filter(|m| m.provider_id != exclude_provider)
        .filter(|m| {
            crate::account_pick::serves_model(
                fam,
                m.models.as_ref(),
                m.model_aliases.as_ref(),
                model,
            )
        })
        .collect();
    if session_user != user_id {
        let usable = futures_util::future::join_all(
            members.iter().map(|m| super::provider_usable_by(state, m.provider_id, session_user)),
        )
        .await;
        members = members.into_iter().zip(usable).filter(|(_, ok)| *ok).map(|(m, _)| m).collect();
    }
    if members.is_empty() {
        return None;
    }

    let usages = futures_util::future::join_all(
        members.iter().map(|m| super::usage_for_soft_limit(state, m.provider_id)),
    )
    .await;
    // A 429 burst moves many sessions off one account at once; counting the
    // ones already moved keeps them from all landing on the same sibling.
    let providers: Vec<Uuid> = members.iter().map(|m| m.provider_id).collect();
    let in_flight = crate::account_resolve::in_flight_by_provider(state, &providers).await;
    let outages: Vec<Option<&'static str>> = members
        .iter()
        .map(|m| {
            outage_failover_enabled()
                .then(|| member_outage(&state.provider_status, &m.provider, m.base_url.as_deref()))
                .flatten()
        })
        .collect();
    let candidates: Vec<crate::account_pick::Candidate> = members
        .iter()
        .zip(usages.iter())
        .map(|(m, usage)| crate::account_pick::Candidate {
            name: m.name.clone(),
            windows: usage
                .as_ref()
                .map(crate::soft_limit::normalize_usage_windows)
                .unwrap_or_default(),
            limits: crate::soft_limit::SoftLimits::from_json(m.soft_limits_json.as_ref()),
            usage_known: usage.is_some(),
            in_flight: in_flight.get(&m.provider_id).copied().unwrap_or(0),
        })
        .collect();

    elect_replacement(
        pool,
        &candidates,
        &providers,
        &outages,
        model,
        chrono::Utc::now(),
        session_id,
        from_account_name,
    )
}

/// Whether a session's stamped pool actually authorises an in-pool move. A
/// session whose token carries no `pool_id` — every dispatched session before
/// the dispatch path started stamping one — can never reach this path.
fn in_pool_failover_armed(pool: Option<&AccountPool>) -> bool {
    pool.is_some_and(|p| p.failover)
}

/// The election itself, over `candidates` paired positionally with
/// `providers` and `outages`. Split from the DB/usage fetch so the rule that
/// decides where a refused session lands is testable without a gateway.
///
/// Members under an upstream incident form a second tier: the healthy ones are
/// elected first, and the degraded ones only when no healthy member has room.
/// With no outage reported (or the flag off) the two tiers are one, and the
/// result is exactly the plain election. A pick the tiering changed is
/// attributed to the incident, never to the pool.
#[allow(clippy::too_many_arguments)]
fn elect_replacement(
    pool: &AccountPool,
    candidates: &[crate::account_pick::Candidate],
    providers: &[Uuid],
    outages: &[Option<&'static str>],
    model: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    session_id: &str,
    from_account_name: &str,
) -> Option<FailoverTarget> {
    let all = vec![true; candidates.len()];
    let plain = elect_among(pool, candidates, &all, model, now)?;
    let healthy: Vec<bool> = candidates
        .iter()
        .enumerate()
        .map(|(i, _)| outages.get(i).copied().flatten().is_none())
        .collect();
    let (idx, reason) = if healthy.iter().all(|h| *h) {
        (plain, REASON_POOL.to_owned())
    } else {
        match elect_among(pool, candidates, &healthy, model, now) {
            Some(idx) if idx != plain => {
                let family = outages.get(plain).copied().flatten().unwrap_or("upstream");
                (idx, outage_reason(family))
            }
            Some(idx) => (idx, REASON_POOL.to_owned()),
            None => (plain, REASON_POOL.to_owned()),
        }
    };
    Some(FailoverTarget {
        session_id: session_id.to_owned(),
        provider_id: *providers.get(idx)?,
        account_name: candidates.get(idx)?.name.clone(),
        from_account_name: from_account_name.to_owned(),
        pool_id: Some(pool.id),
        reason,
    })
}

/// Run the pool's strategy over the candidates `keep` admits and return the
/// winner's index into `candidates`, or `None` when no admitted member has
/// measured room.
fn elect_among(
    pool: &AccountPool,
    candidates: &[crate::account_pick::Candidate],
    keep: &[bool],
    model: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<usize> {
    let admitted: Vec<usize> =
        (0..candidates.len()).filter(|i| keep.get(*i).copied().unwrap_or(false)).collect();
    let subset: Vec<crate::account_pick::Candidate> =
        admitted.iter().map(|i| candidates[*i].clone()).collect();
    let pick = if pool.strategy == crate::store::account_pools::STRATEGY_ORDERED {
        crate::account_pick::pick_in_order(&subset, model, now)
    } else {
        crate::account_pick::pick_account(&subset, model, now)
    };
    let crate::account_pick::Pick::Chosen { name, .. } = pick else { return None };
    // Measured room only: see the note above on why an unreadable member is
    // not a failover target even though it is a launch candidate.
    admitted.into_iter().find(|i| candidates[*i].name == name && candidates[*i].usage_known)
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
        // The audit row is the whole reason a user can trust this feature:
        // the session says, afterwards, that it moved and why. Best-effort —
        // the move already happened, and losing the record must not turn a
        // successful failover into a failed request.
        if let Err(e) = crate::store::account_pools::record_rebind(
            &state.pool,
            &target.session_id,
            target.pool_id,
            &target.from_account_name,
            &target.account_name,
            &target.reason,
        )
        .await
        {
            tracing::warn!(session_id = %target.session_id, error = %e,
                "could not record the session rebind");
        }
        tracing::warn!(
            session_id = %target.session_id,
            from = %from_provider,
            to = %target.provider_id,
            account = %target.account_name,
            reason = %target.reason,
            "gateway failover: rebound session"
        );
    }
    // 0 rows = a concurrent failover won the race; the retry still lands on
    // the fresh binding, so the caller should answer retry-shortly either way.
    true
}

/// The response that sends the worker back around: 429 with an immediate
/// `Retry-After`, in the provider's native error envelope so the CLI renders
/// the message. The harness's own 429 backoff performs the "replay".
pub fn failover_retry_response(
    account_name: &str,
    reason: &str,
    is_anthropic: bool,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Name the mechanism: the difference between "my pool did its job" and
    // "a rule I forgot about moved this" is the difference between a feature
    // and a surprise.
    let because = if reason == REASON_POOL {
        "the next account in its pool".to_owned()
    } else if let Some(family) = outage_family(reason) {
        format!("a pool member clear of the {family} incident its status page reports")
    } else {
        "its configured redirect target".to_owned()
    };
    let message = format!(
        "cctui gateway: the bound account is out of allocation — session moved to \
         account '{account_name}' ({because}). Retry now; the request will be served \
         by that account."
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
        .header("x-cctui-failover-reason", reason)
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
        assert!(!outage_failover_enabled(), "outage-aware ranking must be opt-in");
    }

    #[test]
    fn outage_reasons_round_trip_and_never_collide_with_the_fixed_ones() {
        assert_eq!(outage_reason("anthropic"), "outage:anthropic");
        assert_eq!(outage_family("outage:anthropic"), Some("anthropic"));
        assert_eq!(outage_family("outage:"), None);
        assert_eq!(outage_family(REASON_POOL), None);
        assert_eq!(outage_family(REASON_REDIRECT), None);
    }

    #[tokio::test]
    async fn the_retry_names_the_incident_the_session_was_steered_around() {
        let resp = failover_retry_response("Secours", &outage_reason("openai"), false);
        assert_eq!(resp.headers()["x-cctui-failover-reason"], "outage:openai");
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("openai incident"), "{text}");
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

    fn pool(strategy: &str, failover: bool) -> AccountPool {
        AccountPool {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "work".into(),
            strategy: strategy.to_owned(),
            failover,
            created_at: Utc::now(),
        }
    }

    fn member(name: &str, five_hour_pct: f64, usage_known: bool) -> crate::account_pick::Candidate {
        crate::account_pick::Candidate {
            name: name.to_owned(),
            windows: if usage_known {
                vec![crate::soft_limit::UsageWindow {
                    key: crate::soft_limit::KEY_SESSION.to_owned(),
                    kind: "session".to_owned(),
                    label: "5h".to_owned(),
                    utilization: five_hour_pct,
                    amount_usd: None,
                    resets_at: Some(Utc::now() + chrono::Duration::hours(3)),
                    model_id: None,
                    model_display_name: None,
                }]
            } else {
                Vec::new()
            },
            limits: crate::soft_limit::SoftLimits::default(),
            usage_known,
            in_flight: 0,
        }
    }

    #[test]
    fn an_unstamped_session_is_not_eligible_for_an_in_pool_move() {
        // A null `session_tokens.pool_id` makes `pool.failover` inert.
        assert!(!in_pool_failover_armed(None));
        assert!(!in_pool_failover_armed(Some(&pool(
            crate::store::account_pools::STRATEGY_HEADROOM,
            false
        ))));
        assert!(in_pool_failover_armed(Some(&pool(
            crate::store::account_pools::STRATEGY_HEADROOM,
            true
        ))));
    }

    const NO_OUTAGE: [Option<&str>; 2] = [None, None];

    #[test]
    fn a_refused_session_moves_to_the_member_with_room_and_records_the_pool() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let spare = Uuid::new_v4();
        let candidates = [member("alpha", 100.0, true), member("beta", 9.0, true)];
        let providers = [Uuid::new_v4(), spare];
        let target = elect_replacement(
            &p,
            &candidates,
            &providers,
            &NO_OUTAGE,
            None,
            Utc::now(),
            "sess-1",
            "alpha",
        )
        .expect("a member with room");
        assert_eq!(target.account_name, "beta");
        assert_eq!(target.provider_id, spare);
        assert_eq!(target.from_account_name, "alpha");
        // `rebind_session` writes exactly these two into `session_account_rebinds`,
        // so the move is attributable to the pool afterwards.
        assert_eq!(target.pool_id, Some(p.id));
        assert_eq!(target.reason, REASON_POOL);
    }

    #[test]
    fn an_unmeasurable_member_is_not_a_failover_target() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("beta", 0.0, false)];
        let providers = [Uuid::new_v4()];
        assert!(
            elect_replacement(
                &p,
                &candidates,
                &providers,
                &[None],
                None,
                Utc::now(),
                "sess-1",
                "alpha"
            )
            .is_none()
        );
    }

    #[test]
    fn every_remaining_member_out_leaves_the_session_put() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("alpha", 100.0, true), member("beta", 100.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        assert!(
            elect_replacement(
                &p,
                &candidates,
                &providers,
                &NO_OUTAGE,
                None,
                Utc::now(),
                "sess-1",
                "alpha"
            )
            .is_none()
        );
    }

    /// A cache carrying the fixture readings: anthropic operational, openai in
    /// a major incident.
    fn status_cache() -> crate::provider_status::ProviderStatusCache {
        let cache = crate::provider_status::ProviderStatusCache::default();
        let parse = |raw: &str| serde_json::from_str::<serde_json::Value>(raw).unwrap();
        cache.seed(crate::provider_status::normalize(
            "anthropic",
            &parse(include_str!("../../fixtures/provider_status/anthropic_none.json")),
        ));
        cache.seed(crate::provider_status::normalize(
            "openai",
            &parse(include_str!("../../fixtures/provider_status/openai_major.json")),
        ));
        cache
    }

    #[test]
    fn a_member_is_degraded_only_through_its_native_familys_incident() {
        let cache = status_cache();
        assert_eq!(member_outage(&cache, "openai", None), Some("openai"));
        assert_eq!(member_outage(&cache, "anthropic", None), None, "operational");
        assert_eq!(member_outage(&cache, "fireworks", None), None, "no status page");
        assert_eq!(
            member_outage(&cache, "openai", Some("https://compat.example/v1")),
            None,
            "a compatible endpoint is not the vendor's hosted API"
        );
        assert_eq!(member_outage(&cache, "openai", Some("  ")), Some("openai"));
    }

    fn elect(
        p: &AccountPool,
        candidates: &[crate::account_pick::Candidate],
        providers: &[Uuid],
        outages: &[Option<&'static str>],
    ) -> Option<FailoverTarget> {
        elect_replacement(p, candidates, providers, outages, None, Utc::now(), "sess-1", "src")
    }

    #[test]
    fn a_degraded_member_ranks_behind_a_healthy_one_with_less_room_and_says_why() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("roomy", 5.0, true), member("tight", 60.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let plain = elect(&p, &candidates, &providers, &NO_OUTAGE).unwrap();
        assert_eq!(plain.account_name, "roomy");
        assert_eq!(plain.reason, REASON_POOL);

        let steered = elect(&p, &candidates, &providers, &[Some("openai"), None]).unwrap();
        assert_eq!(steered.account_name, "tight");
        assert_eq!(steered.provider_id, providers[1]);
        assert_eq!(steered.reason, "outage:openai");
        assert_eq!(steered.pool_id, Some(p.id));
    }

    #[test]
    fn an_incident_that_did_not_change_the_pick_is_still_the_pools_doing() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("roomy", 5.0, true), member("tight", 60.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let t = elect(&p, &candidates, &providers, &[None, Some("openai")]).unwrap();
        assert_eq!(t.account_name, "roomy");
        assert_eq!(t.reason, REASON_POOL);
    }

    #[test]
    fn every_healthy_member_out_falls_back_to_the_degraded_tier_as_before() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("down", 10.0, true), member("spent", 100.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let t = elect(&p, &candidates, &providers, &[Some("anthropic"), None]).unwrap();
        assert_eq!(t.account_name, "down");
        assert_eq!(t.reason, REASON_POOL, "the plain election would have landed here too");
    }

    #[test]
    fn a_whole_pool_under_one_incident_ranks_exactly_as_without_it() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("a", 50.0, true), member("b", 20.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let plain = elect(&p, &candidates, &providers, &NO_OUTAGE).unwrap();
        let all_down = elect(&p, &candidates, &providers, &[Some("anthropic"); 2]).unwrap();
        assert_eq!(all_down.account_name, plain.account_name);
        assert_eq!(all_down.provider_id, plain.provider_id);
        assert_eq!(all_down.reason, REASON_POOL);
    }

    #[test]
    fn an_ordered_pool_skips_a_degraded_rung_and_records_the_incident() {
        let p = pool(crate::store::account_pools::STRATEGY_ORDERED, true);
        let candidates = [member("first", 90.0, true), member("second", 10.0, true)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let plain = elect(&p, &candidates, &providers, &NO_OUTAGE).unwrap();
        assert_eq!(plain.account_name, "first", "position beats margin");
        let steered = elect(&p, &candidates, &providers, &[Some("openai"), None]).unwrap();
        assert_eq!(steered.account_name, "second");
        assert_eq!(steered.reason, "outage:openai");
    }

    #[test]
    fn the_steered_target_still_needs_measured_room() {
        let p = pool(crate::store::account_pools::STRATEGY_HEADROOM, true);
        let candidates = [member("down", 10.0, true), member("blind", 0.0, false)];
        let providers = [Uuid::new_v4(), Uuid::new_v4()];
        let t = elect(&p, &candidates, &providers, &[Some("openai"), None]).unwrap();
        assert_eq!(t.account_name, "down");
        assert_eq!(t.reason, REASON_POOL);
    }
}
