//! Session- and machine-addressed daemon requests through the host state.
use cctui_proto::adapter::{AdapterCommand, BootstrapFile};
use cctui_proto::git::GitInfo;
use cctui_proto::ws::ReadFileOk;
use uuid::Uuid;

use super::{BusError, DaemonRequest, DaemonResponse};
use crate::context::KernelContext;

// ---- session/machine-addressed helpers (the retired `daemon_dispatch.rs`) ----
//
// These resolve DB-level addressing (session → machine/adapter) and then use
// the bus primitives. Best-effort: a call that finds no matching session, no
// `adapter_id`, or no connected daemon returns an error which callers can log
// and ignore.

/// Send `command` to the daemon owning `session_id`. Looks up
/// `(machine_uuid, adapter_id)` from the `sessions` table.
pub async fn dispatch<S: KernelContext>(
    state: &S,
    session_id: &str,
    command: AdapterCommand,
) -> Result<(), BusError> {
    super::dispatch_session(state.pool(), state.bus(), session_id, command).await
}

/// Stage mid-chat attachment `files` for `session_id` and return the
/// staged absolute paths reported by the owning daemon.
///
/// Same session
/// resolution as [`dispatch`]; the correlated round-trip (request id, parked
/// oneshot, timeout) lives inside [`Bus::request_daemon`].
pub async fn stage_files<S: KernelContext>(
    state: &S,
    session_id: &str,
    files: Vec<BootstrapFile>,
) -> Result<Vec<String>, BusError> {
    let (adapter_id, machine_uuid) = resolve_session(state, session_id).await?;
    let response = state
        .bus()
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
///
/// Same session resolution as [`dispatch`]; the correlated
/// round-trip (request id, parked oneshot, timeout) lives inside
/// [`Bus::request_daemon`].
pub async fn diagnose<S: KernelContext>(
    state: &S,
    session_id: &str,
) -> Result<Box<cctui_proto::diagnose::SessionDiagnose>, BusError> {
    let (adapter_id, machine_uuid) = resolve_session(state, session_id).await?;
    let response = state
        .bus()
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
pub async fn list_dirs<S: KernelContext>(
    state: &S,
    machine_uuid: Uuid,
    path: String,
) -> Result<Vec<String>, BusError> {
    match state.bus().request_daemon(machine_uuid, DaemonRequest::ListDirs { path }).await? {
        DaemonResponse::Dirs(dirs) => Ok(dirs),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon on `machine_uuid` for the git facts of `path` (spawn
/// dialog branch badge). Machine-addressed — no session involved.
pub async fn git_info<S: KernelContext>(
    state: &S,
    machine_uuid: Uuid,
    path: String,
    include_dirty: bool,
) -> Result<GitInfo, BusError> {
    let request = DaemonRequest::GitInfo { path, include_dirty };
    match state.bus().request_daemon(machine_uuid, request).await? {
        DaemonResponse::GitInfo(info) => Ok(info),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Ask the daemon on `machine_uuid` for the bytes of `path` (a file an agent
/// linked in a message). `cwd` widens the daemon's allow-list to the
/// session's working directory.
pub async fn read_file<S: KernelContext>(
    state: &S,
    machine_uuid: Uuid,
    path: String,
    max_bytes: u64,
    cwd: Option<String>,
) -> Result<ReadFileOk, BusError> {
    let request = DaemonRequest::ReadFile { path, max_bytes, cwd };
    match state.bus().request_daemon(machine_uuid, request).await? {
        DaemonResponse::File(file) => Ok(file),
        _ => Err(BusError::Closed), // unreachable: variant-matched
    }
}

/// Resolve a session id to its `(adapter_id, machine_uuid)` addressing pair.
async fn resolve_session<S: KernelContext>(
    state: &S,
    session_id: &str,
) -> Result<(String, Uuid), BusError> {
    super::resolve_session(state.pool(), session_id).await
}
