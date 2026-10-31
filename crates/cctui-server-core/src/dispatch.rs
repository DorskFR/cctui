//! Shared dispatch preparation, policy, worker identity and forwarding.
use crate::account_resolve::AccountResolutionContext;
use crate::auth::{AuthContext, machine_token, mint_secret, sha256_hex};
use crate::dispatchers::enrolled::EnrolledDispatcher;
use crate::dispatchers::{DispatchError, DispatchSpec, Dispatcher};
use crate::gateway::mint::MintContext;
use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::{ApiError, DispatchRequest, DispatchResponse};

/// Optional host notification; the full host retains its configured delivery.
pub struct DispatchNotification {
    pub title: String,
    pub message: String,
    pub tags: String,
    pub priority: u8,
}

/// Execution resources and explicit host side effects for dispatch preparation.
pub trait DispatchContext: AccountResolutionContext + MintContext {
    fn dispatch_pool(&self) -> &sqlx::PgPool {
        MintContext::pool(self)
    }
    fn dispatch_bus(&self) -> &crate::bus::Bus;
    fn dispatchers(&self) -> &crate::dispatchers::Registry;
    fn notify(&self, notification: DispatchNotification);
    fn cache_spawn_capability(&self, session: &str, capability: cctui_proto::api::SpawnCapability);
    fn spawn_default_capability(
        &self,
    ) -> impl std::future::Future<Output = cctui_proto::api::SpawnCapability> + Send;
    fn register_completion_webhook(
        &self,
        ctx: &AuthContext,
        req: &DispatchRequest,
        session: &str,
    ) -> impl std::future::Future<Output = ()> + Send;
    fn remember_dispatch_context(
        &self,
        user: Option<uuid::Uuid>,
        req: &DispatchRequest,
        session: &str,
    ) -> impl std::future::Future<Output = ()> + Send;
}

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

/// Resolve a user's display name for notifications. Falls back to the raw uuid
/// (or `anonymous` for tokenless callers) so a notification always identifies
/// the caller even if the lookup misses.
async fn caller_label(state: &impl DispatchContext, user_id: Option<uuid::Uuid>) -> String {
    let Some(uid) = user_id else {
        return "anonymous (admin token)".into();
    };
    sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(uid)
        .fetch_optional(state.dispatch_pool())
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
/// secrets) — and truncated so a huge payload doesn't blow up the push
/// body.
fn payload_for_notify(payload: &serde_json::Value) -> String {
    const MAX: usize = 1500;
    let mut p = payload.clone();
    if let Some(obj) = p.as_object_mut() {
        if obj.contains_key("cctui_machine_key") {
            obj.insert("cctui_machine_key".into(), serde_json::Value::String("<redacted>".into()));
        }
        // `payload.env` carries worker environment secrets (the kube
        // dispatcher stores them in a per-Job Secret). Redact the values.
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
/// and return its `(machine_id, machine_key)`.
///
/// Every dispatched worker pod runs a `cctui-daemon` that authenticates with
/// THIS one key, so all dispatched sessions register under one stable machine
/// — no per-pod enroll/deenroll churn and no `dispatch:<origin>` placeholder.
/// The key is stored plaintext (`machines.dispatch_key`) because the server
/// must hand it to pods verbatim. It is long-lived and shared by every
/// dispatched session, so dispatchers must keep it out of anything readable
/// beside the worker itself (the kube dispatcher puts it in a Job-owned Secret). Reused across concurrent pods:
/// they share the machine row and key, and are told apart by `session_id`.
async fn ensure_dispatch_machine(
    state: &impl DispatchContext,
    user_id: uuid::Uuid,
) -> Result<(uuid::Uuid, String), sqlx::Error> {
    if let Some((id, Some(key))) = sqlx::query_as::<_, (uuid::Uuid, Option<String>)>(
        "SELECT id, dispatch_key FROM machines \
         WHERE user_id = $1 AND kind = 'dispatch' AND deleted_at IS NULL \
         ORDER BY first_seen_at LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(state.dispatch_pool())
    .await?
    {
        return Ok((id, key));
    }

    let machine_id = uuid::Uuid::new_v4();
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);
    // Concurrent first dispatches must converge on one machine and key.
    // Preserve an installed key; repair a legacy row only while its key is NULL.
    let machine: (uuid::Uuid, String) = sqlx::query_as(
        "INSERT INTO machines (id, user_id, name, key_hash, kind, dispatch_key) \
         VALUES ($1, $2, 'dispatch', $3, 'dispatch', $4) \
         ON CONFLICT (user_id) WHERE kind = 'dispatch' AND deleted_at IS NULL \
         DO UPDATE SET \
           dispatch_key = COALESCE(machines.dispatch_key, EXCLUDED.dispatch_key), \
           key_hash = CASE WHEN machines.dispatch_key IS NULL THEN EXCLUDED.key_hash \
                           ELSE machines.key_hash END \
         RETURNING id, dispatch_key",
    )
    .bind(machine_id)
    .bind(user_id)
    .bind(&key_hash)
    .bind(&token)
    .fetch_one(state.dispatch_pool())
    .await?;
    tracing::debug!(%user_id, machine_id = %machine.0, "resolved dispatch machine");
    Ok(machine)
}

/// Mint a per-session EPHEMERAL machine credential for a dispatched worker,
/// bound to the pre-minted `session_id` and the user's shared
/// `dispatch` machine (`machine_id`), expiring at the session deadline + grace.
///
/// This replaces the shared per-user `dispatch_key` as the credential
/// handed to the worker pod: a leaked worker key now authenticates only its own
/// session and dies with it, instead of impersonating every dispatched session
/// of the user. It is an additive `auth_keys` row — the auth path
/// already enforces `expires_at` and carries `machine_id` transparently, so the
/// daemon accepts it exactly like any other machine key — with `kind`
/// `'ephemeral'` and `session_id` set so the reaper can revoke it on the
/// session's terminal state. Scopes mirror the owner's ceiling (same as the
/// shared dispatch key did via the legacy machine path). Returns the plaintext
/// token to inject as `CCTUI_MACHINE_KEY`.
async fn mint_ephemeral_dispatch_key(
    state: &impl DispatchContext,
    user_id: uuid::Uuid,
    machine_id: uuid::Uuid,
    session_id: &str,
    timeout_minutes: Option<u32>,
) -> Result<String, sqlx::Error> {
    // TTL = session deadline + grace. Fall back to a generous default when the
    // dispatch carries no timeout so the key still outlives a long run, then is
    // swept by the reaper. Grace covers post-deadline teardown/heartbeat.
    const GRACE_SECS: i64 = 30 * 60;
    const DEFAULT_TTL_SECS: i64 = 24 * 60 * 60;
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);
    let lifetime = timeout_minutes
        .map_or(DEFAULT_TTL_SECS, |m| i64::from(m).saturating_mul(60))
        .saturating_add(GRACE_SECS);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(lifetime);

    let scopes = crate::store::acls::user_ceiling(state.dispatch_pool(), user_id).await?;
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
    .fetch_one(state.dispatch_pool())
    .await?;
    crate::store::acls::grant_key(state.dispatch_pool(), key_id.0, scopes).await?;
    tracing::info!(%user_id, %machine_id, %session_id, "minted ephemeral dispatch key");
    Ok(token)
}

/// Choose the provider family for a pool election.
///
/// The provider family a pool election is scoped to. Dispatch carries no
/// adapter of its own: the entry's provider hint names the family when there is
/// one, otherwise the forwarded payload's adapter does, defaulting to
/// `claude-code` as the rest of the dispatch path does.
pub fn binding_family(
    hint: Option<&str>,
    payload_adapter: &str,
) -> Result<crate::gateway::Family, (StatusCode, Json<ApiError>)> {
    hint.map(str::trim).filter(|p| !p.is_empty()).map_or_else(
        || {
            crate::gateway::Family::from_adapter(payload_adapter)
                .map_err(|e| bad_request(e.to_string()))
        },
        |p| Ok(crate::gateway::Family::from_provider(p)),
    )
}

/// Map a shared resolver failure onto the dispatch error surface. Callers here
/// are machines: a rejection is a 400 whose detail names every account that was
/// considered and why it was skipped.
fn dispatch_resolve_err(e: crate::account_resolve::ResolveError) -> (StatusCode, Json<ApiError>) {
    match e {
        crate::account_resolve::ResolveError::Rejected(msg) => {
            (StatusCode::BAD_REQUEST, Json(ApiError { error: msg }))
        }
        crate::account_resolve::ResolveError::Db => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: "could not provision account session".into() }),
        ),
    }
}

