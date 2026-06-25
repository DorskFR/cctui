//! Dispatch [`AdapterCommand`]s from server routes to the appropriate
//! per-machine daemon WebSocket.
//!
//! Best-effort: a `dispatch` that finds no matching session, no
//! `adapter_id`, or no connected daemon returns an error which callers
//! can log and ignore.

use cctui_proto::adapter::{AdapterCommand, BootstrapFile};
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
    #[error("timed out waiting for the daemon to stage files")]
    Timeout,
    #[error("daemon could not stage files: {0}")]
    Staging(String),
    #[error("daemon could not list the directory: {0}")]
    ListDirs(String),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("reconcile build error: {0}")]
    Reconcile(String),
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
    tx.send(DaemonFrameDown::Command { adapter_id, command: Box::new(command) })
        .await
        .map_err(|_| Error::Closed)?;
    Ok(())
}

/// Rebuild and live-push a fresh [`DaemonFrameDown::Reconcile`] to `machine_id`'s
/// connected daemon (CCT-495). Used when a per-user setting the reconcile derives
/// from (e.g. `harnessMode`) changes, so a daemon picks up the new config without
/// waiting for a reconnect. Best-effort: same `NoDaemon`/`Closed` handling as
/// [`dispatch`] — a machine with no live WS is a no-op error the caller ignores.
pub async fn push_reconcile(state: &AppState, machine_id: Uuid) -> Result<(), Error> {
    let adapters = crate::routes::daemon::load_reconcile(state, machine_id)
        .await
        .map_err(|e| Error::Reconcile(e.to_string()))?;
    let Some(tx) = state.daemon_connections.get(&machine_id) else {
        return Err(Error::NoDaemon(machine_id));
    };
    tx.send(DaemonFrameDown::Reconcile { adapters }).await.map_err(|_| Error::Closed)?;
    Ok(())
}

/// How long the `POST /sessions/{id}/files` route waits for the daemon's
/// `StageFilesResult` before giving up. Staging is local filesystem work after
/// a small base64 decode, so this is generous-but-bounded.
const STAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Stage mid-chat attachment `files` for `session_id` (CCT-236) and return the
/// staged absolute paths reported by the owning daemon.
///
/// Looks up `(adapter_id, machine_uuid)` the same way [`dispatch`] does, sends a
/// [`DaemonFrameDown::StageFiles`] keyed by a fresh `request_id`, registers a
/// oneshot in `state.pending_stage_requests`, and awaits the daemon's
/// `StageFilesResult` (routed back in by the daemon WS read loop).
pub async fn stage_files(
    state: &AppState,
    session_id: &str,
    files: Vec<BootstrapFile>,
) -> Result<Vec<String>, Error> {
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

    let request_id = Uuid::new_v4();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state.pending_stage_requests.insert(request_id, reply_tx);

    let send = tx
        .send(DaemonFrameDown::StageFiles {
            request_id,
            adapter_id,
            local_id: session_id.to_owned(),
            uploads: files,
        })
        .await;
    if send.is_err() {
        state.pending_stage_requests.remove(&request_id);
        return Err(Error::Closed);
    }

    match tokio::time::timeout(STAGE_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(paths))) => Ok(paths),
        Ok(Ok(Err(msg))) => Err(Error::Staging(msg)),
        // Sender dropped (daemon disconnected mid-request) — clean up handled
        // by the read loop on disconnect, but remove defensively.
        Ok(Err(_)) => {
            state.pending_stage_requests.remove(&request_id);
            Err(Error::Closed)
        }
        Err(_) => {
            state.pending_stage_requests.remove(&request_id);
            Err(Error::Timeout)
        }
    }
}

/// How long the `GET /machines/{id}/fs/dirs` route waits for the daemon's
/// `ListDirsResult`. Autocomplete is interactive — better to fail fast than
/// to hold the typeahead open.
const LIST_DIRS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Ask the daemon on `machine_uuid` for the sub-directories of `path`
/// (working-directory autocomplete). Same request_id + oneshot pattern as
/// [`stage_files`], but machine-addressed (no session involved).
pub async fn list_dirs(
    state: &AppState,
    machine_uuid: Uuid,
    path: String,
) -> Result<Vec<String>, Error> {
    let Some(tx) = state.daemon_connections.get(&machine_uuid).map(|r| r.clone()) else {
        return Err(Error::NoDaemon(machine_uuid));
    };

    let request_id = Uuid::new_v4();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state.pending_listdirs_requests.insert(request_id, reply_tx);

    if tx.send(DaemonFrameDown::ListDirs { request_id, path }).await.is_err() {
        state.pending_listdirs_requests.remove(&request_id);
        return Err(Error::Closed);
    }

    match tokio::time::timeout(LIST_DIRS_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(dirs))) => Ok(dirs),
        Ok(Ok(Err(msg))) => Err(Error::ListDirs(msg)),
        // Sender dropped (daemon disconnected mid-request).
        Ok(Err(_)) => {
            state.pending_listdirs_requests.remove(&request_id);
            Err(Error::Closed)
        }
        Err(_) => {
            state.pending_listdirs_requests.remove(&request_id);
            Err(Error::Timeout)
        }
    }
}
