//! Why a session looks silent, as codes rather than sentences.
//!
//! Both clients need the same answer, so the rules live here and the
//! rendering (wording, pluralisation, age formatting) stays client-side.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::diagnose::{CodexDiagnose, OpenCodeDiagnose};

/// How long an outstanding request may go without a frame before it stalls.
pub const STALLED_RPC_MS: i64 = 60_000;

/// One independent reason a session can look silent.
///
/// Each variant carries the numbers its message needs; the client owns the
/// wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SilenceReason {
    /// Requests outstanding with no frame for longer than [`STALLED_RPC_MS`].
    CodexStalledRpc {
        count: u32,
        age_ms: i64,
    },
    /// A shared-connection drop failed every in-flight request on that socket.
    CodexSharedDropped {
        count: u32,
        age_ms: i64,
    },
    /// Frames on the stdio child but none on the shared socket.
    CodexSharedNoFrames,
    CodexNoTurn,
    CodexAuth {
        state: String,
    },
    CodexRegistryMismatch {
        detail: String,
    },
    CodexNotLive,
    OpencodeSseDown,
    OpencodeSseStalled {
        age_ms: i64,
    },
    /// HTTP frames with no SSE frame — every turn observation rides the stream.
    OpencodeSseNoFrames,
    OpencodeHttpErrors {
        count: u32,
        age_ms: i64,
        message: String,
    },
    OpencodeAwaitingPermission {
        count: u32,
    },
    OpencodeIdle,
    OpencodeVersion {
        version: String,
        pinned: String,
    },
    OpencodeNotLive,
}

#[allow(clippy::cast_possible_truncation)]
const fn len_u32(n: usize) -> u32 {
    n as u32
}

/// Derived from the codex facts alone, with no extra sensing.
#[must_use]
pub fn codex_silence_reasons(cx: &CodexDiagnose, generated_at_ms: i64) -> Vec<SilenceReason> {
    let mut out = Vec::new();
    let frames = &cx.rpc_tail;
    let idle_ms = frames.last().map(|f| generated_at_ms - f.ts_ms);
    if let Some(age_ms) = idle_ms
        && cx.pending_rpc_count > 0
        && age_ms > STALLED_RPC_MS
    {
        out.push(SilenceReason::CodexStalledRpc { count: cx.pending_rpc_count, age_ms });
    }
    let dropped: Vec<_> = cx
        .protocol_errors
        .iter()
        .filter(|e| e.transport == "shared" && e.message.contains("connection dropped"))
        .collect();
    if let Some(last) = dropped.last() {
        out.push(SilenceReason::CodexSharedDropped {
            count: len_u32(dropped.len()),
            age_ms: generated_at_ms - last.ts_ms,
        });
    }
    if !frames.is_empty() && !frames.iter().any(|f| f.transport == "shared") {
        out.push(SilenceReason::CodexSharedNoFrames);
    }
    if cx.active_turn_id.is_none() {
        out.push(SilenceReason::CodexNoTurn);
    }
    if let Some(state) = &cx.auth_state
        && !state.starts_with("gateway env present")
    {
        out.push(SilenceReason::CodexAuth { state: state.clone() });
    }
    if let Some(detail) = &cx.registry_live_mismatch {
        out.push(SilenceReason::CodexRegistryMismatch { detail: detail.clone() });
    }
    if !cx.live {
        out.push(SilenceReason::CodexNotLive);
    }
    out
}

