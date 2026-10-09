//! Speech routes for signed-in users.

use super::GET;
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/voice/config",
        "Whether speech is enabled and its effective models and voice.",
        get(routes::voice::config),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/transcribe",
        "Transcribe a multipart `file` audio part (size-capped) to text.",
        post(routes::voice::transcribe),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/voice/speak",
        "Synthesize text to audio with the instance speech service.",
        post(routes::voice::speak),
        Authn::Bearer,
        Human,
    )
}
