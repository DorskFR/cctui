//! Speech routes for signed-in users.

use super::GET;
use crate::authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/voice/config",
        "Whether speech is enabled and its effective models and voice.",
        get(cctui_server_voice::routes::config::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/voice/voices",
        "The voices the instance speech service offers.",
        get(cctui_server_voice::routes::voices::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/transcribe",
        "Transcribe a multipart `file` audio part (size-capped) to text.",
        post(cctui_server_voice::routes::transcribe::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(cctui_server_voice::routes::TRANSCRIBE_BODY_LIMIT)),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/speak",
        "Synthesize text to audio with the instance speech service.",
        post(cctui_server_voice::routes::speak::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
}
