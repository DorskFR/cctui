//! Provider usage endpoints rate-limit per access token: every caller shares one
//! gate per account, whatever the number of sessions.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

pub const READ_INTERVAL: Duration = Duration::from_mins(1);
const BACKOFF_BASE: Duration = Duration::from_mins(3);
const BACKOFF_CAP: Duration = Duration::from_mins(30);
const RETRY_AFTER_CAP: Duration = Duration::from_hours(1);

#[derive(Default)]
struct Slot {
    open_at: Option<Instant>,
    failures: u32,
    retry_after: Option<Duration>,
}

#[derive(Default)]
pub struct ReadGate {
    slots: DashMap<Uuid, Slot>,
}

impl ReadGate {
    pub fn global() -> &'static Self {
        static GATE: OnceLock<ReadGate> = OnceLock::new();
        GATE.get_or_init(Self::default)
    }

    /// Reserve the account's next read, or return how long until one is allowed.
    ///
    /// # Errors
    /// The time left before the gate opens.
    pub fn claim(&self, account: Uuid, now: Instant) -> Result<(), Duration> {
        let mut slot = self.slots.entry(account).or_default();
        let result = match slot.open_at {
            Some(open_at) if open_at > now => Err(open_at - now),
            _ => {
                slot.open_at = Some(now + READ_INTERVAL);
                Ok(())
            }
        };
        drop(slot);
        result
    }

    pub fn succeeded(&self, account: Uuid, now: Instant) {
        self.slots.insert(
            account,
            Slot { open_at: Some(now + READ_INTERVAL), failures: 0, retry_after: None },
        );
    }

    /// Upstream's `Retry-After`: the next [`Self::failed`] waits at least this long.
    pub fn retry_after(&self, account: Uuid, wait: Duration) {
        self.slots.entry(account).or_default().retry_after = Some(wait);
    }

    /// Keep the gate closed until at least `until`.
    pub fn hold(&self, account: Uuid, until: Instant) {
        let mut slot = self.slots.entry(account).or_default();
        slot.open_at = Some(slot.open_at.map_or(until, |at| at.max(until)));
    }

    /// Close the gate after a failed read; returns how long it stays closed.
    #[must_use]
    pub fn failed(&self, account: Uuid, now: Instant) -> Duration {
        let mut slot = self.slots.entry(account).or_default();
        slot.failures = slot.failures.saturating_add(1);
        let hint = slot.retry_after.take().map_or(Duration::ZERO, |w| w.min(RETRY_AFTER_CAP));
        let backoff = exponential(slot.failures).max(hint);
        slot.open_at = Some(now + backoff);
        backoff
    }
}

/// The backoff after `failures` consecutive failed reads.
#[must_use]
pub fn exponential(failures: u32) -> Duration {
    BACKOFF_BASE.saturating_mul(1 << failures.saturating_sub(1).min(8)).min(BACKOFF_CAP)
}

/// `Retry-After` as delta-seconds or an HTTP date.
#[must_use]
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().ok()
}

/// The gate shared by every replica, behind the in-process [`ReadGate`].
pub mod shared {
    use std::time::Duration;

    use sqlx::PgPool;
    use uuid::Uuid;

    #[derive(Debug, PartialEq, Eq)]
    pub enum Claim {
        /// This replica reads; `failures` is the shared count before this read.
        Read { failures: u32 },
        /// Another replica holds the read or the backoff: `reading` is its last
        /// successful read, when one exists, and how old it is.
        Held { wait: Duration, reading: Option<(Option<serde_json::Value>, Duration)> },
    }