/// Choose the explicit account or dispatcher default.
///
/// The account or pool name a dispatch should route through, after applying the
/// fallback precedence: an explicit `req.account` always wins; otherwise the
/// dispatcher's bound default account (if any) is used. The optional provider
/// hint constrains the mint to that provider's family; without one the
/// account's EVERY provider row is minted — the dispatcher default is
/// an identity, so it always mints all providers.
///
/// Pure so the precedence is unit-testable without a DB. `None` means "no
/// account at all" — dispatch then behaves as it did before any account
/// routing existed (no gateway env injected).
#[must_use]
pub fn resolve_dispatch_account(
    explicit_account: Option<&str>,
    explicit_provider: Option<&str>,
    default_account: Option<&str>,
) -> Option<(String, Option<String>)> {
    if let Some(name) = explicit_account.map(str::trim).filter(|a| !a.is_empty()) {
        return Some((
            name.to_string(),
            explicit_provider.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string),
        ));
    }
    default_account.map(|name| (name.to_owned(), None))
}

/// Find duplicate provider families before minting.
///
/// The first provider family that appears twice in the expanded mint set
/// — two same-family provider rows would mint the same env keys
/// (e.g. `ANTHROPIC_AUTH_TOKEN`) and the second mint would silently repoint the
/// session's family token, so the dispatch is rejected instead. Pure
/// for unit-testability.
#[must_use]
pub fn colliding_family(
    families: impl IntoIterator<Item = crate::gateway::Family>,
) -> Option<crate::gateway::Family> {
    let mut seen: Vec<crate::gateway::Family> = Vec::new();
    for f in families {
        if seen.contains(&f) {
            return Some(f);
        }
        seen.push(f);
    }
    None
}

/// Resolve the `(session_id, display_name, dedup_key)` for a dispatch.
///
/// `session_id` is the per-dispatch correlation id the worker registers under
/// and the gateway token binds to. It is ALWAYS a fresh UUID so isolated
/// short-lived pods never chain their logs into one growing conversation.
/// Idempotency rides `dedup_key`, which the dispatcher hashes into the Job name.
/// Claude's daemon derives `short = session_id[..8]` and rejects a dispatch whose
/// `short` isn't `/^[a-f0-9]{8}$/`, so a v4 UUID still satisfies the shape
/// constraint.
///
/// - `None` → fresh UUID, no display name, no dedup (each dispatch is unique).
/// - an already-valid UUID → used as-is (a deliberate retry/resume target) and
///   as its own dedup key, no display name.
/// - any other (human-readable) id → a FRESH UUID session, the original carried
///   as both the display name and the dedup key (so repeat dispatches of the
///   same logical key still coalesce onto one Job while each running round keeps
///   its own isolated session).
#[must_use]
pub fn resolve_dispatch_session_id(
    logical: Option<&str>,
) -> (String, Option<String>, Option<String>) {
    match logical {
        None => (uuid::Uuid::new_v4().to_string(), None, None),
        Some(s) if uuid::Uuid::parse_str(s).is_ok() => (s.to_owned(), None, Some(s.to_owned())),
        Some(s) => (uuid::Uuid::new_v4().to_string(), Some(s.to_owned()), Some(s.to_owned())),
    }
}

/// Apply a resolved account model alias to the payload.
///
/// Rewrite `payload.model` to `mapped` iff it differs from `raw`, returning
/// whether a rewrite happened. The dispatch model-alias decision,
/// factored out of the async per-account resolution loop so it's unit-testable
/// without a DB: an unchanged mapping (an alias miss, since `resolve_account_model`
/// fails soft to its input) is a no-op that leaves `model` — and every other
/// key, e.g. `effort` — verbatim.
#[must_use]
pub fn rewrite_model_if_aliased(payload: &mut serde_json::Value, raw: &str, mapped: &str) -> bool {
    if mapped == raw {
        return false;
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("model".into(), serde_json::Value::String(mapped.to_owned()));
        return true;
    }
    false
}

