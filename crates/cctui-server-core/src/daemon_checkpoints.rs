//! Durable resume marks and capability-aware acknowledgment.
use uuid::Uuid;
/// Advance a session's stored transcript high-water mark to `offset`, keeping
/// the max so a replayed / out-of-order mark can't rewind it.
///
/// The daemon receives this resume point on its next connect.
pub async fn update_transcript_mark(
    state: &crate::state::CoreState,
    local_id: &str,
    offset: u64,
) -> sqlx::Result<()> {
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE sessions SET transcript_offset = $2 WHERE id = $1 AND transcript_offset < $2",
    )
    .bind(local_id)
    .bind(offset)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Tell the daemon the mark is stored, so its reconcile stops re-sending the
/// window behind it. Skipped for a daemon that never advertised `mark_acks`:
/// an unknown frame kills such a connection.
pub async fn ack_transcript_mark(
    state: &crate::state::CoreState,
    machine_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    offset: u64,
) {
    if !state.mark_ack_daemons.contains_key(&machine_id) {
        return;
    }
    let frame = cctui_proto::ws::DaemonFrameDown::TranscriptAck {
        adapter_id: adapter_id.to_owned(),
        session_marks: vec![(local_id.to_owned(), offset)],
    };
    if let Err(err) = state.bus.command_daemon_for_session(machine_id, local_id, frame).await {
        tracing::debug!(%err, %machine_id, "transcript ack not delivered");
    }
}
