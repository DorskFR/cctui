//! Per-reset quota ledger. Nothing is averaged across credentials.

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::pace::window_duration;
use crate::routes::usage_history::clamp_from;
use crate::soft_limit::{KEY_SESSION, KEY_WEEKLY_ALL, WEEKLY_MODEL_PREFIX, is_usd_key, usd_label};
use crate::state::AppState;
use crate::store::usage_ledger::{self, LedgerClose};

pub use cctui_proto::api::usage_history::{AccountLedgerRow, AccountLedgerTotal, AccountUsageLedger};

/// Oldest the last sample before a reset may be for its final reading to stand.
const SHORT_WINDOW_SAMPLING_GAP: Duration = Duration::minutes(30);
const LONG_WINDOW_SAMPLING_GAP: Duration = Duration::hours(6);

#[derive(Debug, Deserialize)]
pub struct LedgerQuery {
    pub from: Option<DateTime<Utc>>,
    pub account: Option<Uuid>,
    pub window: Option<String>,
}

pub fn window_label(key: &str) -> String {
    if key == KEY_SESSION {
        "5h".to_owned()
    } else if key == KEY_WEEKLY_ALL {
        "Weekly (all models)".to_owned()
    } else if is_usd_key(key) {
        usd_label(key).to_owned()
    } else if let Some(model) = key.strip_prefix(WEEKLY_MODEL_PREFIX) {
        format!("Weekly {model}")
    } else {
        key.to_owned()
    }
}

fn sampling_gap_threshold(len: Option<Duration>) -> Duration {
    match len {
        Some(l) if l <= Duration::hours(5) => SHORT_WINDOW_SAMPLING_GAP,
        _ => LONG_WINDOW_SAMPLING_GAP,
    }
}

pub fn ledger_row(c: LedgerClose) -> AccountLedgerRow {
    let len = window_duration(&c.window_key);
    let unused_pct = c.wasted_pct.clamp(0.0, 100.0);
    let sampling_gap =
        c.last_sampled_at.is_none_or(|at| c.resets_at - at > sampling_gap_threshold(len));
    let sampled_in_window =
        c.last_sampled_at.is_some_and(|at| len.is_none_or(|l| at > c.resets_at - l));
    let amount_usd = if is_usd_key(&c.window_key) { c.last_amount_usd } else { None };
    AccountLedgerRow {
        account_id: c.provider_id,
        account_name: c.account_name,
        account_emoji: c.account_emoji,
        provider: c.provider,
        window_label: window_label(&c.window_key),
        unused_capacity_hours: len.map(|l| unused_pct / 100.0 * l.num_seconds() as f64 / 3600.0),
        idle: c.final_utilization <= 0.0 || !sampled_in_window,
        window_key: c.window_key,
        resets_at: c.resets_at,
        final_utilization: c.final_utilization,
        unused_pct,
        amount_usd,
        sampling_gap,
    }
}

#[derive(Default)]
struct Acc {
    closes: u32,
    idle: u32,
    used: u32,
    unused_sum: f64,
    capacity: Option<f64>,
}

pub fn totals(rows: &[AccountLedgerRow]) -> Vec<AccountLedgerTotal> {
    let mut by: BTreeMap<(&str, Uuid, &str), Acc> = BTreeMap::new();
    for r in rows {
        let key = (r.account_name.as_str(), r.account_id, r.window_key.as_str());
        let a = by.entry(key).or_default();
        a.closes += 1;
        if r.idle {
            a.idle += 1;
            continue;
        }
        a.used += 1;
        a.unused_sum += r.unused_pct;
        if let Some(h) = r.unused_capacity_hours {
            a.capacity = Some(a.capacity.unwrap_or(0.0) + h);
        }
    }
    by.into_iter()
        .map(|((name, id, key), a)| AccountLedgerTotal {
            account_id: id,
            account_name: name.to_owned(),
            window_key: key.to_owned(),
            closes: a.closes,
            unused_pct_mean: (a.used > 0).then_some(a.unused_sum / f64::from(a.used)),
            unused_capacity_hours: a.capacity,
            idle_closes: a.idle,
        })
        .collect()
}

