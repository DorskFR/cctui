use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use cctui_crypto::redact::{self, CompiledPatterns};
use cctui_proto::diagnose::{CodexProtocolError, CodexRpcFrame, CodexStderrLine};
use serde_json::Value;

/// How many trailing `codex app-server` stderr lines to retain for crash
/// diagnostics. The app-server logs to stderr; when it dies
/// unexpectedly these lines are the only clue why, so they are folded into
/// the [`EndReason::Crashed`] detail instead of being discarded to
/// `/dev/null`.
const STDERR_RING: usize = 200;

const RPC_RING: usize = 50;

const PROTOCOL_ERROR_RING: usize = 20;

const RPC_FRAME_MAX: usize = 2 * 1024;

/// Prefix of a frame actually scanned for secrets. Larger than [`RPC_FRAME_MAX`]
/// so a token straddling the retention cut is still masked in what is kept.
const RPC_SCAN_MAX: usize = 8 * 1024;

/// Format the retained stderr tail for inclusion in a crash detail. Empty
/// when nothing was captured.
pub(super) fn stderr_tail(rings: &DiagnoseRings) -> String {
    let lines: Vec<String> = rings.stderr_tail().into_iter().map(|l| l.line).collect();
    if lines.is_empty() { String::new() } else { format!("; last stderr:\n{}", lines.join("\n")) }
}

/// The per-session app-server child's stdio pipes.
pub const TRANSPORT_STDIO: &str = "stdio";
/// The process-wide `codex app-server daemon` control socket.
pub const TRANSPORT_SHARED: &str = "shared";

type RingScrub = std::sync::RwLock<Arc<CompiledPatterns>>;

/// The effective detector set for the rings: the builtins, plus whatever custom
/// patterns the server last synced (see [`set_ring_scrub`]). Builtins stay on
/// unconditionally — a session's tool output is echoed into these rings, so
/// "scrubbing disabled" must not mean "tokens in the diagnose report".
fn ring_scrub_cell() -> &'static RingScrub {
    static SCRUB: OnceLock<RingScrub> = OnceLock::new();
    SCRUB.get_or_init(|| {
        std::sync::RwLock::new(Arc::new(redact::compile(true, &[], &cctui_crypto::vault_key())))
    })
}

/// Install the user-configured scrub patterns on the rings. Called by the
/// supervisor whenever the server syncs a `SecretScrubConfig`; the rings live
/// in the driver and have no other route to them.
pub fn set_ring_scrub(user: &[(String, String)]) {
    let compiled = Arc::new(redact::compile(true, user, &cctui_crypto::vault_key()));
    if let Ok(mut guard) = ring_scrub_cell().write() {
        *guard = compiled;
    }
}

fn ring_scrub() -> Arc<CompiledPatterns> {
    ring_scrub_cell().read().map_or_else(|e| Arc::clone(&e.into_inner()), |g| Arc::clone(&g))
}