    /// # Errors
    /// The database refused the claim.
    pub async fn claim(pool: &PgPool, account: Uuid, interval: Duration) -> sqlx::Result<Claim> {
        let claimed: Option<i32> = sqlx::query_scalar(
            "INSERT INTO account_usage_reads (account_id, read_after) \
             VALUES ($1, now() + make_interval(secs => $2)) \
             ON CONFLICT (account_id) DO UPDATE SET read_after = EXCLUDED.read_after \
             WHERE account_usage_reads.read_after <= now() \
             RETURNING failures",
        )
        .bind(account)
        .bind(interval.as_secs_f64())
        .fetch_optional(pool)
        .await?;
        if let Some(failures) = claimed {
            return Ok(Claim::Read { failures: u32::try_from(failures).unwrap_or(0) });
        }
        let held: Option<(f64, Option<serde_json::Value>, Option<f64>)> = sqlx::query_as(
            "SELECT GREATEST(EXTRACT(EPOCH FROM read_after - now()), 0)::float8, reading, \
                    EXTRACT(EPOCH FROM now() - read_at)::float8 \
             FROM account_usage_reads WHERE account_id = $1",
        )
        .bind(account)
        .fetch_optional(pool)
        .await?;
        let (wait, reading, age) = held.unwrap_or((0.0, None, None));
        Ok(Claim::Held { wait: secs(wait), reading: age.map(|age| (reading, secs(age))) })
    }

    fn secs(s: f64) -> Duration {
        Duration::try_from_secs_f64(s).unwrap_or_default()
    }

