//! Pure pieces of the session-diagnose aggregation (CCT-547).
//!
//! The glue that reads `Driver` state lives in `control.rs` (the fields are
//! private to that module); everything decision-shaped is here so it can be
//! unit-tested without a driver.

use std::time::{SystemTime, UNIX_EPOCH};

use cctui_proto::adapter::AdapterEvent;

/// The posture label recorded at spawn/fork time and surfaced by diagnose:
/// the coarse `default`/`auto`/`yolo` vocabulary, except whip keeps its own
/// name (its enforcement hooks are diagnostic signal, CCT-352).
pub(super) const fn permission_label(mode: cctui_proto::adapter::PermissionMode) -> &'static str {
    if mode.is_whip() { "whip" } else { mode.normalized_label() }
}

/// Coarse kind label for a tailed transcript event ("last parsed event" in
/// the diagnose report).
pub(super) const fn event_kind(event: &AdapterEvent) -> &'static str {
    match event {
        AdapterEvent::SessionStarted { .. } => "session_started",
        AdapterEvent::Message { .. } => "message",
        AdapterEvent::ToolUse { .. } => "tool_use",
        AdapterEvent::SessionEnded { .. } => "session_ended",
        AdapterEvent::Status { .. } => "status",
        AdapterEvent::TokenUsage { .. } => "token_usage",
        AdapterEvent::SessionModel { .. } => "session_model",
        _ => "other",
    }
}

/// `SystemTime` → unix epoch millis (0 for a pre-epoch clock).
pub(super) fn to_unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub(super) fn now_unix_ms() -> i64 {
    to_unix_ms(SystemTime::now())
}

/// The raw inputs the effective-state arbitration weighs, in one struct so a
/// caller cannot transpose the bools. The bools ARE the domain here — each is
/// an independent observed signal, not a state machine to encode.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub(super) struct ArbitrationInput<'a> {
    /// An ask/plan form is up in the worker PTY (hook signal, CCT-219).
    pub pending_ask: bool,
    /// A blocking `PreToolUse` permission hook is parked (hook signal,
    /// CCT-342).
    pub parked_perm_hook: bool,
    /// The control socket's `needs` string for a pending permission prompt
    /// (CCT-211).
    pub control_needs: Option<&'a str>,
    /// claude reports the worker dead-but-still-listed (CCT-252).
    pub reported_dead: bool,
    /// The short is in the current live roster.
    pub in_roster: bool,
    /// On-disk job state survives (a non-listed worker is revivable —
    /// hibernated, CCT-228).
    pub state_json_on_disk: bool,
    /// Last control-socket snapshot, when one was observed.
    pub tempo: Option<&'a str>,
    pub state: Option<&'a str>,
}

/// Which input produced the effective state. Matches the ticket's
/// "hook vs activity vs timeout" vocabulary: hook signals win, then
/// control-socket activity, and `timeout` means nothing has been observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerdictSource {
    Hook,
    Activity,
    Timeout,
}

impl VerdictSource {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Activity => "activity",
            Self::Timeout => "timeout",
        }
    }
}

