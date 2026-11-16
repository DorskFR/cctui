const FIELDS: [(&str, u32, u32); 5] = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day of month", 1, 31),
    ("month", 1, 12),
    ("day of week", 0, 7),
];

fn number(raw: &str, field: &str, min: u32, max: u32) -> Result<u32, String> {
    let n: u32 = raw.parse().map_err(|_| format!("cron {field}: '{raw}' is not a number"))?;
    if (min..=max).contains(&n) {
        Ok(n)
    } else {
        Err(format!("cron {field}: {n} is outside {min}-{max}"))
    }
}

fn item(raw: &str, field: &str, min: u32, max: u32) -> Result<(), String> {
    let (range, step) = raw.split_once('/').map_or((raw, None), |(r, s)| (r, Some(s)));
    if let Some(step) = step {
        let step: u32 = step.parse().map_err(|_| format!("cron {field}: bad step in '{raw}'"))?;
        if step == 0 || step > max {
            return Err(format!("cron {field}: step {step} is outside 1-{max}"));
        }
    }
    if range == "*" {
        return Ok(());
    }
    let Some((lo, hi)) = range.split_once('-') else {
        return number(range, field, min, max).map(|_| ());
    };
    let (lo, hi) = (number(lo, field, min, max)?, number(hi, field, min, max)?);
    if lo > hi {
        return Err(format!("cron {field}: range {lo}-{hi} runs backwards"));
    }
    Ok(())
}

/// A five-field numeric cron expression (`*`, `n`, `a-b`, `/step`, `,` lists).
pub fn validate(expr: &str) -> Result<(), String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != FIELDS.len() {
        return Err(format!(
            "cron must have 5 fields (minute hour day-of-month month day-of-week), got {}",
            parts.len()
        ));
    }
    for (part, (field, min, max)) in parts.iter().zip(FIELDS) {
        for raw in part.split(',') {
            if raw.is_empty() {
                return Err(format!("cron {field}: empty list item in '{part}'"));
            }
            item(raw, field, min, max)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;

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
        for expr in ["*/0 * * * *", "5-1 * * * *", "a * * * *", "1,,2 * * * *", "@daily"] {
            assert!(validate(expr).is_err(), "{expr}");
        }
    }
}
