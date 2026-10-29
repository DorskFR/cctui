//! Spend a usage-limit reset the moment it would help, instead of letting it
//! expire unused.
//!
//! Per provider credential, opt-in, policy-driven: the policy lives in `account_providers.provider_settings` under `auto_limit_reset`, so a
//! new knob needs no migration. Usage is read through the per-account cache
//! (never fetched directly), the claim goes through the same path as the
//! button, and a per-provider advisory lock plus the audit table keep a
//! multi-replica deployment from spending one credit twice.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use crate::routes::limit_reset::{self, limit_resets_at};
use crate::soft_limit::{KEY_SESSION, KEY_WEEKLY_ALL, normalize_usage_windows};
use crate::state::AppState;

/// Key under `provider_settings`.
pub const SETTINGS_KEY: &str = "auto_limit_reset";

pub const DEFAULT_USED_PCT: f64 = 90.0;
pub const DEFAULT_EXPIRES_WITHIN_HOURS: f64 = 24.0;
pub const DEFAULT_WEEKLY_MAX_PCT: f64 = 80.0;

/// A reset claimed less than this long ago, by anyone, parks the credential:
/// the cached usage may still describe the pre-claim windows.
const COOLDOWN_SECS: i64 = 15 * 60;

/// Separates these advisory-lock keys from any other user of the same space.
const LOCK_NAMESPACE: i64 = 0x6c69_6d72_7365_7400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub enabled: bool,
    /// Codex: redeem once the 5h window is this used.
    pub used_pct: f64,
    /// Codex: redeem a credit that expires within this many hours regardless.
    pub expires_within_hours: f64,
    /// Claude: never redeem while the weekly window is at or past this.
    pub weekly_max_pct: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            used_pct: DEFAULT_USED_PCT,
            expires_within_hours: DEFAULT_EXPIRES_WITHIN_HOURS,
            weekly_max_pct: DEFAULT_WEEKLY_MAX_PCT,
        }
    }
}

impl Policy {
    /// Read the policy from a stored `provider_settings` blob. Anything absent
    /// or malformed falls back to the default, and the default is off.
    #[must_use]
    pub fn from_provider_settings(settings: Option<&Value>) -> Self {
        let defaults = Self::default();
        let Some(p) = settings.and_then(|s| s.get(SETTINGS_KEY)).filter(|v| v.is_object()) else {
            return defaults;
        };
        let num = |k: &str, default: f64| {
            p.get(k)
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .unwrap_or(default)
        };
        Self {
            enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            used_pct: num("used_pct", defaults.used_pct),
            expires_within_hours: num("expires_within_hours", defaults.expires_within_hours),
            weekly_max_pct: num("weekly_max_pct", defaults.weekly_max_pct),
        }
    }
}

/// What the policy says about a credential's cached usage right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Spend this reset (`credit_id` names a Codex credit; `None` is Claude's
    /// at-wall program).
    Redeem {
        credit_id: Option<String>,
        reason: String,
    },
    Hold(&'static str),
}

