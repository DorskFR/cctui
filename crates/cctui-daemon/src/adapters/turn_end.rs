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

/// Record the turn end for a `CctuiAgent` follow watching this session.
///
/// Ungated: the capability governs only the wire event, while the follow runs
/// in this process. No-op when nothing watches the session.
pub fn note(local_id: &str) {
    crate::childwatch::global().note_turn_end(local_id);
}

/// Whether the connected server accepts the signal at all.
#[must_use]
pub fn supported() -> bool {
    crate::servercaps::server_supports(TURN_END)
}

/// Send the turn-end signal if the server understands it. Best-effort: a full
/// or closed channel costs nothing but the ~2s poll latency this saves.
pub async fn emit(events: &tokio::sync::mpsc::Sender<AdapterEvent>, local_id: &str) {
    emit_gated(events, local_id, supported()).await;
}

/// [`emit`] for a caller that snapshotted [`supported`] earlier — a session
/// holding it as state, rather than re-reading the global per turn.
pub async fn emit_gated(
    events: &tokio::sync::mpsc::Sender<AdapterEvent>,
    local_id: &str,
    server_supports: bool,
) {
    note(local_id);
    let Some(event) = signal(local_id, chrono::Utc::now().timestamp(), server_supports) else {
        return;
    };
    let _ = events.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::childwatch::Assessment;
    use std::time::Instant;

    fn assistant_text(local_id: &str, text: &str) -> AdapterEvent {
        AdapterEvent::Message {
            local_id: local_id.to_owned(),
            payload: serde_json::json!({ "role": "assistant", "text": text }),
            turn_id: None,
        }
    }

    #[tokio::test]
    async fn an_older_server_still_gets_the_follow_its_turn_end() {
        let watch = crate::childwatch::global();
        let h = watch.register_bound("turn-end-seam-1");
        watch.observe(&assistant_text("turn-end-seam-1", "the answer"));
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        emit_gated(&tx, "turn-end-seam-1", false).await;
        assert!(rx.try_recv().is_err(), "the wire event stays gated");
        let Assessment::Finished(out) = h.snapshot().unwrap().assess(Instant::now()) else {
            panic!("the follow must end on the turn end the gate withheld")
        };
        assert_eq!(out.final_text.as_deref(), Some("the answer"));
    }

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
