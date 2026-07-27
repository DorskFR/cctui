//! Session diagnose report.
//!
//! One structured blob answering "everything the daemon knows about this
//! session, dated": every input to the session's derived state, each carried
//! as a [`DiagnoseFact`] with a value, an `observed_at_ms`/`age_ms` pair and a
//! `source`, plus the arbitration verdict. Facts the daemon cannot produce
//! right now come back with `value: None` and a `missing_reason` instead of
//! failing the whole call (fail-soft).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One dated fact in a [`SessionDiagnose`] report.
///
/// `value: None` + `missing_reason` means the daemon could not produce the
/// fact right now; the call as a whole still succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiagnoseFact<T> {
    // Fn-path default: `#[serde(default)]` would bound `T: Default` on the
    // generic derive; a named fn keeps `DiagnoseFact<T>` bound-free.
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    /// Unix epoch millis when the daemon last observed this fact. `None`
    /// when the underlying signal carries no timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    /// Staleness at report-build time: `generated_at_ms - observed_at_ms`,
    /// clamped to `>= 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<i64>,
    /// Which input/subsystem produced the fact (e.g. `hook`,
    /// `control_socket`, `discovery`, `filesystem`).
    pub source: String,
    /// Why `value` is absent. Always `Some` when `value` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_reason: Option<String>,
}

/// serde needs a named fn (not `#[serde(default)]`) for a generic
/// `Option<T>` default that must not require `T: Default`.
const fn none<T>() -> Option<T> {
    None
}

/// `now - observed`, clamped to `>= 0` (a clock skew must not surface as a
/// negative age).
#[must_use]
pub const fn staleness_ms(now_ms: i64, observed_at_ms: i64) -> i64 {
    let age = now_ms - observed_at_ms;
    if age < 0 { 0 } else { age }
}

impl<T> DiagnoseFact<T> {
    /// A fact observed at `observed_at_ms`, aged against `now_ms`.
    #[must_use]
    pub fn observed(value: T, source: &str, observed_at_ms: i64, now_ms: i64) -> Self {
        Self {
            value: Some(value),
            observed_at_ms: Some(observed_at_ms),
            age_ms: Some(staleness_ms(now_ms, observed_at_ms)),
            source: source.to_owned(),
            missing_reason: None,
        }
    }

    /// A fact produced right now (probe at report-build time).
    #[must_use]
    pub fn fresh(value: T, source: &str, now_ms: i64) -> Self {
        Self::observed(value, source, now_ms, now_ms)
    }

    /// A fact carrying a value but no usable timestamp.
    #[must_use]
    pub fn undated(value: T, source: &str) -> Self {
        Self {
            value: Some(value),
            observed_at_ms: None,
            age_ms: None,
            source: source.to_owned(),
            missing_reason: None,
        }
    }

    /// The fail-soft shape: no value, an explicit reason.
    #[must_use]
    pub fn missing(source: &str, reason: &str) -> Self {
        Self {
            value: None,
            observed_at_ms: None,
            age_ms: None,
            source: source.to_owned(),
            missing_reason: Some(reason.to_owned()),
        }
    }
}

/// The arbitration output: the session's effective state and the raw signals
/// it was derived from. The wrapping fact's `source` says which input won
/// (`hook` / `activity` / `timeout`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EffectiveState {
    /// Human-readable verdict, e.g. `awaiting ask answer`,
    /// `blocked: approve Bash: …`, `active/working`, `hibernated`, `dead`.
    pub verdict: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
}

/// The most recent ask/permission/plan hook delivery for the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookEvent {
    /// Hook line kind: `ask`, `resolved`, `plan`, `plan_resolved`,
    /// `perm-request`.
    pub kind: String,
}

/// Persistent-attach keep-alive status (487) for the session's worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AttachStatus {
    /// `held` (attached, socket open), `reconnecting` (backoff between
    /// attempts), or `connecting` (task exists, no cycle finished yet).
    pub phase: String,
    /// Current reconnect backoff in ms, when `phase == "reconnecting"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
    /// Outcome of the last liveness probe (`has`): `Some(true)` alive,
    /// `Some(false)` found dead, `None` no probe yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_alive: Option<bool>,
    /// When the last liveness probe ran (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_at_ms: Option<i64>,
}

