//! Full-host adapters for the shared execution dispatch service.
use crate::auth::AuthContext;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::{ApiError, DispatchRequest, DispatchResponse};
pub use cctui_server_core::dispatch::payload_adapter;
#[cfg(test)]
pub use cctui_server_core::dispatch::resolve_dispatcher;
#[cfg(test)]
use cctui_server_core::dispatch::{
    DefaultBinding, binding_family, colliding_family, default_binding, dispatch_ceiling,
    forward_within_policy, merge_server_env, persist_spawn_capability, resolve_dispatch_account,
    resolve_dispatch_session_id, rewrite_model_if_aliased, shown_queued,
};
use cctui_server_core::dispatch::{
    DispatchContext, DispatchNotification, DispatchResult, Forwarded,
};

impl DispatchContext for AppState {
    fn dispatch_bus(&self) -> &crate::bus::Bus {
        &self.bus
    }
    fn dispatchers(&self) -> &crate::dispatchers::Registry {
        &self.dispatchers
    }
    fn notify(&self, notice: DispatchNotification) {
        crate::ntfy::notify(
            &self.config,
            crate::ntfy::Notification {
                title: notice.title,
                message: notice.message,
                tags: notice.tags,
                priority: notice.priority,
            },
        );
    }
    fn cache_spawn_capability(&self, session: &str, cap: cctui_proto::api::SpawnCapability) {
        self.spawn_capabilities.insert(session.to_owned(), cap);
    }
    async fn spawn_default_capability(&self) -> cctui_proto::api::SpawnCapability {
        crate::routes::server_settings::spawn_default_capability(self).await
    }
    async fn register_completion_webhook(
        &self,
        ctx: &AuthContext,
        req: &DispatchRequest,
        session: &str,
    ) {
        register_completion_webhook(self, ctx, req, session).await;
    }
    async fn remember_dispatch_context(
        &self,
        real_user: Option<uuid::Uuid>,
        req: &DispatchRequest,
        session: &str,
    ) {
        if let Some(uid) = real_user {
            if context_record_is_callers(&self.pool, session, uid).await {
                let scope = dispatch_context_scope(uid, &req.payload);
                crate::routes::context::remember_intent(&self.pool, session, &[], &scope).await;
            } else {
                tracing::warn!(
                    %session,
                    "dispatch names another user's session; its context set is left alone"
                );
            }
        }
    }
}

pub async fn dispatch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<DispatchResponse>), (StatusCode, Json<ApiError>)> {
    cctui_server_core::dispatch::execute_dispatch(&state, &ctx, req).await
}

/// `GET /api/v1/sessions/dispatchers` — the names of every dispatcher the
/// caller can target: their enrolled dispatchers merged with the
/// global env-configured registry. The web UI uses this to populate the
/// dispatch picker. Any authenticated caller may read it (no role gate,
/// matching dispatch itself — for per-user gating).
///
/// Ownership scoping matches [`cctui_server_core::dispatch::resolve_dispatcher`]: a user sees its
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

