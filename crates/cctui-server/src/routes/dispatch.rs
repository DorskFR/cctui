//! `POST /api/v1/sessions/dispatch` (CCT-107 / CCT-191).
//!
//! Routes a [`DispatchRequest`] to the named [`Dispatcher`] and returns the
//! handle. It does NOT create a session row — the worker pod's `cctui-daemon`
//! registers the real session directly under the shared `dispatch` machine
//! (CCT-191), so a pre-minted placeholder can't strand alongside it.
//!
//! Auth: any authenticated caller. A user-scoped token also gets the caller's
//! stable `dispatch` machine key injected into the forwarded payload so the
//! worker runs as that one machine.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::api::{ApiError, DispatchRequest, DispatchResponse};

use crate::auth::{AuthContext, machine_token, mint_secret, sha256_hex};
use crate::dispatchers::{DispatchError, DispatchSpec};
use crate::ntfy::{self, Notification};
use crate::state::AppState;

/// Resolve a user's display name for notifications. Falls back to the raw uuid
/// (or `anonymous` for tokenless callers) so a notification always identifies
/// the caller even if the lookup misses.
async fn caller_label(state: &AppState, user_id: Option<uuid::Uuid>) -> String {
    let Some(uid) = user_id else {
        return "anonymous (agent/admin token)".into();
    };
    sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(uid)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .map_or_else(|| uid.to_string(), |name| format!("{name} ({uid})"))
}

/// Best-effort one-line summary of what's being run, pulled from the opaque
/// payload. The dispatch payload carries no literal prompt — the actual prompt
/// lives in a named template (`prompt_file`) resolved worker-side — so we
/// surface the keys that identify the run: flow, prompt file, model, etc. The
/// full payload is shown separately; this is just a glanceable header. Returns
/// `(no recognized fields)` when none of the known keys are present.
fn summarize(payload: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    for key in ["flow", "prompt_file", "model", "effort", "repo"] {
        if let Some(s) = payload.get(key).and_then(|v| v.as_str()) {
            parts.push(format!("{key}={s}"));
        }
    }
    // Common nested location for the target project/repo context.
    if let Some(proj) = payload.pointer("/context/project").and_then(|v| v.as_str()) {
        parts.push(format!("project={proj}"));
    }
    if parts.is_empty() { "(no recognized fields)".into() } else { parts.join("  ") }
}

