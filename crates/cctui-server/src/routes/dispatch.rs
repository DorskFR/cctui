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

use crate::auth::{AuthContext, machine_token, mint_secret, sha256_hex};
use crate::dispatchers::{DispatchError, DispatchSpec};
use crate::state::AppState;

/// Lazily fetch (or create) the caller's single persistent "dispatch" machine
/// and return its `(machine_id, machine_key)` (CCT-191).
///
/// Every dispatched worker pod runs a `cctui-daemon` that authenticates with
/// THIS one key, so all dispatched sessions register under one stable machine
/// — no per-pod enroll/deenroll churn and no `dispatch:<origin>` placeholder.
/// The key is stored plaintext (`machines.dispatch_key`) because the server
/// must hand it to pods verbatim; it ends up in pod env regardless, so the DB
/// is not a meaningfully weaker home for it. Reused across concurrent pods:
/// they share the machine row and key, and are told apart by `session_id`.
async fn ensure_dispatch_machine(
    state: &AppState,
    user_id: uuid::Uuid,
) -> anyhow::Result<(uuid::Uuid, String)> {
    if let Some((id, key)) = sqlx::query_as::<_, (uuid::Uuid, Option<String>)>(
        "SELECT id, dispatch_key FROM machines \
         WHERE user_id = $1 AND kind = 'dispatch' AND deleted_at IS NULL \
         ORDER BY first_seen_at LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    {
        if let Some(key) = key {
            return Ok((id, key));
        }
        // A dispatch machine exists but predates `dispatch_key` (or it was
        // cleared): rotate a fresh key into it rather than orphaning the row.
        let secret = mint_secret();
        let token = machine_token(&secret);
        let key_hash = sha256_hex(&token);
        sqlx::query("UPDATE machines SET dispatch_key = $2, key_hash = $3 WHERE id = $1")
            .bind(id)
            .bind(&token)
            .bind(&key_hash)
            .execute(&state.pool)
            .await?;
        return Ok((id, token));
    }

    let machine_id = uuid::Uuid::new_v4();
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);
    sqlx::query(
        "INSERT INTO machines (id, user_id, name, key_hash, kind, dispatch_key) \
         VALUES ($1, $2, 'dispatch', $3, 'dispatch', $4)",
    )
    .bind(machine_id)
    .bind(user_id)
    .bind(&key_hash)
    .bind(&token)
    .execute(&state.pool)
    .await?;
    // Same default adapters as a normal enroll so the daemon gets a meaningful
    // Reconcile and the claude-code/codex adapters surface sessions.
    let _ = sqlx::query(
        "INSERT INTO adapters_enabled (machine_id, adapter_id, config, enabled) \
         VALUES ($1, 'claude-code', '{}'::jsonb, TRUE), \
                ($1, 'codex', '{}'::jsonb, TRUE) \
         ON CONFLICT (machine_id, adapter_id) DO NOTHING",
    )
    .bind(machine_id)
    .execute(&state.pool)
    .await;
    tracing::info!(%user_id, %machine_id, "created dispatch machine");
    Ok((machine_id, token))
}

/// `GET /api/v1/sessions/dispatchers` — the ids of every configured
/// dispatcher (e.g. `["claude-worker"]`). The web UI uses this to decide
/// whether to offer the "Dispatch to k8s" mode and which dispatcher to target.
/// Any authenticated caller may read it (no role gate, matching dispatch
/// itself — see CCT-185 for per-user gating).
pub async fn list_dispatchers(
    State(state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
) -> Json<Vec<String>> {
    let mut ids = state.dispatchers.ids();
    ids.sort();
    Json(ids)
}

#[allow(clippy::too_many_lines)]
pub async fn dispatch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
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

    // Resolve the caller's stable dispatch machine (CCT-191). The web UI
    // dispatches with a user token, so `user_id` is present; admin/agent-token
    // callers (no owning user) fall back to the legacy placeholder identity.
    let dispatch_machine = match ctx.user_id {
        Some(uid) => match ensure_dispatch_machine(&state, uid).await {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::error!("ensure_dispatch_machine failed: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not resolve dispatch machine".into() }),
                ));
            }
        },
        None => None,
    };
    let machine_uuid = dispatch_machine.as_ref().map(|(id, _)| *id);
    // Bind the session to the dispatch machine up front so the card shows under
    // a real, named machine immediately — not a `dispatch:<origin>` placeholder
    // that only resolves once the pod re-registers.
    let machine_id_str =
        machine_uuid.map_or_else(|| format!("dispatch:{origin}"), |id| id.to_string());

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
        r"INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, registered_at, last_heartbeat, metadata, origin)
           VALUES ($1, $2, $3, $4, 'new', $5, $5, $6, $7)
           ON CONFLICT (id) DO NOTHING
           RETURNING id",
    )
    .bind(&session_id)
    .bind(&machine_id_str)
    .bind(machine_uuid)
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

    // Forward the dispatch machine key to the pod via a reserved payload key
    // (CCT-191). The dispatcher lifts it into `CCTUI_MACHINE_KEY` env and keeps
    // it OUT of the generic TASK_PAYLOAD_JSON, so the worker's daemon runs as
    // the shared dispatch machine without a per-pod enroll. It is deliberately
    // NOT persisted in `metadata` above (a bearer credential, like reply_url).
    let mut forwarded_payload = req.payload.clone();
    if let (Some((_, key)), Some(obj)) =
        (dispatch_machine.as_ref(), forwarded_payload.as_object_mut())
    {
        obj.insert("cctui_machine_key".into(), serde_json::Value::String(key.clone()));
    }

    let spec = DispatchSpec {
        session_id: &session_id,
        timeout_minutes: req.timeout,
        reply_url: req.reply_url.as_deref(),
        payload: &forwarded_payload,
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
