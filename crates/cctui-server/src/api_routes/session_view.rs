//! Per-session read routes: detail, conversation and attachments.

use super::{GET, sess_read, sess_write};
use crate::authz::Routes;
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch};
use cctui_server_kernel::authz::Authn;
use cctui_server_sessions::routes::{attachments, diagnose, fs, sessions};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Per-session routes — ownership enforced by the `Resource(Session)`
        // guard. The `enforce_route` resolves `machine_uuid ->
        // machines.user_id` and applies `admin || owner == caller` before the
        // handler (404 unknown / 403 cross-user). Reads → `Action::Read`,
        // mutations/control → `Action::Write` (the action is recorded for
        // RBAC; the owner rule is identical for both today).
        .add(
            &[GET],
            "/sessions/{id}",
            "Get one session's details.",
            get(sessions::get_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::PATCH],
            "/sessions/{id}",
            "Rename a session.",
            patch(sessions::rename_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/conversation",
            "Fetch a session's normalized conversation transcript.",
            get(sessions::get_conversation::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/search",
            "Find-in-conversation hit list for one session's transcript.",
            get(sessions::search_conversation::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/images/{image_id}",
            "Fetch an agent-posted image blob.",
            get(cctui_server_sessions::routes::images::get_session_image::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/blobs/{hash}",
            "Resolve a content-addressed embedded-attachment blob.",
            get(cctui_server_blobs::routes::get_blob::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/attachments",
            "List the files the user uploaded into a session (served via blobs).",
            get(attachments::get_session_attachments::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/linked-file",
            "Read a path linked in this session or the one that linked it, on its machine.",
            get(fs::read_linked_file::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/linked-file-owner",
            "Which session and machine linked a path, when this session did not.",
            get(fs::linked_file_owner::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/diagnose",
            "Snapshot everything the daemon knows about a session, dated.",
            get(diagnose::diagnose_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/langfuse",
            "Langfuse cost/usage rollup for a session.",
            get(cctui_server_extras::routes::langfuse::session_langfuse::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/events",
            "One session's lifecycle events, newest first.",
            get(cctui_server_sessions::routes::events::session_events::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
}

pub(super) fn register_result(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/sessions/{id}/result",
        "Completion state and final assistant text of a session.",
        get(sessions::get_result::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
}

pub(super) fn register_voice_notes(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/sessions/{id}/voice-notes/{note_id}",
        "Fetch an agent voice note's audio.",
        get(routes::voice_notes::get_voice_note),
        Authn::Bearer,
        sess_read(),
    )
}
