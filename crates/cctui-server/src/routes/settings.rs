//! `GET`/`PUT /api/v1/settings` — server-persisted per-user settings (CCT-425,
//! epic CCT-357).
//!
//! Each user owns a single row in `user_settings` holding a `version` and an
//! open JSON `data` blob. The blob is deliberately schema-less on the wire (we
//! store `serde_json::Value`, not a rigid struct) so the webui can grow new
//! keys without a server change or SQL migration.
//!
//! ## Versioning & lazy persistence
//!
//! `data` is versioned by an integer. Upgrades between payload shapes are
//! applied **in code** by [`migrate`] (NOT by SQL) — a pure, sequential chain
//! from the stored version up to [`CURRENT_VERSION`]. The chain runs on both
//! read and write:
//!   - On `GET` we upgrade the stored payload in memory so callers always see
//!     the current shape, but we do NOT write the upgraded row back. Persistence
//!     is **lazy**: the upgraded payload is written on the *next* `PUT` (the
//!     user's next settings edit), at which point the stored `version` advances.
//!     This keeps reads side-effect-free and avoids a write storm on deploy.
//!   - On `PUT` we upgrade the incoming payload to `CURRENT_VERSION` before the
//!     upsert and store that version, so writes are always current-shaped.
//!
//! ## Reserved keys
//!
//! `keymap` and `shortcutsEnabled` are reserved for the later keyboard-shortcuts
//! feature. Because `data` is an open JSON object, no work is needed now to
//! "reserve" them — adding those keys later is a no-op on the storage side.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::auth::AuthContext;
use crate::state::AppState;

/// Current settings payload schema version. Bump when adding a `migrate` arm.
const CURRENT_VERSION: i32 = 1;

#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SettingsPayload {
    pub version: i32,
    pub data: Value,
}

/// Upgrade a settings `data` payload from `from` up to [`CURRENT_VERSION`].
///
/// Pure function: applies sequential per-version transforms. For v1 there are no
/// prior versions, so this is a passthrough that simply clamps unknown/older
/// versions forward to the current shape. Adding a v1->v2 transform later is a
/// one-line `1 => { /* mutate data */ }` arm in the match.
fn migrate(mut data: Value, from: i32) -> Value {
    let mut v = from.max(0);
    while v < CURRENT_VERSION {
        upgrade_step(&mut data, v);
        v += 1;
    }
    data
}

/// Upgrade `data` in place from version `v` to `v + 1`. No prior versions exist
/// yet, so this is a no-op; add a `1 => { /* v1 -> v2 */ }` arm here when bumping
/// [`CURRENT_VERSION`].
fn upgrade_step(data: &mut Value, v: i32) {
    let _ = (data, v);
}

pub async fn get_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SettingsPayload>, StatusCode> {
    let row = sqlx::query_as::<_, (i32, Value)>(
        "SELECT version, data FROM user_settings WHERE user_id = $1",
    )
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let payload = match row {
        // Upgrade in memory only — do NOT persist on read (lazy persistence; the
        // upgraded shape is written on the next PUT).
        Some((version, data)) => {
            SettingsPayload { version: CURRENT_VERSION, data: migrate(data, version) }
        }
        None => SettingsPayload { version: CURRENT_VERSION, data: json!({}) },
    };

    Ok(Json(payload))
}

pub async fn put_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<SettingsPayload>,
) -> Result<Json<SettingsPayload>, StatusCode> {
    // Upgrade the incoming payload to the current shape before persisting, so
    // stored rows are always current-versioned.
    let data = migrate(body.data, body.version);

    let (version, data) = sqlx::query_as::<_, (i32, Value)>(
        "INSERT INTO user_settings (user_id, version, data, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (user_id) DO UPDATE \
         SET version = EXCLUDED.version, data = EXCLUDED.data, updated_at = now() \
         RETURNING version, data",
    )
    .bind(ctx.user_id)
    .bind(CURRENT_VERSION)
    .bind(data)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(SettingsPayload { version, data }))
}
