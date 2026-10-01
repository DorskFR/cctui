//! The one age rule both clients read: how long ago something happened,
//! reduced to a unit and a count.
//!
//! The wording is the caller's, because the web UI translates it — so this
//! settles only which unit a duration falls in. An age carries no "ago": a
//! sentence that already says "last … ago" or "no frame for …" supplies its own
//! preposition, and appending one here is what produced "last 5m ago ago".

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgeUnit {
    Millis,
    Seconds,
    Minutes,
    Hours,
}

impl AgeUnit {
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Millis => "ms",
            Self::Seconds => "s",
            Self::Minutes => "m",
            Self::Hours => "h",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Age {
    pub unit: AgeUnit,
    pub value: i64,
}

#[must_use]
pub fn age(ms: i64) -> Age {
    let ms = ms.max(0);
    match ms {
        ms if ms < 1_000 => Age { unit: AgeUnit::Millis, value: ms },
        ms if ms < 60_000 => Age { unit: AgeUnit::Seconds, value: ms / 1_000 },
        ms if ms < 3_600_000 => Age { unit: AgeUnit::Minutes, value: ms / 60_000 },
        ms => Age { unit: AgeUnit::Hours, value: ms / 3_600_000 },
    }
}

/// `"2m"` — for a sentence that supplies its own preposition.
#[must_use]
pub fn age_bare(ms: i64) -> String {
    let a = age(ms);
    format!("{}{}", a.value, a.unit.suffix())
}

/// `"2m ago"` — for an age standing on its own.
#[must_use]
pub fn age_ago(ms: i64) -> String {
    format!("{} ago", age_bare(ms))
}

#[cfg(test)]
mod tests {
    use super::{AgeUnit, age, age_ago, age_bare};

    #[test]
    fn tiers_and_wording() {
        assert_eq!(age(0), super::Age { unit: AgeUnit::Millis, value: 0 });
        assert_eq!(age(-5).value, 0);
        assert_eq!(age_bare(500), "500ms");
        assert_eq!(age_bare(2_000), "2s");
        assert_eq!(age_bare(120_000), "2m");
        assert_eq!(age_bare(7_200_000), "2h");
        assert_eq!(age_ago(120_000), "2m ago");
    }
}