fn window_pct(usage: &Value, key: &str) -> Option<f64> {
    normalize_usage_windows(usage).into_iter().find(|w| w.key == key).map(|w| w.utilization)
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

/// Pure: the policy applied to one usage payload at `now`. Off ⇒ `Hold`.
#[must_use]
pub fn decide(provider: &str, policy: &Policy, usage: &Value, now: DateTime<Utc>) -> Decision {
    if !policy.enabled {
        return Decision::Hold("auto-redeem is off");
    }
    match provider {
        "openai" => decide_codex(policy, usage, now),
        "anthropic" => decide_claude(policy, usage),
        _ => Decision::Hold("provider has no limit reset"),
    }
}

fn decide_codex(policy: &Policy, usage: &Value, now: DateTime<Utc>) -> Decision {
    let session = window_pct(usage, KEY_SESSION);
    let horizon = chrono::Duration::seconds((policy.expires_within_hours * 3600.0).round() as i64);
    let mut saw_credit = false;
    for entry in limit_resets_at("openai", usage, now).into_iter().filter(|e| e.usable) {
        saw_credit = true;
        if let Some(pct) = session
            && pct >= policy.used_pct
        {
            return Decision::Redeem {
                credit_id: Some(entry.id),
                reason: format!("5h window at {pct:.0}% (threshold {:.0}%)", policy.used_pct),
            };
        }
        if let Some(expires) = entry.expires_at.as_deref().and_then(parse_rfc3339)
            && expires - now <= horizon
        {
            return Decision::Redeem {
                credit_id: Some(entry.id),
                reason: format!(
                    "credit expires within {:.0}h ({})",
                    policy.expires_within_hours,
                    expires.to_rfc3339()
                ),
            };
        }
    }
    if saw_credit {
        Decision::Hold("credit available, threshold not met")
    } else {
        Decision::Hold("no credit available")
    }
}

fn decide_claude(policy: &Policy, usage: &Value) -> Decision {
    let Some(jt) = usage.get("juniper_tide").filter(|v| !v.is_null()) else {
        return Decision::Hold("no at-wall reset program");
    };
    let flag = |k: &str| jt.get(k).and_then(Value::as_bool).unwrap_or(false);
    if !(flag("available") && flag("eligible")) {
        return Decision::Hold("reset not available or not eligible");
    }
    let Some(session) = window_pct(usage, KEY_SESSION) else {
        return Decision::Hold("5h window unknown");
    };
    if session < 100.0 {
        return Decision::Hold("5h window not at the wall");
    }
    let Some(weekly) = window_pct(usage, KEY_WEEKLY_ALL) else {
        return Decision::Hold("weekly window unknown");
    };
    if weekly >= policy.weekly_max_pct {
        return Decision::Hold("weekly window past the ceiling");
    }
    Decision::Redeem {
        credit_id: None,
        reason: format!(
            "5h window at the wall, weekly at {weekly:.0}% (ceiling {:.0}%)",
            policy.weekly_max_pct
        ),
    }
}

/// `true` when a reset was claimed on the credential within `within_secs`.
pub async fn recently_redeemed(pool: &PgPool, provider_id: Uuid, within_secs: i64) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_limit_resets \
         WHERE provider_id = $1 AND at > now() - make_interval(secs => $2::double precision))",
    )
    .bind(provider_id)
    .bind(within_secs as f64)
    .fetch_one(pool)
    .await
    .unwrap_or(true)
}

/// `true` when a named credit already settled as spent, by any replica or by hand.
pub async fn already_claimed(pool: &PgPool, provider_id: Uuid, credit_id: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_limit_resets \
         WHERE provider_id = $1 AND credit_id = $2 \
           AND outcome IN ('reset', 'already_redeemed', 'already_used'))",
    )
    .bind(provider_id)
    .bind(credit_id)
    .fetch_one(pool)
    .await
    .unwrap_or(true)
}

fn lock_key(provider_id: Uuid) -> i64 {
    let bytes = provider_id.as_bytes();
    let mut head = [0u8; 8];
    head.copy_from_slice(&bytes[..8]);
    i64::from_le_bytes(head) ^ LOCK_NAMESPACE
}

/// A session-level Postgres advisory lock on one credential, held on its own
/// pooled connection.
///
/// Whichever replica takes it owns that credential's claim until it releases;
/// the others skip the credential this tick.
pub struct ClaimLock {
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    key: i64,
}

impl ClaimLock {
    pub async fn try_acquire(pool: &PgPool, provider_id: Uuid) -> Option<Self> {
        let key = lock_key(provider_id);
        let mut conn = pool.acquire().await.ok()?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .ok()?;
        got.then_some(Self { conn: Some(conn), key })
    }

    /// Unlock. A connection whose unlock failed is closed rather than returned
    /// to the pool, so a stale lock never rides along into another query.
    pub async fn release(mut self) {
        let Some(mut conn) = self.conn.take() else { return };
        let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(self.key)
            .fetch_one(&mut *conn)
            .await;
        if !matches!(unlocked, Ok(true)) {
            let _ = conn.detach().close().await;
        }
    }
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: Uuid,
    provider: String,
    account_name: String,
    provider_settings: Option<Value>,
}