/// What a dispatcher routes an account-less dispatch through.
#[derive(Debug, PartialEq, Eq)]
pub enum DefaultBinding {
    /// One account identity, by the *name* `mint` resolution consumes —
    /// default injection mints ALL of that identity's providers, so no
    /// provider hint travels with it.
    Account(String),
    /// A pool, by id: elected per dispatch, and no account name can shadow it.
    Pool(uuid::Uuid),
}

/// An account binding is the more specific instruction, so it wins when a
/// dispatcher carries both. Pure so the precedence is unit-testable.
#[must_use]
pub fn default_binding(
    account: Option<String>,
    pool_id: Option<uuid::Uuid>,
) -> Option<DefaultBinding> {
    account.map(DefaultBinding::Account).or_else(|| pool_id.map(DefaultBinding::Pool))
}

/// The identity a dispatcher is bound to for dispatches that name no account.
///
/// `None` when the row carries neither binding, or when the one it carries
/// points at a deleted row (the `ON DELETE SET NULL` FKs clear it). A DB error
/// degrades to `None` so a lookup hiccup never blocks an otherwise-valid
/// dispatch.
async fn dispatcher_default_binding(
    state: &impl DispatchContext,
    dispatcher_name: &str,
    user_id: uuid::Uuid,
) -> Option<DefaultBinding> {
    let row: Option<(Option<String>, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT a.name, d.default_pool_id \
         FROM dispatchers d \
         LEFT JOIN accounts a ON a.id = d.default_account_id \
         WHERE d.name = $1 AND d.user_id = $2 \
           AND d.deleted_at IS NULL AND d.revoked_at IS NULL \
         ORDER BY d.created_at LIMIT 1",
    )
    .bind(dispatcher_name)
    .bind(user_id)
    .fetch_optional(state.dispatch_pool())
    .await
    .ok()
    .flatten();
    row.and_then(|(account, pool_id)| default_binding(account, pool_id))
}

/// Resolve an authorized enrolled or configured dispatcher.
///
/// Resolve a dispatcher *name* for the caller: an enrolled dispatcher
/// takes precedence, falling back to the global env-configured http registry.
///
/// Returns `Ok(None)` to mean "no such name anywhere" so the caller can 404
/// distinctly from a permission denial. An enrolled dispatcher resolves to an
/// [`EnrolledDispatcher`] that sends Dispatch commands over the WS hub.
///
/// Ownership scoping mirrors machines & connectors: a user token sees
/// only its own enrolled dispatchers (`user_id = caller`); the admin token
/// (`user_id` is `None` — the only authenticated role without a user) gets the
/// same god-view it has elsewhere and resolves by name across ALL owners. Names
/// are unique per `(user_id, name)` but not globally, so the admin path takes a
/// deterministic `ORDER BY created_at LIMIT 1` — acceptable until dispatchers
/// are addressable by id; if two users enroll the same name, admin hits the
/// oldest.
///
/// A user whose dispatch policy names `name` resolves it across owners too:
/// that is how a service identity reaches a dispatcher another user enrolled.
pub async fn resolve_dispatcher(
    state: &impl DispatchContext,
    user_id: Option<uuid::Uuid>,
    name: &str,
) -> Result<Option<std::sync::Arc<dyn Dispatcher>>, DispatchError> {
    let owner_filter = match user_id {
        Some(uid) => {
            let granted = crate::store::dispatch_policies::grants_dispatcher(
                state.dispatch_pool(),
                uid,
                name,
            )
            .await
            .map_err(|e| DispatchError::Backend(format!("dispatch policy lookup: {e}")))?;
            if granted { None } else { Some(uid) }
        }
        None => None,
    };
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM dispatchers \
         WHERE ($1::uuid IS NULL OR user_id = $1) AND name = $2 \
         AND deleted_at IS NULL AND revoked_at IS NULL \
         ORDER BY created_at LIMIT 1",
    )
    .bind(owner_filter)
    .bind(name)
    .fetch_optional(state.dispatch_pool())
    .await
    .map_err(|e| DispatchError::Backend(format!("dispatcher lookup: {e}")))?;
    if let Some((dispatcher_id,)) = row {
        return Ok(Some(std::sync::Arc::new(EnrolledDispatcher::new(
            name,
            dispatcher_id,
            state.dispatch_bus().clone(),
        ))));
    }
    // Fall back to a global env-configured http dispatcher (escape hatch).
    match state.dispatchers().get(name) {
        Ok(d) => Ok(Some(d)),
        Err(DispatchError::UnknownDispatcher(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub type DispatchResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

/// A dispatch that passed every pre-flight check: the dispatcher it targets,
/// the session identity it will run under, and the payload to forward (with the
/// display name carried over, no credentials injected yet).
struct ValidatedDispatch {
    dispatcher: std::sync::Arc<dyn Dispatcher>,
    session_id: String,
    dedup_key: Option<String>,
    caller: String,
    forwarded_payload: serde_json::Value,
}

/// The gateway routing a dispatch resolved to: the provider rows to mint
/// session tokens for and the pool (if any) the session is stamped with.
struct AccountRouting {
    mints: Vec<crate::gateway::mint::ProviderRow>,
    bound_pool: Option<uuid::Uuid>,
}

fn provision_err() -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError { error: "could not provision account session".into() }),
    )
}

