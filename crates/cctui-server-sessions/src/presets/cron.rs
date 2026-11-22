use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};

const FIELDS: [(&str, u32, u32); 5] = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day of month", 1, 31),
    ("month", 1, 12),
    ("day of week", 0, 7),
];

/// Long enough to reach the next Feb 29 from any date.
const HORIZON_DAYS: i64 = 4 * 366 + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    any_day: bool,
    any_weekday: bool,
}

fn number(raw: &str, field: &str, min: u32, max: u32) -> Result<u32, String> {
    let n: u32 = raw.parse().map_err(|_| format!("cron {field}: '{raw}' is not a number"))?;
    if (min..=max).contains(&n) {
        Ok(n)
    } else {
        Err(format!("cron {field}: {n} is outside {min}-{max}"))
    }
}

fn item(raw: &str, field: &str, min: u32, max: u32) -> Result<u64, String> {
    let (range, step) = raw.split_once('/').map_or((raw, None), |(r, s)| (r, Some(s)));
    let step = match step {
        None => 1,
        Some(step) => {
            let step: u32 =
                step.parse().map_err(|_| format!("cron {field}: bad step in '{raw}'"))?;
            if step == 0 || step > max {
                return Err(format!("cron {field}: step {step} is outside 1-{max}"));
            }
            step
        }
    };
    let (lo, hi) = if range == "*" {
        (min, max)
    } else if let Some((lo, hi)) = range.split_once('-') {
        let (lo, hi) = (number(lo, field, min, max)?, number(hi, field, min, max)?);
        if lo > hi {
            return Err(format!("cron {field}: range {lo}-{hi} runs backwards"));
        }
        (lo, hi)
    } else {
        let n = number(range, field, min, max)?;
        (n, if step > 1 { max } else { n })
    };
    Ok((lo..=hi).filter(|n| (n - lo) % step == 0).fold(0, |bits, n| bits | (1u64 << n)))
}

fn field(part: &str, field: &str, min: u32, max: u32) -> Result<u64, String> {
    part.split(',').try_fold(0, |bits, raw| {
        if raw.is_empty() {
            return Err(format!("cron {field}: empty list item in '{part}'"));
        }
        Ok(bits | item(raw, field, min, max)?)
    })
}

/// Day of week 0 and 7 are Sunday. As in cron, a day of month and a day of week
/// that both start with a number match either; otherwise both must match.
pub fn parse(expr: &str) -> Result<Schedule, String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != FIELDS.len() {
        return Err(format!(
            "cron must have 5 fields (minute hour day-of-month month day-of-week), got {}",
            parts.len()
        ));
    }
    let mut bits = [0u64; 5];
    for ((slot, part), (name, min, max)) in bits.iter_mut().zip(&parts).zip(FIELDS) {
        *slot = field(part, name, min, max)?;
    }
    let [minutes, hours, days, months, weekdays] = bits;
    let sunday = u64::from(has(weekdays, 7));
    Ok(Schedule {
        minutes,
        hours,
        days,
        months,
        weekdays: (weekdays | sunday) & 0x7f,
        any_day: parts[2].starts_with('*'),
        any_weekday: parts[4].starts_with('*'),
    })
}

pub fn validate(expr: &str) -> Result<(), String> {
    parse(expr)?
        .next_after(Utc::now())
        .map(|_| ())
        .ok_or_else(|| format!("cron '{expr}' never fires"))
}

const fn has(bits: u64, n: u32) -> bool {
    (bits & (1u64 << n)) != 0
}

impl Schedule {
    fn day_matches(&self, date: NaiveDate) -> bool {
        if !has(self.months, date.month()) {
            return false;
        }
        let day = has(self.days, date.day());
        let weekday = has(self.weekdays, date.weekday().num_days_from_sunday());
        if self.any_day || self.any_weekday { day && weekday } else { day || weekday }
    }

    fn first_time(&self, from_hour: u32, from_minute: u32) -> Option<(u32, u32)> {
        (from_hour..24).filter(|&h| has(self.hours, h)).find_map(|h| {
            let start = if h == from_hour { from_minute } else { 0 };
            (start..60).find(|&m| has(self.minutes, m)).map(|m| (h, m))
        })
    }

    /// `None` when nothing fires within the horizon, e.g. Feb 30.
    #[must_use]
    pub fn next_after_naive(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let start = after.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let first = start.date();
        (0..HORIZON_DAYS).find_map(|offset| {
            let date = first + Duration::days(offset);
            if !self.day_matches(date) {
                return None;
            }
            let (from_hour, from_minute) =
                if offset == 0 { (start.hour(), start.minute()) } else { (0, 0) };
            let (h, m) = self.first_time(from_hour, from_minute)?;
            date.and_hms_opt(h, m, 0)
        })
    }

    #[must_use]
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.next_after_naive(after.naive_utc()).map(|at| Utc.from_utc_datetime(&at))
    }

    /// An every-hour schedule runs by elapsed time across a clock change; one
    /// pinned to hours runs once per wall-clock time.
    #[must_use]
    pub const fn any_hour(&self) -> bool {
        self.hours == (1 << 24) - 1
    }
}