/// Arbitrate the session's effective state from the raw signals. Priority:
///   1. hook-delivered prompts (an up form / parked permission hook) — these
///      are invisible to the control socket, which reports `done` while a
///      form is pending (see CCT-167), so they must win;
///   2. the control socket's own blocked/`needs` signal;
///   3. dead / off-roster lifecycle (dead → hibernated/ended);
///   4. the last status snapshot (`tempo`/`state`);
///   5. nothing observed → unknown (`timeout` source).
pub(super) fn arbitrate(input: &ArbitrationInput<'_>) -> (String, VerdictSource) {
    if input.pending_ask {
        return ("blocked: awaiting ask answer".to_owned(), VerdictSource::Hook);
    }
    if input.parked_perm_hook {
        return ("blocked: awaiting permission decision".to_owned(), VerdictSource::Hook);
    }
    if let Some(needs) = input.control_needs {
        return (format!("blocked: {needs}"), VerdictSource::Activity);
    }
    if input.reported_dead || !input.in_roster {
        let verdict = if input.state_json_on_disk {
            "hibernated (worker exited; revivable on reply)"
        } else if input.reported_dead {
            "dead (claude reports process gone)"
        } else {
            "ended (not in live roster, no job state on disk)"
        };
        return (verdict.to_owned(), VerdictSource::Activity);
    }
    match (input.tempo, input.state) {
        (None, None) => ("unknown (no status observed yet)".to_owned(), VerdictSource::Timeout),
        (tempo, state) => {
            (format!("{}/{}", tempo.unwrap_or("?"), state.unwrap_or("?")), VerdictSource::Activity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live<'a>() -> ArbitrationInput<'a> {
        ArbitrationInput {
            in_roster: true,
            state_json_on_disk: true,
            tempo: Some("active"),
            state: Some("working"),
            ..Default::default()
        }
    }

    #[test]
    fn hook_signals_win_over_activity() {
        // A pending ask form reports state:"done" on the control socket
        // (CCT-167) — the hook signal must produce the verdict.
        let input = ArbitrationInput { pending_ask: true, ..live() };
        let (verdict, source) = arbitrate(&input);
        assert_eq!(source, VerdictSource::Hook);
        assert!(verdict.contains("ask"), "{verdict}");

        let input = ArbitrationInput { parked_perm_hook: true, ..live() };
        let (verdict, source) = arbitrate(&input);
        assert_eq!(source, VerdictSource::Hook);
        assert!(verdict.contains("permission"), "{verdict}");

        // Ask outranks the parked permission hook when both are set.
        let input = ArbitrationInput { pending_ask: true, parked_perm_hook: true, ..live() };
        assert!(arbitrate(&input).0.contains("ask"));
    }

    #[test]
    fn control_needs_beats_snapshot() {
        let input = ArbitrationInput { control_needs: Some("approve Bash: rm -rf /"), ..live() };
        let (verdict, source) = arbitrate(&input);
        assert_eq!(source, VerdictSource::Activity);
        assert_eq!(verdict, "blocked: approve Bash: rm -rf /");
    }

    #[test]
    fn dead_and_off_roster_map_to_lifecycle_verdicts() {
        // Dead-but-listed with job state on disk → hibernated (CCT-252/228).
        let input = ArbitrationInput { reported_dead: true, ..live() };
        assert!(arbitrate(&input).0.starts_with("hibernated"));

        // Dead with no job state → dead.
        let input = ArbitrationInput { reported_dead: true, state_json_on_disk: false, ..live() };
        assert!(arbitrate(&input).0.starts_with("dead"));

        // Off roster, job state survives → hibernated.
        let input = ArbitrationInput { in_roster: false, ..live() };
        assert!(arbitrate(&input).0.starts_with("hibernated"));

        // Off roster, nothing on disk → ended.
        let input = ArbitrationInput { in_roster: false, state_json_on_disk: false, ..live() };
        assert!(arbitrate(&input).0.starts_with("ended"));
    }

    #[test]
    fn snapshot_produces_activity_verdict() {
        let (verdict, source) = arbitrate(&live());
        assert_eq!(source, VerdictSource::Activity);
        assert_eq!(verdict, "active/working");

        // Partial snapshots still verbalize.
        let input = ArbitrationInput { state: None, ..live() };
        assert_eq!(arbitrate(&input).0, "active/?");
    }

    #[test]
    fn nothing_observed_is_timeout_source() {
        let input =
            ArbitrationInput { in_roster: true, state_json_on_disk: true, ..Default::default() };
        let (verdict, source) = arbitrate(&input);
        assert_eq!(source, VerdictSource::Timeout);
        assert!(verdict.starts_with("unknown"));
    }

    #[test]
    fn unix_ms_conversion_is_sane() {
        let ms = to_unix_ms(UNIX_EPOCH + std::time::Duration::from_millis(1234));
        assert_eq!(ms, 1234);
        assert!(now_unix_ms() > 1_700_000_000_000, "clock should be past 2023");
    }
}
