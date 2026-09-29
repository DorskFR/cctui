//! Bounded, redacted observability rings shared by every harness adapter.
//!
//! A harness speaks its own protocol — JSON-RPC over stdio for codex, HTTP +
//! SSE for opencode — but what a stuck session needs to show is the same in
//! both: the last frames each way, the protocol errors, the process stderr.
//! The rings are that common surface; an adapter only has to call
//! [`TrafficRings::note_rpc`] from wherever its own transport lives.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use cctui_crypto::redact::{self, CompiledPatterns};
use cctui_proto::diagnose::{TrafficError, TrafficFrame, TrafficStderrLine};
use serde_json::Value;

/// How many trailing harness stderr lines to retain for crash diagnostics.
/// When the process dies unexpectedly these lines are the only clue why, so
/// they are folded into the [`EndReason::Crashed`] detail instead of being
/// discarded to `/dev/null`.
const STDERR_RING: usize = 200;

const RPC_RING: usize = 50;

const PROTOCOL_ERROR_RING: usize = 20;

const RPC_FRAME_MAX: usize = 2 * 1024;

/// Prefix of a frame actually scanned for secrets. Larger than [`RPC_FRAME_MAX`]
/// so a token straddling the retention cut is still masked in what is kept.
const RPC_SCAN_MAX: usize = 8 * 1024;

/// The per-session codex app-server child's stdio pipes.
pub const TRANSPORT_STDIO: &str = "stdio";
/// The process-wide `codex app-server daemon` control socket.
pub const TRANSPORT_SHARED: &str = "shared";
/// The `opencode serve` request/response HTTP API.
pub const TRANSPORT_HTTP: &str = "http";
/// The `opencode serve` `GET /event` stream.
pub const TRANSPORT_SSE: &str = "sse";

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

/// Install the user-configured scrub patterns on the rings of every adapter.
///
/// Called by the supervisor whenever the server syncs a `SecretScrubConfig`;
/// the rings live in the drivers and have no other route to them.
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

/// Merge two tails, oldest first. Neither side is truncated against the other:
/// the whole point of the transport tagging is that a flood on one path must
/// not hide the silence of another.
pub fn merge_by_ts<T: Clone, F: Fn(&T) -> i64>(a: Vec<T>, b: Vec<T>, ts: F) -> Vec<T> {
    let mut out = a;
    out.extend(b);
    out.sort_by_key(|e| ts(e));
    out
}

/// Bounded, redacted observability rings for the diagnose report.
///
/// Every producer sits on a protocol write path or a read loop, so the locks
/// are `try_lock` only: a contended ring drops the entry rather than stalling
/// the session.
#[derive(Debug)]
pub struct TrafficRings {
    /// Stamped onto entries that name no transport of their own, so a reader
    /// can tell "no frames on this path" from "no frames at all".
    transport: &'static str,
    stderr: StdMutex<VecDeque<TrafficStderrLine>>,
    rpc: StdMutex<VecDeque<TrafficFrame>>,
    errors: StdMutex<VecDeque<TrafficError>>,
}

impl Default for TrafficRings {
    fn default() -> Self {
        Self::new(TRANSPORT_STDIO)
    }
}

impl TrafficRings {
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

    pub fn note_stderr(&self, line: &str) {
        Self::push(
            &self.stderr,
            STDERR_RING,
            TrafficStderrLine { ts_ms: now_ms(), line: redact_text(line) },
        );
    }