/// Register a server-emitted completion webhook when the caller supplied
/// `notify_url`. The server fires it once the dispatched session reaches a
/// terminal state — covering crash cases the worker's `REPLY_URL` exit trap can
/// miss. Scoped to a real owning user (admin-token dispatches carry no owner,
/// so they keep the `REPLY_URL` trap only). Best-effort: a registration failure
/// never blocks the dispatch. The `task_id` echoed back is the dispatch
/// payload's `task_id` if present, else the session id.
async fn register_completion_webhook(
    state: &AppState,
    ctx: &AuthContext,
    req: &DispatchRequest,
    session_id: &str,
) {
    if let (Some(uid), Some(notify_url)) =
        (ctx.owner_filter(), req.notify_url.as_deref().filter(|u| !u.trim().is_empty()))
    {
        let task_id =
            req.payload.get("task_id").and_then(serde_json::Value::as_str).unwrap_or(session_id);
        crate::webhook::register(
            state,
            session_id,
            uid,
            notify_url,
            req.notify_secret.as_deref().filter(|s| !s.trim().is_empty()),
            task_id,
        )
        .await;
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct FailedDispatchesQuery {
    /// Also list the ones the caller dismissed.
    #[serde(default)]
    history: bool,
}

/// `GET /api/v1/dispatch-records/failed` — the caller's dispatches that were
/// refused or never ran to a session; every owner's for an admin. The ones
/// the caller dismissed are left out unless `history` is set.
pub async fn list_failed_dispatches(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Query(q): axum::extract::Query<FailedDispatchesQuery>,
) -> Result<Json<Vec<cctui_proto::api::FailedDispatch>>, crate::error::AppError> {
    use crate::store::dispatch_records::{Listing, list};
    let listing = Listing {
        owner: ctx.owner_filter(),
        viewer: Some(ctx.user_id),
        include_dismissed: q.history,
    };
    Ok(Json(list(&state.pool, listing).await?))
}

/// `POST /api/v1/dispatch-records/{id}/dismiss` — hide a failed dispatch from
/// the caller's list, on every device.
pub async fn dismiss_failed_dispatch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<StatusCode, crate::error::AppError> {
    let found = crate::store::dispatch_records::dismiss(
        &state.pool,
        ctx.user_id,
        ctx.owner_filter(),
        &session_id,
    )
    .await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(crate::error::AppError::new(StatusCode::NOT_FOUND, "no such dispatch record"))
    }
}

/// `POST /api/v1/dispatch-records/dismiss-all` — dismiss every failed
/// dispatch the caller's list shows.
pub async fn dismiss_all_failed_dispatches(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<cctui_proto::api::DismissedDispatches>, crate::error::AppError> {
    let dismissed =
        crate::store::dispatch_records::dismiss_all(&state.pool, ctx.user_id, ctx.owner_filter())
            .await?;
    Ok(Json(cctui_proto::api::DismissedDispatches { dismissed }))
}

const fn conflict(error: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::CONFLICT, Json(ApiError { error }))
}

fn rerun_refusal(
    blocker: cctui_proto::api::RerunBlocker,
    record: &crate::store::dispatch_records::StoredRecord,
) -> String {
    use cctui_proto::api::RerunBlocker;
    match blocker {
        RerunBlocker::Env => format!(
            "the dispatch carried env values ({}) that the server does not keep; \
             re-send it from its original caller",
            record.env_keys.join(", ")
        ),
        RerunBlocker::PromptTruncated => {
            "only the head of the prompt was stored; re-send it from its original caller".into()
        }
        RerunBlocker::PayloadTooLarge => {
            "the payload was too large to store; re-send it from its original caller".into()
        }
    }
}

/// `POST /api/v1/dispatch-records/{id}/redispatch` — send a recorded
/// dispatch again as a new one, for its dispatcher or an admin. Only stored
/// fields are re-sent: no env, no reply or notify callback.
pub async fn redispatch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> DispatchResult<Forwarded> {
    use crate::store::dispatch_records as records;
    let db_err = |e: sqlx::Error| {
        tracing::error!(session = %session_id, error = %e, "dispatch record lookup failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: "could not read the dispatch record".into() }),
        )
    };
    let record = records::get(&state.pool, &session_id)
        .await
        .map_err(db_err)?
        .filter(|r| ctx.is_admin() || r.user_id == Some(ctx.user_id))
        .ok_or_else(|| {
            (StatusCode::NOT_FOUND, Json(ApiError { error: "no such dispatch record".into() }))
        })?;
    if let Some(blocker) = record.rerun_blocker() {
        return Err(conflict(rerun_refusal(blocker, &record)));
    }
    let already = || conflict("this dispatch was already re-dispatched".into());
    if record.redispatched_as.is_some() {
        return Err(already());
    }
    let req = record
        .replay_request()
        .ok_or_else(|| conflict("the stored dispatch could not be read back".into()))?;
    if !records::claim_redispatch(&state.pool, &session_id).await.map_err(db_err)? {
        return Err(already());
    }
    let out = dispatch(State(state.clone()), Extension(ctx), Json(req)).await;
    let new_id = out.as_ref().ok().map(|(_, r)| r.session_id.as_str());
    if let Err(e) = records::finish_redispatch(&state.pool, &session_id, new_id).await {
        tracing::warn!(session = %session_id, error = %e, "re-dispatch not recorded");
    }
    out
}

/// A dispatch may name an existing session id as its retry target, so the
/// context set it records must not overwrite one that belongs to someone else:
/// neither a set already keyed by that id, nor a registered session, may be
/// another user's. A lookup failure keeps the set untouched.
async fn context_record_is_callers(pool: &sqlx::PgPool, session_id: &str, uid: uuid::Uuid) -> bool {
    let foreign: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_context \
                        WHERE spawn_key = $1 AND scope->>'user_id' <> $2) \
            OR EXISTS (SELECT 1 FROM sessions \
                        WHERE id = $1 AND user_id IS NOT NULL AND user_id <> $3)",
    )
    .bind(session_id)
    .bind(uid.to_string())
    .bind(uid)
    .fetch_one(pool)
    .await;
    match foreign {
        Ok(foreign) => !foreign,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "context ownership lookup failed");
            false
        }
    }
}

