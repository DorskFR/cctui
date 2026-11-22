use sqlx::PgConnection;

use super::prefs::{QuietHours, parse_hhmm};

const DAY_MINUTES: u32 = 24 * 60;
pub const BYPASS_SEVERITY: &str = "critical";

#[must_use]
pub fn holds(severity: &str) -> bool {
    severity != BYPASS_SEVERITY
}

#[must_use]
pub const fn hold_minutes(local_minute: u32, start: u32, end: u32) -> u32 {
    let inside = if start < end {
        local_minute >= start && local_minute < end
    } else {
        start != end && (local_minute >= start || local_minute < end)
    };
    if inside { (end + DAY_MINUTES - local_minute) % DAY_MINUTES } else { 0 }
}

/// An unknown zone reads as UTC: settings validate it, and an error here would abort the emit.
const LOCAL_MINUTE_SQL: &str = "\
    SELECT (EXTRACT(HOUR FROM t) * 60 + EXTRACT(MINUTE FROM t))::int \
    FROM (SELECT (now() + make_interval(secs => $1)) AT TIME ZONE \
            COALESCE((SELECT name FROM pg_timezone_names WHERE name = $2), 'UTC') AS t) local";

pub async fn local_minute(conn: &mut PgConnection, delay_secs: i32, tz: &str) -> sqlx::Result<u32> {
    let minute: i32 =
        sqlx::query_scalar(LOCAL_MINUTE_SQL).bind(delay_secs).bind(tz).fetch_one(conn).await?;
    Ok(minute.unsigned_abs())
}

pub async fn hold(conn: &mut PgConnection, q: &QuietHours, delay_secs: i32) -> sqlx::Result<i32> {
    let (Some(start), Some(end)) = (parse_hhmm(&q.start), parse_hhmm(&q.end)) else {
        return Ok(delay_secs);
    };
    let at = local_minute(conn, delay_secs, &q.tz).await?;
    let extra = i32::try_from(hold_minutes(at, start, end)).unwrap_or(0) * 60;
    Ok(delay_secs.saturating_add(extra))
}

#[cfg(test)]
mod tests {
    use super::{hold_minutes, holds};
    use crate::notify::prefs::parse_hhmm;

    fn hm(raw: &str) -> u32 {
        parse_hhmm(raw).unwrap()
    }

    #[test]
    fn an_overnight_window_holds_until_its_end_across_midnight() {
        let (start, end) = (hm("22:00"), hm("07:00"));
        assert_eq!(hold_minutes(hm("23:30"), start, end), 7 * 60 + 30);
        assert_eq!(hold_minutes(hm("00:00"), start, end), 7 * 60);
        assert_eq!(hold_minutes(hm("06:59"), start, end), 1);
        assert_eq!(hold_minutes(hm("07:00"), start, end), 0, "the end is outside");
        assert_eq!(hold_minutes(hm("21:59"), start, end), 0);
        assert_eq!(hold_minutes(hm("22:00"), start, end), 9 * 60, "the start is inside");
    }

    #[test]
    fn a_daytime_window_holds_only_inside_it() {
        let (start, end) = (hm("12:00"), hm("14:00"));
        assert_eq!(hold_minutes(hm("13:00"), start, end), 60);
        assert_eq!(hold_minutes(hm("11:00"), start, end), 0);
        assert_eq!(hold_minutes(hm("15:00"), start, end), 0);
    }

    #[test]
    fn an_empty_window_never_holds() {
        assert_eq!(hold_minutes(hm("09:00"), hm("09:00"), hm("09:00")), 0);
        assert_eq!(hold_minutes(hm("10:00"), hm("09:00"), hm("09:00")), 0);
    }

    #[test]
    fn only_critical_bypasses_quiet_hours() {
        assert!(!holds("critical"));
        for severity in ["info", "high", "low", "min"] {
            assert!(holds(severity), "{severity}");
        }
    }

    #[test]
    fn hhmm_parsing_rejects_out_of_range_and_loose_forms() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("23:59"), Some(23 * 60 + 59));
        for bad in ["24:00", "12:60", "7:00", "07:0", "0700", "ab:cd", ""] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn local_minute_follows_the_time_zone_and_falls_back_to_utc() {
        let Some(url) = cctui_server_kernel::test_support::db_url("notify_quiet_tz") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let mut conn = pool.acquire().await.unwrap();
        let utc = super::local_minute(&mut conn, 0, "UTC").await.unwrap();
        let tokyo = super::local_minute(&mut conn, 0, "Asia/Tokyo").await.unwrap();
        let unknown = super::local_minute(&mut conn, 0, "Mars/Olympus").await.unwrap();
        let shifted = |m: u32| (m + 9 * 60) % (24 * 60);
        assert!(tokyo == shifted(utc) || tokyo == shifted(utc + 1), "{utc} {tokyo}");
        assert!(unknown == utc || unknown == (utc + 1) % (24 * 60), "{utc} {unknown}");
        let later = super::local_minute(&mut conn, 3600, "UTC").await.unwrap();
        assert!(later == (utc + 60) % (24 * 60) || later == (utc + 61) % (24 * 60));
    }

    #[tokio::test]
    async fn hold_moves_a_delay_inside_the_window_to_its_end() {
        let Some(url) = cctui_server_kernel::test_support::db_url("notify_quiet_hold") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let mut conn = pool.acquire().await.unwrap();
        let now = super::local_minute(&mut conn, 0, "UTC").await.unwrap();
        let fmt = |m: u32| format!("{:02}:{:02}", (m / 60) % 24, m % 60);
        let around = crate::notify::prefs::QuietHours {
            start: fmt((now + 24 * 60 - 60) % (24 * 60)),
            end: fmt((now + 120) % (24 * 60)),
            tz: "UTC".into(),
        };
        let held = super::hold(&mut conn, &around, 0).await.unwrap();
        assert!((115 * 60..=120 * 60).contains(&held), "{held}");
        let elsewhere = crate::notify::prefs::QuietHours {
            start: fmt((now + 180) % (24 * 60)),
            end: fmt((now + 240) % (24 * 60)),
            tz: "UTC".into(),
        };
        assert_eq!(super::hold(&mut conn, &elsewhere, 0).await.unwrap(), 0);
    }
}
