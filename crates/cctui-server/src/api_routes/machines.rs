//! Per-machine command, filesystem and model routes.

use super::GET;
use crate::authz::Authz::{self, Authenticated};
use crate::authz::{Action, Authn, IdFrom, ResourceKind, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/machines/{machine_id}/commands/pending",
        "Poll a machine's pending spawn/control commands.",
        get(routes::spawn::get_machine_commands),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/machines/{machine_id}/fs/dirs",
        "List directories on a machine (spawn dir picker).",
        get(routes::fs::list_dirs),
        Authn::Bearer,
        // Machine-owner guard: `machines.user_id`, id from the
        // `{machine_id}` path param.
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[GET],
        "/machines/{machine_id}/fs/gitinfo",
        "Git branch / detached HEAD of a directory on a machine (spawn dir badge).",
        get(routes::fs::git_info),
        Authn::Bearer,
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[GET],
        "/machines/{machine_id}/fs/file",
        "Read one file on a machine (agent-linked path): inline or blob redirect.",
        get(routes::fs::read_file),
        Authn::Bearer,
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[GET],
        "/machines/{machine_id}/codex-models",
        "Machine/account-scoped codex model catalog.",
        get(routes::codex_models::get_codex_models),
        Authn::Bearer,
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[Method::POST],
        "/machines/{machine_id}/codex-models/refresh",
        "Re-read every OpenAI account's codex model catalog from upstream.",
        post(routes::codex_models::refresh_codex_models),
        Authn::Bearer,
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[GET],
        "/models/codex",
        "Codex model catalog merged across every machine (newest report wins).",
        get(routes::codex_models::get_merged_codex_models),
        Authn::Bearer,
        Authenticated,
    )
}
