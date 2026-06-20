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
use crate::dispatchers::enrolled::EnrolledDispatcher;
use crate::dispatchers::{DispatchError, DispatchSpec, Dispatcher};
use crate::ntfy::{self, Notification};
use crate::state::AppState;

/// Resolve a user's display name for notifications. Falls back to the raw uuid
/// (or `anonymous` for tokenless callers) so a notification always identifies
/// the caller even if the lookup misses.
async fn caller_label(state: &AppState, user_id: Option<uuid::Uuid>) -> String {
    let Some(uid) = user_id else {
        return "anonymous (admin token)".into();
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

/// Mint a per-session EPHEMERAL machine credential for a dispatched worker
/// (CCT-296), bound to the pre-minted `session_id` and the user's shared
/// `dispatch` machine (`machine_id`), expiring at the session deadline + grace.
///
/// This replaces the shared per-user `dispatch_key` (CCT-191) as the credential
/// handed to the worker pod: a leaked worker key now authenticates only its own
/// session and dies with it, instead of impersonating every dispatched session
/// of the user. It is an additive `auth_keys` row (CCT-410) — the auth path
/// already enforces `expires_at` and carries `machine_id` transparently, so the
/// daemon accepts it exactly like any other machine key — with `kind`
/// `'ephemeral'` and `session_id` set so the reaper can revoke it on the
/// session's terminal state. Scopes mirror the owner's ceiling (same as the
/// shared dispatch key did via the legacy machine path). Returns the plaintext
/// token to inject as `CCTUI_MACHINE_KEY`.
async fn mint_ephemeral_dispatch_key(
    state: &AppState,
    user_id: uuid::Uuid,
    machine_id: uuid::Uuid,
    session_id: &str,
    timeout_minutes: Option<u32>,
) -> anyhow::Result<String> {
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);
    // TTL = session deadline + grace. Fall back to a generous default when the
    // dispatch carries no timeout so the key still outlives a long run, then is
    // swept by the reaper. Grace covers post-deadline teardown/heartbeat.
    const GRACE_SECS: i64 = 30 * 60;
    const DEFAULT_TTL_SECS: i64 = 24 * 60 * 60;
    let lifetime = timeout_minutes
        .map_or(DEFAULT_TTL_SECS, |m| i64::from(m).saturating_mul(60))
        .saturating_add(GRACE_SECS);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(lifetime);

    let scopes = crate::auth::ceiling_of(&state.pool, user_id).await;
    let key_id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO auth_keys \
           (user_id, key_hash, key_preview, label, kind, machine_id, session_id, expires_at) \
         VALUES ($1, $2, $3, $4, 'ephemeral', $5, $6, $7) \
         ON CONFLICT (key_hash) DO UPDATE SET label = EXCLUDED.label \
         RETURNING id",
    )
    .bind(user_id)
    .bind(&key_hash)
    .bind(crate::auth::token_preview(&token))
    .bind(format!("dispatch session {session_id}"))
    .bind(machine_id)
    .bind(session_id)
    .bind(expires_at)
    .fetch_one(&state.pool)
    .await?;
    for scope in scopes {
        sqlx::query("INSERT INTO key_acls (key_id, scope) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(key_id.0)
            .bind(scope.as_str())
            .execute(&state.pool)
            .await?;
    }
    tracing::info!(%user_id, %machine_id, %session_id, "minted ephemeral dispatch key");
    Ok(token)
}

/// The account a dispatch should route through, after applying the CCT-427
/// fallback precedence: an explicit `req.account` always wins; otherwise the
/// dispatcher's bound default account (if any) is used. Each carries the
/// optional provider hint that disambiguates a name shared across providers.
///
/// Pure so the precedence is unit-testable without a DB. `None` means "no
/// account at all" — dispatch then behaves as it did before any account
/// routing existed (no gateway env injected).
fn resolve_dispatch_account(
    explicit_account: Option<&str>,
    explicit_provider: Option<&str>,
    default_account: Option<&(String, Option<String>)>,
) -> Option<(String, Option<String>)> {
    if let Some(name) = explicit_account.map(str::trim).filter(|a| !a.is_empty()) {
        return Some((
            name.to_string(),
            explicit_provider.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string),
        ));
    }
    default_account.cloned()
}

