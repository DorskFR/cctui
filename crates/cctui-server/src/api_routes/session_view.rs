//! Per-session read routes: detail, conversation and attachments.

use super::{GET, sess_read, sess_write};
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Per-session routes — ownership enforced by the `Resource(Session)`
        // guard. The `authz_layer` resolves `machine_uuid ->
        // machines.user_id` and applies `admin || owner == caller` before the
        // handler (404 unknown / 403 cross-user). Reads → `Action::Read`,
        // mutations/control → `Action::Write` (the action is recorded for
        // RBAC; the owner rule is identical for both today).
        .add(
            &[GET],
            "/sessions/{id}",
            "Get one session's details.",
            get(routes::sessions::get_session),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::PATCH],
            "/sessions/{id}",
            "Rename a session.",
            patch(routes::sessions::rename_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/conversation",
            "Fetch a session's normalized conversation transcript.",
            get(routes::sessions::get_conversation),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/images/{image_id}",
            "Fetch an agent-posted image blob.",
            get(routes::images::get_session_image),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/blobs/{hash}",
            "Resolve a content-addressed embedded-attachment blob.",
            get(routes::blobs::get_blob),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/attachments",
            "List the files the user uploaded into a session (served via blobs).",
            get(routes::attachments::get_session_attachments),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/diagnose",
            "Snapshot everything the daemon knows about a session, dated.",
            get(routes::diagnose::diagnose_session),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/langfuse",
            "Langfuse cost/usage rollup for a session.",
            get(routes::langfuse::session_langfuse),
            Authn::Bearer,
            sess_read(),
        )
}