/// Render the payload for a notification with secrets redacted — the injected
/// machine key (a bearer secret) and any `env` map (user-supplied environment
/// secrets, CCT-202) — and truncated so a huge payload doesn't blow up the push
/// body.
fn payload_for_notify(payload: &serde_json::Value) -> String {
    const MAX: usize = 1500;
    let mut p = payload.clone();
    if let Some(obj) = p.as_object_mut() {
        if obj.contains_key("cctui_machine_key") {
            obj.insert("cctui_machine_key".into(), serde_json::Value::String("<redacted>".into()));
        }
        // `payload.env` carries environment secrets for the k8s worker (the
        // external dispatcher turns them into pod env / an ephemeral Secret).
        // Redact the values — they must never land in a notification.
        if let Some(env) = obj.get("env").and_then(serde_json::Value::as_object) {
            let redacted: serde_json::Map<String, serde_json::Value> = env
                .keys()
                .map(|k| (k.clone(), serde_json::Value::String("<redacted>".into())))
                .collect();
            obj.insert("env".into(), serde_json::Value::Object(redacted));
        }
    }
    let mut s = serde_json::to_string_pretty(&p).unwrap_or_else(|_| p.to_string());
    if s.len() > MAX {
        s.truncate(MAX);
        s.push_str("\n… (truncated)");
    }
    s
}

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

    // Alert that a dispatch arrived (CCT-198). Built from the *original* payload
    // (before the machine key is injected) and no-ops unless ntfy is configured.
    let caller = caller_label(&state, ctx.user_id).await;
    let summary = summarize(&req.payload);
    ntfy::notify(
        &state.config,
        Notification {
            title: format!("Dispatch received → {}", req.dispatcher),
            message: format!(
                "user: {caller}\nsession: {session_id}\ndispatcher: {}\n\n{summary}\n\npayload:\n{}",
                req.dispatcher,
                payload_for_notify(&req.payload),
            ),
            tags: "inbox_tray".into(),
            priority: 3,
        },
    );

    // We do NOT pre-create a session row (CCT-191): `claude --bg` mints its own
    // session id and ignores `--session-id`, so a pre-minted row can never be
    // adopted by the worker's real session — it would just linger as an empty
    // `dispatch:<origin>` placeholder alongside the real session. Instead the
    // worker's cctui-daemon registers the real session directly under the
    // shared `dispatch` machine (so it shows up like any other session). Double
    // dispatch is still idempotent: the dispatcher derives the k8s Job name from
    // `sha(session_id)`, so a repeat maps to the same Job (409 → same handle).

    // Resolve the caller's stable dispatch machine and forward its key to the
    // pod via a reserved payload key (CCT-191). The dispatcher lifts it into
    // `CCTUI_MACHINE_KEY` and keeps it OUT of TASK_PAYLOAD_JSON, so the worker's
    // daemon runs AS this one machine without a per-pod enroll. The web UI
    // dispatches with a user token (user_id present); admin/agent-token callers
    // (no owning user) dispatch without the shared identity.
    let mut forwarded_payload = req.payload.clone();
    if let Some(uid) = ctx.user_id {
        let (_, key) = ensure_dispatch_machine(&state, uid).await.map_err(|e| {
            tracing::error!("ensure_dispatch_machine failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not resolve dispatch machine".into() }),
            )
        })?;
        if let Some(obj) = forwarded_payload.as_object_mut() {
            obj.insert("cctui_machine_key".into(), serde_json::Value::String(key));
        }
    }

    let spec = DispatchSpec {
        session_id: &session_id,
        timeout_minutes: req.timeout,
        reply_url: req.reply_url.as_deref(),
        payload: &forwarded_payload,
    };

    let handle = match dispatcher.dispatch(&spec).await {
        Ok(h) => {
            ntfy::notify(
                &state.config,
                Notification {
                    title: format!("Dispatch forwarded → {}", req.dispatcher),
                    message: format!(
                        "user: {caller}\nsession: {session_id}\nhandle: {}\nnamespace: {}\nstatus: {}",
                        h.handle,
                        h.namespace.as_deref().unwrap_or("-"),
                        h.status.as_deref().unwrap_or("dispatched"),
                    ),
                    tags: "white_check_mark".into(),
                    priority: 3,
                },
            );
            h
        }
        Err(e) => {
            ntfy::notify(
                &state.config,
                Notification {
                    title: format!("Dispatch FAILED → {}", req.dispatcher),
                    message: format!("user: {caller}\nsession: {session_id}\nerror: {e}"),
                    tags: "rotating_light".into(),
                    priority: 5,
                },
            );
            let (code, msg) = match &e {
                DispatchError::InvalidIntent(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                DispatchError::UnknownDispatcher(_) => (StatusCode::NOT_FOUND, e.to_string()),
                DispatchError::Backend(_) => (StatusCode::BAD_GATEWAY, e.to_string()),
            };
            return Err((code, Json(ApiError { error: msg })));
        }
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(DispatchResponse {
            session_id,
            dispatcher: origin.to_string(),
            handle: handle.handle,
            namespace: handle.namespace,
            // Surface the dispatcher's outcome verbatim so a re-dispatch onto a
            // terminal Job reads as `redispatched` (a fresh run) rather than a
            // misleading `dispatched` (CCT-207). Older dispatchers omit it →
            // preserve the historical `dispatched`.
            status: handle.status.unwrap_or_else(|| "dispatched".into()),
        }),
    ))
}
