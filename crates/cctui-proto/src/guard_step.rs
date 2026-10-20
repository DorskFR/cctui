//! The workflow guard's live step state, mirrored up from the worker pod.
//!
//! The guard daemon in the pod stays authoritative: cctui only mirrors what
//! `GET /state` reports and relays transition requests to it.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// The exit pseudo-step, as the guard persists it.
pub const STEP_EXITED: i64 = -1;

/// One snapshot of the pod's workflow step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct GuardStepState {
    /// `-1` once the session exited the workflow.
    pub step: i64,
    #[serde(default)]
    pub title: String,
    /// Raw `[allowed]` rule text.
    #[serde(default)]
    pub allowed: String,
    /// Raw `[disallowed]` rule text.
    #[serde(default)]
    pub disallowed: String,
    /// Numeric steps the guard accepts a transition to from here.
    #[serde(default)]
    pub transitions: Vec<u32>,
    /// Whether `Exit` is listed; the guard always accepts it regardless.
    #[serde(default)]
    pub exit: bool,
}

impl GuardStepState {
    #[must_use]
    pub const fn exited(&self) -> bool {
        self.step < 0
    }

    /// Every target a transition request may name: the listed steps, then
    /// `exit`, which the guard accepts from any step.
    #[must_use]
    pub fn targets(&self) -> Vec<String> {
        if self.exited() {
            return Vec::new();
        }
        let mut out: Vec<String> = self.transitions.iter().map(u32::to_string).collect();
        out.push("exit".to_owned());
        out
    }
}

/// A transition request's target: a step number or `exit`, as the guard's
/// `POST /transition` takes it.
#[must_use]
pub fn transition_target(step: &str) -> serde_json::Value {
    let step = step.trim();
    step.parse::<u32>().map_or_else(|_| serde_json::Value::String(step.to_owned()), Into::into)
}

/// Parse a guard `GET /state` body.
#[must_use]
pub fn parse_state(body: &serde_json::Value) -> Option<GuardStepState> {
    let step = body.get("step")?.as_i64()?;
    let text =
        |key: &str| body.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_owned();
    let transitions = body
        .get("transitions")
        .and_then(serde_json::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(serde_json::Value::as_u64)
                .filter_map(|n| u32::try_from(n).ok())
                .collect()
        })
        .unwrap_or_default();
    Some(GuardStepState {
        step,
        title: text("title"),
        allowed: text("allowed"),
        disallowed: text("disallowed"),
        transitions,
        exit: body.get("exit").and_then(serde_json::Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_guard_state_body_and_tolerates_old_daemons() {
        let full = parse_state(&json!({
            "step": 2, "title": "Implement", "allowed": "all-read, Bash",
            "disallowed": "remote-write", "transitions": [3, 5], "exit": true
        }))
        .unwrap();
        assert_eq!(full.step, 2);
        assert_eq!(full.title, "Implement");
        assert_eq!(full.transitions, vec![3, 5]);
        assert!(full.exit);
        assert_eq!(full.targets(), vec!["3", "5", "exit"]);

        let old = parse_state(&json!({ "step": 1, "title": "Research" })).unwrap();
        assert!(old.transitions.is_empty());
        assert!(!old.exit);
        assert_eq!(old.targets(), vec!["exit"], "exit is always a valid request");

        assert!(parse_state(&json!({ "error": "internal guard error" })).is_none());
    }

    #[test]
    fn an_exited_session_offers_no_targets() {
        let state = parse_state(&json!({ "step": STEP_EXITED, "title": "unknown" })).unwrap();
        assert!(state.exited());
        assert!(state.targets().is_empty());
    }

    #[test]
    fn a_transition_target_is_a_number_or_the_exit_word() {
        assert_eq!(transition_target(" 3 "), json!(3));
        assert_eq!(transition_target("exit"), json!("exit"));
        assert_eq!(transition_target("Exit"), json!("Exit"));
    }

    #[test]
    fn state_roundtrips_json() {
        let state = GuardStepState {
            step: 4,
            title: "Ship".into(),
            transitions: vec![5],
            ..Default::default()
        };
        let back: GuardStepState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);
    }
}