/// The same question for opencode.
///
/// Its transports are HTTP and SSE, and the asymmetry matters: a
/// request/response call failing is loud (the caller sees the status), while
/// the event stream going down is completely silent.
#[must_use]
pub fn opencode_silence_reasons(oc: &OpenCodeDiagnose, generated_at_ms: i64) -> Vec<SilenceReason> {
    let mut out = Vec::new();
    let frames = &oc.rpc_tail;
    let working = oc.turn_status == "working";
    if !oc.sse_connected {
        out.push(SilenceReason::OpencodeSseDown);
    }
    if let Some(last_event) = oc.last_sse_event_ms
        && working
        && oc.sse_connected
    {
        let age_ms = generated_at_ms - last_event;
        if age_ms > STALLED_RPC_MS {
            out.push(SilenceReason::OpencodeSseStalled { age_ms });
        }
    }
    if !frames.is_empty() && !frames.iter().any(|f| f.transport == "sse") {
        out.push(SilenceReason::OpencodeSseNoFrames);
    }
    let rejected: Vec<_> = oc.protocol_errors.iter().filter(|e| e.transport == "http").collect();
    if let Some(last) = rejected.last() {
        out.push(SilenceReason::OpencodeHttpErrors {
            count: len_u32(rejected.len()),
            age_ms: generated_at_ms - last.ts_ms,
            message: last.message.clone(),
        });
    }
    if !oc.pending_permissions.is_empty() {
        out.push(SilenceReason::OpencodeAwaitingPermission {
            count: len_u32(oc.pending_permissions.len()),
        });
    }
    if !working {
        out.push(SilenceReason::OpencodeIdle);
    }
    if oc.version_matches == Some(false)
        && let Some(version) = &oc.server_version
    {
        out.push(SilenceReason::OpencodeVersion {
            version: version.clone(),
            pinned: oc.pinned_version.clone(),
        });
    }
    if !oc.live {
        out.push(SilenceReason::OpencodeNotLive);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnose::{TrafficError, TrafficFrame};

    const NOW: i64 = 1_700_000_000_000;

    fn frame(ts_ms: i64, transport: &str) -> TrafficFrame {
        TrafficFrame {
            ts_ms,
            direction: "out".into(),
            label: "thread/list".into(),
            json: "{}".into(),
            transport: transport.into(),
        }
    }

    fn protocol_error(ts_ms: i64, transport: &str, message: &str) -> TrafficError {
        TrafficError { ts_ms, message: message.into(), transport: transport.into() }
    }

    fn codex() -> CodexDiagnose {
        CodexDiagnose {
            codex_version: None,
            min_version: "0.153.0".into(),
            version_supported: None,
            transport: "stdio".into(),
            app_server_pid: None,
            live: true,
            registered: true,
            thread_id: None,
            active_turn_id: Some("turn-1".into()),
            turn_status: "working".into(),
            pending_rpc_count: 0,
            pending_rpc_methods: vec![],
            protocol_errors: vec![],
            stderr_tail: vec![],
            rpc_tail: vec![frame(NOW - 1_000, "stdio"), frame(NOW - 500, "shared")],
            rollout_path: None,
            rollout_size_bytes: None,
            auth_state: Some("gateway env present".into()),
            registry_live_mismatch: None,
        }
    }

    fn opencode() -> OpenCodeDiagnose {
        OpenCodeDiagnose {
            server_url: None,
            server_pid: None,
            pinned_version: "1.18.7".into(),
            server_version: Some("1.18.7".into()),
            version_matches: Some(true),
            live: true,
            owned_sessions: vec!["ses_1".into()],
            turn_status: "working".into(),
            sse_connected: true,
            last_sse_event_ms: Some(NOW - 1_000),
            pending_permissions: vec![],
            protocol_errors: vec![],
            stderr_tail: vec![],
            rpc_tail: vec![frame(NOW - 2_000, "http"), frame(NOW - 1_000, "sse")],
        }
    }

    #[test]
    fn a_busy_healthy_codex_session_has_no_reason() {
        assert_eq!(codex_silence_reasons(&codex(), NOW), vec![]);
    }

    #[test]
    fn an_rpc_outstanding_with_no_frame_for_over_a_minute_is_stalled() {
        let cx = CodexDiagnose {
            pending_rpc_count: 2,
            rpc_tail: vec![frame(NOW - 120_000, "shared")],
            ..codex()
        };
        assert!(
            codex_silence_reasons(&cx, NOW)
                .contains(&SilenceReason::CodexStalledRpc { count: 2, age_ms: 120_000 })
        );
    }

    #[test]
    fn a_young_pending_rpc_is_not_stalled() {
        let cx = CodexDiagnose {
            pending_rpc_count: 2,
            rpc_tail: vec![frame(NOW - 5_000, "shared")],
            ..codex()
        };
        assert!(
            !codex_silence_reasons(&cx, NOW)
                .iter()
                .any(|r| matches!(r, SilenceReason::CodexStalledRpc { .. }))
        );
    }

    #[test]
    fn a_shared_connection_drop_is_surfaced_and_a_stdio_error_is_not() {
        let cx = CodexDiagnose {
            protocol_errors: vec![protocol_error(
                NOW - 2_000,
                "shared",
                "connection dropped before request 7 was answered",
            )],
            ..codex()
        };
        assert!(
            codex_silence_reasons(&cx, NOW)
                .contains(&SilenceReason::CodexSharedDropped { count: 1, age_ms: 2_000 })
        );

        let cx = CodexDiagnose {
            protocol_errors: vec![protocol_error(NOW - 2_000, "stdio", "turn/start: boom")],
            ..codex()
        };
        assert!(
            !codex_silence_reasons(&cx, NOW)
                .iter()
                .any(|r| matches!(r, SilenceReason::CodexSharedDropped { .. }))
        );
    }

    #[test]
    fn stdio_traffic_with_nothing_on_the_shared_socket_is_a_blind_spot() {
        let cx = CodexDiagnose { rpc_tail: vec![frame(NOW - 500, "stdio")], ..codex() };
        assert!(codex_silence_reasons(&cx, NOW).contains(&SilenceReason::CodexSharedNoFrames));
    }

    #[test]
    fn no_traffic_at_all_is_not_a_shared_blind_spot() {
        let cx = CodexDiagnose { rpc_tail: vec![], ..codex() };
        assert!(!codex_silence_reasons(&cx, NOW).contains(&SilenceReason::CodexSharedNoFrames));
    }

    #[test]
    fn no_turn_bad_auth_registry_mismatch_and_a_dead_child_all_report() {
        let cx = CodexDiagnose {
            active_turn_id: None,
            auth_state: Some("no gateway env".into()),
            registry_live_mismatch: Some("registered but not live".into()),
            live: false,
            ..codex()
        };
        assert_eq!(
            codex_silence_reasons(&cx, NOW),
            vec![
                SilenceReason::CodexNoTurn,
                SilenceReason::CodexAuth { state: "no gateway env".into() },
                SilenceReason::CodexRegistryMismatch { detail: "registered but not live".into() },
                SilenceReason::CodexNotLive,
            ]
        );
    }

    #[test]
    fn a_busy_healthy_opencode_session_has_no_reason() {
        assert_eq!(opencode_silence_reasons(&opencode(), NOW), vec![]);
    }

    #[test]
    fn a_disconnected_event_stream_reports() {
        let oc = OpenCodeDiagnose { sse_connected: false, ..opencode() };
        assert!(opencode_silence_reasons(&oc, NOW).contains(&SilenceReason::OpencodeSseDown));
    }

    #[test]
    fn a_turn_in_flight_with_no_event_for_over_a_minute_is_stalled() {
        let oc = OpenCodeDiagnose { last_sse_event_ms: Some(NOW - 120_000), ..opencode() };
        assert!(
            opencode_silence_reasons(&oc, NOW)
                .contains(&SilenceReason::OpencodeSseStalled { age_ms: 120_000 })
        );
        let oc = OpenCodeDiagnose { last_sse_event_ms: Some(NOW - 5_000), ..opencode() };
        assert!(
            !opencode_silence_reasons(&oc, NOW)
                .iter()
                .any(|r| matches!(r, SilenceReason::OpencodeSseStalled { .. }))
        );
    }

    #[test]
    fn only_http_frames_names_the_event_path_but_no_traffic_does_not() {
        let oc = OpenCodeDiagnose {
            rpc_tail: vec![frame(NOW - 1_000, "http")],
            last_sse_event_ms: None,
            ..opencode()
        };
        assert!(opencode_silence_reasons(&oc, NOW).contains(&SilenceReason::OpencodeSseNoFrames));
        let oc = OpenCodeDiagnose { rpc_tail: vec![], last_sse_event_ms: None, ..opencode() };
        assert!(!opencode_silence_reasons(&oc, NOW).contains(&SilenceReason::OpencodeSseNoFrames));
    }

    #[test]
    fn rejected_http_calls_surface_and_sse_errors_are_ignored() {
        let oc = OpenCodeDiagnose {
            protocol_errors: vec![
                protocol_error(NOW - 3_000, "sse", "event stream closed"),
                protocol_error(NOW - 2_000, "http", "POST /session/ses_1/prompt_async: 500"),
            ],
            ..opencode()
        };
        assert!(opencode_silence_reasons(&oc, NOW).contains(&SilenceReason::OpencodeHttpErrors {
            count: 1,
            age_ms: 2_000,
            message: "POST /session/ses_1/prompt_async: 500".into(),
        }));
    }

    #[test]
    fn a_pending_permission_an_idle_turn_a_version_drift_and_a_dead_server_all_report() {
        let oc = OpenCodeDiagnose {
            pending_permissions: vec!["perm_1".into(), "perm_2".into()],
            turn_status: "idle".into(),
            server_version: Some("1.20.0".into()),
            version_matches: Some(false),
            live: false,
            ..opencode()
        };
        assert_eq!(
            opencode_silence_reasons(&oc, NOW),
            vec![
                SilenceReason::OpencodeAwaitingPermission { count: 2 },
                SilenceReason::OpencodeIdle,
                SilenceReason::OpencodeVersion {
                    version: "1.20.0".into(),
                    pinned: "1.18.7".into()
                },
                SilenceReason::OpencodeNotLive,
            ]
        );
    }
}
