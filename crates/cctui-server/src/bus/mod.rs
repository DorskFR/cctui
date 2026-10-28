//! Full-host addressing and reconcile adapters for the shared delivery bus.
pub mod peer;
use crate::state::AppState;
use cctui_proto::adapter::{AdapterCommand, BootstrapFile};
use cctui_proto::git::GitInfo;
use cctui_proto::ws::{DaemonFrameDown, ReadFileOk};
pub use cctui_server_core::bus::*;
use uuid::Uuid;

// ---- session/machine-addressed helpers (the retired `daemon_dispatch.rs`) ----
//
// These resolve DB-level addressing (session → machine/adapter) and then use
// the bus primitives. Best-effort: a call that finds no matching session, no
// `adapter_id`, or no connected daemon returns an error which callers can log
// and ignore.

/// Send `command` to the daemon owning `session_id`. Looks up
/// `(machine_uuid, adapter_id)` from the `sessions` table.
pub async fn dispatch(
    state: &AppState,
    session_id: &str,
    command: AdapterCommand,
) -> Result<(), BusError> {
    let (adapter_id, machine_uuid) = resolve_session(state, session_id).await?;
    state
        .bus
        .command_daemon_for_session(
            machine_uuid,
            session_id,
            DaemonFrameDown::Command { adapter_id, command: Box::new(command) },
        )
        .await
}

/// Rebuild and live-push a fresh [`DaemonFrameDown::Reconcile`] to `machine_id`'s
/// connected daemon. Used when a per-user setting the reconcile derives
/// from (e.g. `harnessMode`) changes, so a daemon picks up the new config without
/// waiting for a reconnect. Best-effort: same `NoDaemon`/`Closed` handling as
/// [`dispatch`] — a machine with no live WS is a no-op error the caller ignores.
pub async fn push_reconcile(state: &AppState, machine_id: Uuid) -> Result<(), BusError> {
    let adapters = crate::routes::daemon::load_reconcile(state, machine_id)
        .await
        .map_err(|e| BusError::Reconcile(e.to_string()))?;
    let secret_scrub = crate::routes::daemon::load_scrub_config(state, machine_id).await;
    state
        .bus
        .command_daemon(machine_id, DaemonFrameDown::Reconcile { adapters, secret_scrub })
        .await
}

/// Stage mid-chat attachment `files` for `session_id` and return the
/// staged absolute paths reported by the owning daemon. Same session
/// resolution as [`dispatch`]; the correlated round-trip (request id, parked
/// oneshot, timeout) lives inside [`Bus::request_daemon`].
pub async fn stage_files(
    state: &AppState,
    session_id: &str,
    files: Vec<BootstrapFile>,
) -> Result<Vec<String>, BusError> {
    let (adapter_id, machine_uuid) = resolve_session(state, session_id).await?;
    let response = state
        .bus
        .request_daemon_for_session(
            machine_uuid,
            session_id,
            DaemonRequest::StageFiles {
                adapter_id,
                local_id: session_id.to_owned(),
                uploads: files,
            },
        )
        .await?;
    match response {
        DaemonResponse::StagedFiles(paths) => Ok(paths),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon owning `session_id` for its session-diagnose report.
/// Same session resolution as [`dispatch`]; the correlated
/// round-trip (request id, parked oneshot, timeout) lives inside
/// [`Bus::request_daemon`].
pub async fn diagnose(
    state: &AppState,
    session_id: &str,
) -> Result<Box<cctui_proto::diagnose::SessionDiagnose>, BusError> {
    let (adapter_id, machine_uuid) = resolve_session(state, session_id).await?;
    let response = state
        .bus
        .request_daemon_for_session(
            machine_uuid,
            session_id,
            DaemonRequest::Diagnose { adapter_id, local_id: session_id.to_owned() },
        )
        .await?;
    match response {
        DaemonResponse::Diagnose(report) => Ok(report),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon on `machine_uuid` for the sub-directories of `path`
/// (working-directory autocomplete). Machine-addressed — no session involved.
pub async fn list_dirs(
    state: &AppState,
    machine_uuid: Uuid,
    path: String,
) -> Result<Vec<String>, BusError> {
    match state.bus.request_daemon(machine_uuid, DaemonRequest::ListDirs { path }).await? {
        DaemonResponse::Dirs(dirs) => Ok(dirs),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon on `machine_uuid` for the git facts of `path` (spawn
/// dialog branch badge). Machine-addressed — no session involved.
pub async fn git_info(
    state: &AppState,
    machine_uuid: Uuid,
    path: String,
    include_dirty: bool,
) -> Result<GitInfo, BusError> {
    let request = DaemonRequest::GitInfo { path, include_dirty };
    match state.bus.request_daemon(machine_uuid, request).await? {
        DaemonResponse::GitInfo(info) => Ok(info),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon on `machine_uuid` for the bytes of `path` (a file an agent
/// linked in a message). `cwd` widens the daemon's allow-list to the
/// session's working directory.
pub async fn read_file(
    state: &AppState,
    machine_uuid: Uuid,
    path: String,
    max_bytes: u64,
    cwd: Option<String>,
) -> Result<ReadFileOk, BusError> {
    let request = DaemonRequest::ReadFile { path, max_bytes, cwd };
    match state.bus.request_daemon(machine_uuid, request).await? {
        DaemonResponse::File(file) => Ok(file),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Resolve a session id to its `(adapter_id, machine_uuid)` addressing pair.
async fn resolve_session(state: &AppState, session_id: &str) -> Result<(String, Uuid), BusError> {
    let row: Option<(Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT adapter_id, machine_uuid FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    let (adapter_id, machine_uuid) = row.ok_or(BusError::NotFound)?;
    let adapter_id = adapter_id.ok_or(BusError::NoAdapter)?;
    let machine_uuid = machine_uuid.ok_or(BusError::NoMachine)?;
    Ok((adapter_id, machine_uuid))
}