/// The OAuth account a dispatcher is bound to (CCT-427), resolved to the
/// `(name, provider)` `mint_session_env` consumes. Returns `None` when the
/// dispatcher row carries no `default_account_id` or it points at a deleted
/// account (the `ON DELETE SET NULL` FK clears the binding). A DB error
/// degrades to `None` so a lookup hiccup never blocks an otherwise-valid
/// dispatch.
async fn dispatcher_default_account(
    state: &AppState,
    dispatcher_name: &str,
    user_id: uuid::Uuid,
) -> Option<(String, Option<String>)> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT oa.name, d.default_account_provider \
         FROM dispatchers d \
         JOIN oauth_accounts oa ON oa.id = d.default_account_id \
         WHERE d.name = $1 AND d.user_id = $2 \
           AND d.deleted_at IS NULL AND d.revoked_at IS NULL \
         ORDER BY d.created_at LIMIT 1",
    )
    .bind(dispatcher_name)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Resolve a dispatcher *name* for the caller: an enrolled dispatcher (CCT-285)
/// takes precedence, falling back to the global env-configured http registry.
/// Returns `Ok(None)` to mean "no such name anywhere" so the caller can 404
/// distinctly from a permission denial. An enrolled dispatcher resolves to an
/// [`EnrolledDispatcher`] that sends Dispatch commands over the WS hub.
///
/// Ownership scoping mirrors machines & connectors (CCT-407): a user token sees
/// only its own enrolled dispatchers (`user_id = caller`); the admin token
/// (`user_id` is `None` — the only authenticated role without a user) gets the
/// same god-view it has elsewhere and resolves by name across ALL owners. Names
/// are unique per `(user_id, name)` but not globally, so the admin path takes a
/// deterministic `ORDER BY created_at LIMIT 1` — acceptable until dispatchers
/// are addressable by id; if two users enroll the same name, admin hits the
/// oldest.
async fn resolve_dispatcher(
    state: &AppState,
    user_id: Option<uuid::Uuid>,
    name: &str,
) -> Result<Option<std::sync::Arc<dyn Dispatcher>>, DispatchError> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM dispatchers \
         WHERE ($1::uuid IS NULL OR user_id = $1) AND name = $2 \
         AND deleted_at IS NULL AND revoked_at IS NULL \
         ORDER BY created_at LIMIT 1",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| DispatchError::Backend(format!("dispatcher lookup: {e}")))?;
    if let Some((dispatcher_id,)) = row {
        return Ok(Some(std::sync::Arc::new(EnrolledDispatcher::new(
            name,
            dispatcher_id,
            state.clone(),
        ))));
    }
    // Fall back to a global env-configured http dispatcher (escape hatch).
    match state.dispatchers.get(name) {
        Ok(d) => Ok(Some(d)),
        Err(DispatchError::UnknownDispatcher(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET /api/v1/sessions/dispatchers` — the names of every dispatcher the
/// caller can target: their enrolled dispatchers (CCT-235) merged with the
/// global env-configured registry. The web UI uses this to populate the
/// dispatch picker. Any authenticated caller may read it (no role gate,
/// matching dispatch itself — see CCT-185 for per-user gating).
///
/// Ownership scoping matches [`resolve_dispatcher`] (CCT-407): a user sees its
/// own enrolled dispatchers; the admin token (`user_id` is `None`) sees ALL of
/// them, so the picker offers everything an admin can actually dispatch to.
pub async fn list_dispatchers(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Json<Vec<String>> {
    let mut ids = state.dispatchers.ids();
    if let Ok(names) = sqlx::query_scalar::<_, String>(
        "SELECT name FROM dispatchers \
         WHERE ($1::uuid IS NULL OR user_id = $1) AND deleted_at IS NULL",
    )
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    {
        ids.extend(names);
    }
    ids.sort();
    ids.dedup();
    Json(ids)
}

#[allow(clippy::too_many_lines)]
pub async fn dispatch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<DispatchResponse>), (StatusCode, Json<ApiError>)> {
    let dispatcher = match resolve_dispatcher(&state, ctx.owner_filter(), &req.dispatcher).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            let known = state.dispatchers.ids().join(", ");
            tracing::warn!(
                "dispatch rejected: unknown dispatcher {} (known: {known})",
                req.dispatcher
            );
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: format!("unknown dispatcher: {}. known: [{known}]", req.dispatcher),
                }),
            ));
        }
        Err(e) => {
            tracing::error!("dispatcher resolution failed: {e}");
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(ApiError { error: format!("dispatcher unavailable: {e}") }),
            ));
        }
    };

    let session_id = req.session_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let origin = dispatcher.id();

    // Alert that a dispatch arrived (CCT-198). Built from the *original* payload
    // (before the machine key is injected) and no-ops unless ntfy is configured.
    let caller = caller_label(&state, ctx.owner_filter()).await;
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
    // daemon runs AS this one machine without a per-pod enroll. The web UI and
    // automation dispatch with a user token (user_id present); the admin token (no
    // owning user) dispatches without the shared identity.
    // Dispatch permission is now the `dispatch` scope (CCT-410), enforced
    // uniformly for every caller. The migration backfilled `dispatch` into
    // user_acls only where the legacy `can_dispatch` flag was set, so this is
    // transparent: a user previously toggled off has no `dispatch` scope and is
    // still denied. Admin holds the scope by ceiling.
    ctx.requires(crate::auth::Scope::Dispatch).map_err(|s| {
        tracing::warn!(uid = %ctx.user_id, "dispatch denied: caller lacks dispatch scope");
        (s, Json(ApiError { error: "dispatch is not permitted for this token".into() }))
    })?;

    let mut forwarded_payload = req.payload.clone();
    // The shared dispatch-machine identity + account routing apply to a real
    // owning user (the web UI / automation dispatch with a user token). An admin token
    // (`owner_filter` is `None`) dispatches without the shared identity.
    if let Some(uid) = ctx.owner_filter() {
        // The shared `dispatch` machine still groups every dispatched session
        // under one logical machine (UI grouping unchanged, CCT-191) — but the
        // credential handed to the pod is now a PER-SESSION ephemeral key
        // (CCT-296), so a leaked worker key only impersonates its own session
        // and expires with it. `ensure_dispatch_machine` is kept for the row
        // (and `dispatch_key` rotation) it owns.
        let (machine_id, _shared_key) =
            ensure_dispatch_machine(&state, uid).await.map_err(|e| {
                tracing::error!("ensure_dispatch_machine failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not resolve dispatch machine".into() }),
                )
            })?;
        let key = mint_ephemeral_dispatch_key(&state, uid, machine_id, &session_id, req.timeout)
            .await
            .map_err(|e| {
                tracing::error!("mint_ephemeral_dispatch_key failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not mint dispatch credential".into() }),
                )
            })?;
        if let Some(obj) = forwarded_payload.as_object_mut() {
            obj.insert("cctui_machine_key".into(), serde_json::Value::String(key));
        }

        // Account-scoped routing on the dispatch path (CCT-399 / CCT-427): mint
        // a session-scoped gateway token bound to this (session, account) and
        // merge the gateway base-url + token into `payload.env` so the worker
        // pod routes through the passthrough gateway under that account. The
        // account is the explicit `req.account` if the caller picked one, else
        // the dispatcher's bound default account (CCT-427) — an empty
        // `req.account` falls back, an explicit one always overrides. With no
        // account either way, dispatch injects no gateway env (unchanged). The
        // dispatch path runs a claude-worker, so the family falls back to
        // anthropic when no provider is given.
        let default_account = if req.account.as_deref().map(str::trim).is_none_or(str::is_empty) {
            dispatcher_default_account(&state, &req.dispatcher, uid).await
        } else {
            None
        };
        if let Some((account_name, account_provider)) = resolve_dispatch_account(
            req.account.as_deref(),
            req.provider.as_deref(),
            default_account.as_ref(),
        ) {
            match crate::routes::gateway::mint_session_env(
                &state,
                uid,
                &account_name,
                account_provider.as_deref().filter(|p| !p.trim().is_empty()),
                "claude-code",
                &session_id,
            )
            .await
            {
                Ok(Some(gateway_env)) => {
                    if let Some(obj) = forwarded_payload.as_object_mut() {
                        let env = obj
                            .entry("env")
                            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                        if let Some(env_obj) = env.as_object_mut() {
                            for (k, v) in gateway_env {
                                env_obj.insert(k, serde_json::Value::String(v));
                            }
                        }
                    }
                }
                Ok(None) => {
                    return Err((
                        StatusCode::NOT_FOUND,
                        Json(ApiError { error: format!("no account named {account_name:?}") }),
                    ));
                }
                Err(e) => {
                    tracing::error!("mint_session_env (dispatch) failed: {e}");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiError { error: "could not provision account session".into() }),
                    ));
                }
            }
        }
    }

    // Register a server-emitted completion webhook (CCT-294) when the caller
    // supplied `notify_url`. The server fires it once the dispatched session
    // reaches a terminal state — covering crash cases the worker's REPLY_URL
    // exit trap can miss. Scoped to a real owning user (admin-token dispatches
    // carry no owner, so they keep the REPLY_URL trap only). Best-effort: a
    // registration failure never blocks the dispatch. The `task_id` echoed back
    // is the dispatch payload's `task_id` if present, else the session id.
    if let (Some(uid), Some(notify_url)) =
        (ctx.owner_filter(), req.notify_url.as_deref().filter(|u| !u.trim().is_empty()))
    {
        let task_id =
            req.payload.get("task_id").and_then(serde_json::Value::as_str).unwrap_or(&session_id);
        crate::webhook::register(
            &state,
            &session_id,
            uid,
            notify_url,
            req.notify_secret.as_deref().filter(|s| !s.trim().is_empty()),
            task_id,
        )
        .await;
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
                DispatchError::Unsupported(_) => (StatusCode::NOT_IMPLEMENTED, e.to_string()),
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

#[cfg(test)]
mod tests {
    use super::resolve_dispatch_account;

    // Helper: the bound default account a dispatcher carries (CCT-427).
    fn bound(name: &str, provider: Option<&str>) -> (String, Option<String>) {
        (name.to_string(), provider.map(str::to_string))
    }

    #[test]
    fn explicit_account_is_used_verbatim() {
        // An explicit `req.account` routes through that account regardless of any
        // dispatcher binding (the override case).
        let got = resolve_dispatch_account(Some("work"), Some("anthropic"), None);
        assert_eq!(got, Some(("work".into(), Some("anthropic".into()))));
    }

    #[test]
    fn explicit_account_overrides_bound_default() {
        // CCT-427: explicit account wins over the dispatcher's default, and uses
        // the explicit provider hint (not the bound one).
        let default = bound("automation-account", Some("openai"));
        let got = resolve_dispatch_account(Some("work"), Some("anthropic"), Some(&default));
        assert_eq!(got, Some(("work".into(), Some("anthropic".into()))));
    }

    #[test]
    fn empty_account_falls_back_to_bound_default() {
        // CCT-427: an empty / whitespace `req.account` falls back to the
        // dispatcher's bound default account (name + its provider hint).
        let default = bound("automation-account", Some("anthropic"));
        assert_eq!(
            resolve_dispatch_account(None, None, Some(&default)),
            Some(("automation-account".into(), Some("anthropic".into())))
        );
        assert_eq!(
            resolve_dispatch_account(Some("   "), None, Some(&default)),
            Some(("automation-account".into(), Some("anthropic".into())))
        );
    }

    #[test]
    fn no_account_and_no_binding_is_none() {
        // Unbound dispatcher + no explicit account: no gateway env injected
        // (behaves as before any account routing existed).
        assert_eq!(resolve_dispatch_account(None, None, None), None);
        assert_eq!(resolve_dispatch_account(Some(""), None, None), None);
    }
}
