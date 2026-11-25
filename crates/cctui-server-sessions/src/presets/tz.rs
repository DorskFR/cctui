use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use sqlx::PgPool;

use super::cron::{self, Schedule};

pub const UTC: &str = "UTC";
const STEP_MINUTES: i64 = 15;
const FIRST_WINDOW_DAYS: i64 = 8;

/// A zone's UTC offset over a window: the offset at its start, then each change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    changes: Vec<(DateTime<Utc>, i32)>,
}

impl Zone {
    #[cfg(test)]
    #[must_use]
    pub fn fixed(offset_secs: i32) -> Self {
        Self { changes: vec![(DateTime::<Utc>::MIN_UTC, offset_secs)] }
    }

    /// `changes` is ordered; its first entry is the offset the window opens with.
    #[must_use]
    pub const fn new(changes: Vec<(DateTime<Utc>, i32)>) -> Self {
        Self { changes }
    }

    fn offset_at(&self, at: DateTime<Utc>) -> i32 {
        let before = self.changes.iter().rev().find(|(from, _)| *from <= at);
        before.or_else(|| self.changes.first()).map_or(0, |(_, offset)| *offset)
    }

    fn first_change_in(
        &self,
        after: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Option<(DateTime<Utc>, i32)> {
        self.changes.iter().skip(1).copied().find(|(at, _)| *at > after && *at <= until)
    }
}

fn local(at: DateTime<Utc>, offset: i32) -> NaiveDateTime {
    at.naive_utc() + Duration::seconds(i64::from(offset))
}

fn instant(wall: NaiveDateTime, offset: i32) -> DateTime<Utc> {
    Utc.from_utc_datetime(&(wall - Duration::seconds(i64::from(offset))))
}

/// A wall-clock time the clock skips fires once, shifted by the skipped span.
/// A wall-clock time the clock repeats fires on its first pass only, unless the
/// schedule runs every hour.
#[must_use]
pub fn next_fire(schedule: &Schedule, zone: &Zone, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let mut cursor = after;
    let mut offset = zone.offset_at(after);
    let mut from = local(after, offset);
    loop {
        let wall = schedule.next_after_naive(from)?;
        let fire = instant(wall, offset);
        let Some((change, shifted)) = zone.first_change_in(cursor, fire) else {
            return Some(fire);
        };
        if shifted > offset && wall < local(change, shifted) {
            return Some(fire);
        }
        let repeated = shifted < offset && !schedule.any_hour();
        from = local(change, if repeated { offset } else { shifted }) - Duration::minutes(1);
        cursor = change;
        offset = shifted;
    }
}

pub async fn exists(pool: &PgPool, name: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)")
        .bind(name)
        .fetch_one(pool)
        .await
}