/// Resolve the dispatcher, mint the session identity, alert that a dispatch
/// arrived, then run the caller/payload checks.
async fn validate_dispatch(
    state: &impl DispatchContext,
    ctx: &AuthContext,
    req: &DispatchRequest,
) -> DispatchResult<ValidatedDispatch> {
    let dispatcher = match resolve_dispatcher(state, ctx.owner_filter(), &req.dispatcher).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            let known = state.dispatchers().ids().join(", ");
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

    // Cross-replica delivery: if a live PEER replica holds this
    // dispatcher's WS, the bus's peer transport forwards the Dispatch frame to
    // it inside `EnrolledDispatcher::dispatch` — side effects (ntfy, ephemeral
    // key mint, webhook registration) run here exactly once either way.

    // Claude's daemon derives `short = session_id[..8]` and rejects a dispatch
    // unless `short` matches /^[a-f0-9]{8}$/ — so the worker's session id must be
    // UUID-shaped. A non-UUID logical id becomes the display name and
    // `dedup_key` (hashed into the Job name, so duplicate webhooks coalesce)
    // while each round gets a fresh UUID session.
    let (session_id, display_name, dedup_key) =
        resolve_dispatch_session_id(req.session_id.as_deref());

    // Alert that a dispatch arrived. Built from the *original* payload
    // (before the machine key is injected) and no-ops unless ntfy is configured.
    let caller = caller_label(state, ctx.owner_filter()).await;
    let summary = summarize(&req.payload);
    state.notify(DispatchNotification {
        title: format!("Dispatch received → {}", req.dispatcher),
        message: format!(
            "user: {caller}\nsession: {session_id}\ndispatcher: {}\n\n{summary}\n\npayload:\n{}",
            req.dispatcher,
            payload_for_notify(&req.payload),
        ),
        tags: "inbox_tray".into(),
        priority: 3,
    });

    // We do NOT pre-create a session row: the worker's cctui-daemon
    // self-dispatches the real session on boot and registers it
    // directly under the shared `dispatch` machine — forcing this pre-minted
    // (UUID-shaped) `session_id` so the registered id matches the id
    // the gateway token is bound to. A pre-minted row would just linger as an
    // empty `dispatch:<origin>` placeholder alongside it. Double dispatch is
    // still idempotent: the dispatcher derives the k8s Job name from
    // `sha(dedup_key)` (the caller's logical key), so a repeat of the
    // same key maps to the same Job (409 → same handle) even though each
    // dispatch mints a fresh `session_id`.

    // Dispatch permission is the `dispatch` scope, enforced uniformly for every
    // caller. Admin holds the scope by ceiling.
    ctx.requires(crate::auth::Scope::Dispatch).map_err(|s| {
        tracing::warn!(uid = %ctx.user_id, "dispatch denied: caller lacks dispatch scope");
        (s, Json(ApiError { error: "dispatch is not permitted for this token".into() }))
    })?;

    // Gated on `owner_filter()` to match webhook registration: an admin-token
    // dispatch has no owning user and never registers a webhook.
    if let (Some(_), Some(notify_url)) =
        (ctx.owner_filter(), req.notify_url.as_deref().filter(|u| !u.trim().is_empty()))
    {
        crate::outbound::validate_outbound_url(notify_url, &[]).await.map_err(|e| {
            tracing::warn!(uid = %ctx.user_id, "dispatch rejected: unsafe notify_url ({e})");
            (StatusCode::BAD_REQUEST, Json(ApiError { error: format!("invalid notify_url: {e}") }))
        })?;
    }

    cctui_proto::worker_env::check_caller_payload_env(&req.payload).map_err(|e| {
        tracing::warn!(uid = %ctx.user_id, "dispatch rejected: {e}");
        (StatusCode::BAD_REQUEST, Json(ApiError { error: e }))
    })?;

    let mut forwarded_payload = req.payload.clone();
    if let Some(obj) = forwarded_payload.as_object_mut() {
        obj.remove(cctui_proto::worker_env::SERVER_ENV_KEYS_FIELD);
    }
    // Carry the caller's logical id as the session display name (the session id
    // itself is a derived UUID) so the UI still shows e.g.
    // `triage-PROJ-2026…`. Only when the caller didn't already name the session.
    if let Some(name) = &display_name
        && let Some(obj) = forwarded_payload.as_object_mut()
    {
        obj.entry("name").or_insert_with(|| serde_json::Value::String(name.clone()));
    }

    Ok(ValidatedDispatch { dispatcher, session_id, dedup_key, caller, forwarded_payload })
}

/// Resolve the caller's stable dispatch machine and forward a per-session
/// ephemeral key for it to the pod via a reserved payload key. The dispatcher
/// lifts it into `CCTUI_MACHINE_KEY` and keeps it OUT of `TASK_PAYLOAD_JSON`, so
/// the worker's daemon runs AS this one machine without a per-pod enroll. The
/// shared `dispatch` machine still groups every dispatched session under one
/// logical machine, but a leaked worker key only impersonates its own session
/// and expires with it. `ensure_dispatch_machine` is kept for the row (and
/// `dispatch_key` rotation) it owns.
async fn inject_worker_credential(
    state: &impl DispatchContext,
    uid: uuid::Uuid,
    session_id: &str,
    timeout_minutes: Option<u32>,
    forwarded_payload: &mut serde_json::Value,
) -> DispatchResult<()> {
    let (machine_id, _shared_key) = ensure_dispatch_machine(state, uid).await.map_err(|e| {
        tracing::error!("ensure_dispatch_machine failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: "could not resolve dispatch machine".into() }),
        )
    })?;
    let key = mint_ephemeral_dispatch_key(state, uid, machine_id, session_id, timeout_minutes)
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
    Ok(())
}

