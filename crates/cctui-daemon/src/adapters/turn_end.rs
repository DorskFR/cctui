//! The one place a turn end becomes a client-visible signal.
//!
//! Harness-neutral seam: claude-code sources the turn end from its `Stop` hook,
//! and codex (`task_complete`) / opencode (`session.idle`) can route their own
//! through [`emit`] unchanged. Whoever calls it does not need to know how far
//! back the server it is talking to is.

use cctui_proto::adapter::AdapterEvent;
use cctui_proto::capability::TURN_END;

/// The event to send, or `None` when `server_supports` says the server would
/// reject the kind.
///
/// A server too old to advertise [`TURN_END`] would fail the whole frame parse
/// on the unknown `kind` — and a batched frame's siblings with it — so it gets
/// nothing and keeps falling back to its 2s status poll.
#[must_use]
pub fn signal(local_id: &str, ts: i64, server_supports: bool) -> Option<AdapterEvent> {
    server_supports.then(|| AdapterEvent::TurnEnd { local_id: local_id.to_owned(), ts: Some(ts) })
}

/// Send the turn-end signal if the server understands it. Best-effort: a full
/// or closed channel costs nothing but the ~2s poll latency this saves.
pub async fn emit(events: &tokio::sync::mpsc::Sender<AdapterEvent>, local_id: &str) {
    let supported = crate::servercaps::server_supports(TURN_END);
    let Some(event) = signal(local_id, chrono::Utc::now().timestamp(), supported) else {
        return;
    };
    let _ = events.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_server_is_sent_nothing() {
        assert!(signal("sess-1", 7, false).is_none());
    }

    #[test]
    fn a_capable_server_gets_the_turn_end_for_the_right_session() {
        match signal("sess-1", 7, true) {
            Some(AdapterEvent::TurnEnd { local_id, ts }) => {
                assert_eq!(local_id, "sess-1");
                assert_eq!(ts, Some(7));
            }
            other => panic!("expected a turn_end, got {other:?}"),
        }
    }
}
