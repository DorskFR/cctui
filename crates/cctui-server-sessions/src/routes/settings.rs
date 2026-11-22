//! `GET`/`PUT /api/v1/settings` — server-persisted per-user settings.
//!
//! Each user owns one `user_settings` row: an integer `version` and an open,
//! schema-less JSON `data` blob, so the webui can add keys without a migration.
//!
//! Shape upgrades run **in code** through [`migrate`], a pure sequential chain
//! up to [`CURRENT_VERSION`]. `GET` upgrades in memory without writing back
//! (reads stay side-effect-free, no write storm on deploy); `PUT` upgrades the
//! incoming payload and stores the current version.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde_json::{Value, json};

pub use cctui_proto::api::settings::SettingsPayload;

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;

/// Current settings payload schema version. Bump when adding a `migrate` arm.
const CURRENT_VERSION: i32 = 1;

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

/// The recognized harness modes. Anything else (typo, missing) clamps
/// to `bg` so a bad value can't wedge a daemon.
const HARNESS_MODES: [&str; 3] = ["bg", "sdk", "oneshot"];
const DEFAULT_HARNESS_MODE: &str = "bg";

/// Read the user-facing `harnessMode` from a settings `data` blob, clamping an
/// unknown/missing value to [`DEFAULT_HARNESS_MODE`].
pub fn harness_mode_of(data: &Value) -> &'static str {
    data.get("harnessMode").and_then(Value::as_str).map_or(DEFAULT_HARNESS_MODE, |m| {
        HARNESS_MODES.into_iter().find(|&v| v == m).unwrap_or(DEFAULT_HARNESS_MODE)
    })
}

/// Clamp `data.harnessMode` to a known value in place (rejecting typos), so a
/// stored row never carries an out-of-whitelist mode.
pub fn clamp_harness_mode(data: &mut Value) {
    let clamped = harness_mode_of(data);
    if let Some(obj) = data.as_object_mut() {
        // Only normalize when the key is present; absence means "default", which
        // we leave implicit rather than materializing.
        if obj.contains_key("harnessMode") {
            obj.insert("harnessMode".to_owned(), Value::String(clamped.to_owned()));
        }
    }
}

/// Recognized UI locales. An unknown/missing value is left as the
/// implicit "auto" (no key), so the webui falls back to the browser language.
const LOCALES: [&str; 2] = ["en", "fr"];

/// Clamp `data.locale` in place on write: drop the key unless it is a
/// recognized locale, so a stored row never carries an unknown language token
/// (which the webui would ignore anyway).
///
/// Absence means "auto".
pub fn clamp_locale(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if !obj.contains_key("locale") {
        return;
    }
    let ok = obj.get("locale").and_then(Value::as_str).is_some_and(|l| LOCALES.contains(&l));
    if !ok {
        obj.remove("locale");
    }
}

const WHIP_MODES: [&str; 2] = ["extend", "replace"];
const DEFAULT_WHIP_MODE: &str = "extend";
pub const MAX_WHIP_PHRASES: usize = 200;
const MAX_WHIP_PHRASE_CHARS: usize = 200;
const MAX_WHIP_GUIDANCE_CHARS: usize = 2000;