/// The `(account, provider hint)` pairs a dispatch routes through, every pool
/// name already elected to a member, plus the pool that owns the session's
/// failover boundary. An explicit `req.accounts` list wins — it's the
/// cross-account mix form, each entry optionally family-constrained by its
/// provider hint. Otherwise the singular `req.account` (or the dispatcher's
/// bound default identity) is used. A bare account name mints EVERY provider
/// the identity carries. With no account either way, nothing is returned.
async fn resolve_routed_accounts(
    state: &impl DispatchContext,
    req: &DispatchRequest,
    uid: uuid::Uuid,
    requested_model: Option<&str>,
    payload_adapter: &str,
) -> DispatchResult<(Vec<(String, Option<String>)>, Option<uuid::Uuid>)> {
    let mut bound_pool: Option<uuid::Uuid> = None;
    let accounts: Vec<(String, Option<String>)> = if !req.accounts.is_empty() {
        req.accounts.iter().map(|a| (a.account.clone(), a.provider.clone())).collect()
    } else if let Some(explicit) =
        resolve_dispatch_account(req.account.as_deref(), req.provider.as_deref(), None)
    {
        vec![explicit]
    } else {
        // Nothing named: the dispatcher's own binding decides. A bound pool
        // elects here, per dispatch — the whole point of binding one.
        match dispatcher_default_binding(state, &req.dispatcher, uid).await {
            Some(DefaultBinding::Account(name)) => vec![(name, None)],
            Some(DefaultBinding::Pool(pool_id)) => {
                let (account, pool_id) = crate::account_resolve::resolve_pool_by_id(
                    state,
                    uid,
                    binding_family(None, payload_adapter)?,
                    requested_model,
                    pool_id,
                )
                .await
                .map_err(dispatch_resolve_err)?;
                bound_pool = Some(pool_id);
                vec![(account, None)]
            }
            None => Vec::new(),
        }
    };

    // Every name accepted here may be a pool: elect a member now, by the
    // pool's own strategy, and keep the pool so the minted token can be
    // stamped with it. An account of the caller's answering to the same
    // name always wins.
    let mut resolved: Vec<(String, Option<String>)> = Vec::with_capacity(accounts.len());
    for (name, hint) in accounts {
        let family = binding_family(hint.as_deref(), payload_adapter)?;
        let bound = crate::account_resolve::resolve_account_or_pool(
            state,
            uid,
            family,
            requested_model,
            &name,
        )
        .await
        .map_err(dispatch_resolve_err)?;
        if bound.pool_id.is_some() {
            // One column, one pool: the first pool named owns the session's
            // failover boundary. A second pool in the same cross-family list
            // still elects, it just cannot also claim the stamp.
            bound_pool = bound_pool.or(bound.pool_id);
        }
        resolved.push((bound.account, hint));
    }
    Ok((resolved, bound_pool))
}

/// Map `payload.model` through the resolved account(s) aliases, mirroring the
/// spawn path. Fails soft: a non-alias model or an unresolved account passes
/// through untouched; `effort` is never touched.
async fn apply_model_aliases(
    state: &impl DispatchContext,
    uid: uuid::Uuid,
    accounts: &[(String, Option<String>)],
    forwarded_payload: &mut serde_json::Value,
) {
    let Some(raw_model) = forwarded_payload
        .get("model")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
    else {
        return;
    };
    for (account_name, hint) in accounts {
        let Ok(Some(rows)) =
            crate::gateway::mint::account_provider_rows(state, uid, account_name).await
        else {
            continue;
        };
        let hint = hint.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let mut candidates = Vec::new();
        for row in alias_rows(&rows, hint) {
            let flipped = crate::gateway::mint::flip_model_redirect(
                state,
                uid,
                row.id,
                row.family(),
                &raw_model,
            )
            .await;
            candidates.push((row, flipped));
        }
        let mapped = pick_model_alias(&candidates, hint.is_some(), &raw_model);
        if rewrite_model_if_aliased(forwarded_payload, &raw_model, &mapped) {
            return;
        }
    }
}

/// The provider rows a dispatch resolves aliases against, in priority order:
/// the hinted family's row only, else one row per family, Anthropic first.
#[must_use]
pub fn alias_rows<'a>(
    rows: &'a [crate::gateway::mint::ProviderRow],
    hint: Option<&str>,
) -> Vec<&'a crate::gateway::mint::ProviderRow> {
    use crate::gateway::Family;
    let families = hint.map_or_else(
        || vec![Family::Anthropic, Family::Openai, Family::Fireworks, Family::Google],
        |p| vec![Family::from_provider(p)],
    );
    families.into_iter().filter_map(|f| rows.iter().find(|r| r.family() == f)).collect()
}

/// The first rewrite among `(row, redirect-flipped model)` candidates, or
/// `raw` when none rewrites. Only a hinted dispatch takes a catalog's
/// first-entry fallback, as a spawn on that family would; a bare dispatch
/// must not let a catalog capture names it does not list.
#[must_use]
pub fn pick_model_alias(
    candidates: &[(&crate::gateway::mint::ProviderRow, String)],
    hinted: bool,
    raw: &str,
) -> String {
    candidates
        .iter()
        .map(|(row, model)| crate::gateway::mint::resolve_row_model(row, model, hinted))
        .find(|mapped| mapped != raw)
        .unwrap_or_else(|| raw.to_owned())
}

/// Expand each named account into the provider rows to mint: the hinted
/// family's row only, or every row for a bare name. Two same-family rows would
/// mint the same env keys (e.g. `ANTHROPIC_AUTH_TOKEN`) and silently repoint the
/// session's family token; reject rather than clobber.
async fn expand_provider_rows(
    state: &impl DispatchContext,
    uid: uuid::Uuid,
    accounts: Vec<(String, Option<String>)>,
) -> DispatchResult<Vec<crate::gateway::mint::ProviderRow>> {
    let mut mints: Vec<crate::gateway::mint::ProviderRow> = Vec::new();
    for (account_name, provider_hint) in accounts {
        let rows =
            match crate::gateway::mint::account_provider_rows(state, uid, &account_name).await {
                Ok(Some(rows)) if !rows.is_empty() => rows,
                Ok(_) => {
                    return Err((
                        StatusCode::NOT_FOUND,
                        Json(ApiError {
                            error: format!(
                                "no account named {account_name:?} with a connected provider"
                            ),
                        }),
                    ));
                }
                Err(e) => {
                    tracing::error!("resolving dispatch account {account_name:?}: {e}");
                    return Err(provision_err());
                }
            };
        match provider_hint.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => {
                let family = crate::gateway::Family::from_provider(p);
                let before = mints.len();
                mints.extend(rows.into_iter().filter(|r| r.family() == family));
                if mints.len() == before {
                    return Err((
                        StatusCode::NOT_FOUND,
                        Json(ApiError {
                            error: format!(
                                "account {account_name:?} has no {} provider",
                                family.label()
                            ),
                        }),
                    ));
                }
            }
            None => mints.extend(rows),
        }
    }

    if let Some(family) =
        colliding_family(mints.iter().map(crate::gateway::mint::ProviderRow::family))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!(
                    "multiple dispatch accounts resolve to the {} provider family; \
                     specify at most one account per family",
                    family.label()
                ),
            }),
        ));
    }
    Ok(mints)
}

