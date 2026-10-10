//! Throttles for the unauthenticated `/cctuiverse/v1/*` surface, on limiters
//! of their own so callers minting keys here can never flush the agent-tool
//! and login throttles.

use std::sync::LazyLock;
use std::time::Instant;

use axum::http::StatusCode;

use crate::error::AppError;
use crate::routes::peer::Limiter;

const PER_IP_PER_MIN: usize = 120;
const JOIN_PER_IP_PER_MIN: usize = 20;
pub const INBOUND_PER_LINK_PER_MIN: usize = crate::routes::peer::SEND_PER_MIN;
pub const READS_PER_LINK_PER_MIN: usize = 10;

static BY_IP: LazyLock<Limiter> = LazyLock::new(Limiter::default);
static BY_LINK: LazyLock<Limiter> = LazyLock::new(Limiter::default);

fn too_many() -> AppError {
    AppError::new(StatusCode::TOO_MANY_REQUESTS, "too many requests")
}

/// Before any lookup: every request from `caller`.
pub fn ip(caller: &str) -> Result<(), AppError> {
    if BY_IP.admit(&format!("any:{caller}"), PER_IP_PER_MIN, Instant::now()) {
        Ok(())
    } else {
        Err(too_many())
    }
}

pub fn join_ip(caller: &str) -> Result<(), AppError> {
    ip(caller)?;
    if BY_IP.admit(&format!("join:{caller}"), JOIN_PER_IP_PER_MIN, Instant::now()) {
        Ok(())
    } else {
        Err(too_many())
    }
}

/// After the signature verified, so only a key holder spends a link's budget.
pub fn link(key: &str, max: usize) -> Result<(), AppError> {
    if BY_LINK.admit(key, max, Instant::now()) { Ok(()) } else { Err(too_many()) }
}