/// PTY output freshness/throughput sensed by the held-attach drain loop:
/// the second, hook-independent activity signal. `missing` until
/// the drain loop has read bytes for the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PtyOutputStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_age_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_bytes_per_min: Option<f64>,
}

/// Which `claude daemon` control socket discovery picked, and whether it
/// answered a live probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SocketStatus {
    /// The live socket path, `None` when no candidate answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub live: bool,
    /// Every candidate `control.sock` discovery enumerated.
    pub candidates: Vec<String>,
}

/// The pinned transcript file for the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TranscriptStatus {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Byte offset the tail has consumed up to.
    pub tail_offset: u64,
    /// Kind of the last event parsed out of the tail (e.g. `message`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_parsed_event: Option<String>,
    /// When that event was parsed (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_parsed_at_ms: Option<i64>,
}

/// Live ask/permission prompt state for the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingPrompts {
    /// An `AskUserQuestion`/plan form is up in the worker PTY.
    pub pending_ask: bool,
    /// A blocking `PreToolUse` permission hook is parked awaiting a decision.
    pub parked_perm_hook: bool,
    /// The control socket's `needs` string for a pending tool-permission
    /// prompt, when one is up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_needs: Option<String>,
    /// The synthesized `request_id` of the pending permission prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perm_request_id: Option<String>,
}

/// Dispatched-pod turn-complete watcher state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchStatus {
    pub seen_busy: bool,
    pub done: bool,
    pub marker_path: String,
}

/// What the daemon knows about gateway routing for this session. The
/// authoritative account binding lives server-side (see
/// [`SessionDiagnoseResponse::server`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GatewayStatus {
    /// Whether the daemon has an authenticated server client + machine key
    /// for the launch-time gateway-env pull.
    pub server_configured: bool,
}

/// Codex-adapter-specific diagnostics.
///
/// Present only when the session is driven by the codex adapter; `None` for
/// claude-code, whose facts are the neutral top-level fields instead. Kept as
/// an optional tagged section so the claude wire shape stays unchanged
/// (additive-only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodexDiagnose {
    /// Discovered `codex app-server` version (from the `initialize` userAgent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_version: Option<String>,
    /// The Codex version cctui is built/tested against (`CODEX_PINNED_VERSION`).
    pub pinned_version: String,
    /// The minimum app-server protocol version still spoken (`CODEX_MIN_VERSION`).
    pub min_version: String,
    /// Whether the discovered version is at or above `min_version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_supported: Option<bool>,
    /// Transport to the app-server child (always `stdio` today).
    pub transport: String,
    /// app-server child PID, when a live session owns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_server_pid: Option<u32>,
    /// Whether a live command channel exists for this session.
    pub live: bool,
    /// Whether the durable session registry holds a resumable record.
    pub registered: bool,
    /// The active thread id (the codex `local_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// In-flight turn id, when a turn is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    /// Turn status: `working` (a turn is in flight) or `idle`.
    pub turn_status: String,
    /// Count of outstanding outbound JSON-RPC requests.
    pub pending_rpc_count: u32,
    /// Methods of the outstanding JSON-RPC requests.
    pub pending_rpc_methods: Vec<String>,
    /// Last JSON-RPC protocol error seen on this session, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_protocol_error: Option<String>,
    /// Rollout (transcript) file path for the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_path: Option<String>,
    /// Rollout file size in bytes at report time — the tail-offset analogue for
    /// an app-server-owned rollout (no external tail consumes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_size_bytes: Option<u64>,
    /// Cheap auth/account posture: whether the launch env carries gateway
    /// routing credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_state: Option<String>,
    /// Registry↔live mismatch description, when the two disagree abnormally
    /// (e.g. a live channel with no durable record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_live_mismatch: Option<String>,
}