/// Normalize a raw `whipStopPhrases` value into the clamped block
/// `{ mode, phrases, guidance? }`, or `None` when it reduces to the default
/// (`extend` + no phrases + no guidance) so an absent setting stays implicit.
///
/// Phrases are trimmed, lowercased (matching is case-insensitive substring), had
/// empties dropped, deduped, and capped in per-phrase length and count so a
/// pathological blob can't bloat every worker spawn.
fn normalize_whip_stop_phrases(raw: Option<&Value>) -> Option<Value> {
    let obj = raw.and_then(Value::as_object);
    let mode = obj
        .and_then(|o| o.get("mode"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| WHIP_MODES.contains(m))
        .unwrap_or(DEFAULT_WHIP_MODE);
    let mut phrases: Vec<String> = Vec::new();
    if let Some(arr) = obj.and_then(|o| o.get("phrases")).and_then(Value::as_array) {
        for p in arr {
            let Some(s) = p.as_str() else { continue };
            let s: String = s.trim().to_lowercase().chars().take(MAX_WHIP_PHRASE_CHARS).collect();
            if s.is_empty() || phrases.iter().any(|e| e == &s) {
                continue;
            }
            phrases.push(s);
            if phrases.len() >= MAX_WHIP_PHRASES {
                break;
            }
        }
    }
    let guidance = obj
        .and_then(|o| o.get("guidance"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(|g| g.chars().take(MAX_WHIP_GUIDANCE_CHARS).collect::<String>());
    if mode == DEFAULT_WHIP_MODE && phrases.is_empty() && guidance.is_none() {
        return None;
    }
    let mut block = serde_json::Map::new();
    block.insert("mode".to_owned(), Value::String(mode.to_owned()));
    block.insert(
        "phrases".to_owned(),
        Value::Array(phrases.into_iter().map(Value::String).collect()),
    );
    if let Some(g) = guidance {
        block.insert("guidance".to_owned(), Value::String(g));
    }
    Some(Value::Object(block))
}

/// Clamp `data.whipStopPhrases` in place on write, removing the key when
/// it reduces to the default so a stored row never carries a no-op block.
pub fn clamp_whip_stop_phrases(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if !obj.contains_key("whipStopPhrases") {
        return;
    }
    match normalize_whip_stop_phrases(obj.get("whipStopPhrases")) {
        Some(v) => {
            obj.insert("whipStopPhrases".to_owned(), v);
        }
        None => {
            obj.remove("whipStopPhrases");
        }
    }
}

/// The clamped `whipStopPhrases` block for the daemon gateway-env pull,
/// or `None` when unset / reduced to the default (hook uses compiled defaults).
#[must_use]
pub fn whip_stop_phrases_of(data: &Value) -> Option<Value> {
    normalize_whip_stop_phrases(data.get("whipStopPhrases"))
}

use cctui_server_kernel::execution_settings::clamp_secret_scrub;
pub use cctui_server_kernel::execution_settings::secret_scrub_of;

/// Clamp `data.sessionEmojiPrefix` in place on write: coerce a present value
/// to a real boolean so a stored row never carries a truthy string. Absence
/// means "off", which stays implicit.
///
/// The flag is read back in SQL, not here: `update_status_signals` joins
/// `user_settings` while it persists the agent-supplied session name (see
/// `crate::session_emoji`), so a status update costs no extra round-trip.
pub fn clamp_session_emoji_prefix(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if obj.contains_key("sessionEmojiPrefix") {
        let on = obj.get("sessionEmojiPrefix").and_then(Value::as_bool).unwrap_or(false);
        obj.insert("sessionEmojiPrefix".to_owned(), Value::Bool(on));
    }
}

/// Clamp `data.autoResumeOnConnectionLoss` in place on write, like
/// [`clamp_session_emoji_prefix`]. Read back in SQL by `crate::auto_resume`,
/// which joins `user_settings` while it looks for stuck sessions.
const MAX_MACROS: usize = 100;
const MAX_MACRO_TITLE_CHARS: usize = 120;
const MAX_MACRO_PROMPT_CHARS: usize = 20_000;
const MAX_MACRO_FIELD_CHARS: usize = 400;

fn macro_str(entry: &Value, key: &str, max: usize) -> String {
    entry.get(key).and_then(Value::as_str).unwrap_or_default().trim().chars().take(max).collect()
}

/// Normalize the `macros` block (`{ enabled, items }`) the webui stores:
///
/// coerce `enabled` to a bool, keep only well-formed items (a title and a
/// prompt), cap lengths and count, and drop the key when nothing is left and
/// the feature is off. The server never runs a macro itself — the webui
/// turns one into a spawn — so this only guards the blob's size and shape.
pub fn clamp_macros(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    let Some(raw) = obj.get("macros") else { return };
    let enabled = raw.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let items: Vec<Value> = raw
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|e| e.is_object())
                .filter_map(|e| {
                    let title = macro_str(e, "title", MAX_MACRO_TITLE_CHARS);
                    let prompt = macro_str(e, "prompt", MAX_MACRO_PROMPT_CHARS);
                    if title.is_empty() || prompt.is_empty() {
                        return None;
                    }
                    let mut id = macro_str(e, "id", MAX_MACRO_FIELD_CHARS);
                    if id.is_empty() {
                        id = uuid::Uuid::new_v4().to_string();
                    }
                    let opt = |k: &str| {
                        let v = macro_str(e, k, MAX_MACRO_FIELD_CHARS);
                        if v.is_empty() { Value::Null } else { Value::String(v) }
                    };
                    let adapter = {
                        let a = macro_str(e, "adapter", MAX_MACRO_FIELD_CHARS);
                        if a.is_empty() { "claude-code".to_owned() } else { a }
                    };
                    Some(serde_json::json!({
                        "id": id,
                        "title": title,
                        "prompt": prompt,
                        "adapter": adapter,
                        "machine_id": opt("machine_id"),
                        "working_dir": opt("working_dir"),
                        "model": opt("model"),
                        "effort": opt("effort"),
                        "pool_id": opt("pool_id"),
                        "permission_mode": opt("permission_mode"),
                        "confirm": e.get("confirm").and_then(Value::as_bool).unwrap_or(true),
                    }))
                })
                .take(MAX_MACROS)
                .collect()
        })
        .unwrap_or_default();
    if !enabled && items.is_empty() {
        obj.remove("macros");
    } else {
        obj.insert("macros".to_owned(), serde_json::json!({ "enabled": enabled, "items": items }));
    }
}