#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn next_fire(expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    parse(expr).ok()?.next_after(after)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{next_fire, validate};

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn accepts_the_common_shapes() {
        for expr in ["* * * * *", "0 9 * * 1-5", "*/15 8-18 * * *", "0,30 6 1 1,6 0", "5 4 * * 7"] {
            assert_eq!(validate(expr), Ok(()), "{expr}");
        }
    }

    #[test]
    fn rejects_malformed_expressions() {
        for expr in
            ["", "* * * *", "* * * * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * * 13 *"]
        {
            assert!(validate(expr).is_err(), "{expr}");
        }
        for expr in
            ["*/0 * * * *", "5-1 * * * *", "a * * * *", "1,,2 * * * *", "@daily", "0 0 30 2 *"]
        {
            assert!(validate(expr).is_err(), "{expr}");
        }
    }

    #[test]
    fn every_five_minutes_fires_on_the_next_multiple_strictly_after() {
        let expr = "*/5 * * * *";
        assert_eq!(next_fire(expr, at(2026, 11, 16, 9, 2, 30)), Some(at(2026, 11, 16, 9, 5, 0)));
        assert_eq!(next_fire(expr, at(2026, 11, 16, 9, 5, 0)), Some(at(2026, 11, 16, 9, 10, 0)));
        assert_eq!(next_fire(expr, at(2026, 11, 16, 23, 58, 0)), Some(at(2026, 11, 17, 0, 0, 0)));
    }

    #[test]
    fn daily_at_nine_rolls_to_tomorrow_once_nine_has_passed() {
        let expr = "0 9 * * *";
        assert_eq!(next_fire(expr, at(2026, 11, 16, 8, 59, 59)), Some(at(2026, 11, 16, 9, 0, 0)));
        assert_eq!(next_fire(expr, at(2026, 11, 16, 9, 0, 0)), Some(at(2026, 11, 17, 9, 0, 0)));
    }

    #[test]
    fn weekday_ranges_skip_the_weekend() {
        let expr = "30 8 * * 1-5";
        let friday_late = at(2026, 11, 20, 9, 0, 0);
        assert_eq!(next_fire(expr, friday_late), Some(at(2026, 11, 23, 8, 30, 0)), "monday");
        let sunday = "0 12 * * 7";
        assert_eq!(next_fire(sunday, at(2026, 11, 16, 0, 0, 0)), Some(at(2026, 11, 22, 12, 0, 0)));
        let monday = at(2026, 11, 16, 0, 0, 0);
        assert_eq!(next_fire("0 12 * * 0", monday), next_fire(sunday, monday));
    }

    #[test]
    fn month_and_year_roll_over() {
        assert_eq!(
            next_fire("0 0 1 * *", at(2026, 11, 16, 0, 0, 0)),
            Some(at(2026, 12, 1, 0, 0, 0))
        );
        assert_eq!(
            next_fire("0 0 31 * *", at(2026, 11, 16, 0, 0, 0)),
            Some(at(2026, 12, 31, 0, 0, 0)),
            "november has no 31st"
        );
        assert_eq!(
            next_fire("15 10 * 1 *", at(2026, 12, 31, 23, 59, 0)),
            Some(at(2027, 1, 1, 10, 15, 0))
        );
    }

    #[test]
    fn feb_29_waits_for_the_next_leap_year_and_feb_30_never_fires() {
        assert_eq!(
            next_fire("0 0 29 2 *", at(2026, 3, 1, 0, 0, 0)),
            Some(at(2028, 2, 29, 0, 0, 0))
        );
        assert_eq!(next_fire("0 0 30 2 *", at(2026, 3, 1, 0, 0, 0)), None);
    }

    #[test]
    fn a_restricted_day_of_month_and_weekday_match_either() {
        let expr = "0 0 13 * 5";
        assert_eq!(
            next_fire(expr, at(2026, 11, 16, 0, 0, 0)),
            Some(at(2026, 11, 20, 0, 0, 0)),
            "the friday comes before the 13th"
        );
        assert_eq!(
            next_fire("0 0 */10 * *", at(2026, 11, 16, 0, 0, 0)),
            Some(at(2026, 11, 21, 0, 0, 0)),
            "a stepped star still restricts the day: 1, 11, 21, 31"
        );
    }

    #[test]
    fn lists_steps_and_offsets_combine() {
        let expr = "10/20 6,18 * * *";
        assert_eq!(next_fire(expr, at(2026, 11, 16, 6, 30, 0)), Some(at(2026, 11, 16, 6, 50, 0)));
        assert_eq!(next_fire(expr, at(2026, 11, 16, 6, 50, 0)), Some(at(2026, 11, 16, 18, 10, 0)));
        assert_eq!(
            next_fire("0 8-18/5 * * *", at(2026, 11, 16, 13, 0, 0)),
            Some(at(2026, 11, 16, 18, 0, 0))
        );
    }
}
