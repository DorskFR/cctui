//! Dispatch endpoints and the failed-dispatch records.
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::{ApiError, DispatchRequest, DispatchResponse};
use cctui_server_kernel::auth::AuthContext;

use crate::context::DispatchCore;
pub use crate::dispatch::payload_adapter;
#[cfg(test)]
use crate::dispatch::{
    DefaultBinding, binding_family, colliding_family, default_binding, dispatch_ceiling,
    forward_within_policy, merge_server_env, resolve_dispatch_account, resolve_dispatch_session_id,
    rewrite_model_if_aliased,
};
use crate::dispatch::{DispatchContext, DispatchResult, Forwarded};
use crate::hooks::DispatchNotify;

pub async fn dispatch<S: DispatchCore + DispatchContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<DispatchResponse>), (StatusCode, Json<ApiError>)> {
    crate::dispatch::execute_dispatch(&state, &ctx, req).await
}

/// `GET /api/v1/sessions/dispatchers` — the names of every dispatcher the
/// caller can target: their enrolled dispatchers merged with the
/// global env-configured registry.
///
/// The web UI uses this to populate the
/// dispatch picker. Any authenticated caller may read it (no role gate,
/// matching dispatch itself — for per-user gating).
///
/// Ownership scoping matches [`crate::dispatch::resolve_dispatcher`]: a user sees its
/// own enrolled dispatchers; the admin token (`user_id` is `None`) sees ALL of
/// them, so the picker offers everything an admin can actually dispatch to.
pub async fn list_dispatchers<S: DispatchCore>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Json<Vec<String>> {
    let mut ids = state.dispatcher_registry().ids();
    if let Ok(names) = sqlx::query_scalar::<_, String>(
        "SELECT name FROM dispatchers \
         WHERE ($1::uuid IS NULL OR user_id = $1) AND deleted_at IS NULL",
    )
    .bind(ctx.owner_filter())
    .fetch_all(state.pool())
    .await
    {
        ids.extend(names);
    }
    ids.sort();
    ids.dedup();
    Json(ids)
}

#[derive(Debug, serde::Deserialize)]
pub struct FailedDispatchesQuery {
    /// Also list the ones the caller dismissed.
    #[serde(default)]
    history: bool,
}

/// `GET /api/v1/dispatch-records/failed` — the caller's dispatches that were
/// refused or never ran to a session; every owner's for an admin.
///
/// The ones
/// the caller dismissed are left out unless `history` is set.
pub async fn list_failed_dispatches<S: DispatchCore>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Query(q): axum::extract::Query<FailedDispatchesQuery>,
) -> Result<Json<Vec<cctui_proto::api::FailedDispatch>>, cctui_server_kernel::error::AppError> {
    use crate::execution_failures::list;
    use crate::store::dispatch_records::Listing;
    let listing = Listing {
        owner: ctx.owner_filter(),
        viewer: Some(ctx.user_id),
        include_dismissed: q.history,
    };
    Ok(Json(list(state.pool(), listing).await?))
}

/// `POST /api/v1/dispatch-records/{id}/dismiss` — hide a failed dispatch from
/// the caller's list, on every device.
pub async fn dismiss_failed_dispatch<S: DispatchCore + DispatchNotify>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<StatusCode, cctui_server_kernel::error::AppError> {
    let found = crate::store::dispatch_records::dismiss(
        state.pool(),
        ctx.user_id,
        ctx.owner_filter(),
        &session_id,
    )
    .await?;
    if found {
        state.resolve_notices(ctx.user_id, &[session_id]).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(cctui_server_kernel::error::AppError::new(
            StatusCode::NOT_FOUND,
            "no such dispatch record",
        ))
    }
}

/// `POST /api/v1/dispatch-records/dismiss-all` — dismiss every failed
/// dispatch the caller's list shows.
pub async fn dismiss_all_failed_dispatches<S: DispatchCore + DispatchNotify>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<cctui_proto::api::DismissedDispatches>, cctui_server_kernel::error::AppError> {
    use crate::execution_failures::list;
    use crate::store::dispatch_records::{Listing, dismiss_many};
    let listing =
        Listing { owner: ctx.owner_filter(), viewer: Some(ctx.user_id), include_dismissed: false };
    let ids: Vec<String> =
        list(state.pool(), listing).await?.into_iter().map(|f| f.session_id).collect();
    let dismissed = dismiss_many(state.pool(), ctx.user_id, &ids).await?;
    state.resolve_notices(ctx.user_id, &ids).await;
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
            "the dispatch carried env values the server does not keep ({}): only the \
             context-pack selectors without credentials are stored; re-send it from its \
             original caller",
            record.missing_env().join(", ")
        ),
        RerunBlocker::PromptTruncated => {
            "only the head of the prompt was stored; re-send it from its original caller".into()
        }
        RerunBlocker::PromptRedacted => {
            "<redacted> spans were stored only masked; re-send it from its original caller".into()
        }
        RerunBlocker::PayloadTooLarge => {
            "the payload was too large to store; re-send it from its original caller".into()
        }
    }
}