const MAX_PLUGIN_ENTRIES: usize = 32;
const MAX_PLUGIN_ID_CHARS: usize = 64;

/// Normalize the `plugins` block the webui stores:
///
/// `{ enabled: { id: bool }, config: { id: { key: string } } }`. `declared`
/// gives an installed plugin's setting keys (`None` = not installed). Unknown
/// plugins and keys are dropped, values are strings of at most
/// [`cctui_server_kernel::extras_types::MAX_SETTING_VALUE_CHARS`], and the key goes away when
/// nothing is left.
pub fn clamp_plugins(data: &mut Value, declared: impl Fn(&str) -> Option<Vec<String>>) {
    let Some(obj) = data.as_object_mut() else { return };
    let Some(raw) = obj.get("plugins") else { return };
    let valid_id = |k: &String| !k.is_empty() && k.chars().count() <= MAX_PLUGIN_ID_CHARS;
    let enabled: serde_json::Map<String, Value> = raw
        .get("enabled")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, v)| valid_id(k) && v.is_boolean() && declared(k).is_some())
                .take(MAX_PLUGIN_ENTRIES)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let config: serde_json::Map<String, Value> = raw
        .get("config")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, _)| valid_id(k))
                .filter_map(|(id, values)| {
                    let keys = declared(id)?;
                    let values = values.as_object()?;
                    let kept: serde_json::Map<String, Value> = keys
                        .iter()
                        .filter_map(|key| {
                            let v = values.get(key)?.as_str()?;
                            (!v.is_empty()
                                && v.chars().count()
                                    <= cctui_server_kernel::extras_types::MAX_SETTING_VALUE_CHARS)
                                .then(|| (key.clone(), Value::String(v.to_owned())))
                        })
                        .collect();
                    (!kept.is_empty()).then(|| (id.clone(), Value::Object(kept)))
                })
                .take(MAX_PLUGIN_ENTRIES)
                .collect()
        })
        .unwrap_or_default();
    let mut block = serde_json::Map::new();
    if !enabled.is_empty() {
        block.insert("enabled".to_owned(), Value::Object(enabled));
    }
    if !config.is_empty() {
        block.insert("config".to_owned(), Value::Object(config));
    }
    if block.is_empty() {
        obj.remove("plugins");
    } else {
        obj.insert("plugins".to_owned(), Value::Object(block));
    }
}

pub fn clamp_auto_resume(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if obj.contains_key("autoResumeOnConnectionLoss") {
        let on = obj.get("autoResumeOnConnectionLoss").and_then(Value::as_bool).unwrap_or(false);
        obj.insert("autoResumeOnConnectionLoss".to_owned(), Value::Bool(on));
    }
}

pub async fn get_settings<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SettingsPayload>, StatusCode> {
    let row = sqlx::query_as::<_, (i32, Value, String)>(
        "SELECT version, data, updated_at::text FROM user_settings WHERE user_id = $1",
    )
    .bind(ctx.user_id)
    .fetch_optional(state.pool())
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let payload = match row {
        // Upgrade in memory only — do NOT persist on read (lazy persistence; the
        // upgraded shape is written on the next PUT).
        Some((version, data, revision)) => {
            let mut data = migrate(data, version);
            state.migrate_sound(&mut data);
            state.plugin_host_token_strip(&mut data);
            SettingsPayload { version: CURRENT_VERSION, data, revision: Some(revision) }
        }
        None => SettingsPayload { version: CURRENT_VERSION, data: json!({}), revision: None },
    };

    Ok(Json(payload))
}

