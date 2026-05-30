//! Dispatch [`AdapterCommand`]s from server routes to the appropriate
//! per-machine daemon WebSocket.
//!
//! Best-effort: a `dispatch` that finds no matching session, no
//! `adapter_id`, or no connected daemon returns an error which callers
//! can log and ignore.

use cctui_proto::adapter::AdapterCommand;
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("session not found")]
    NotFound,
    #[error("session has no adapter_id (legacy path)")]
    NoAdapter,
    #[error("session has no machine_uuid yet")]
    NoMachine,
    #[error("no daemon connected for machine {0}")]
    NoDaemon(Uuid),
    #[error("daemon channel closed")]
    Closed,
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Send `command` to the daemon owning `session_id`. Looks up
/// `(machine_uuid, adapter_id)` from the `sessions` table.
pub async fn dispatch(
    state: &AppState,
    session_id: &str,
    command: AdapterCommand,
) -> Result<(), Error> {
    let row: Option<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT adapter_id, machine_uuid FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    let (adapter_id, machine_uuid) = row.ok_or(Error::NotFound)?;
    let adapter_id = adapter_id.ok_or(Error::NoAdapter)?;
    let machine_uuid = machine_uuid.ok_or(Error::NoMachine)?;
    let Some(tx) = state.daemon_connections.get(&machine_uuid) else {
        return Err(Error::NoDaemon(machine_uuid));
    };
    tx.send(DaemonFrameDown::Command { adapter_id, command }).await.map_err(|_| Error::Closed)?;
    Ok(())
}
