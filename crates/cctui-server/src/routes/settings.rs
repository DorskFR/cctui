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
const fn upgrade_step(data: &mut Value, v: i32) {
    let _ = (data, v);
}

/// The recognized harness modes (CCT-495). Anything else (typo, missing) clamps
/// to `bg` so a bad value can't wedge a daemon.
const HARNESS_MODES: [&str; 3] = ["bg", "sdk", "oneshot"];
const DEFAULT_HARNESS_MODE: &str = "bg";

/// Read the user-facing `harnessMode` from a settings `data` blob, clamping an
/// unknown/missing value to [`DEFAULT_HARNESS_MODE`].
fn harness_mode_of(data: &Value) -> &'static str {
    data.get("harnessMode").and_then(Value::as_str).map_or(DEFAULT_HARNESS_MODE, |m| {
        HARNESS_MODES.into_iter().find(|&v| v == m).unwrap_or(DEFAULT_HARNESS_MODE)
    })
}

/// Clamp `data.harnessMode` to a known value in place (rejecting typos), so a
/// stored row never carries an out-of-whitelist mode (CCT-495).
fn clamp_harness_mode(data: &mut Value) {
    let clamped = harness_mode_of(data);
    if let Some(obj) = data.as_object_mut() {
        // Only normalize when the key is present; absence means "default", which
        // we leave implicit rather than materializing.
        if obj.contains_key("harnessMode") {
            obj.insert("harnessMode".to_owned(), Value::String(clamped.to_owned()));
        }
    }
}

/// Map a user-facing harness mode (`bg`/`sdk`/`oneshot`, or unknown) to the
/// claude-code adapter's internal `config["mode"]` token (CCT-495). Today `bg`
/// is served by the existing `claude-daemon` path; `sdk`/`oneshot` pass through
/// as-is. Any unknown/missing value defaults to `bg`.
#[must_use]
pub fn harness_mode_to_adapter_token(harness_mode: Option<&str>) -> String {
    let mode = harness_mode.filter(|m| HARNESS_MODES.contains(m)).unwrap_or(DEFAULT_HARNESS_MODE);
    match mode {
        "bg" => "claude-daemon".to_owned(),
        other => other.to_owned(),
    }
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
    let mut data = migrate(body.data, body.version);
    // Whitelist harnessMode on write so a typo can't be stored (and later
    // wedge a daemon's reconcile); unknown → bg (CCT-495).
    clamp_harness_mode(&mut data);
    let new_mode = harness_mode_of(&data);

    // Snapshot the stored harnessMode before the upsert so we only push a fresh
    // Reconcile when it actually changes — unrelated settings edits must not
    // trigger a reconcile storm across the user's machines.
    let prev: Option<Value> =
        sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    let prev_mode = prev.as_ref().map_or(DEFAULT_HARNESS_MODE, |d| harness_mode_of(d));

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

    // Live-push a fresh Reconcile to every machine the user owns the instant the
    // harness mode changes, so connected daemons pick up the new mode without a
    // reconnect (CCT-495). Best-effort, only on change.
    if new_mode != prev_mode {
        let machines: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1 AND deleted_at IS NULL")
                .bind(ctx.user_id)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("db error listing machines for reconcile push: {e}");
                    Vec::new()
                });
        for machine_id in machines {
            if let Err(err) = crate::bus::push_reconcile(&state, machine_id).await {
                tracing::debug!(%machine_id, %err, "push_reconcile after harnessMode change failed");
            }
        }
    }

    Ok(Json(SettingsPayload { version, data }))
}

#[cfg(test)]
mod tests {
    use super::{clamp_harness_mode, harness_mode_of, harness_mode_to_adapter_token};
    use serde_json::json;

    #[test]
    fn harness_mode_of_clamps_unknown_and_missing_to_bg() {
        assert_eq!(harness_mode_of(&json!({})), "bg");
        assert_eq!(harness_mode_of(&json!({ "harnessMode": "wat" })), "bg");
        assert_eq!(harness_mode_of(&json!({ "harnessMode": 3 })), "bg");
        assert_eq!(harness_mode_of(&json!({ "harnessMode": "sdk" })), "sdk");
        assert_eq!(harness_mode_of(&json!({ "harnessMode": "oneshot" })), "oneshot");
    }

    #[test]
    fn clamp_rewrites_a_typo_but_leaves_absence_implicit() {
        let mut bad = json!({ "harnessMode": "wat", "theme": "dark" });
        clamp_harness_mode(&mut bad);
        assert_eq!(bad["harnessMode"], "bg");
        assert_eq!(bad["theme"], "dark");

        let mut none = json!({ "theme": "dark" });
        clamp_harness_mode(&mut none);
        assert!(none.get("harnessMode").is_none());
    }

    #[test]
    fn adapter_token_maps_bg_to_claude_daemon_and_passes_others_through() {
        assert_eq!(harness_mode_to_adapter_token(Some("bg")), "claude-daemon");
        assert_eq!(harness_mode_to_adapter_token(None), "claude-daemon");
        assert_eq!(harness_mode_to_adapter_token(Some("typo")), "claude-daemon");
        assert_eq!(harness_mode_to_adapter_token(Some("sdk")), "sdk");
        assert_eq!(harness_mode_to_adapter_token(Some("oneshot")), "oneshot");
    }
}