fn redact_text(text: &str) -> String {
    let mut value = Value::String(text.to_owned());
    redact::redact_json(&mut value, &ring_scrub());
    match value {
        Value::String(s) => s,
        _ => text.to_owned(),
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// Bounded, redacted observability rings for the diagnose report.
///
/// Every producer sits on the JSON-RPC write path or the stdout read loop, so
/// the locks are `try_lock` only: a contended ring drops the entry rather than
/// stalling the session.
#[derive(Debug)]
pub struct DiagnoseRings {
    /// Stamped onto every entry so a reader can tell "no frames on the shared
    /// connection" from "no frames at all".
    transport: &'static str,
    stderr: StdMutex<VecDeque<CodexStderrLine>>,
    rpc: StdMutex<VecDeque<CodexRpcFrame>>,
    errors: StdMutex<VecDeque<CodexProtocolError>>,
}

impl Default for DiagnoseRings {
    fn default() -> Self {
        Self::new(TRANSPORT_STDIO)
    }
}

/// The rings for the shared `codex app-server daemon` connection. That socket
/// is process-wide, not per-session, so its frames are collected once here and
/// merged into every session's diagnose snapshot.
pub fn shared_rings() -> &'static Arc<DiagnoseRings> {
    static RINGS: OnceLock<Arc<DiagnoseRings>> = OnceLock::new();
    RINGS.get_or_init(|| Arc::new(DiagnoseRings::new(TRANSPORT_SHARED)))
}

/// Merge a session's stdio tail with the shared connection's, oldest first.
/// Neither side is truncated against the other: the whole point of the tagging
/// is that a flood on one transport must not hide the silence of the other.
fn merge_by_ts<T: Clone, F: Fn(&T) -> i64>(a: Vec<T>, b: Vec<T>, ts: F) -> Vec<T> {
    let mut out = a;
    out.extend(b);
    out.sort_by_key(|e| ts(e));
    out
}

impl DiagnoseRings {
    #[must_use]
    pub fn new(transport: &'static str) -> Self {
        Self {
            transport,
            stderr: StdMutex::default(),
            rpc: StdMutex::default(),
            errors: StdMutex::default(),
        }
    }

    fn push<T>(ring: &StdMutex<VecDeque<T>>, cap: usize, item: T) {
        let Ok(mut guard) = ring.try_lock() else { return };
        while guard.len() >= cap {
            guard.pop_front();
        }
        guard.push_back(item);
    }

    fn snapshot<T: Clone>(ring: &StdMutex<VecDeque<T>>) -> Vec<T> {
        ring.try_lock().map(|g| g.iter().cloned().collect()).unwrap_or_default()
    }

    pub(super) fn note_stderr(&self, line: &str) {
        Self::push(
            &self.stderr,
            STDERR_RING,
            CodexStderrLine { ts_ms: now_ms(), line: redact_text(line) },
        );
    }

    pub fn note_rpc(&self, direction: &'static str, value: &Value) {
        let label = value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| value.get("id").map(ToString::to_string))
            .unwrap_or_else(|| "frame".to_owned());
        let raw = value.to_string();
        let json = truncate_chars(&redact_text(&truncate_chars(&raw, RPC_SCAN_MAX)), RPC_FRAME_MAX);
        Self::push(
            &self.rpc,
            RPC_RING,
            CodexRpcFrame {
                ts_ms: now_ms(),
                direction: direction.to_owned(),
                label: redact_text(&label),
                json,
                transport: self.transport.to_owned(),
            },
        );
    }

    pub fn note_protocol_error(&self, message: &str) {
        Self::push(
            &self.errors,
            PROTOCOL_ERROR_RING,
            CodexProtocolError {
                ts_ms: now_ms(),
                message: redact_text(message),
                transport: self.transport.to_owned(),
            },
        );
    }

    pub(super) fn stderr_tail(&self) -> Vec<CodexStderrLine> {
        Self::snapshot(&self.stderr)
    }

    pub fn rpc_tail(&self) -> Vec<CodexRpcFrame> {
        Self::snapshot(&self.rpc)
    }

    pub fn protocol_errors(&self) -> Vec<CodexProtocolError> {
        Self::snapshot(&self.errors)
    }

    /// This ring's frames plus the shared connection's, oldest first.
    pub(super) fn rpc_tail_with_shared(&self) -> Vec<CodexRpcFrame> {
        merge_by_ts(self.rpc_tail(), shared_rings().rpc_tail(), |f| f.ts_ms)
    }

    pub(super) fn protocol_errors_with_shared(&self) -> Vec<CodexProtocolError> {
        merge_by_ts(self.protocol_errors(), shared_rings().protocol_errors(), |e| e.ts_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnose_rings_redact_tool_output_secrets() {
        let token = "ghp_0123456789abcdefghijABCDEFGHIJ0123";
        let rings = DiagnoseRings::default();
        rings.note_rpc(
            "in",
            &json!({
                "method": "item/completed",
                "params": { "item": { "output": format!("export GITHUB_TOKEN={token}") } }
            }),
        );
        rings.note_stderr(&format!("tool stdout: {token}"));
        rings.note_protocol_error(&format!("turn/start: upstream rejected {token}"));

        let rpc = rings.rpc_tail();
        let stderr = rings.stderr_tail();
        let errors = rings.protocol_errors();
        assert!(!rpc[0].json.contains(token), "{}", rpc[0].json);
        assert!(rpc[0].json.contains("[REDACTED:github_token"), "{}", rpc[0].json);
        assert_eq!(rpc[0].label, "item/completed");
        assert!(!stderr[0].line.contains(token), "{}", stderr[0].line);
        assert!(!errors[0].message.contains(token), "{}", errors[0].message);
    }

    /// Without the tag a flood of stdio frames makes an entirely dead shared
    /// connection look healthy — the blind spot CCT-966 opened.
    #[test]
    fn ring_entries_carry_the_transport_that_produced_them() {
        let stdio = DiagnoseRings::default();
        let shared = DiagnoseRings::new(TRANSPORT_SHARED);
        stdio.note_rpc("out", &json!({"method": "turn/start"}));
        shared.note_rpc("out", &json!({"method": "thread/list"}));
        stdio.note_protocol_error("turn/start: boom");
        shared.note_protocol_error("connection dropped before request 4 was answered");

        assert_eq!(stdio.rpc_tail()[0].transport, "stdio");
        assert_eq!(shared.rpc_tail()[0].transport, "shared");
        assert_eq!(stdio.protocol_errors()[0].transport, "stdio");
        assert_eq!(shared.protocol_errors()[0].transport, "shared");
    }

    #[test]
    fn a_session_snapshot_merges_the_shared_connection_tail_oldest_first() {
        let stdio = DiagnoseRings::default();
        shared_rings().note_rpc("in", &json!({"method": "thread/list"}));
        stdio.note_rpc("out", &json!({"method": "turn/start"}));

        let merged = stdio.rpc_tail_with_shared();
        assert!(merged.iter().any(|f| f.transport == "shared"), "{merged:?}");
        assert!(merged.iter().any(|f| f.transport == "stdio"), "{merged:?}");
        assert!(merged.windows(2).all(|w| w[0].ts_ms <= w[1].ts_ms), "{merged:?}");
    }

    /// The builtins alone would let a user-configured secret through; the
    /// rings echo tool output, so this is a live leak path.
    #[test]
    fn diagnose_rings_apply_user_configured_scrub_patterns() {
        set_ring_scrub(&[("acme_key".to_owned(), "ACME-[0-9]{6}".to_owned())]);
        let rings = DiagnoseRings::default();
        rings.note_rpc(
            "in",
            &json!({"method": "item/completed", "params": {"output": "token ACME-424242 ok"}}),
        );
        rings.note_stderr("leaked ACME-424242 to stderr");

        let frame = &rings.rpc_tail()[0];
        assert!(!frame.json.contains("ACME-424242"), "{}", frame.json);
        assert!(frame.json.contains("[REDACTED:acme_key"), "{}", frame.json);
        assert!(!rings.stderr_tail()[0].line.contains("ACME-424242"));

        set_ring_scrub(&[]);
    }

    #[test]
    fn diagnose_rings_are_bounded_and_frames_truncated() {
        let rings = DiagnoseRings::default();
        for i in 0..(RPC_RING + 10) {
            rings.note_rpc("out", &json!({ "id": i, "method": "turn/start" }));
        }
        for i in 0..(STDERR_RING + 10) {
            rings.note_stderr(&format!("line {i}"));
        }
        for i in 0..(PROTOCOL_ERROR_RING + 10) {
            rings.note_protocol_error(&format!("err {i}"));
        }
        assert_eq!(rings.rpc_tail().len(), RPC_RING);
        assert_eq!(rings.stderr_tail().len(), STDERR_RING);
        assert_eq!(rings.protocol_errors().len(), PROTOCOL_ERROR_RING);
        assert_eq!(rings.stderr_tail()[0].line, format!("line {}", 10));

        rings.note_rpc("out", &json!({ "method": "turn/start", "text": "x".repeat(64 * 1024) }));
        let last = rings.rpc_tail().pop().unwrap();
        assert!(last.json.len() <= RPC_FRAME_MAX + 4, "{}", last.json.len());
    }
}