    /// # Errors
    /// The database refused the write.
    pub async fn succeeded(
        pool: &PgPool,
        account: Uuid,
        reading: Option<&serde_json::Value>,
        interval: Duration,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE account_usage_reads SET failures = 0, reading = $2, read_at = now(), \
                    read_after = now() + make_interval(secs => $3) \
             WHERE account_id = $1",
        )
        .bind(account)
        .bind(reading)
        .bind(interval.as_secs_f64())
        .execute(pool)
        .await
        .map(drop)
    }

    /// # Errors
    /// The database refused the write.
    pub async fn failed(
        pool: &PgPool,
        account: Uuid,
        failures: u32,
        backoff: Duration,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE account_usage_reads SET failures = $2, \
                    read_after = now() + make_interval(secs => $3) \
             WHERE account_id = $1",
        )
        .bind(account)
        .bind(i32::try_from(failures).unwrap_or(i32::MAX))
        .bind(backoff.as_secs_f64())
        .execute(pool)
        .await
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_read_per_interval_however_many_callers() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        assert_eq!(gate.claim(id, t0), Ok(()));
        for _ in 0..50 {
            assert!(gate.claim(id, t0 + Duration::from_secs(1)).is_err());
        }
        gate.succeeded(id, t0);
        assert!(
            gate.claim(id, (t0 + READ_INTERVAL).checked_sub(Duration::from_secs(1)).unwrap())
                .is_err()
        );
        assert_eq!(gate.claim(id, t0 + READ_INTERVAL), Ok(()));
    }

    #[test]
    fn failures_back_off_exponentially_up_to_the_cap() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        let waits: Vec<_> = (0..6)
            .map(|_| {
                gate.claim(id, t0 + Duration::from_hours(5)).ok();
                gate.failed(id, t0).as_secs() / 60
            })
            .collect();
        assert_eq!(waits, [3, 6, 12, 24, 30, 30]);
        assert_eq!(gate.claim(id, t0 + Duration::from_mins(29)), Err(Duration::from_mins(1)));
    }

    #[test]
    fn retry_after_only_lengthens_the_backoff() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        gate.retry_after(id, Duration::from_mins(10));
        assert_eq!(gate.failed(id, t0), Duration::from_mins(10));
        gate.retry_after(id, Duration::from_mins(1));
        assert_eq!(gate.failed(id, t0), Duration::from_mins(6));
        gate.retry_after(id, Duration::from_hours(9));
        assert_eq!(gate.failed(id, t0), RETRY_AFTER_CAP);
    }

    #[test]
    fn a_success_resets_the_backoff() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        let _ = gate.failed(id, t0);
        let _ = gate.failed(id, t0);
        gate.succeeded(id, t0);
        assert_eq!(gate.failed(id, t0), BACKOFF_BASE);
    }

    #[test]
    fn retry_after_parses_seconds_and_dates() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_mins(2)));
        let at = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc2822();
        headers.insert(reqwest::header::RETRY_AFTER, at.parse().unwrap());
        let parsed = parse_retry_after(&headers).unwrap();
        assert!(parsed > Duration::from_mins(4) && parsed <= Duration::from_mins(5));
        headers.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }

    async fn seeded_account(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
        let (uid, acct, prov) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("usage-gate-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'usage-gate')")
            .bind(acct)
            .bind(uid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_providers (id, user_id, provider, account_id) \
             VALUES ($1, $2, 'anthropic', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(pool)
        .await
        .unwrap();
        (uid, prov)
    }

    async fn shared_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
        Some(sqlx::postgres::PgPoolOptions::new().max_connections(8).connect(&url).await.unwrap())
    }

    #[tokio::test]
    async fn replicas_share_one_read_per_interval_and_its_reading() {
        let Some(pool) = shared_pool("usage_gate_shared_read").await else { return };
        let (uid, id) = seeded_account(&pool).await;
        let claims = futures_util::future::join_all(
            (0..20).map(|_| shared::claim(&pool, id, READ_INTERVAL)),
        )
        .await;
        let reads = claims.iter().filter(|c| matches!(c, Ok(shared::Claim::Read { .. }))).count();
        assert_eq!(reads, 1, "{claims:?}");
        assert!(claims.iter().all(|c| match c {
            Ok(shared::Claim::Held { reading, wait }) =>
                reading.is_none() && *wait <= READ_INTERVAL,
            Ok(shared::Claim::Read { failures }) => *failures == 0,
            Err(_) => false,
        }));

        let usage = serde_json::json!({ "five_hour": { "utilization": 12.0 } });
        shared::succeeded(&pool, id, Some(&usage), READ_INTERVAL).await.unwrap();
        match shared::claim(&pool, id, READ_INTERVAL).await.unwrap() {
            shared::Claim::Held { reading: Some((served, age)), .. } => {
                assert_eq!(served, Some(usage));
                assert!(age < Duration::from_secs(30));
            }
            other => panic!("the other replica serves the reading: {other:?}"),
        }
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[tokio::test]
    async fn after_a_failure_no_replica_reads_until_the_shared_backoff_passes() {
        let Some(pool) = shared_pool("usage_gate_shared_backoff").await else { return };
        let (uid, id) = seeded_account(&pool).await;
        assert_eq!(
            shared::claim(&pool, id, READ_INTERVAL).await.unwrap(),
            shared::Claim::Read { failures: 0 }
        );
        shared::failed(&pool, id, 1, BACKOFF_BASE).await.unwrap();
        for _ in 0..3 {
            match shared::claim(&pool, id, READ_INTERVAL).await.unwrap() {
                shared::Claim::Held { wait, .. } => {
                    assert!(wait > READ_INTERVAL && wait <= BACKOFF_BASE, "{wait:?}");
                }
                other @ shared::Claim::Read { .. } => {
                    panic!("the backoff holds every replica: {other:?}")
                }
            }
        }
        sqlx::query(
            "UPDATE account_usage_reads SET read_after = now() - interval '1 second' \
             WHERE account_id = $1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            shared::claim(&pool, id, READ_INTERVAL).await.unwrap(),
            shared::Claim::Read { failures: 1 },
            "the next read carries the shared failure count"
        );
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[test]
    fn hold_only_lengthens_the_closed_gate() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        gate.hold(id, t0 + Duration::from_mins(5));
        gate.hold(id, t0 + Duration::from_mins(1));
        assert_eq!(gate.claim(id, t0 + Duration::from_mins(4)), Err(Duration::from_mins(1)));
        assert_eq!(gate.claim(id, t0 + Duration::from_mins(5)), Ok(()));
    }
}