pub async fn zone(
    pool: &PgPool,
    name: &str,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> sqlx::Result<Zone> {
    let step = STEP_MINUTES * 60;
    let start =
        DateTime::from_timestamp(from.timestamp().div_euclid(step) * step, 0).unwrap_or(from);
    let changes: Vec<(DateTime<Utc>, i32)> = sqlx::query_as(
        "WITH samples AS ( \
             SELECT t, EXTRACT(EPOCH FROM ((t AT TIME ZONE $1) - (t AT TIME ZONE 'UTC')))::int \
                    AS off \
             FROM generate_series($2::timestamptz, $3::timestamptz, \
                                  make_interval(mins => $4)) t) \
         SELECT t, off FROM (SELECT t, off, lag(off) OVER (ORDER BY t) AS prev FROM samples) s \
         WHERE prev IS DISTINCT FROM off ORDER BY t",
    )
    .bind(name)
    .bind(start)
    .bind(until)
    .bind(i32::try_from(STEP_MINUTES).unwrap_or(15))
    .fetch_all(pool)
    .await?;
    Ok(Zone::new(changes))
}

/// `None` when the expression never fires. The zone is read from Postgres, so
/// every replica agrees on it.
pub async fn next_fire_in(
    pool: &PgPool,
    expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    let Ok(schedule) = cron::parse(expr) else { return Ok(None) };
    if timezone == UTC {
        return Ok(schedule.next_after(after));
    }
    let near = after + Duration::days(FIRST_WINDOW_DAYS);
    let found = next_fire(&schedule, &zone(pool, timezone, after, near).await?, after);
    match found {
        Some(at) if at + Duration::days(1) > near => {
            let wide = zone(pool, timezone, after, at + Duration::days(2)).await?;
            Ok(next_fire(&schedule, &wide, after))
        }
        found => Ok(found),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, TimeZone, Utc};

    use super::{Zone, next_fire, next_fire_in, zone};
    use crate::presets::cron::parse;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn new_york_2026() -> Zone {
        Zone::new(vec![
            (at(2026, 1, 1, 0, 0), -5 * 3600),
            (at(2026, 3, 8, 7, 0), -4 * 3600),
            (at(2026, 11, 1, 6, 0), -5 * 3600),
        ])
    }

    fn fire(expr: &str, zone: &Zone, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        next_fire(&parse(expr).unwrap(), zone, after)
    }

    #[test]
    fn weekday_seven_in_tokyo_fires_once_a_day_at_22_utc_the_day_before() {
        let tokyo = Zone::fixed(9 * 3600);
        let mut after = at(2026, 11, 19, 21, 59);
        let mut fires = Vec::new();
        for _ in 0..6 {
            after = fire("0 7 * * 1-5", &tokyo, after).unwrap();
            fires.push(after);
        }
        assert_eq!(
            fires,
            [
                at(2026, 11, 19, 22, 0),
                at(2026, 11, 22, 22, 0),
                at(2026, 11, 23, 22, 0),
                at(2026, 11, 24, 22, 0),
                at(2026, 11, 25, 22, 0),
                at(2026, 11, 26, 22, 0),
            ],
            "friday 07:00 JST, then monday to friday"
        );
    }

    #[test]
    fn a_daily_time_keeps_its_wall_clock_across_both_dst_changes() {
        let ny = new_york_2026();
        assert_eq!(fire("0 9 * * *", &ny, at(2026, 3, 7, 14, 0)), Some(at(2026, 3, 8, 13, 0)));
        assert_eq!(fire("0 9 * * *", &ny, at(2026, 3, 6, 14, 0)), Some(at(2026, 3, 7, 14, 0)));
        assert_eq!(fire("0 9 * * *", &ny, at(2026, 10, 31, 13, 0)), Some(at(2026, 11, 1, 14, 0)));
    }

    #[test]
    fn a_time_the_clock_skips_fires_once_shifted_forward() {
        let ny = new_york_2026();
        let shifted = at(2026, 3, 8, 7, 30);
        assert_eq!(fire("30 2 * * *", &ny, at(2026, 3, 7, 8, 0)), Some(shifted), "03:30 EDT");
        assert_eq!(fire("30 2 * * *", &ny, shifted), Some(at(2026, 3, 9, 6, 30)));
        assert_eq!(fire("30 * * * *", &ny, at(2026, 3, 8, 6, 30)), Some(shifted));
        assert_eq!(fire("30 * * * *", &ny, shifted), Some(at(2026, 3, 8, 8, 30)));
        assert_eq!(fire("0 4 * * *", &ny, at(2026, 3, 8, 6, 30)), Some(at(2026, 3, 8, 8, 0)));
    }

    #[test]
    fn a_time_the_clock_repeats_fires_once_for_a_pinned_hour() {
        let ny = new_york_2026();
        let first = at(2026, 11, 1, 5, 30);
        assert_eq!(fire("30 1 * * *", &ny, at(2026, 10, 31, 6, 0)), Some(first), "01:30 EDT");
        assert_eq!(fire("30 1 * * *", &ny, first), Some(at(2026, 11, 2, 6, 30)), "not 01:30 EST");
        assert_eq!(fire("0 2 * * *", &ny, at(2026, 11, 1, 5, 59)), Some(at(2026, 11, 1, 7, 0)));
    }

    #[test]
    fn an_every_hour_schedule_runs_by_elapsed_time_through_the_repeated_hour() {
        let ny = new_york_2026();
        let mut after = at(2026, 11, 1, 4, 0);
        let mut fires = Vec::new();
        for _ in 0..4 {
            after = fire("0 * * * *", &ny, after).unwrap();
            fires.push(after);
        }
        let hours = [5, 6, 7, 8].map(|h| at(2026, 11, 1, h, 0));
        assert_eq!(fires, hours, "01:00 EDT, 01:00 EST, 02:00 EST, 03:00 EST");
        assert_eq!(fire("*/20 * * * *", &ny, at(2026, 11, 1, 5, 50)), Some(at(2026, 11, 1, 6, 0)));
    }

    async fn pool(name: &str) -> Option<sqlx::PgPool> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
        Some(sqlx::PgPool::connect(&url).await.expect("connect test db"))
    }

    #[tokio::test]
    async fn postgres_reports_the_zone_changes_and_rejects_unknown_names() {
        let Some(pool) = pool("postgres_reports_the_zone_changes").await else { return };
        let from = at(2026, 1, 1, 0, 7);
        let ny = zone(&pool, "America/New_York", from, at(2026, 12, 31, 0, 0)).await.unwrap();
        assert_eq!(ny, new_york_2026());
        let tokyo = zone(&pool, "Asia/Tokyo", from, from + Duration::days(400)).await.unwrap();
        assert_eq!(tokyo, Zone::new(vec![(at(2026, 1, 1, 0, 0), 9 * 3600)]));
        assert!(super::exists(&pool, "Asia/Tokyo").await.unwrap());
        for unknown in ["Mars/Olympus", "EST5EDT4", "", "asia/tokyo "] {
            assert!(!super::exists(&pool, unknown).await.unwrap(), "{unknown}");
        }
    }

    #[tokio::test]
    async fn the_next_fire_is_read_in_the_presets_zone() {
        let Some(pool) = pool("the_next_fire_is_read_in_the_presets_zone").await else { return };
        let friday = at(2026, 11, 20, 0, 0);
        let next = |tz: &'static str, expr: &'static str, after| {
            let pool = pool.clone();
            async move { next_fire_in(&pool, expr, tz, after).await.unwrap() }
        };
        assert_eq!(next("Asia/Tokyo", "0 7 * * 1-5", friday).await, Some(at(2026, 11, 22, 22, 0)));
        assert_eq!(next("UTC", "0 7 * * 1-5", friday).await, Some(at(2026, 11, 20, 7, 0)));
        assert_eq!(
            next("America/New_York", "30 2 * * *", at(2026, 3, 7, 8, 0)).await,
            Some(at(2026, 3, 8, 7, 30))
        );
        assert_eq!(
            next("America/New_York", "0 0 29 2 *", at(2026, 3, 1, 0, 0)).await,
            Some(at(2028, 2, 29, 5, 0)),
            "a far fire is recomputed over a window that reaches it"
        );
        assert_eq!(next("Europe/Paris", "0 0 30 2 *", friday).await, None);
    }
}
