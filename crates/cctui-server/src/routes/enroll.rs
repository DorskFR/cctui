use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cctui_proto::api::ApiError;

use crate::auth::{AuthContext, Scope, machine_token, mint_secret, sha256_hex};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct EnrollRequest {
    pub hostname: String,
    #[serde(default, rename = "os")]
    pub _os: Option<String>,
    #[serde(default, rename = "arch")]
    pub _arch: Option<String>,
    /// Machine kind: `persistent` (a real dev-machine daemon, the
    /// default) or `ephemeral` (a dispatch/worker pod — one machine per
    /// dispatched session). Ephemeral machines are hidden from the New-session
    /// picker and reaped once they go stale. Unknown values fall back to
    /// `persistent`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Serialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
    pub server_version: &'static str,
}

pub async fn enroll(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, (StatusCode, Json<ApiError>)> {
    // Enrolling a machine requires the `enroll` scope. Admin holds it
    // by ceiling, so admin can now enroll (previously a `require_user` 403 —
    // bug #3 in the ticket). The machine is owned by the caller's user.
    ctx.requires(Scope::Enroll)
        .map_err(|s| (s, Json(ApiError { error: "the enroll scope is required".into() })))?;
    let user_id = ctx.user_id;

    if req.hostname.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "hostname required".into() }),
        ));
    }

    let machine_id = Uuid::new_v4();
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);

    // Only `ephemeral` is honoured as a non-default; anything else (including a
    // missing field, for older daemons) stays `persistent`.
    let kind = match req.kind.as_deref() {
        Some("ephemeral") => "ephemeral",
        _ => "persistent",
    };

    sqlx::query(
        "INSERT INTO machines (id, user_id, name, key_hash, kind, key_preview) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(machine_id)
    .bind(user_id)
    .bind(&req.hostname)
    .bind(&key_hash)
    .bind(kind)
    .bind(crate::auth::token_preview(&token))
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    // Register the machine key in the unified api_keys table with a
    // grant = the owner's full ceiling, so it behaves exactly like the owner
    // (matching the legacy machine-key semantics). The legacy machines.key_hash
    // is still written above for the dual-read cutover window.
    let grant = crate::auth::ceiling_of(&state.pool, user_id).await;
    let preview = crate::auth::token_preview(&token);
    if let Err(e) = crate::auth::register_key(
        &state.pool,
        crate::auth::NewKey {
            user_id,
            key_hash: &key_hash,
            key_preview: Some(&preview),
            label: Some(&req.hostname),
            kind: "machine",
            machine_id: Some(machine_id),
            dispatcher_id: None,
        },
        grant,
    )
    .await
    {
        tracing::warn!("failed to register machine key in api_keys: {e}");
    }

    // Default the new machine to the claude-code adapter so the daemon
    // gets a meaningful Reconcile out of the box and either harness can be
    // spawned/observed without a manual table edit. The codex
    // adapter only launches a `codex app-server` on an explicit Spawn and its
    // log-tail no-ops when `~/.codex/sessions` is absent, so enabling it is
    // safe even on machines without codex installed. Users can disable
    // adapters via direct table edits.
    let _ = sqlx::query(
        "INSERT INTO adapters_enabled (machine_id, adapter_id, config, enabled) \
         VALUES ($1, 'claude-code', '{}'::jsonb, TRUE), \
                ($1, 'codex', '{}'::jsonb, TRUE) \
         ON CONFLICT (machine_id, adapter_id) DO NOTHING",
    )
    .bind(machine_id)
    .execute(&state.pool)
    .await
    .map_err(|e| tracing::warn!("failed to insert default adapters_enabled rows: {e}"));

    tracing::info!(
        user_id = %user_id,
        machine_id = %machine_id,
        hostname = %req.hostname,
        kind,
        "machine enrolled"
    );

    Ok(Json(EnrollResponse {
        machine_id,
        machine_key: token,
        server_version: env!("CARGO_PKG_VERSION"),
    }))
}

/// `POST /api/v1/deenroll` — a machine removes itself, authenticated by its
/// own machine key. Used by ephemeral workers (one machine per pod) to clean
/// up on graceful exit so they don't accumulate in the machines tab. Revokes
/// and soft-deletes the row in one step (the admin purge flow requires a
/// prior revoke; self-deenroll does both). Unexpected pod deaths leave a row
/// to be cleaned via the admin revoke/delete UI.
pub async fn deenroll(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let machine_id = ctx.machine_id.ok_or_else(|| {
        (StatusCode::FORBIDDEN, Json(ApiError { error: "machine token required".into() }))
    })?;

    sqlx::query(
        "UPDATE machines SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(machine_id)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    tracing::info!(machine_id = %machine_id, "machine deenrolled (self)");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/machines/{machine_id}/status` — enrolment/connectivity
/// snapshot for one machine. Backs `cctui-daemon enroll
/// <ssh-target>`'s verification step: the operator polls this until the
/// freshly installed daemon shows `connected`. Owner-or-admin via the
/// route's `Authz::Resource(Machine, Read)` guard.
#[derive(Serialize)]
pub struct MachineStatusResponse {
    pub machine_id: Uuid,
    pub name: String,
    /// A daemon WS for this machine is currently terminated by some pod
    /// (this one, or a peer per `ws_presence`). Stronger than `liveness`,
    /// which a just-inserted row satisfies before any daemon ever connects.
    pub connected: bool,
    pub liveness: cctui_proto::models::MachineLiveness,
    pub last_seen_at: DateTime<Utc>,
    pub revoked: bool,
    /// Last-known per-subsystem daemon bandwidth. `None` until the
    /// machine's daemon has sent a heartbeat carrying counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<MachineBandwidth>,
}

/// Persisted per-subsystem byte totals for one machine.
#[derive(Serialize, sqlx::FromRow)]
pub struct MachineBandwidth {
    pub forward: i64,
    pub retransmit: i64,
    pub backfill: i64,
    pub self_update: i64,
    pub blob_put: i64,
    pub heartbeat: i64,
    pub updated_at: DateTime<Utc>,
}

pub async fn machine_status(
    State(state): State<AppState>,
    Path(machine_id): Path<Uuid>,
) -> Result<Json<MachineStatusResponse>, (StatusCode, Json<ApiError>)> {
    let row: Option<(String, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT name, last_seen_at, revoked_at FROM machines \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(machine_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    let Some((name, last_seen_at, revoked_at)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    // Local pod first; fall back to `ws_presence` so a WS terminated by a
    // peer replica still reads as connected. The 45s freshness window
    // mirrors `presence::LIVE_WITHIN_SECS`.
    let connected = state.bus.daemon_connected(machine_id)
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM ws_presence \
             WHERE kind = 'daemon' AND entity_id = $1 \
               AND heartbeat_at > now() - interval '45 seconds')",
        )
        .bind(machine_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);
    let bandwidth: Option<MachineBandwidth> = sqlx::query_as(
        "SELECT forward, retransmit, backfill, self_update, blob_put, heartbeat, updated_at \
         FROM machine_bandwidth WHERE machine_id = $1",
    )
    .bind(machine_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);
    Ok(Json(MachineStatusResponse {
        machine_id,
        name,
        connected,
        liveness: crate::machine_liveness::derive(last_seen_at),
        last_seen_at,
        revoked: revoked_at.is_some(),
        bandwidth,
    }))
}