    /// A JSON frame labelled by its own `method`/`id`, on this ring's transport.
    pub fn note_rpc(&self, direction: &str, value: &Value) {
        let label = value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| value.get("id").map(ToString::to_string))
            .unwrap_or_else(|| "frame".to_owned());
        self.note_frame(self.transport, direction, &label, &value.to_string());
    }

    /// A frame whose label the caller supplies, on an explicit transport: one
    /// adapter can own several paths (opencode's HTTP calls and SSE stream).
    pub fn note_frame(&self, transport: &str, direction: &str, label: &str, body: &str) {
        let json =
            truncate_chars(&redact_text(&truncate_chars(body, RPC_SCAN_MAX)), RPC_FRAME_MAX);
        Self::push(
            &self.rpc,
            RPC_RING,
            TrafficFrame {
                ts_ms: now_ms(),
                direction: direction.to_owned(),
                label: redact_text(label),
                json,
                transport: transport.to_owned(),
            },
        );
    }

    pub fn note_protocol_error(&self, message: &str) {
        self.note_protocol_error_on(self.transport, message);
    }

    pub fn note_protocol_error_on(&self, transport: &str, message: &str) {
        Self::push(
            &self.errors,
            PROTOCOL_ERROR_RING,
            TrafficError {
                ts_ms: now_ms(),
                message: redact_text(message),
                transport: transport.to_owned(),
            },
        );
    }

    #[must_use]
    pub fn stderr_tail(&self) -> Vec<TrafficStderrLine> {
        Self::snapshot(&self.stderr)
    }

    #[must_use]
    pub fn rpc_tail(&self) -> Vec<TrafficFrame> {
        Self::snapshot(&self.rpc)
    }

    #[must_use]
    pub fn protocol_errors(&self) -> Vec<TrafficError> {
        Self::snapshot(&self.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rings_redact_tool_output_secrets() {
        let token = "ghp_0123456789abcdefghijABCDEFGHIJ0123";
        let rings = TrafficRings::default();
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
        assert!(!rpc[0].json.contains(token), "{}", rpc[0].json);
        assert!(rpc[0].json.contains("[REDACTED:github_token"), "{}", rpc[0].json);
        assert_eq!(rpc[0].label, "item/completed");
        assert!(!rings.stderr_tail()[0].line.contains(token));
        assert!(!rings.protocol_errors()[0].message.contains(token));
    }

    /// A ring can serve several transports, and the entry must carry the one it
    /// came from: without the tag a flood of HTTP frames makes a dead event
    /// stream look healthy.
    #[test]
    fn entries_carry_the_transport_that_produced_them() {
        let rings = TrafficRings::new(TRANSPORT_HTTP);
        rings.note_frame(TRANSPORT_HTTP, "out", "POST /session", "{}");
        rings.note_frame(TRANSPORT_SSE, "in", "session.idle", "{}");
        rings.note_protocol_error("POST /session: 500");
        rings.note_protocol_error_on(TRANSPORT_SSE, "event stream dropped");

        let tail = rings.rpc_tail();
        assert_eq!(tail[0].transport, TRANSPORT_HTTP);
        assert_eq!(tail[1].transport, TRANSPORT_SSE);
        let errors = rings.protocol_errors();
        assert_eq!(errors[0].transport, TRANSPORT_HTTP);
        assert_eq!(errors[1].transport, TRANSPORT_SSE);
    }

    /// The builtins alone would let a user-configured secret through; the rings
    /// echo tool output, so this is a live leak path.
    #[test]
    fn rings_apply_user_configured_scrub_patterns() {
        set_ring_scrub(&[("acme_key".to_owned(), "ACME-[0-9]{6}".to_owned())]);
        let rings = TrafficRings::default();
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
    fn rings_are_bounded_and_frames_truncated() {
        let rings = TrafficRings::default();
        for i in 0..(RPC_RING + 10) {
            rings.note_rpc("out", &json!({ "method": "ping", "i": i }));
        }
        assert_eq!(rings.rpc_tail().len(), RPC_RING);

        rings.note_rpc("out", &json!({ "method": "big", "blob": "x".repeat(RPC_SCAN_MAX * 2) }));
        let last = rings.rpc_tail().pop().expect("a frame");
        assert!(last.json.chars().count() <= RPC_FRAME_MAX + 1, "{}", last.json.len());
    }

    #[test]
    fn merge_orders_two_tails_oldest_first() {
        let a = vec![TrafficStderrLine { ts_ms: 30, line: "a".to_owned() }];
        let b = vec![
            TrafficStderrLine { ts_ms: 10, line: "b".to_owned() },
            TrafficStderrLine { ts_ms: 20, line: "c".to_owned() },
        ];
        let merged = merge_by_ts(a, b, |l| l.ts_ms);
        assert_eq!(merged.iter().map(|l| l.ts_ms).collect::<Vec<_>>(), vec![10, 20, 30]);
    }
}
