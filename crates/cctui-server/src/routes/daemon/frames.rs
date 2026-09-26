use cctui_proto::adapter::AdapterEvent;
use cctui_proto::ws::DaemonFrameUp;
use uuid::Uuid;

use super::bumps::Bumps;
use super::connection::{event_kind, event_local_id};
use super::events::handle_event;
use super::heartbeat::on_heartbeat;
use super::ingest::Stored;
use super::registration::register_announced_session;
use crate::state::AppState;

// Breadth-of-match dispatch over inbound daemon frames; complexity is per-frame
// handling, not nesting.
#[allow(clippy::cognitive_complexity)]
fn resolve_read_file_result(
    state: &AppState,
    request_id: Uuid,
    ok: bool,
    file: Option<cctui_proto::ws::ReadFileOk>,
    error_kind: Option<cctui_proto::ws::ReadFileErrorKind>,
    error: Option<String>,
) {
    let outcome = match (ok, file) {
        (true, Some(file)) => Ok(file),
        (true, None) => Err((
            cctui_proto::ws::ReadFileErrorKind::Io,
            "daemon returned no file payload".to_owned(),
        )),
        (false, _) => Err((
            error_kind.unwrap_or(cctui_proto::ws::ReadFileErrorKind::Io),
            error.unwrap_or_else(|| "daemon reported a read failure".to_owned()),
        )),
    };
    if !state.bus.resolve_read_file(request_id, outcome) {
        tracing::debug!(%request_id, "ReadFileResult for unknown request (timed out?)");
    }
}

pub(super) async fn process_frame(
    state: &AppState,
    bumps: &Bumps,
    machine_id: Uuid,
    user_id: Uuid,
    frame: DaemonFrameUp,
) -> anyhow::Result<()> {
    match frame {
        DaemonFrameUp::SessionRegistered { adapter_id, local_id } => {
            register_announced_session(state, machine_id, user_id, &adapter_id, &local_id).await
        }
        DaemonFrameUp::Event { adapter_id, event } => {
            // Machine-scoped codex model catalog: cache it by
            // machine_id — it is not a session event and never reaches the
            // per-session handler below.
            if let AdapterEvent::CodexModels { catalog } = event {
                crate::routes::codex_models::store_catalog(state, machine_id, catalog).await;
                return Ok(());
            }
            tracing::debug!(
                %adapter_id,
                kind = event_kind(&event),
                local_id = event_local_id(&event),
                "received event",
            );
            handle_event(state, bumps, machine_id, user_id, &adapter_id, event, Stored::Pending)
                .await
        }
        DaemonFrameUp::StageFilesResult { request_id, ok, paths, error } => {
            // Mid-chat attachment reply: fire the oneshot the
            // `POST /sessions/{id}/files` round-trip parked in the bus.
            let outcome = if ok {
                Ok(paths)
            } else {
                Err(error.unwrap_or_else(|| "daemon reported staging failure".to_owned()))
            };
            if !state.bus.resolve_stage_files(request_id, outcome) {
                tracing::debug!(%request_id, "StageFilesResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::ListDirsResult { request_id, ok, dirs, error } => {
            // Working-dir autocomplete reply: fire the oneshot the
            // `GET /machines/{id}/fs/dirs` round-trip parked in the bus.
            let outcome = if ok {
                Ok(dirs)
            } else {
                Err(error.unwrap_or_else(|| "daemon reported a listing failure".to_owned()))
            };
            if !state.bus.resolve_list_dirs(request_id, outcome) {
                tracing::debug!(%request_id, "ListDirsResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::GitInfoResult { request_id, ok, info, error } => {
            let outcome = match (ok, info) {
                (true, Some(info)) => Ok(info),
                (true, None) => Err("daemon returned no git info".to_owned()),
                (false, _) => {
                    Err(error.unwrap_or_else(|| "daemon reported a git info failure".to_owned()))
                }
            };
            if !state.bus.resolve_git_info(request_id, outcome) {
                tracing::debug!(%request_id, "GitInfoResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::ReadFileResult { request_id, ok, file, error_kind, error } => {
            resolve_read_file_result(state, request_id, ok, file, error_kind, error);
            Ok(())
        }
        frame @ DaemonFrameUp::Heartbeat { .. } => {
            on_heartbeat(state, machine_id, frame);
            Ok(())
        }
        frame if crate::preview::is_preview_frame(&frame) => {
            crate::preview::on_frame(state, machine_id, user_id, frame).await;
            Ok(())
        }
        // Any future #[non_exhaustive] variants are no-ops.
        _ => Ok(()),
    }
}