/// `POST /api/v1/dispatch-records/{id}/redispatch` — send a recorded
/// dispatch again as a new one, for its dispatcher or an admin.
///
/// Only stored
/// fields are re-sent: no secret env, no reply or notify callback.
pub async fn redispatch<S: DispatchCore + DispatchContext + DispatchNotify>(
    State(state): State<S>,
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
    let record = records::get(DispatchCore::pool(&state), &session_id)
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
    if !records::claim_redispatch(DispatchCore::pool(&state), &session_id).await.map_err(db_err)? {
        return Err(already());
    }
    let caller = ctx.user_id;
    let out = dispatch(State(state.clone()), Extension(ctx), Json(req)).await;
    let new_id = out.as_ref().ok().map(|(_, r)| r.session_id.as_str());
    if let Err(e) =
        records::finish_redispatch(DispatchCore::pool(&state), &session_id, new_id).await
    {
        tracing::warn!(session = %session_id, error = %e, "re-dispatch not recorded");
    }
    if out.is_ok() {
        state.resolve_notices(caller, std::slice::from_ref(&session_id)).await;
    }
    out
}

/// Delete the workload of a killed dispatched session no daemon took the kill
/// for, so a worker that never started (a `Pending` pod) stops holding its
/// slot. `true` once the dispatcher dropped it.
pub async fn cancel_dispatched_workload(
    state: &(impl DispatchCore + DispatchContext),
    session_id: &str,
) -> bool {
    let row: Result<Option<(String, String, Option<uuid::Uuid>)>, sqlx::Error> = sqlx::query_as(
        "SELECT dh.dispatcher_name, dh.handle, COALESCE(s.user_id, r.user_id) \
         FROM dispatch_handles dh \
         LEFT JOIN sessions s ON s.id = dh.session_id \
         LEFT JOIN dispatch_records r ON r.session_id = dh.session_id \
         WHERE dh.session_id = $1 AND dh.gone_at IS NULL",
    )
    .bind(session_id)
    .fetch_optional(DispatchCore::pool(state))
    .await;
    let (name, handle, owner) = match row {
        Ok(Some(row)) => row,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "dispatch handle lookup failed");
            return false;
        }
    };
    let Some(dispatcher) = killed_sessions_dispatcher(state, owner, &name).await else {
        tracing::warn!(%session_id, dispatcher = %name, "killed session's dispatcher unavailable");
        return false;
    };
    if let Err(e) = dispatcher.cancel(&handle).await {
        tracing::warn!(%session_id, %handle, error = %e, "killed session's workload not deleted");
        return false;
    }
    let marked = sqlx::query(
        "UPDATE dispatch_handles SET gone_at = now() WHERE session_id = $1 AND handle = $2",
    )
    .bind(session_id)
    .bind(&handle)
    .execute(DispatchCore::pool(state))
    .await;
    if let Err(e) = marked {
        tracing::warn!(%session_id, error = %e, "cancelled dispatch handle not marked gone");
    }
    tracing::info!(%session_id, %handle, "killed session's workload deleted");
    true
}

/// The dispatcher a handle was forwarded to, resolved the way the liveness
/// sweep does: the owner's, else across owners for an admin dispatch.
async fn killed_sessions_dispatcher(
    state: &impl DispatchContext,
    owner: Option<uuid::Uuid>,
    name: &str,
) -> Option<std::sync::Arc<dyn crate::dispatchers::Dispatcher>> {
    use crate::dispatch::resolve_dispatcher as resolve;
    match resolve(state, owner, name).await {
        Ok(Some(d)) => Some(d),
        _ if owner.is_none() => None,
        _ => resolve(state, None, name).await.ok().flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DefaultBinding, binding_family, colliding_family, default_binding, dispatch_ceiling,
        resolve_dispatch_account, resolve_dispatch_session_id, rewrite_model_if_aliased,
    };
    use cctui_proto::adapter::PermissionMode;
    use cctui_server_kernel::accounts_types::Family;

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
                exposure: None,
            }),
        ))
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
        let Some(url) = cctui_server_kernel::test_support::db_url("dispatch_policy_use_per_run")
        else {
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
        let Some(url) = cctui_server_kernel::test_support::db_url("dispatch_policy_cap_race")
        else {
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
        let Some(url) = cctui_server_kernel::test_support::db_url("dispatch_no_policy_race") else {
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