/// Mint a session-scoped gateway token per provider row and merge the gateway
/// base-url + token env into `payload.env` so the worker pod routes through
/// the passthrough gateway.
async fn mint_gateway_env(
    state: &impl DispatchContext,
    mints: Vec<crate::gateway::mint::ProviderRow>,
    session_id: &str,
    forwarded_payload: &mut serde_json::Value,
) -> DispatchResult<()> {
    for row in mints {
        match crate::gateway::mint::mint_session_env_for_account(state, row.id, session_id).await {
            Ok(Some(gateway_env)) => {
                merge_server_env(forwarded_payload, gateway_env);
            }
            Ok(None) => {
                tracing::error!(
                    provider_id = %row.id,
                    "mint_session_env_for_account (dispatch): provider row vanished mid-dispatch"
                );
                return Err(provision_err());
            }
            Err(e) => {
                tracing::error!(provider_id = %row.id, "mint_session_env_for_account (dispatch) failed: {e}");
                return Err(provision_err());
            }
        }
    }
    Ok(())
}

/// Merge server-minted env into `payload.env` and name its keys in
/// `server_env_keys` so dispatchers exempt them from the reserved-name check.
pub fn merge_server_env(
    payload: &mut serde_json::Value,
    minted: std::collections::BTreeMap<String, String>,
) {
    use cctui_proto::worker_env::SERVER_ENV_KEYS_FIELD;
    let Some(obj) = payload.as_object_mut() else { return };
    let keys: Vec<serde_json::Value> =
        minted.keys().map(|k| serde_json::Value::String(k.clone())).collect();
    if let Some(env_obj) = obj
        .entry("env")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
    {
        for (k, v) in minted {
            env_obj.insert(k, serde_json::Value::String(v));
        }
    }
    if let Some(list) = obj
        .entry(SERVER_ENV_KEYS_FIELD)
        .or_insert_with(|| serde_json::Value::Array(Vec::new()))
        .as_array_mut()
    {
        for k in keys {
            if !list.contains(&k) {
                list.push(k);
            }
        }
    }
}

/// Account-scoped routing on the dispatch path: resolve which accounts the
/// dispatch runs through, map the model through their aliases, and expand them
/// into the provider rows to mint.
async fn resolve_account_routing(
    state: &impl DispatchContext,
    req: &DispatchRequest,
    uid: uuid::Uuid,
    forwarded_payload: &mut serde_json::Value,
) -> DispatchResult<AccountRouting> {
    let requested_model = req
        .payload
        .get("model")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let payload_adapter = payload_adapter(&req.payload).unwrap_or("claude-code").to_owned();
    let (accounts, bound_pool) =
        resolve_routed_accounts(state, req, uid, requested_model.as_deref(), &payload_adapter)
            .await?;
    apply_model_aliases(state, uid, &accounts, forwarded_payload).await;
    let mints = expand_provider_rows(state, uid, accounts).await?;
    Ok(AccountRouting { mints, bound_pool })
}

/// Persist the worker child-spawn ceiling before forwarding.
///
/// `payload.spawn_capability` declares what the dispatched worker may spawn
/// through `CctuiAgent`. It is read here, server-side, and never forwarded —
/// the worker must not be able to read or restate its own capability. A
/// policy-bound worker may spawn no children: they would escape the policy,
/// so its dispatch is refused when that ceiling cannot be persisted.
pub async fn persist_spawn_capability(
    state: &impl DispatchContext,
    session_id: &str,
    forwarded_payload: &mut serde_json::Value,
    policy_bound: bool,
) -> DispatchResult<()> {
    let declared = forwarded_payload
        .as_object_mut()
        .and_then(|obj| obj.remove("spawn_capability"))
        .and_then(|raw| serde_json::from_value::<cctui_proto::api::SpawnCapability>(raw).ok())
        .filter(|cap| !cap.is_empty());
    let mut cap = match declared {
        Some(cap) => cap,
        None => state.spawn_default_capability().await,
    };
    if cap.max_permission_mode.is_none() {
        cap.max_permission_mode = Some(dispatch_ceiling(forwarded_payload));
    }
    if policy_bound {
        cap.max_children = Some(0);
    }
    if let Err(e) =
        crate::store::spawn_capabilities::upsert(state.dispatch_pool(), session_id, &cap).await
    {
        tracing::error!(
            session = %session_id,
            error = %e,
            "spawn-capability persist failed — CctuiAgent will be lost on server restart"
        );
        if policy_bound {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not record the worker's spawn capability".into() }),
            ));
        }
    }
    state.cache_spawn_capability(session_id, cap);
    Ok(())
}

/// Derive the child permission ceiling from the worker mode.
///
/// Child posture ceiling when the dispatcher names none: the worker's own
/// mode. Dispatched workers run approvals-off (the pod is the sandbox), so this
/// is `Yolo` unless `payload.permission_mode` says otherwise.
///
/// By design: a yolo parent may spawn yolo children. `spawn_child` still clamps
/// to the parent's reported mode, so a child never outranks its parent. Callers
/// that want a stricter tree set `spawn_capability.max_permission_mode`.
#[must_use]
pub fn dispatch_ceiling(payload: &serde_json::Value) -> cctui_proto::adapter::PermissionMode {
    payload
        .get("permission_mode")
        .and_then(serde_json::Value::as_str)
        .and_then(cctui_proto::adapter::PermissionMode::from_session_label)
        .unwrap_or(cctui_proto::adapter::PermissionMode::Yolo)
}