/// Everything the daemon knows about one session, dated. Assembled
/// by the adapter from state it already tracks — aggregation, not new sensing.
///
/// The named facts below are the adapter-neutral / claude-code set. Adapters
/// with their own diagnostics attach an optional tagged section (currently
/// [`SessionDiagnose::codex`]); this keeps the claude wire shape stable while
/// letting each adapter carry its own payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDiagnose {
    /// The stable session id the server keys on.
    pub local_id: String,
    /// The 8-hex worker shortcode, when resolvable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    /// When the daemon assembled this report (unix ms). Ages are relative to
    /// this instant.
    pub generated_at_ms: i64,
    pub adapter: String,
    /// Effective state + arbitration verdict; the fact's `source` says which
    /// input produced it (`hook` vs `activity` vs `timeout`).
    pub effective_state: DiagnoseFact<EffectiveState>,
    pub last_hook_event: DiagnoseFact<HookEvent>,
    pub attach: DiagnoseFact<AttachStatus>,
    /// Held-attach PTY output age/throughput; the second activity
    /// signal, hook-independent.
    pub pty_output: DiagnoseFact<PtyOutputStats>,
    pub claude_socket: DiagnoseFact<SocketStatus>,
    pub transcript: DiagnoseFact<TranscriptStatus>,
    pub prompts: DiagnoseFact<PendingPrompts>,
    /// Launch permission posture (`default`/`auto`/`yolo`/`whip`), when the
    /// daemon recorded it at spawn time.
    pub permission_mode: DiagnoseFact<String>,
    pub dispatch: DiagnoseFact<DispatchStatus>,
    pub gateway: DiagnoseFact<GatewayStatus>,
    /// Codex-adapter-specific section; `None` for claude-code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexDiagnose>,
}

/// Server-side facts merged into the diagnose response (the daemon cannot see
/// its own DB row or the gateway token bindings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerDiagnose {
    /// `sessions.status` (`active`/`ended`/`archived`/…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    /// Whether a live (non-revoked) gateway session token binds this session
    /// to an account.
    pub account_bound: bool,
    /// Names of the bound account(s), one per provider family.
    pub accounts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// `machines.last_seen_at` as unix ms — the daemon heartbeat freshness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_last_seen_ms: Option<i64>,
}

