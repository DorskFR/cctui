//! `POST /api/v1/sessions/dispatch` (CCT-107).
//!
//! Routes a [`DispatchRequest`] to the named [`Dispatcher`], pre-mints a
//! session row tagged with `origin = '<dispatcher_id>'` (status `new`),
//! and returns the handle. The actual transcript / event stream lands
//! later via the daemon — see CCT-107 follow-up.
//!
//! Auth: admin token or agent token. The intent is server-validated by
//! the dispatcher; route only checks the dispatcher name exists.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::Utc;

use cctui_proto::api::{ApiError, DispatchRequest, DispatchResponse};

use crate::auth::AuthContext;
use crate::dispatchers::{DispatchError, DispatchSpec};
use crate::state::AppState;

pub async fn dispatch(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
    Json(req): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<DispatchResponse>), (StatusCode, Json<ApiError>)> {
    let dispatcher = state.dispatchers.get(&req.dispatcher).map_err(|e| {
        let known = state.dispatchers.ids().join(", ");
        tracing::warn!("dispatch rejected: {e} (known: {known})");
        (StatusCode::NOT_FOUND, Json(ApiError { error: format!("{e}. known: [{known}]") }))
    })?;

    let session_id = req.session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let origin = dispatcher.id();
    let now = Utc::now();

    // Insert the session row first so that even if the backend dispatch
    // fails we have an audit trail of what was attempted. `payload` is
    // opaque but stored for observability; `reply_url` is a bearer
    // capability and is deliberately NOT persisted or logged.
    let metadata = serde_json::json!({
        "timeout": req.timeout,
        "payload": req.payload,
    });

    // Idempotency (CCT-107): `session_id` doubles as the dedup key. The
    // insert returns the id only when a NEW row was created; on conflict it
    // returns nothing and we short-circuit to the existing session WITHOUT
    // dispatching a second runtime job (the double-spawn this closes).
    let inserted: Option<String> = sqlx::query_scalar(
        r"INSERT INTO sessions (id, machine_id, working_dir, status, registered_at, last_heartbeat, metadata, origin)
           VALUES ($1, $2, $3, 'new', $4, $4, $5, $6)
           ON CONFLICT (id) DO NOTHING
           RETURNING id",
    )
    .bind(&session_id)
    .bind(format!("dispatch:{origin}"))
    .bind(format!("dispatch:{origin}"))
    .bind(now)
    .bind(&metadata)
    .bind(origin)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error inserting dispatched session: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    if inserted.is_none() {
        // Duplicate dispatch for an existing session_id — return the prior
        // handle/status instead of launching a second job.
        let existing: Option<(Option<String>, String)> =
            sqlx::query_as("SELECT dispatch_handle, status FROM sessions WHERE id = $1")
                .bind(&session_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(|e| {
                    tracing::error!("db error fetching existing dispatched session: {e}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiError { error: "database error".into() }),
                    )
                })?;
        let (handle, status) = existing.unwrap_or_else(|| (None, "new".into()));
        tracing::info!(%session_id, "dispatch deduped: returning existing session");
        return Ok((
            StatusCode::OK,
            Json(DispatchResponse {
                session_id,
                dispatcher: origin.to_string(),
                handle: handle.unwrap_or_default(),
                namespace: None,
                status: format!("duplicate ({status})"),
            }),
        ));
    }

    let spec = DispatchSpec {
        session_id: &session_id,
        timeout_minutes: req.timeout,
        reply_url: req.reply_url.as_deref(),
        payload: &req.payload,
    };

    let handle = match dispatcher.dispatch(&spec).await {
        Ok(h) => h,
        Err(e) => {
            let (code, msg) = match &e {
                DispatchError::InvalidIntent(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                DispatchError::UnknownDispatcher(_) => (StatusCode::NOT_FOUND, e.to_string()),
                DispatchError::Backend(_) => (StatusCode::BAD_GATEWAY, e.to_string()),
            };
            // Mark the session as failed-before-launch so it doesn't sit
            // in `new` forever.
            let _ = sqlx::query(
                "UPDATE sessions SET status = 'failed', last_heartbeat = $2 WHERE id = $1",
            )
            .bind(&session_id)
            .bind(Utc::now())
            .execute(&state.pool)
            .await;
            return Err((code, Json(ApiError { error: msg })));
        }
    };

    // Record the dispatcher handle so the UI can deep-link / kubectl.
    let _ = sqlx::query("UPDATE sessions SET dispatch_handle = $2 WHERE id = $1")
        .bind(&session_id)
        .bind(&handle.handle)
        .execute(&state.pool)
        .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(DispatchResponse {
            session_id,
            dispatcher: origin.to_string(),
            handle: handle.handle,
            namespace: handle.namespace,
            status: "dispatched".into(),
        }),
    ))
}
