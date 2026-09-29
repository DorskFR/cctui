//! Codex's use of the adapter-neutral traffic rings: the per-session stdio
//! child gets its own set, the process-wide app-server socket one shared set,
//! and a session snapshot merges the two.

use std::sync::{Arc, OnceLock};

use cctui_proto::diagnose::{TrafficError, TrafficFrame};

use crate::adapters::traffic_rings::merge_by_ts;
pub use crate::adapters::traffic_rings::{TRANSPORT_SHARED, TrafficRings as DiagnoseRings};

/// Format the retained stderr tail for inclusion in a crash detail. Empty
/// when nothing was captured.
pub(super) fn stderr_tail(rings: &DiagnoseRings) -> String {
    let lines: Vec<String> = rings.stderr_tail().into_iter().map(|l| l.line).collect();
    if lines.is_empty() { String::new() } else { format!("; last stderr:\n{}", lines.join("\n")) }
}

/// The rings for the shared `codex app-server daemon` connection. That socket
/// is process-wide, not per-session, so its frames are collected once here and
/// merged into every session's diagnose snapshot.
pub fn shared_rings() -> &'static Arc<DiagnoseRings> {
    static RINGS: OnceLock<Arc<DiagnoseRings>> = OnceLock::new();
    RINGS.get_or_init(|| Arc::new(DiagnoseRings::new(TRANSPORT_SHARED)))
}

/// A session ring's frames plus the shared connection's, oldest first.
pub(super) fn rpc_tail_with_shared(rings: &DiagnoseRings) -> Vec<TrafficFrame> {
    merge_by_ts(rings.rpc_tail(), shared_rings().rpc_tail(), |f| f.ts_ms)
}

pub(super) fn protocol_errors_with_shared(rings: &DiagnoseRings) -> Vec<TrafficError> {
    merge_by_ts(rings.protocol_errors(), shared_rings().protocol_errors(), |e| e.ts_ms)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::adapters::traffic_rings::TRANSPORT_STDIO;

    #[test]
    fn a_session_snapshot_merges_the_shared_connection_tail_oldest_first() {
        let stdio = DiagnoseRings::default();
        shared_rings().note_rpc("in", &json!({"method": "thread/list"}));
        stdio.note_rpc("out", &json!({"method": "turn/start"}));

        let merged = rpc_tail_with_shared(&stdio);
        assert!(merged.iter().any(|f| f.transport == TRANSPORT_SHARED), "{merged:?}");
        assert!(merged.iter().any(|f| f.transport == TRANSPORT_STDIO), "{merged:?}");
        assert!(merged.windows(2).all(|w| w[0].ts_ms <= w[1].ts_ms), "{merged:?}");
    }

    #[test]
    fn the_shared_errors_merge_into_a_session_tail_too() {
        let stdio = DiagnoseRings::default();
        shared_rings().note_protocol_error("connection dropped before request 4 was answered");
        stdio.note_protocol_error("turn/start: boom");

        let merged = protocol_errors_with_shared(&stdio);
        assert!(merged.iter().any(|e| e.transport == TRANSPORT_SHARED), "{merged:?}");
        assert!(merged.iter().any(|e| e.transport == TRANSPORT_STDIO), "{merged:?}");
    }
}