/// The coordinates a dispatched worker's rules resolve against. No human
/// watches a worker, so everything it saves through `CctuiContext` waits for
/// approval.
fn dispatch_context_scope(
    uid: uuid::Uuid,
    payload: &serde_json::Value,
) -> crate::routes::context::StoredScope {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    crate::routes::context::StoredScope {
        user_id: uid,
        coords: crate::routes::context::SpawnScope {
            machine_id: None,
            working_dir: text("working_dir").or_else(|| text("cwd")),
            label_ids: Vec::new(),
            adapter: Some(payload_adapter(payload).unwrap_or("claude-code").to_owned()),
        },
        dispatched: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DefaultBinding, binding_family, colliding_family, context_record_is_callers,
        default_binding, dispatch_ceiling, dispatch_context_scope, payload_adapter,
        resolve_dispatch_account, resolve_dispatch_session_id, rewrite_model_if_aliased,
    };
    use crate::routes::gateway::Family;
    use cctui_proto::adapter::PermissionMode;

    /// A dispatch naming an existing session id must not re-scope that
    /// session's context set to the dispatcher: only the user who recorded the
    /// set, or who owns the registered session, may overwrite it.
    #[tokio::test]
    async fn a_dispatch_only_overwrites_its_own_users_context_set() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_context_owner") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (owner, other) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        for uid in [owner, other] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("dctx-{uid}"))
                .bind(format!("kh-{uid}"))
                .execute(&pool)
                .await
                .expect("seed user");
        }
        let key = uuid::Uuid::new_v4().to_string();
        let scope = dispatch_context_scope(owner, &serde_json::json!({}));
        crate::routes::context::remember_intent(&pool, &key, &[], &scope).await;
        let registered = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
             VALUES ($1, 'm1', '/w', $2, 'claude-code')",
        )
        .bind(&registered)
        .bind(owner)
        .execute(&pool)
        .await
        .expect("seed session");

        assert!(context_record_is_callers(&pool, &key, owner).await, "the recording user");
        assert!(!context_record_is_callers(&pool, &key, other).await, "another user's set");
        assert!(context_record_is_callers(&pool, &registered, owner).await, "the session owner");
        assert!(
            !context_record_is_callers(&pool, &registered, other).await,
            "another user's registered session"
        );
        assert!(
            context_record_is_callers(&pool, &uuid::Uuid::new_v4().to_string(), other).await,
            "a fresh id records freely"
        );

        sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
            .bind(&key)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&registered)
            .execute(&pool)
            .await
            .ok();
        for uid in [owner, other] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }

    #[test]
    fn a_dispatched_worker_resolves_rules_by_adapter_and_cwd_and_saves_for_approval() {
        let uid = uuid::Uuid::new_v4();
        let scope = dispatch_context_scope(
            uid,
            &serde_json::json!({"adapter_id": "codex", "working_dir": " /w/repo "}),
        );
        assert!(scope.dispatched);
        assert_eq!(scope.user_id, uid);
        assert_eq!(scope.coords.adapter.as_deref(), Some("codex"));
        assert_eq!(scope.coords.working_dir.as_deref(), Some("/w/repo"));
        assert!(scope.coords.machine_id.is_none(), "a worker pod is no enrolled machine");

        let bare = dispatch_context_scope(uid, &serde_json::json!({"cwd": "/srv"}));
        assert_eq!(bare.coords.adapter.as_deref(), Some("claude-code"));
        assert_eq!(bare.coords.working_dir.as_deref(), Some("/srv"));
    }

    /// The daemon reads `payload.adapter`; the server used to read only
    /// `adapter_id`, so a codex dispatch could be routed to an anthropic
    /// account. Both spellings are read, `adapter` first.
    #[test]
    fn the_payload_adapter_prefers_adapter_over_adapter_id() {
        let cases = [
            (serde_json::json!({"adapter": "codex"}), Some("codex")),
            (serde_json::json!({"adapter_id": "codex"}), Some("codex")),
            (serde_json::json!({"adapter": "opencode", "adapter_id": "codex"}), Some("opencode")),
            (serde_json::json!({"adapter": "", "adapter_id": "codex"}), Some("codex")),
            (serde_json::json!({"adapter": " codex "}), Some("codex")),
            (serde_json::json!({"adapter": 7}), None),
            (serde_json::json!({}), None),
        ];
        for (payload, want) in cases {
            assert_eq!(payload_adapter(&payload), want, "payload {payload}");
        }
        let scope =
            dispatch_context_scope(uuid::Uuid::new_v4(), &serde_json::json!({"adapter": "codex"}));
        assert_eq!(scope.coords.adapter.as_deref(), Some("codex"));
    }

    #[test]
    fn dispatch_ceiling_defaults_to_the_workers_own_yolo_mode() {
        assert_eq!(dispatch_ceiling(&serde_json::json!({})), PermissionMode::Yolo);
        assert_eq!(
            dispatch_ceiling(&serde_json::json!({"permission_mode": "nonsense"})),
            PermissionMode::Yolo
        );
        assert_eq!(
            dispatch_ceiling(&serde_json::json!({"permission_mode": "auto"})),
            PermissionMode::Auto
        );
        assert_eq!(
            dispatch_ceiling(&serde_json::json!({"permission_mode": "ask"})),
            PermissionMode::Ask
        );
    }

    #[test]
    fn session_id_human_logical_mints_fresh_uuid_keeps_key_as_dedup_and_name() {
        // each dispatch of the same logical key gets a DISTINCT session
        // (no log chaining), while the logical key is preserved as both the
        // display name and the dedup key (idempotency now lives there).
        let (a, na, ka) = resolve_dispatch_session_id(Some("triage-PROJ-202606231511"));
        let (b, nb, kb) = resolve_dispatch_session_id(Some("triage-PROJ-202606231511"));
        assert_ne!(a, b, "fresh, distinct session per dispatch");
        assert!(uuid::Uuid::parse_str(&a).is_ok(), "UUID-shaped: {a}");
        assert!(a[..8].chars().all(|c| c.is_ascii_hexdigit()), "hex short");
        assert_eq!(na.as_deref(), Some("triage-PROJ-202606231511"), "logical id kept as name");
        assert_eq!(nb, na, "display name is the logical key both times");
        assert_eq!(ka.as_deref(), Some("triage-PROJ-202606231511"), "logical id is the dedup key");
        assert_eq!(kb, ka, "dedup key is stable across dispatches (Job coalesces)");
    }

    #[test]
    fn session_id_real_uuid_passes_through_as_its_own_dedup_key() {
        let u = "a1b2c3d4-0000-4000-8000-000000000000";
        let (id, name, dedup) = resolve_dispatch_session_id(Some(u));
        assert_eq!(id, u);
        assert!(name.is_none());
        assert_eq!(dedup.as_deref(), Some(u), "explicit uuid is its own dedup target");
    }

    #[test]
    fn session_id_none_mints_fresh_uuid_no_dedup() {
        let (id, name, dedup) = resolve_dispatch_session_id(None);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert!(name.is_none());
        assert!(dedup.is_none(), "no logical key ⇒ no dedup, each dispatch unique");
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
        // explicit account wins over the dispatcher's default, and uses
        // the explicit provider hint.
        let got =
            resolve_dispatch_account(Some("work"), Some("anthropic"), Some("automation-account"));
        assert_eq!(got, Some(("work".into(), Some("anthropic".into()))));
    }

    #[test]
    fn empty_account_falls_back_to_bound_default_identity_all_providers() {
        // / an empty / whitespace `req.account` falls back to
        // the dispatcher's bound default account IDENTITY — no provider hint, so
        // every provider the identity carries gets minted.
        assert_eq!(
            resolve_dispatch_account(None, None, Some("automation-account")),
            Some(("automation-account".into(), None))
        );
        assert_eq!(
            resolve_dispatch_account(Some("   "), None, Some("automation-account")),
            Some(("automation-account".into(), None))
        );
    }

    #[test]
    fn bare_explicit_account_carries_no_provider_hint() {
        // `account: "acme"` alone means "all of acme's providers" —
        // the resolution must not invent a family constraint.
        assert_eq!(resolve_dispatch_account(Some("acme"), None, None), Some(("acme".into(), None)));
    }

    #[test]
    fn no_account_and_no_binding_is_none() {
        // Unbound dispatcher + no explicit account: no gateway env injected
        // (behaves as before any account routing existed).
        assert_eq!(resolve_dispatch_account(None, None, None), None);
        assert_eq!(resolve_dispatch_account(Some(""), None, None), None);
    }

    #[test]
    fn disjoint_families_do_not_collide() {
        // one identity's claude + codex rows mint disjoint env keys.
        assert!(colliding_family([Family::Anthropic, Family::Openai]).is_none());
        assert!(colliding_family([]).is_none());
        assert!(colliding_family([Family::Openai]).is_none());
    }

    #[test]
    fn same_family_twice_collides() {
        // two same-family rows would fight over ANTHROPIC_AUTH_TOKEN /
        // OPENAI_API_KEY — the guard names the colliding family.
        assert!(matches!(
            colliding_family([Family::Anthropic, Family::Anthropic]),
            Some(Family::Anthropic)
        ));
        assert!(matches!(
            colliding_family([Family::Anthropic, Family::Openai, Family::Openai]),
            Some(Family::Openai)
        ));
    }

    #[test]
    fn alias_hit_rewrites_model_and_leaves_effort_untouched() {
        // a resolved model differing from the request model rewrites
        // `payload.model` in place; `effort` (and any other key) is untouched.
        let mut payload =
            serde_json::json!({ "model": "opus", "effort": "high", "flow": "triage" });
        assert!(rewrite_model_if_aliased(&mut payload, "opus", "claude-opus-4-8[1m]"));
        assert_eq!(payload["model"], "claude-opus-4-8[1m]");
        assert_eq!(payload["effort"], "high", "effort never rewritten");
        assert_eq!(payload["flow"], "triage", "unrelated keys preserved");
    }

    #[test]
    fn alias_miss_passes_model_through_verbatim() {
        // when resolution fails soft (mapped == raw), nothing changes —
        // no account / no alias leaves the forwarded model verbatim.
        let mut payload = serde_json::json!({ "model": "opus", "effort": "high" });
        assert!(!rewrite_model_if_aliased(&mut payload, "opus", "opus"));
        assert_eq!(payload["model"], "opus", "unchanged on alias miss");
        assert_eq!(payload["effort"], "high");
    }

    #[test]
    fn a_provider_hint_scopes_the_binding_family() {
        assert_eq!(binding_family(Some("openai"), "claude-code").unwrap(), Family::Openai);
        assert_eq!(binding_family(Some("fireworks"), "claude-code").unwrap(), Family::Fireworks);
    }

    #[test]
    fn without_a_hint_the_payload_adapter_scopes_the_binding_family() {
        assert_eq!(binding_family(None, "codex").unwrap(), Family::Openai);
        assert_eq!(binding_family(Some("  "), "codex").unwrap(), Family::Openai);
        assert_eq!(binding_family(None, "claude-code").unwrap(), Family::Anthropic);
        assert_eq!(binding_family(None, "gemini").unwrap(), Family::Google);
        assert!(binding_family(None, "kilo").is_err());
    }

    #[test]
    fn a_bound_account_beats_a_bound_pool() {
        // Both columns set: the account is the more specific instruction, the
        // same rule a name that denotes both an account and a pool follows.
        let pool = uuid::Uuid::new_v4();
        assert_eq!(
            default_binding(Some("alpha".into()), Some(pool)),
            Some(DefaultBinding::Account("alpha".into()))
        );
    }

    #[test]
    fn a_bound_pool_alone_elects_per_dispatch() {
        let pool = uuid::Uuid::new_v4();
        assert_eq!(default_binding(None, Some(pool)), Some(DefaultBinding::Pool(pool)));
    }

    #[test]
    fn an_unbound_dispatcher_injects_nothing() {
        assert_eq!(default_binding(None, None), None);
    }

    async fn seed_user(pool: &sqlx::PgPool) -> uuid::Uuid {
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("dispatch-svc-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    #[allow(clippy::unnecessary_wraps)] // the shape forward_within_policy's forward closure returns
    fn forwarded(status: &str) -> super::DispatchResult<super::Forwarded> {
        Ok((
            axum::http::StatusCode::ACCEPTED,
            axum::Json(cctui_proto::api::DispatchResponse {
                session_id: "s".into(),
                dispatcher: "d".into(),
                handle: "h".into(),
                namespace: None,
                status: status.into(),
                queue_position: None,
            }),
        ))
    }

    fn shown(status: &str, queue_position: Option<u32>) -> (String, Option<u32>) {
        let (_, axum::Json(mut r)) = forwarded(status).unwrap();
        r.queue_position = queue_position;
        let (_, axum::Json(r)) =
            super::shown_queued((axum::http::StatusCode::ACCEPTED, axum::Json(r)));
        (r.status, r.queue_position)
    }

    #[test]
    fn a_resend_onto_a_queued_run_answers_queued_with_its_position() {
        assert_eq!(shown("deduplicated", Some(2)), ("queued".into(), Some(2)));
        assert_eq!(shown("queued", Some(1)), ("queued".into(), Some(1)));
        assert_eq!(
            shown("deduplicated", None),
            ("deduplicated".into(), None),
            "joined a running run"
        );
        assert_eq!(shown("dispatched", None), ("dispatched".into(), None));
    }

    async fn uses(pool: &sqlx::PgPool, uid: uuid::Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM dispatch_policy_uses WHERE user_id = $1")
            .bind(uid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Re-dispatching a session id onto a fresh run counts again; joining the
    /// run still in flight, or failing to place one, does not.
    #[tokio::test]
    async fn every_fresh_run_of_a_session_counts_and_a_deduplicated_replay_does_not() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_use_per_run") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let sid = uuid::Uuid::new_v4().to_string();
        let rules = cctui_proto::api::admin::DispatchPolicyRules::default();
        for status in ["dispatched", "deduplicated", "redispatched", "queued", "deduplicated"] {
            let (_code, _body) =
                super::forward_within_policy(&pool, Some((uid, &rules)), &sid, || async {
                    forwarded(status)
                })
                .await
                .unwrap();
        }
        let refused = super::forward_within_policy(&pool, Some((uid, &rules)), &sid, || async {
            Err((
                axum::http::StatusCode::BAD_GATEWAY,
                axum::Json(cctui_proto::api::ApiError { error: "down".into() }),
            ))
        })
        .await;
        assert!(refused.is_err());
        assert_eq!(uses(&pool, uid).await, 3);
        assert_eq!(crate::store::dispatch_policies::dispatches_today(&pool, uid).await.unwrap(), 3);
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// Runs `n` dispatches of `uid` at once through a forward that takes a
    /// while; returns how many were forwarded and every refusal.
    async fn race(
        pool: &sqlx::PgPool,
        uid: uuid::Uuid,
        rules: Option<&cctui_proto::api::admin::DispatchPolicyRules>,
        n: usize,
    ) -> (usize, Vec<(axum::http::StatusCode, String)>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let forwards = AtomicUsize::new(0);
        let runs = (0..n).map(|i| {
            let forwards = &forwards;
            let sid = format!("race-{i}-{uid}");
            let forward = move || async move {
                forwards.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                forwarded("dispatched")
            };
            async move {
                super::forward_within_policy(pool, rules.map(|r| (uid, r)), &sid, forward).await
            }
        });
        let refusals = futures_util::future::join_all(runs)
            .await
            .into_iter()
            .filter_map(Result::err)
            .map(|(code, body)| (code, body.0.error))
            .collect();
        (forwards.load(Ordering::SeqCst), refusals)
    }

    /// Concurrent policy-bound dispatches racing a daily cap of 1: exactly one
    /// is forwarded and counted, every other one is refused on the cap.
    #[tokio::test]
    async fn concurrent_dispatches_never_exceed_the_daily_cap() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_cap_race") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = seed_user(&pool).await;
        let rules = cctui_proto::api::admin::DispatchPolicyRules {
            daily_cap: Some(1),
            ..Default::default()
        };
        let (forwarded, refusals) = race(&pool, uid, Some(&rules), 8).await;
        assert_eq!(forwarded, 1);
        assert_eq!(refusals.len(), 7);
        for (code, error) in &refusals {
            assert_eq!(*code, axum::http::StatusCode::FORBIDDEN);
            assert!(error.contains("(daily_cap)"), "{error}");
        }
        assert_eq!(uses(&pool, uid).await, 1);
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// Without a policy every concurrent dispatch is forwarded and none is
    /// counted.
    #[tokio::test]
    async fn without_a_policy_concurrent_dispatches_all_forward_uncounted() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_no_policy_race") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let (forwarded, refusals) = race(&pool, uid, None, 8).await;
        assert_eq!(forwarded, 8);
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(uses(&pool, uid).await, 0);
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    type Call = (String, Option<String>, serde_json::Value);

    /// Records every dispatch it receives and refuses the first.
    struct RefuseFirst {
        calls: std::sync::Mutex<Vec<Call>>,
        refuse: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl crate::dispatchers::Dispatcher for RefuseFirst {
        fn id(&self) -> &'static str {
            "refuse-first"
        }

        async fn dispatch(
            &self,
            spec: &crate::dispatchers::DispatchSpec<'_>,
        ) -> Result<crate::dispatchers::DispatchHandle, crate::dispatchers::DispatchError> {
            self.calls.lock().unwrap().push((
                spec.session_id.to_owned(),
                spec.reply_url.map(str::to_owned),
                spec.payload.clone(),
            ));
            if self.refuse.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Err(crate::dispatchers::DispatchError::Backend("quota exceeded".into()));
            }
            Ok(crate::dispatchers::DispatchHandle {
                handle: format!("jobs/{}", spec.session_id),
                namespace: None,
                status: None,
                queue_position: None,
            })
        }
    }

    fn user_ctx(uid: uuid::Uuid) -> crate::auth::AuthContext {
        crate::auth::AuthContext {
            user_id: uid,
            key_id: uid,
            machine_id: None,
            scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
        }
    }

    /// A dispatch the dispatcher refused is listed with its stored fields; a
    /// re-dispatch sends them as a new dispatch without the callback, once,
    /// and only for its own user.
    #[tokio::test]
    async fn a_refused_dispatch_is_listed_and_re_dispatched_with_its_stored_fields() {
        use axum::extract::{Path, State};
        use axum::{Extension, Json};
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_record_redispatch") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let stub = std::sync::Arc::new(RefuseFirst {
            calls: std::sync::Mutex::default(),
            refuse: std::sync::atomic::AtomicBool::new(true),
        });
        let mut state = crate::state::AppState::for_test(pool.clone());
        state.dispatchers =
            std::sync::Arc::new(crate::dispatchers::Registry::new().with(stub.clone()));
        let (uid, other) = (seed_user(&pool).await, seed_user(&pool).await);
        let req: cctui_proto::api::DispatchRequest = serde_json::from_value(serde_json::json!({
            "dispatcher": "refuse-first",
            "session_id": "triage-PROJ-9",
            "timeout": 20,
            "reply_url": "https://n8n/reply/CAPABILITY",
            "payload": {"profile": "tonbo", "repo": "cctui", "model": "opus", "prompt": "fix it"},
        }))
        .unwrap();
        let (code, _) = super::dispatch(State(state.clone()), Extension(user_ctx(uid)), Json(req))
            .await
            .unwrap_err();
        assert_eq!(code, axum::http::StatusCode::BAD_GATEWAY);

        let failed = crate::store::dispatch_records::list_failed(&pool, Some(uid)).await.unwrap();
        assert_eq!(failed.len(), 1);
        let original = failed[0].session_id.clone();
        assert_eq!(failed[0].reason, cctui_proto::api::FailedDispatchReason::Refused);
        assert!(failed[0].detail.as_deref().unwrap_or_default().contains("quota exceeded"));
        assert_eq!(failed[0].name.as_deref(), Some("triage-PROJ-9"));
        assert_eq!(failed[0].prompt_head.as_deref(), Some("fix it"));
        assert_eq!(failed[0].rerun_blocked, None);

        let (code, _) = super::redispatch(
            State(state.clone()),
            Extension(user_ctx(other)),
            Path(original.clone()),
        )
        .await
        .unwrap_err();
        assert_eq!(code, axum::http::StatusCode::NOT_FOUND, "another user's record");

        let (code, Json(resp)) = super::redispatch(
            State(state.clone()),
            Extension(user_ctx(uid)),
            Path(original.clone()),
        )
        .await
        .unwrap();
        assert_eq!(code, axum::http::StatusCode::ACCEPTED);
        assert_ne!(resp.session_id, original, "a new dispatch");
        let calls = stub.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        let (sid, reply_url, payload) = &calls[1];
        assert_eq!(sid, &resp.session_id);
        assert_eq!(*reply_url, None);
        for (key, want) in [
            ("profile", "tonbo"),
            ("repo", "cctui"),
            ("model", "opus"),
            ("prompt", "fix it"),
            ("name", "triage-PROJ-9"),
        ] {
            assert_eq!(payload[key], want, "{key}");
        }
        assert!(
            crate::store::dispatch_records::list_failed(&pool, Some(uid)).await.unwrap().is_empty(),
            "a re-run record leaves the list"
        );
        let (code, _) =
            super::redispatch(State(state.clone()), Extension(user_ctx(uid)), Path(original))
                .await
                .unwrap_err();
        assert_eq!(code, axum::http::StatusCode::CONFLICT, "re-run once");

        for u in [uid, other] {
            for sql in [
                "DELETE FROM auth_keys WHERE user_id = $1",
                "DELETE FROM machines WHERE user_id = $1",
            ] {
                sqlx::query(sql).bind(u).execute(&pool).await.ok();
            }
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
        }
    }

    /// A policy-bound worker gets a capability that refuses every child,
    /// whatever it declared; without a policy the declared or default
    /// capability is kept as is.
    #[tokio::test]
    async fn a_policy_bound_dispatch_grants_no_children() {
        use crate::routes::spawn_child::{Denied, Usage, authorize};
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_no_children") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let state = crate::state::AppState::for_test(pool.clone());
        let declared = serde_json::json!({
            "spawn_capability": {"adapters": ["claude-code", "codex"], "max_children": 5}
        });
        let persisted = |sid: String, payload: serde_json::Value, bound: bool| {
            let state = state.clone();
            async move {
                let mut payload = payload;
                super::persist_spawn_capability(&state, &sid, &mut payload, bound)
                    .await
                    .expect("persisted");
                assert!(payload.get("spawn_capability").is_none(), "never forwarded");
                let cached = state.spawn_capabilities.get(&sid).map(|c| c.value().clone()).unwrap();
                let stored =
                    crate::store::spawn_capabilities::get(&state.pool, &sid).await.unwrap();
                assert_eq!(stored.as_ref(), Some(&cached));
                sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
                    .bind(&sid)
                    .execute(&state.pool)
                    .await
                    .ok();
                cached
            }
        };
        let sid = || format!("dispatch-cap-{}", uuid::Uuid::new_v4());

        for payload in [declared.clone(), serde_json::json!({})] {
            let cap = persisted(sid(), payload, true).await;
            assert_eq!(cap.max_children, Some(0));
            let child = cctui_proto::api::SpawnChildRequest {
                adapter: "claude-code".into(),
                prompt: "go".into(),
                ..Default::default()
            };
            assert_eq!(
                authorize(Some(&cap), &child, &Usage::default()),
                Err(Denied::TooManyChildren { max: 0 })
            );
        }

        let cap = persisted(sid(), declared, false).await;
        assert_eq!(cap.adapters, ["claude-code", "codex"]);
        assert_eq!(cap.max_children, Some(5));
        assert_eq!(cap.max_permission_mode, Some(PermissionMode::Yolo));

        let default = crate::routes::server_settings::spawn_default_capability(&state).await;
        let cap = persisted(sid(), serde_json::json!({}), false).await;
        assert_eq!(
            cap,
            cctui_proto::api::SpawnCapability {
                max_permission_mode: Some(PermissionMode::Yolo),
                ..default
            }
        );
    }

    /// A policy-bound dispatch whose capability cannot be stored is refused
    /// before it is forwarded; without a policy it still goes out.
    #[tokio::test]
    async fn a_policy_bound_dispatch_is_refused_when_its_capability_is_not_stored() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        pool.close().await;
        let state = crate::state::AppState::for_test(pool);
        let payload = serde_json::json!({"spawn_capability": {"adapters": ["claude-code"]}});

        let sid = format!("dispatch-cap-{}", uuid::Uuid::new_v4());
        let (code, _) = super::persist_spawn_capability(&state, &sid, &mut payload.clone(), true)
            .await
            .unwrap_err();
        assert_eq!(code, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(state.spawn_capabilities.get(&sid).is_none());

        let sid = format!("dispatch-cap-{}", uuid::Uuid::new_v4());
        let mut unbound = payload;
        super::persist_spawn_capability(&state, &sid, &mut unbound, false)
            .await
            .expect("no policy: a persist failure does not block the dispatch");
        assert!(state.spawn_capabilities.get(&sid).is_some());
    }

    #[test]
    fn minted_reserved_env_passes_the_dispatcher_check() {
        let mut payload = serde_json::json!({ "env": { "FEATURE": "1" } });
        let minted: std::collections::BTreeMap<String, String> = [
            ("CCTUI_CODEX_CONFIG_TOML".to_owned(), "model = \"x\"".to_owned()),
            ("HTTPS_PROXY".to_owned(), "http://p".to_owned()),
        ]
        .into();
        super::merge_server_env(&mut payload, minted);
        cctui_proto::worker_env::check_payload_env(&payload).expect("minted keys exempt");
        assert_eq!(payload["env"]["CCTUI_CODEX_CONFIG_TOML"], "model = \"x\"");
        assert_eq!(payload["env"]["FEATURE"], "1");

        payload["env"]["LD_PRELOAD"] = "/evil.so".into();
        let err = cctui_proto::worker_env::check_payload_env(&payload).unwrap_err();
        assert!(err.contains("LD_PRELOAD"), "{err}");
    }
}
