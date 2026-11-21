//! Speech routes for signed-in users.

use super::GET;
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/voice/config",
        "Whether speech is enabled and its effective models and voice.",
        get(routes::voice::config::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/voice/voices",
        "The voices the instance speech service offers.",
        get(routes::voice::voices::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/transcribe",
        "Transcribe a multipart `file` audio part (size-capped) to text.",
        post(routes::voice::transcribe::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(routes::voice::TRANSCRIBE_BODY_LIMIT)),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/speak",
        "Synthesize text to audio with the instance speech service.",
        post(routes::voice::speak::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
}