/// Body of `GET /api/v1/sessions/{id}/diagnose`. Fail-soft: when the daemon
/// round-trip fails (offline, timeout) `daemon` is `None` and `daemon_error`
/// says why; the server facts are still served.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDiagnoseResponse {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<SessionDiagnose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_error: Option<String>,
    pub server: ServerDiagnose,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> SessionDiagnose {
        SessionDiagnose {
            local_id: "6e189420-f9a4-493f-b3d9-e0a80ac254c1".into(),
            short: Some("6e189420".into()),
            generated_at_ms: 1_700_000_100_000,
            adapter: "claude-code".into(),
            effective_state: DiagnoseFact::observed(
                EffectiveState {
                    verdict: "active/working".into(),
                    tempo: Some("active".into()),
                    state: Some("working".into()),
                    detail: Some("running tests".into()),
                    activity: None,
                },
                "activity",
                1_700_000_099_000,
                1_700_000_100_000,
            ),
            last_hook_event: DiagnoseFact::missing("hook", "no hook delivery seen"),
            attach: DiagnoseFact::undated(
                AttachStatus {
                    phase: "held".into(),
                    backoff_ms: None,
                    last_probe_alive: Some(true),
                    last_probe_at_ms: Some(1_700_000_090_000),
                },
                "attach",
            ),
            pty_output: DiagnoseFact::missing("pty", "depends on CCT-546 (not landed)"),
            claude_socket: DiagnoseFact::fresh(
                SocketStatus {
                    path: Some("/tmp/cc-daemon-1000/ab/control.sock".into()),
                    live: true,
                    candidates: vec!["/tmp/cc-daemon-1000/ab/control.sock".into()],
                },
                "discovery",
                1_700_000_100_000,
            ),
            transcript: DiagnoseFact::undated(
                TranscriptStatus {
                    path: "/home/u/.claude/projects/x/6e189420.jsonl".into(),
                    mtime_ms: Some(1_700_000_050_000),
                    size_bytes: Some(4096),
                    tail_offset: 4096,
                    last_parsed_event: Some("message".into()),
                    last_parsed_at_ms: Some(1_700_000_050_500),
                },
                "filesystem",
            ),
            prompts: DiagnoseFact::fresh(
                PendingPrompts {
                    pending_ask: false,
                    parked_perm_hook: false,
                    control_needs: None,
                    perm_request_id: None,
                },
                "hook",
                1_700_000_100_000,
            ),
            permission_mode: DiagnoseFact::undated("yolo".to_owned(), "spawn"),
            dispatch: DiagnoseFact::missing("dispatch", "not a dispatched session"),
            gateway: DiagnoseFact::fresh(
                GatewayStatus { server_configured: true },
                "daemon-config",
                1_700_000_100_000,
            ),
            codex: None,
        }
    }

    #[test]
    fn fact_helpers_compute_age_and_reasons() {
        let f = DiagnoseFact::observed(1_u32, "src", 1_000, 3_500);
        assert_eq!(f.age_ms, Some(2_500));
        assert_eq!(f.observed_at_ms, Some(1_000));
        assert_eq!(f.missing_reason, None);

        let f = DiagnoseFact::fresh("x", "src", 42);
        assert_eq!(f.age_ms, Some(0));

        let f: DiagnoseFact<u32> = DiagnoseFact::missing("src", "why");
        assert!(f.value.is_none());
        assert_eq!(f.missing_reason.as_deref(), Some("why"));
    }

    #[test]
    fn staleness_clamps_negative_to_zero() {
        assert_eq!(staleness_ms(100, 40), 60);
        // Clock skew: an observation "from the future" reads as age 0, not
        // negative.
        assert_eq!(staleness_ms(100, 200), 0);
    }

    #[test]
    fn report_round_trips() {
        let report = sample_report();
        let json = serde_json::to_string(&report).unwrap();
        let back: SessionDiagnose = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn missing_fact_serializes_without_value() {
        let report = sample_report();
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["pty_output"].get("value").is_none(), "None value must be skipped");
        assert_eq!(json["pty_output"]["missing_reason"], "depends on CCT-546 (not landed)");
        assert_eq!(json["effective_state"]["age_ms"], 1_000);
        assert_eq!(json["effective_state"]["source"], "activity");
    }

    #[test]
    fn claude_report_omits_codex_section() {
        let json = serde_json::to_value(sample_report()).unwrap();
        assert!(json.get("codex").is_none(), "claude report must not carry a codex section");
        for key in [
            "effective_state",
            "last_hook_event",
            "attach",
            "pty_output",
            "claude_socket",
            "transcript",
            "prompts",
            "permission_mode",
            "dispatch",
            "gateway",
        ] {
            assert!(json.get(key).is_some(), "claude wire shape must keep `{key}`");
        }
    }

    #[test]
    fn codex_section_round_trips() {
        let mut report = sample_report();
        report.adapter = "codex".into();
        report.codex = Some(CodexDiagnose {
            codex_version: Some("0.144.1".into()),
            pinned_version: "0.144.1".into(),
            min_version: "0.142.0".into(),
            version_supported: Some(true),
            transport: "stdio".into(),
            app_server_pid: Some(4242),
            live: true,
            registered: true,
            thread_id: Some("019e6628".into()),
            active_turn_id: Some("turn-1".into()),
            turn_status: "working".into(),
            pending_rpc_count: 1,
            pending_rpc_methods: vec!["turn/start".into()],
            last_protocol_error: None,
            rollout_path: Some("/home/u/.codex/sessions/x/019e6628.jsonl".into()),
            rollout_size_bytes: Some(2048),
            auth_state: Some("gateway env present".into()),
            registry_live_mismatch: None,
        });
        let json = serde_json::to_string(&report).unwrap();
        let back: SessionDiagnose = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        assert_eq!(back.codex.unwrap().active_turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn response_round_trips_with_and_without_daemon_report() {
        let server = ServerDiagnose {
            status: Some("active".into()),
            adapter_id: Some("claude-code".into()),
            account_bound: true,
            accounts: vec!["main".into()],
            machine_id: Some(uuid::Uuid::nil().to_string()),
            machine_last_seen_ms: Some(1_700_000_000_000),
        };
        let with = SessionDiagnoseResponse {
            session_id: "s1".into(),
            daemon: Some(sample_report()),
            daemon_error: None,
            server: server.clone(),
        };
        let json = serde_json::to_string(&with).unwrap();
        let back: SessionDiagnoseResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with);

        let without = SessionDiagnoseResponse {
            session_id: "s1".into(),
            daemon: None,
            daemon_error: Some("no daemon connected".into()),
            server,
        };
        let json = serde_json::to_string(&without).unwrap();
        assert!(!json.contains(r#""daemon":"#), "None daemon must be skipped: {json}");
        let back: SessionDiagnoseResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.daemon_error.as_deref(), Some("no daemon connected"));
    }
}