/// Forward the prepared dispatch, persist the returned handle and map a
/// placement failure onto the error surface.
async fn forward_dispatch(
    state: &impl DispatchContext,
    req: &DispatchRequest,
    v: &ValidatedDispatch,
) -> DispatchResult<(StatusCode, Json<DispatchResponse>)> {
    let ValidatedDispatch { dispatcher, session_id, dedup_key, caller, forwarded_payload } = v;
    let origin = dispatcher.id();
    let spec = DispatchSpec {
        session_id,
        timeout_minutes: req.timeout,
        reply_url: req.reply_url.as_deref(),
        dedup_key: dedup_key.as_deref(),
        payload: forwarded_payload,
    };

    let handle = match dispatcher.dispatch(&spec).await {
        Ok(h) => {
            state.notify(DispatchNotification {
                title: format!("Dispatch forwarded → {}", req.dispatcher),
                message: format!(
                    "user: {caller}\nsession: {session_id}\nhandle: {}\nnamespace: {}\nstatus: {}",
                    h.handle,
                    h.namespace.as_deref().unwrap_or("-"),
                    h.status.as_deref().unwrap_or("dispatched"),
                ),
                tags: "white_check_mark".into(),
                priority: 3,
            });
            // Persist the opaque handle so the completion-webhook sweep can ask
            // this dispatcher whether the workload later died without a
            // conclusion. Owner-scoped re-resolution in the sweep uses
            // the dispatcher *name* + the session's owning user, so only the
            // name/handle/namespace are stored. Best-effort: a failure here must
            // never block an otherwise-valid dispatch.
            let store = sqlx::query(
                "INSERT INTO dispatch_handles (session_id, dispatcher_name, handle, namespace) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (session_id) DO UPDATE SET \
                   dispatcher_name = EXCLUDED.dispatcher_name, \
                   handle = EXCLUDED.handle, \
                   namespace = EXCLUDED.namespace, \
                   created_at = now()",
            )
            .bind(session_id)
            .bind(origin)
            .bind(&h.handle)
            .bind(h.namespace.as_deref())
            .execute(state.dispatch_pool())
            .await;
            if let Err(e) = store {
                tracing::warn!(%session_id, "failed to persist dispatch handle: {e}");
            }
            h
        }
        Err(e) => {
            tracing::error!(
                dispatcher = %req.dispatcher,
                session = %session_id,
                error = %e,
                "dispatch not placed — no worker exists for this session; any claim the \
                 caller took (tag/assignee) is now stale"
            );
            state.notify(DispatchNotification {
                title: format!("Dispatch FAILED → {}", req.dispatcher),
                message: format!("user: {caller}\nsession: {session_id}\nerror: {e}"),
                tags: "rotating_light".into(),
                priority: 5,
            });
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
            session_id: session_id.clone(),
            dispatcher: origin.to_string(),
            handle: handle.handle,
            namespace: handle.namespace,
            // Surface the dispatcher's outcome verbatim so a re-dispatch onto a
            // terminal Job reads as `redispatched` (a fresh run) rather than a
            // misleading `dispatched`. Older dispatchers omit it →
            // preserve the historical `dispatched`.
            status: handle.status.unwrap_or_else(|| "dispatched".into()),
            queue_position: handle.queue_position,
        }),
    ))
}

/// Authorize, prepare and forward a dispatch through shared execution services.
///
/// validate → resolve identity/accounts → execute. The shared dispatch-machine
/// identity + account routing key on the AUTHENTICATED USER — `ctx.user_id`,
/// NOT `owner_filter()`, which is a query-result scoping switch that returns
/// `None` for admins. Only the env admin token has no real user (`user_id` is
/// nil); it dispatches without the shared identity.
pub async fn execute_dispatch(
    state: &impl DispatchContext,
    ctx: &AuthContext,
    mut req: DispatchRequest,
) -> DispatchResult<Forwarded> {
    let real_user = (ctx.user_id != uuid::Uuid::nil()).then_some(ctx.user_id);
    let policy = match real_user {
        Some(uid) => crate::dispatch_policy::enforce(state.dispatch_pool(), uid, &mut req).await?,
        None => None,
    };
    let mut v = validate_dispatch(state, ctx, &req).await?;

    if let Some(uid) = real_user {
        inject_worker_credential(state, uid, &v.session_id, req.timeout, &mut v.forwarded_payload)
            .await?;
        let routing = resolve_account_routing(state, &req, uid, &mut v.forwarded_payload).await?;
        mint_gateway_env(state, routing.mints, &v.session_id, &mut v.forwarded_payload).await?;
        if let Some(pool_id) = routing.bound_pool {
            crate::account_resolve::stamp_pool(state, &v.session_id, pool_id).await;
        }
    }

    state.register_completion_webhook(ctx, &req, &v.session_id).await;
    persist_spawn_capability(state, &v.session_id, &mut v.forwarded_payload, policy.is_some())
        .await?;
    state.remember_dispatch_context(real_user, &req, &v.session_id).await;
    let record = crate::store::dispatch_records::NewRecord::from_request(
        &v.session_id,
        real_user,
        &req,
        v.forwarded_payload.get("name").and_then(serde_json::Value::as_str),
        payload_adapter(&req.payload),
    );
    let bound = real_user.zip(policy.as_ref().map(|p| &p.rules));
    forward_within_policy(state.dispatch_pool(), bound, &v.session_id, || {
        forward_recorded(state, &req, &v, &record)
    })
    .await
    .map(shown_queued)
}

pub type Forwarded = (StatusCode, Json<DispatchResponse>);

/// A re-send that joined a run still held in the queue answers `queued`, so a
/// retrying caller sees it waits; policy and records saw `deduplicated`.
pub fn shown_queued((code, Json(mut r)): Forwarded) -> Forwarded {
    if r.status == "deduplicated" && r.queue_position.is_some() {
        r.status = "queued".into();
    }
    (code, Json(r))
}

/// A `deduplicated` dispatch joined a run that is still going and already
/// counted; every other outcome, `redispatched` included, starts a fresh run.
fn counts_as_use(status: &str) -> bool {
    status != "deduplicated"
}

fn cap_refused(
    uid: uuid::Uuid,
    rule: crate::dispatch_policy::Rule,
    detail: String,
) -> (StatusCode, Json<ApiError>) {
    let v = crate::dispatch_policy::Violation { rule, detail };
    tracing::warn!(%uid, "dispatch refused: {v}");
    (StatusCode::FORBIDDEN, Json(ApiError { error: v.to_string() }))
}