/// One pass over every credential with the toggle on.
///
/// Each is judged against the usage the soft-limit path already caches
/// (refreshed at most once per cache TTL, never on demand here), and a credit
/// is spent at most once across replicas. Best-effort: every failure is logged
/// and the next tick retries.
pub async fn sweep(state: &AppState) {
    let rows: Vec<Candidate> = match sqlx::query_as(
        "SELECT p.id, p.provider, a.name AS account_name, p.provider_settings \
         FROM account_providers p JOIN accounts a ON a.id = p.account_id \
         WHERE p.provider IN ('openai', 'anthropic') \
           AND (p.provider_settings -> $1 ->> 'enabled') = 'true'",
    )
    .bind(SETTINGS_KEY)
    .fetch_all(&state.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("auto-redeem sweep query failed: {e}");
            return;
        }
    };
    for row in rows {
        let policy = Policy::from_provider_settings(row.provider_settings.as_ref());
        if !policy.enabled {
            continue;
        }
        let Some(usage) = crate::routes::gateway::usage_for_soft_limit(state, row.id).await else {
            continue;
        };
        let Decision::Redeem { credit_id, reason } =
            decide(&row.provider, &policy, &usage, Utc::now())
        else {
            continue;
        };
        redeem_once(state, &row, credit_id, &reason).await;
    }
}

