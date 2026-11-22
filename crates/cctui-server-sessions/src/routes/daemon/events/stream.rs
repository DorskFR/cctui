use cctui_proto::adapter::AdapterEvent;
use uuid::Uuid;

use super::Inserted;
use crate::context::SessionsContext;
use crate::routes::daemon::ingest::{NewEvent, Stored, insert_event, note_insert};

/// Persist a `Message`/`ToolUse` row unless the batched ingest already did.
pub(super) async fn on_stream_event<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    event: AdapterEvent,
    stored: Stored,
) -> anyhow::Result<Inserted> {
    let seq = match event {
        AdapterEvent::Message { local_id, mut payload, turn_id } => {
            if let Stored::Done(seq) = stored {
                seq
            } else {
                crate::keepalive::observe_message(state, &local_id, &mut payload).await;
                let row = NewEvent::unchunked(&local_id, "message", payload, turn_id);
                insert_event(state.pool(), machine_id, user_id, row).await?
            }
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            if let Stored::Done(seq) = stored {
                seq
            } else {
                let row = NewEvent::unchunked(&local_id, "tool_use", payload, None);
                insert_event(state.pool(), machine_id, user_id, row).await?
            }
        }
        _ => return Ok(Inserted::UNSTORED),
    };
    let inserted = Inserted::from_seq(seq);
    note_insert(state, machine_id, inserted.newly);
    Ok(inserted)
}