/// Reserve policy capacity before forwarding and refund failed or deduplicated uses.
///
/// Forward a policy-bound dispatch only once its use is reserved against the
/// caps, refunding the use when the forward fails or joins a counted run.
///
/// The reservation is a short per-user locked transaction, so no connection
/// is held across the forward. Without a policy, `forward` runs as is.
pub async fn forward_within_policy<F, Fut>(
    pool: &sqlx::PgPool,
    bound: Option<(uuid::Uuid, &cctui_proto::api::admin::DispatchPolicyRules)>,
    session_id: &str,
    forward: F,
) -> DispatchResult<Forwarded>
where
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = DispatchResult<Forwarded>> + Send,
{
    use crate::dispatch_policy::Rule;
    use crate::store::dispatch_policies::{Reservation, release_use, reserve_use};
    let Some((uid, rules)) = bound else {
        return forward().await;
    };
    let reserved = reserve_use(pool, uid, session_id, rules.max_concurrent, rules.daily_cap)
        .await
        .map_err(|e| {
            tracing::error!(%uid, error = %e, "dispatch policy use reservation failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not evaluate dispatch policy".into() }),
            )
        })?;
    let use_id = match reserved {
        Reservation::Granted(id) => id,
        Reservation::OverConcurrency { live, max } => {
            return Err(cap_refused(
                uid,
                Rule::Concurrency,
                format!("{live} dispatched sessions are still live; the limit is {max}"),
            ));
        }
        Reservation::OverDailyCap { today, cap } => {
            return Err(cap_refused(
                uid,
                Rule::DailyCap,
                format!("{today} dispatches today; the daily cap is {cap}"),
            ));
        }
    };
    let out = forward().await;
    if !out.as_ref().is_ok_and(|(_, r)| counts_as_use(&r.status))
        && let Err(e) = release_use(pool, use_id).await
    {
        tracing::warn!(%uid, session = %session_id, error = %e, "dispatch policy use not refunded");
    }
    out
}

/// A replay that coalesced onto another session's run leaves no record: its
/// own session never registers.
async fn forward_recorded(
    state: &impl DispatchContext,
    req: &DispatchRequest,
    v: &ValidatedDispatch,
    record: &crate::store::dispatch_records::NewRecord,
) -> DispatchResult<Forwarded> {
    use crate::store::dispatch_records as records;
    let sid = v.session_id.as_str();
    if let Err(e) = records::insert(state.dispatch_pool(), record).await {
        tracing::warn!(session = %sid, error = %e, "dispatch record not stored");
    }
    let out = forward_dispatch(state, req, v).await;
    let settled = match &out {
        Ok((_, r)) if !counts_as_use(&r.status) && v.dedup_key.as_deref() != Some(sid) => {
            records::delete(state.dispatch_pool(), sid).await
        }
        Ok((_, r)) => {
            match records::set_outcome(state.dispatch_pool(), sid, "forwarded", None).await {
                Ok(()) if r.status == "queued" => {
                    records::set_queued(state.dispatch_pool(), sid, true).await.map(drop)
                }
                other => other,
            }
        }
        Err((_, e)) => {
            records::set_outcome(state.dispatch_pool(), sid, "refused", Some(e.error.as_str()))
                .await
        }
    };
    if let Err(e) = settled {
        tracing::warn!(session = %sid, error = %e, "dispatch record outcome not stored");
    }
    out
}

#[must_use]
pub fn payload_adapter(payload: &serde_json::Value) -> Option<&str> {
    ["adapter", "adapter_id"]
        .into_iter()
        .filter_map(|key| payload.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .find(|s| !s.is_empty())
}

#[cfg(test)]
mod alias_tests {
    use crate::gateway::mint::ProviderRow;
    use serde_json::json;

    use super::{alias_rows, pick_model_alias};

    fn row(
        provider: &str,
        aliases: Option<serde_json::Value>,
        models: Option<serde_json::Value>,
    ) -> ProviderRow {
        ProviderRow {
            id: uuid::Uuid::new_v4(),
            provider: provider.to_owned(),
            model_aliases: aliases,
            models,
        }
    }

    fn account() -> Vec<ProviderRow> {
        let catalog = json!([{"model": "kimi-k3", "label": "Kimi K3"}]);
        let anthropic = json!({"opus": "claude-opus-5-5", "fable": "claude-fable-5-1"});
        let openai = json!({"sol": "gpt-sol", "luna": "gpt-luna", "astra": "gpt-astra"});
        vec![
            row("fireworks", None, Some(catalog)),
            row("anthropic", Some(anthropic), None),
            row("openai", Some(openai), None),
        ]
    }

    fn dispatched(rows: &[ProviderRow], hint: Option<&str>, raw: &str) -> String {
        let candidates: Vec<_> =
            alias_rows(rows, hint).into_iter().map(|r| (r, raw.to_owned())).collect();
        pick_model_alias(&candidates, hint.is_some(), raw)
    }

    #[test]
    fn unaliased_claude_models_are_never_captured_by_the_fireworks_catalog() {
        let rows = account();
        for raw in ["claude-haiku-5-5", "haiku", "claude-opus-5-5"] {
            assert_eq!(dispatched(&rows, None, raw), raw, "{raw} must pass through");
            assert_eq!(dispatched(&rows, Some("anthropic"), raw), raw, "{raw} must pass through");
        }
    }

    #[test]
    fn bare_dispatch_still_rewrites_explicit_aliases_and_catalog_labels() {
        let rows = account();
        assert_eq!(dispatched(&rows, None, "opus"), "claude-opus-5-5");
        assert_eq!(dispatched(&rows, None, "sol"), "gpt-sol");
        assert_eq!(dispatched(&rows, None, "Kimi K3"), "kimi-k3");
    }

    #[test]
    fn fireworks_hint_resolves_an_unknown_model_through_the_catalog_like_a_spawn() {
        let rows = account();
        assert_eq!(dispatched(&rows, Some("fireworks"), "something-unlisted"), "kimi-k3");
        assert_eq!(dispatched(&rows, Some("fireworks"), "Kimi K3"), "kimi-k3");
    }

    #[test]
    fn anthropic_hint_rewrites_an_anthropic_alias() {
        let rows = account();
        assert_eq!(dispatched(&rows, Some("anthropic"), "opus"), "claude-opus-5-5");
    }

    #[test]
    fn a_hint_only_consults_that_familys_row() {
        let rows = account();
        assert_eq!(dispatched(&rows, Some("openai"), "opus"), "opus");
        assert_eq!(dispatched(&rows, Some("anthropic"), "sol"), "sol");
        let hinted: Vec<&str> =
            alias_rows(&rows, Some("openai")).iter().map(|r| r.family().label()).collect();
        assert_eq!(hinted, vec!["openai"]);
        let bare: Vec<&str> = alias_rows(&rows, None).iter().map(|r| r.family().label()).collect();
        assert_eq!(bare, vec!["anthropic", "openai", "fireworks"]);
    }
}