async fn redeem_once(state: &AppState, row: &Candidate, credit_id: Option<String>, reason: &str) {
    if recently_redeemed(&state.pool, row.id, COOLDOWN_SECS).await {
        return;
    }
    if let Some(c) = credit_id.as_deref()
        && already_claimed(&state.pool, row.id, c).await
    {
        return;
    }
    let Some(lock) = ClaimLock::try_acquire(&state.pool, row.id).await else {
        return;
    };
    if !recently_redeemed(&state.pool, row.id, COOLDOWN_SECS).await {
        tracing::info!(account = %row.id, provider = %row.provider, "auto-redeem: {reason}");
        match limit_reset::redeem(state, row.id, &row.provider, credit_id, None).await {
            Ok(out) => {
                tracing::info!(account = %row.id, outcome = %out.outcome, "auto-redeem settled");
                if limit_reset::invalidates_usage(&out.outcome) {
                    state.bus.publish_server(cctui_proto::ws::ServerEvent::LimitResetRedeemed {
                        account_id: row.id,
                        account_name: row.account_name.clone(),
                        provider: row.provider.clone(),
                        outcome: out.outcome,
                        credit_id: out.credit_id,
                    });
                }
            }
            Err(e) => tracing::warn!(account = %row.id, "auto-redeem failed: {e}"),
        }
    }
    lock.release().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        parse_rfc3339("2026-10-07T12:00:00Z").unwrap()
    }

    fn on() -> Policy {
        Policy { enabled: true, ..Policy::default() }
    }

    fn codex_usage(used: f64, expires_at: &str) -> Value {
        json!({
            "five_hour": { "utilization": used, "resets_at": "2026-10-07T15:00:00Z" },
            "seven_day": { "utilization": 40.0 },
            "reset_credits": {
                "available_count": 1,
                "credits": [{
                    "id": "credit-1", "status": "available", "reset_type": "full",
                    "expires_at": expires_at, "title": "Reset"
                }]
            }
        })
    }

    fn claude_usage(session: f64, weekly: f64, available: bool, eligible: bool) -> Value {
        json!({
            "five_hour": { "utilization": session },
            "seven_day": { "utilization": weekly },
            "juniper_tide": {
                "eligible": eligible, "available": available,
                "next_available_at": "2026-10-12T00:00:00Z"
            }
        })
    }

    #[test]
    fn policy_reads_settings_and_defaults_off() {
        assert_eq!(Policy::from_provider_settings(None), Policy::default());
        assert!(!Policy::default().enabled);
        let settings = json!({
            "other": 1,
            "auto_limit_reset": { "enabled": true, "used_pct": 75, "weekly_max_pct": 50 }
        });
        let p = Policy::from_provider_settings(Some(&settings));
        assert!(p.enabled);
        assert!((p.used_pct - 75.0).abs() < f64::EPSILON);
        assert!((p.expires_within_hours - DEFAULT_EXPIRES_WITHIN_HOURS).abs() < f64::EPSILON);
        assert!((p.weekly_max_pct - 50.0).abs() < f64::EPSILON);

        let junk = json!({ "auto_limit_reset": { "enabled": "yes", "used_pct": -3 } });
        let p = Policy::from_provider_settings(Some(&junk));
        assert!(!p.enabled);
        assert!((p.used_pct - DEFAULT_USED_PCT).abs() < f64::EPSILON);
    }

    #[test]
    fn disabled_policy_never_redeems() {
        let usage = codex_usage(100.0, "2026-10-07T13:00:00Z");
        assert_eq!(
            decide("openai", &Policy::default(), &usage, now()),
            Decision::Hold("auto-redeem is off")
        );
        let usage = claude_usage(100.0, 10.0, true, true);
        assert_eq!(
            decide("anthropic", &Policy::default(), &usage, now()),
            Decision::Hold("auto-redeem is off")
        );
    }

    #[test]
    fn codex_redeems_on_used_threshold() {
        let d = decide("openai", &on(), &codex_usage(92.0, "2026-10-20T00:00:00Z"), now());
        assert!(
            matches!(d, Decision::Redeem { credit_id: Some(ref c), .. } if c == "credit-1"),
            "{d:?}"
        );
        let d = decide("openai", &on(), &codex_usage(89.0, "2026-10-20T00:00:00Z"), now());
        assert_eq!(d, Decision::Hold("credit available, threshold not met"));
    }

    #[test]
    fn codex_redeems_a_credit_about_to_expire() {
        let d = decide("openai", &on(), &codex_usage(10.0, "2026-10-08T06:00:00Z"), now());
        assert!(matches!(d, Decision::Redeem { credit_id: Some(_), .. }), "{d:?}");
        let d = decide("openai", &on(), &codex_usage(10.0, "2026-10-08T13:00:00Z"), now());
        assert_eq!(d, Decision::Hold("credit available, threshold not met"));
    }

    #[test]
    fn codex_ignores_spent_or_expired_credits() {
        let mut usage = codex_usage(99.0, "2026-10-20T00:00:00Z");
        usage["reset_credits"]["credits"][0]["status"] = json!("redeemed");
        assert_eq!(decide("openai", &on(), &usage, now()), Decision::Hold("no credit available"));
        let usage = codex_usage(99.0, "2026-10-01T00:00:00Z");
        assert_eq!(decide("openai", &on(), &usage, now()), Decision::Hold("no credit available"));
        assert_eq!(
            decide("openai", &on(), &json!({ "five_hour": { "utilization": 99.0 } }), now()),
            Decision::Hold("no credit available")
        );
    }

    #[test]
    fn claude_redeems_only_at_the_wall_with_weekly_headroom() {
        let d = decide("anthropic", &on(), &claude_usage(100.0, 30.0, true, true), now());
        assert!(matches!(d, Decision::Redeem { credit_id: None, .. }), "{d:?}");
        assert_eq!(
            decide("anthropic", &on(), &claude_usage(97.0, 30.0, true, true), now()),
            Decision::Hold("5h window not at the wall")
        );
        assert_eq!(
            decide("anthropic", &on(), &claude_usage(100.0, 85.0, true, true), now()),
            Decision::Hold("weekly window past the ceiling")
        );
        assert_eq!(
            decide("anthropic", &on(), &claude_usage(100.0, 30.0, false, true), now()),
            Decision::Hold("reset not available or not eligible")
        );
        assert_eq!(
            decide("anthropic", &on(), &claude_usage(100.0, 30.0, true, false), now()),
            Decision::Hold("reset not available or not eligible")
        );
        assert_eq!(
            decide("anthropic", &on(), &json!({ "five_hour": { "utilization": 100.0 } }), now()),
            Decision::Hold("no at-wall reset program")
        );
    }

    #[test]
    fn claude_holds_when_a_window_is_unknown() {
        let mut usage = claude_usage(100.0, 30.0, true, true);
        usage.as_object_mut().unwrap().remove("seven_day");
        assert_eq!(
            decide("anthropic", &on(), &usage, now()),
            Decision::Hold("weekly window unknown")
        );
        let mut usage = claude_usage(100.0, 30.0, true, true);
        usage.as_object_mut().unwrap().remove("five_hour");
        assert_eq!(decide("anthropic", &on(), &usage, now()), Decision::Hold("5h window unknown"));
    }

    #[test]
    fn claude_ceiling_is_configurable() {
        let p = Policy { weekly_max_pct: 95.0, ..on() };
        let d = decide("anthropic", &p, &claude_usage(100.0, 85.0, true, true), now());
        assert!(matches!(d, Decision::Redeem { .. }), "{d:?}");
    }

    #[test]
    fn lock_keys_are_stable_and_distinct() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(lock_key(a), lock_key(a));
        assert_ne!(lock_key(a), lock_key(b));
    }

    async fn fixture(pool: &PgPool) -> Uuid {
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("auto-redeem-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap();
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(user)
                .bind(format!("auto-redeem-{}", Uuid::new_v4()))
                .fetch_one(pool)
                .await
                .unwrap();
        sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider, provider_settings) \
             VALUES ($1, $2, 'openai', $3) RETURNING id",
        )
        .bind(user)
        .bind(account)
        .bind(json!({ SETTINGS_KEY: { "enabled": true } }))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Two replicas judging the same credential at once: one takes the lock,
    /// the other skips; once the first has written its audit row, the recent-
    /// claim gate and the per-credit gate both refuse a second spend.
    #[tokio::test]
    async fn one_credit_is_spent_once_across_replicas() {
        let Some(url) =
            crate::routes::gateway::test_db_url("one_credit_is_spent_once_across_replicas")
        else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let provider = fixture(&pool).await;

        assert!(!recently_redeemed(&pool, provider, COOLDOWN_SECS).await);
        assert!(!already_claimed(&pool, provider, "credit-1").await);

        let first = ClaimLock::try_acquire(&pool, provider).await.expect("first replica locks");
        assert!(
            ClaimLock::try_acquire(&pool, provider).await.is_none(),
            "second replica must skip while the first holds the lock"
        );
        let other = Uuid::new_v4();
        let unrelated =
            ClaimLock::try_acquire(&pool, other).await.expect("another credential is independent");
        unrelated.release().await;

        sqlx::query(
            "INSERT INTO account_limit_resets (provider_id, idempotency_key, credit_id, outcome) \
             VALUES ($1, $2, 'credit-1', 'reset')",
        )
        .bind(provider)
        .bind(Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();
        first.release().await;

        let again =
            ClaimLock::try_acquire(&pool, provider).await.expect("lock is free after release");
        assert!(recently_redeemed(&pool, provider, COOLDOWN_SECS).await);
        assert!(already_claimed(&pool, provider, "credit-1").await);
        assert!(!already_claimed(&pool, provider, "credit-2").await);
        again.release().await;

        sqlx::query("DELETE FROM account_providers WHERE id = $1")
            .bind(provider)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sweep_query_selects_only_enabled_first_party_rows() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "sweep_query_selects_only_enabled_first_party_rows",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let enabled = fixture(&pool).await;
        let off = fixture(&pool).await;
        sqlx::query("UPDATE account_providers SET provider_settings = $2 WHERE id = $1")
            .bind(off)
            .bind(json!({ SETTINGS_KEY: { "enabled": false } }))
            .execute(&pool)
            .await
            .unwrap();
        let rows: Vec<Candidate> = sqlx::query_as(
            "SELECT p.id, p.provider, a.name AS account_name, p.provider_settings \
             FROM account_providers p JOIN accounts a ON a.id = p.account_id \
             WHERE p.provider IN ('openai', 'anthropic') \
               AND (p.provider_settings -> $1 ->> 'enabled') = 'true' AND p.id IN ($2, $3)",
        )
        .bind(SETTINGS_KEY)
        .bind(enabled)
        .bind(off)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![enabled]);
        assert!(Policy::from_provider_settings(rows[0].provider_settings.as_ref()).enabled);
        for id in [enabled, off] {
            sqlx::query("DELETE FROM account_providers WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
