use axum::http::Method;
use axum::routing::{get, post};

use super::sess_read;
use crate::authz::{Authn, Routes};
use crate::preview::routes;

pub fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/sessions/{id}/previews",
        "Dev-server previews open on a session (owner only).",
        get(routes::list),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/previews/{pid}/ticket",
        "Mint a 60 s single-use ticket that signs the owner into a preview host.",
        post(routes::ticket),
        Authn::Bearer,
        sess_read(),
    )
}