/// `GET /accounts/usage/ledger?from=&account=&window=`
pub async fn account_usage_ledger(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<LedgerQuery>,
) -> Result<Json<AccountUsageLedger>, AppError> {
    let from = clamp_from(q.from, Utc::now());
    let closes =
        usage_ledger::closes(&state.pool, ctx.owner_filter(), q.account, q.window.as_deref(), from)
            .await?;
    let rows: Vec<AccountLedgerRow> = closes.into_iter().map(ledger_row).collect();
    let totals = totals(&rows);
    Ok(Json(AccountUsageLedger { rows, totals }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::auth::Scope;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn near(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    fn close(key: &str, final_util: f64, last_sample_before: Option<Duration>) -> LedgerClose {
        let resets_at = t("2026-10-20T12:00:00Z");
        LedgerClose {
            provider_id: Uuid::nil(),
            account_name: "personal".into(),
            account_emoji: None,
            provider: "anthropic".into(),
            window_key: key.into(),
            resets_at,
            final_utilization: final_util,
            wasted_pct: (100.0 - final_util).clamp(0.0, 100.0),
            last_sampled_at: last_sample_before.map(|d| resets_at - d),
            last_amount_usd: Some(4.0),
        }
    }

    #[test]
    fn unused_capacity_is_the_unused_share_of_the_window_length() {
        let s = ledger_row(close("session", 60.0, Some(Duration::minutes(5))));
        assert!(near(s.unused_capacity_hours, 2.0));
        let w = ledger_row(close("weekly_all", 90.0, Some(Duration::minutes(5))));
        assert!(near(w.unused_capacity_hours, 16.8));
        let m = ledger_row(close("weekly_model:fable", 50.0, Some(Duration::minutes(5))));
        assert!(near(m.unused_capacity_hours, 84.0));
        assert_eq!(m.window_label, "Weekly fable");
        assert!(ledger_row(close("odd", 50.0, None)).unused_capacity_hours.is_none());
    }

    #[test]
    fn sampling_gap_depends_on_the_window_length() {
        assert!(!ledger_row(close("session", 50.0, Some(Duration::minutes(29)))).sampling_gap);
        assert!(ledger_row(close("session", 50.0, Some(Duration::minutes(31)))).sampling_gap);
        assert!(!ledger_row(close("weekly_all", 50.0, Some(Duration::hours(5)))).sampling_gap);
        assert!(ledger_row(close("weekly_all", 50.0, Some(Duration::hours(7)))).sampling_gap);
        assert!(ledger_row(close("session", 50.0, None)).sampling_gap);
    }

    #[test]
    fn idle_means_nothing_used_or_nothing_sampled_in_the_window() {
        assert!(ledger_row(close("session", 0.0, Some(Duration::minutes(5)))).idle);
        assert!(ledger_row(close("session", 30.0, Some(Duration::hours(6)))).idle);
        assert!(ledger_row(close("session", 30.0, None)).idle);
        assert!(!ledger_row(close("session", 30.0, Some(Duration::hours(4)))).idle);
    }

    #[test]
    fn amount_only_for_dollar_windows() {
        let pct = ledger_row(close("session", 50.0, Some(Duration::minutes(5))));
        assert!(pct.amount_usd.is_none());
        let usd = ledger_row(close("usd_5h", 50.0, Some(Duration::minutes(5))));
        assert!(near(usd.amount_usd, 4.0));
        assert_eq!(usd.window_label, "5h spend");
    }

    #[test]
    fn totals_stay_per_credential_and_leave_idle_closes_out() {
        let other = Uuid::new_v4();
        let mut rows = vec![
            ledger_row(close("session", 60.0, Some(Duration::minutes(5)))),
            ledger_row(close("session", 80.0, Some(Duration::minutes(5)))),
            ledger_row(close("session", 0.0, Some(Duration::minutes(5)))),
        ];
        let mut theirs = ledger_row(close("session", 10.0, Some(Duration::minutes(5))));
        theirs.account_id = other;
        theirs.account_name = "work".into();
        rows.push(theirs);

        let t = totals(&rows);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].account_name, "personal");
        assert_eq!((t[0].closes, t[0].idle_closes), (3, 1));
        assert!(near(t[0].unused_pct_mean, 30.0));
        assert!(near(t[0].unused_capacity_hours, 3.0));
        assert_eq!(t[1].account_id, other);
        assert!(near(t[1].unused_pct_mean, 90.0));

        let idle_only = totals(&rows[2..3]);
        assert!(idle_only[0].unused_pct_mean.is_none());
        assert!(idle_only[0].unused_capacity_hours.is_none());
    }

    #[test]
    fn ledger_serializes_with_the_contract_shape() {
        let rows = vec![ledger_row(close("session", 60.0, Some(Duration::minutes(5))))];
        let ledger = AccountUsageLedger { totals: totals(&rows), rows };
        let body = serde_json::to_value(ledger).unwrap();
        for k in [
            "account_id",
            "account_name",
            "account_emoji",
            "provider",
            "window_key",
            "window_label",
            "resets_at",
            "final_utilization",
            "unused_pct",
            "unused_capacity_hours",
            "amount_usd",
            "sampling_gap",
            "idle",
        ] {
            assert!(body["rows"][0].get(k).is_some(), "row missing {k}");
        }
        for k in [
            "account_id",
            "account_name",
            "window_key",
            "closes",
            "unused_pct_mean",
            "unused_capacity_hours",
            "idle_closes",
        ] {
            assert!(body["totals"][0].get(k).is_some(), "total missing {k}");
        }
    }

    async fn test_state(tag: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(AppState::for_test(pool))
    }

    fn ctx(user_id: Uuid, admin: bool) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: if admin { BTreeSet::from([Scope::Admin]) } else { BTreeSet::new() },
        }
    }

    async fn tenant(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("ledger-{uid}"))
            .bind(format!("hledger-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        let account: Uuid = sqlx::query_scalar(
            "INSERT INTO accounts (user_id, name, emoji) VALUES ($1, $2, '🦊') RETURNING id",
        )
        .bind(uid)
        .bind(format!("ledger-account-{uid}"))
        .fetch_one(pool)
        .await
        .unwrap();
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, 'anthropic') RETURNING id",
        )
        .bind(uid)
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap();
        (uid, account, provider)
    }

    async fn seed_close(
        pool: &sqlx::PgPool,
        provider: Uuid,
        key: &str,
        resets_at: DateTime<Utc>,
        final_util: f64,
        last_sample_before: Duration,
    ) {
        sqlx::query(
            "INSERT INTO account_window_closes \
                 (provider_id, window_key, resets_at, final_utilization, wasted_pct) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(provider)
        .bind(key)
        .bind(resets_at)
        .bind(final_util)
        .bind(100.0 - final_util)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account_usage_samples \
                 (provider_id, window_key, utilization, amount_usd, resets_at, sampled_at) \
             VALUES ($1, $2, $3, 2.5, $4, $5)",
        )
        .bind(provider)
        .bind(key)
        .bind(final_util)
        .bind(resets_at)
        .bind(resets_at - last_sample_before)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn ledger(
        state: &AppState,
        ctx: AuthContext,
        account: Option<Uuid>,
    ) -> AccountUsageLedger {
        let Json(l) = account_usage_ledger(
            State(state.clone()),
            Extension(ctx),
            Query(LedgerQuery { from: None, account, window: None }),
        )
        .await
        .unwrap();
        l
    }

    #[tokio::test]
    async fn a_user_sees_only_their_own_credentials_and_an_admin_sees_all() {
        let Some(state) = test_state("ledger_visibility").await else { return };
        let now = Utc::now();
        let (alice, _, alice_p) = tenant(&state.pool).await;
        let (bob, _, bob_p) = tenant(&state.pool).await;
        let at = now - Duration::hours(1);
        seed_close(&state.pool, alice_p, "session", at, 50.0, Duration::minutes(5)).await;
        seed_close(&state.pool, bob_p, "session", at, 70.0, Duration::minutes(5)).await;

        let mine = ledger(&state, ctx(alice, false), None).await;
        assert_eq!(mine.rows.len(), 1);
        assert_eq!(mine.rows[0].account_id, alice_p);
        assert!(mine.totals.iter().all(|t| t.account_id == alice_p));

        assert!(ledger(&state, ctx(alice, false), Some(bob_p)).await.rows.is_empty());

        let admin = ledger(&state, ctx(bob, true), None).await;
        assert!(admin.rows.iter().any(|r| r.account_id == alice_p));
        assert!(admin.rows.iter().any(|r| r.account_id == bob_p));
    }

    #[tokio::test]
    async fn rows_carry_idle_gap_capacity_and_no_amount_for_percent_windows() {
        let Some(state) = test_state("ledger_rows").await else { return };
        let now = Utc::now();
        let (uid, account, p) = tenant(&state.pool).await;
        let pool = &state.pool;
        seed_close(pool, p, "session", now - Duration::hours(1), 60.0, Duration::minutes(10)).await;
        seed_close(pool, p, "session", now - Duration::hours(6), 0.0, Duration::minutes(10)).await;
        seed_close(pool, p, "weekly_all", now - Duration::days(2), 90.0, Duration::hours(8)).await;
        seed_close(pool, p, "session", now - Duration::days(9), 10.0, Duration::minutes(1)).await;

        let l = ledger(&state, ctx(uid, false), Some(account)).await;
        assert_eq!(l.rows.len(), 3, "the close before the default week is out");
        assert!(l.rows.windows(2).all(|w| w[0].resets_at >= w[1].resets_at));

        let fresh = &l.rows[0];
        assert_eq!((fresh.window_key.as_str(), fresh.window_label.as_str()), ("session", "5h"));
        assert!(!fresh.idle && !fresh.sampling_gap);
        assert!(near(fresh.unused_capacity_hours, 2.0));
        assert!(fresh.amount_usd.is_none());
        assert_eq!(fresh.account_emoji.as_deref(), Some("🦊"));
        assert_eq!(fresh.provider, "anthropic");

        assert!(l.rows[1].idle);
        let weekly = &l.rows[2];
        assert!(weekly.sampling_gap && !weekly.idle);
        assert!(near(weekly.unused_capacity_hours, 16.8));

        let session = l.totals.iter().find(|t| t.window_key == "session").unwrap();
        assert_eq!((session.closes, session.idle_closes), (2, 1));
        assert!(near(session.unused_pct_mean, 40.0));
        assert!(near(session.unused_capacity_hours, 2.0));
        let weekly = l.totals.iter().find(|t| t.window_key == "weekly_all").unwrap();
        assert!(near(weekly.unused_pct_mean, 10.0));
    }
}
