//! Per-session diagnose endpoint.
//!
//! `GET /api/v1/sessions/{id}/diagnose` — one call that snapshots everything
//! the owning daemon knows about the session (each fact dated + sourced, plus
//! the arbitration verdict) and merges the server-side facts the daemon can't
//! see (DB row status, gateway/account binding, machine heartbeat freshness).
//!
//! Fail-soft: a daemon that is offline, slow, or yields
//! `daemon: null` + `daemon_error`, with the server facts still served —
//! exactly the situation this endpoint exists to debug.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::diagnose::{ServerDiagnose, SessionDiagnoseResponse};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub async fn diagnose_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionDiagnoseResponse>, AppError> {
    // Ownership is enforced by the authz layer (`sess_read`); this lookup
    // supplies the server-side facts and 404s a genuinely unknown id.
    let row: Option<(Option<String>, Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT status, adapter_id, machine_uuid FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((status, adapter_id, machine_uuid)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };

    // Gateway/account binding: the live (non-revoked) session tokens and the
    // account identities they resolve to — the same join the session list
    // uses for `account_name`, all bindings rather than the latest.
    let accounts: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT a.name \
         FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = $1 AND st.revoked_at IS NULL \
         ORDER BY a.name",
    )
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await?;

    let machine_last_seen: Option<chrono::DateTime<chrono::Utc>> = match machine_uuid {
        Some(machine) => sqlx::query_scalar("SELECT last_seen_at FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_optional(&state.pool)
            .await?
            .flatten(),
        None => None,
    };

    let server = ServerDiagnose {
        status,
        adapter_id,
        account_bound: !accounts.is_empty(),
        accounts,
        machine_id: machine_uuid.map(|m| m.to_string()),
        machine_last_seen_ms: machine_last_seen.map(|t| t.timestamp_millis()),
    };

    // The daemon round-trip (server → daemon WS → adapter → back). Any
    // failure — offline machine, timeout, daemon that drops the
    // unknown command — degrades to `daemon: null` + the error string.
    let (daemon, daemon_error) = match crate::bus::diagnose(&state, &session_id).await {
        Ok(report) => (Some(*report), None),
        Err(err) => (None, Some(err.to_string())),
    };

    Ok(Json(SessionDiagnoseResponse { session_id, daemon, daemon_error, server }))
}
