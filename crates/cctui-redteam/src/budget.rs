//! Per-run and per-scenario spend caps for agent scenarios.

use serde::{Deserialize, Serialize};

const MIN_VIABLE_USD: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutOff {
    Budget,
    Turns,
    WallClock,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScenarioBudget {
    pub usd: f64,
    pub max_turns: Option<u32>,
    pub timeout_secs: u64,
}

impl ScenarioBudget {
    #[must_use]
    pub fn check(&self, cost: f64, turns: u32, elapsed_secs: u64) -> Option<CutOff> {
        if cost >= self.usd {
            Some(CutOff::Budget)
        } else if self.max_turns.is_some_and(|m| turns >= m) {
            Some(CutOff::Turns)
        } else if elapsed_secs >= self.timeout_secs {
            Some(CutOff::WallClock)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Admission {
    Admitted(f64),
    Exhausted,
}

/// Run-wide ledger: each scenario is admitted with a cap no larger than what is left.
#[derive(Debug, Clone, PartialEq)]
pub struct Ledger {
    total: f64,
    per_scenario: f64,
    spent: f64,
}

impl Ledger {
    #[must_use]
    pub const fn new(total_usd: f64, per_scenario_usd: f64) -> Self {
        Self { total: total_usd, per_scenario: per_scenario_usd, spent: 0.0 }
    }

    #[must_use]
    pub const fn spent(&self) -> f64 {
        self.spent
    }

    #[must_use]
    pub const fn remaining(&self) -> f64 {
        (self.total - self.spent).max(0.0)
    }

    /// Admit a scenario asking for `requested`, capped by the per-scenario and remaining budget.
    #[must_use]
    pub const fn admit(&self, requested: f64) -> Admission {
        let cap = requested.min(self.per_scenario).min(self.remaining());
        if cap.is_finite() && cap >= MIN_VIABLE_USD {
            Admission::Admitted(cap)
        } else {
            Admission::Exhausted
        }
    }

    /// Charge what a scenario actually cost; returns whether it overran its cap.
    pub const fn settle(&mut self, cap: f64, actual: f64) -> bool {
        let actual = if actual.is_finite() { actual.max(0.0) } else { cap };
        self.spent += actual;
        actual > cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ledger_caps_each_scenario_and_cuts_off_when_the_run_is_spent() {
        let mut ledger = Ledger::new(1.0, 0.4);
        assert_eq!(ledger.admit(0.5), Admission::Admitted(0.4));
        assert!(!ledger.settle(0.4, 0.4));
        assert_eq!(ledger.admit(0.3), Admission::Admitted(0.3));
        assert!(!ledger.settle(0.3, 0.3));
        let Admission::Admitted(last) = ledger.admit(1.0) else { panic!("0.3 left") };
        assert!((last - 0.3).abs() < 1e-9);
        assert!(!ledger.settle(last, last));
        assert_eq!(ledger.admit(1.0), Admission::Exhausted);
        assert!(ledger.remaining() < 1e-9);
    }

    #[test]
    fn an_overrun_is_reported_and_charged() {
        let mut ledger = Ledger::new(1.0, 1.0);
        assert!(ledger.settle(0.2, 0.25));
        assert!((ledger.spent() - 0.25).abs() < 1e-9);
        assert!(!ledger.settle(0.2, f64::NAN));
        assert!((ledger.spent() - 0.45).abs() < 1e-9);
    }

    #[test]
    fn a_zero_or_tiny_remaining_budget_admits_nothing() {
        assert_eq!(Ledger::new(0.0, 1.0).admit(1.0), Admission::Exhausted);
        assert_eq!(Ledger::new(0.01, 1.0).admit(1.0), Admission::Exhausted);
        assert_eq!(Ledger::new(5.0, 1.0).admit(0.0), Admission::Exhausted);
    }

    #[test]
    fn a_scenario_budget_reports_the_first_limit_reached() {
        let b = ScenarioBudget { usd: 1.0, max_turns: Some(30), timeout_secs: 600 };
        assert_eq!(b.check(0.5, 10, 100), None);
        assert_eq!(b.check(1.0, 10, 100), Some(CutOff::Budget));
        assert_eq!(b.check(0.5, 30, 100), Some(CutOff::Turns));
        assert_eq!(b.check(0.5, 10, 600), Some(CutOff::WallClock));
        let unbounded_turns = ScenarioBudget { max_turns: None, ..b };
        assert_eq!(unbounded_turns.check(0.1, 10_000, 1), None);
    }
}