async fn clamp_notifications<S: SessionsContext>(
    state: &S,
    data: &mut Value,
) -> Result<(), AppError> {
    let mut conn = state.pool().acquire().await?;
    state
        .clamp_settings(&mut conn, data)
        .await
        .map_err(|msg| AppError::new(StatusCode::BAD_REQUEST, msg))
}

pub async fn put_settings<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<SettingsPayload>,
) -> Result<Json<SettingsPayload>, AppError> {
    // Upgrade the incoming payload to the current shape before persisting, so
    // stored rows are always current-versioned.
    let mut data = migrate(body.data, body.version);
    // Whitelist harnessMode on write so a typo can't be stored (and later
    // wedge a daemon's reconcile); unknown → bg.
    clamp_harness_mode(&mut data);
    clamp_whip_stop_phrases(&mut data);
    // Reject the whole PUT if any user scrub regex fails to compile.
    if let Err(msg) = clamp_secret_scrub(&mut data) {
        tracing::info!("rejecting settings PUT: {msg}");
        return Err(AppError::new(StatusCode::BAD_REQUEST, msg));
    }
    clamp_notifications(&state, &mut data).await?;
    clamp_locale(&mut data);
    clamp_session_emoji_prefix(&mut data);
    clamp_auto_resume(&mut data);
    state.dispatch_archive_clamp(&mut data);
    clamp_macros(&mut data);
    crate::routes::spawn_memory::clamp(&mut data);
    clamp_plugins(&mut data, |id| state.plugin_setting_keys(id));
    let new_mode = harness_mode_of(&data);
    let new_scrub = serde_json::to_value(secret_scrub_of(&data)).unwrap_or(Value::Null);

    // Snapshot the stored harnessMode before the upsert so we only push a fresh
    // Reconcile when it actually changes — unrelated settings edits must not
    // trigger a reconcile storm across the user's machines.
    let stored: Option<(Value, String)> =
        sqlx::query_as("SELECT data, updated_at::text FROM user_settings WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_optional(state.pool())
            .await?;
    // A blob is saved whole, so a copy read before another tab's write would
    // erase that write. Refuse it before anything below acts on it.
    if let (Some(expected), Some((_, current))) = (&body.revision, &stored)
        && expected != current
    {
        return Err(stale_copy());
    }
    let prev = stored.map(|(data, _)| data);
    let prev_mode = prev.as_ref().map_or(DEFAULT_HARNESS_MODE, |d| harness_mode_of(d));
    let prev_scrub = prev
        .as_ref()
        .and_then(|d| serde_json::to_value(secret_scrub_of(d)).ok())
        .unwrap_or(Value::Null);

    state.plugin_host_token_reconcile(ctx.user_id, prev.as_ref(), &mut data).await?;

    // The revision check above can race a concurrent write; the conditional
    // update closes it. Without a revision the write is unconditional.
    let (version, mut data, revision) = sqlx::query_as::<_, (i32, Value, String)>(
        "INSERT INTO user_settings (user_id, version, data, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (user_id) DO UPDATE \
         SET version = EXCLUDED.version, data = EXCLUDED.data, updated_at = now() \
         WHERE $4::text IS NULL OR user_settings.updated_at::text = $4 \
         RETURNING version, data, updated_at::text",
    )
    .bind(ctx.user_id)
    .bind(CURRENT_VERSION)
    .bind(data)
    .bind(body.revision.as_deref())
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(stale_copy)?;

    // Live-push a fresh Reconcile to every machine the user owns the instant the
    // harness mode changes, so connected daemons pick up the new mode without a
    // reconnect. Best-effort, only on change.
    if new_mode != prev_mode || new_scrub != prev_scrub {
        let machines: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1 AND deleted_at IS NULL")
                .bind(ctx.user_id)
                .fetch_all(state.pool())
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("db error listing machines for reconcile push: {e}");
                    Vec::new()
                });
        for machine_id in machines {
            if let Err(err) =
                crate::targets::daemon::reconcile::push_reconcile(&state, machine_id).await
            {
                tracing::debug!(%machine_id, %err, "push_reconcile after harnessMode change failed");
            }
        }
    }

    state.plugin_host_token_strip(&mut data);
    Ok(Json(SettingsPayload { version, data, revision: Some(revision) }))
}

fn stale_copy() -> AppError {
    AppError::new(StatusCode::CONFLICT, "settings changed since this copy was read")
}
