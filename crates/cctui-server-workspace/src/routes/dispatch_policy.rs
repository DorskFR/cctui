//! Admin-managed per-user dispatch policy: `/admin/users/{id}/dispatch-policy`
//! plus the checks `POST /sessions/dispatch` runs before forwarding. A user
//! without a policy row is never touched by anything here.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use cctui_proto::api::admin::{DispatchPolicy, DispatchPolicyRules};
#[cfg(test)]
use cctui_proto::api::{ApiError, DispatchRequest};

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;
use crate::store::dispatch_policies as store;

fn admin_only(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

async fn user_exists(pool: &sqlx::PgPool, id: Uuid) -> Result<(), AppError> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if found { Ok(()) } else { Err(AppError::new(StatusCode::NOT_FOUND, "user not found")) }
}

/// `GET /admin/users/{id}/dispatch-policy` — the user's policy, `null` when
/// the user dispatches unrestricted.
pub async fn get_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Option<DispatchPolicy>>, AppError> {
    admin_only(&ctx)?;
    user_exists(&state.pool, user_id).await?;
    Ok(Json(store::get(&state.pool, user_id).await?))
}

/// `PUT /admin/users/{id}/dispatch-policy` — replace the whole policy.
pub async fn set_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(mut rules): Json<DispatchPolicyRules>,
) -> Result<Json<DispatchPolicy>, AppError> {
    admin_only(&ctx)?;
    user_exists(&state.pool, user_id).await?;
    normalize(&mut rules).map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    store::upsert(&state.pool, user_id, &rules).await?;
    tracing::info!(%user_id, "dispatch policy set");
    store::get(&state.pool, user_id)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "policy vanished"))
}

/// `DELETE /admin/users/{id}/dispatch-policy` — the user dispatches
/// unrestricted again.
pub async fn delete_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    admin_only(&ctx)?;
    if !store::delete(&state.pool, user_id).await? {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no dispatch policy for this user"));
    }
    tracing::info!(%user_id, "dispatch policy removed");
    Ok(StatusCode::NO_CONTENT)
}

use cctui_server_core::dispatch_policy::normalize;

#[cfg(test)]
use cctui_server_core::dispatch_policy::{Rule, Violation, check_rules};

#[cfg(test)]
async fn enforce(
    state: &AppState,
    user_id: Uuid,
    req: &mut DispatchRequest,
) -> Result<Option<DispatchPolicy>, (StatusCode, Json<ApiError>)> {
    cctui_server_core::dispatch_policy::enforce(&state.pool, user_id, req).await
}

#[cfg(test)]
mod tests {
    use super::{Rule, check_rules, normalize};
    use cctui_proto::api::DispatchRequest;
    use cctui_proto::api::admin::{ContextPackGrant, DispatchPolicyRules};

    fn req(dispatcher: &str, payload: serde_json::Value) -> DispatchRequest {
        DispatchRequest {
            dispatcher: dispatcher.into(),
            session_id: None,
            timeout: None,
            reply_url: None,
            notify_url: None,
            notify_secret: None,
            payload,
            account: None,
            provider: None,
            accounts: Vec::new(),
        }
    }

    fn pack(
        name: &str,
        url: &str,
        git_ref: Option<&str>,
        subdir: Option<&str>,
    ) -> ContextPackGrant {
        ContextPackGrant {
            name: name.into(),
            url: url.into(),
            git_ref: git_ref.map(str::to_owned),
            subdir: subdir.map(str::to_owned),
        }
    }

    fn rule_of(result: Result<(), super::Violation>) -> Option<Rule> {
        result.err().map(|v| v.rule)
    }

    #[test]
    fn empty_rules_restrict_nothing() {
        let rules = DispatchPolicyRules::default();
        let mut r = req("anything", serde_json::json!({"model": "x", "adapter": "codex"}));
        assert_eq!(check_rules(&rules, &mut r), Ok(()));
        assert_eq!(r.timeout, None, "no ceiling, no injected timeout");
    }

    #[test]
    fn the_dispatcher_rule_allows_listed_names_and_refuses_others() {
        let rules =
            DispatchPolicyRules { dispatchers: vec!["cctui-kube".into()], ..Default::default() };
        assert_eq!(check_rules(&rules, &mut req("cctui-kube", serde_json::json!({}))), Ok(()));
        let err = check_rules(&rules, &mut req("local", serde_json::json!({}))).unwrap_err();
        assert_eq!(err.rule, Rule::Dispatcher);
        assert!(err.to_string().contains("(dispatcher)"), "{err}");
        assert!(err.to_string().contains("cctui-kube"), "names the allowed set: {err}");
    }

    #[test]
    fn the_profile_rule_requires_a_listed_profile() {
        let rules = DispatchPolicyRules { profiles: vec!["tonbo".into()], ..Default::default() };
        assert_eq!(
            check_rules(&rules, &mut req("d", serde_json::json!({"profile": "tonbo"}))),
            Ok(())
        );
        assert_eq!(
            rule_of(check_rules(&rules, &mut req("d", serde_json::json!({"profile": "lean"})))),
            Some(Rule::Profile)
        );
        assert_eq!(
            rule_of(check_rules(&rules, &mut req("d", serde_json::json!({})))),
            Some(Rule::Profile),
            "an absent profile would select the dispatcher default"
        );
    }

    #[test]
    fn the_adapter_rule_reads_the_payload_adapter_with_its_default() {
        let rules =
            DispatchPolicyRules { adapters: vec!["claude-code".into()], ..Default::default() };
        assert_eq!(check_rules(&rules, &mut req("d", serde_json::json!({}))), Ok(()));
        assert_eq!(
            check_rules(&rules, &mut req("d", serde_json::json!({"adapter_id": "claude-code"}))),
            Ok(())
        );
        assert_eq!(
            rule_of(check_rules(&rules, &mut req("d", serde_json::json!({"adapter": "codex"})))),
            Some(Rule::Adapter)
        );
    }

    #[test]
    fn the_model_rule_requires_a_listed_model() {
        let rules = DispatchPolicyRules { models: vec!["sonnet".into()], ..Default::default() };
        assert_eq!(
            check_rules(&rules, &mut req("d", serde_json::json!({"model": "sonnet"}))),
            Ok(())
        );
        assert_eq!(
            rule_of(check_rules(&rules, &mut req("d", serde_json::json!({"model": "opus"})))),
            Some(Rule::Model)
        );
        assert_eq!(
            rule_of(check_rules(&rules, &mut req("d", serde_json::json!({})))),
            Some(Rule::Model)
        );
    }

    #[test]
    fn the_model_rule_covers_codex_model_and_model_env_overrides() {
        let rules = DispatchPolicyRules { models: vec!["sonnet".into()], ..Default::default() };
        let with = |key: &str, value: serde_json::Value| {
            let mut payload = serde_json::json!({"model": "sonnet"});
            payload[key] = value;
            req("d", payload)
        };
        let codex = |model: &str| with("codex_model", model.into());
        assert_eq!(check_rules(&rules, &mut codex("sonnet")), Ok(()));
        assert_eq!(
            check_rules(&rules, &mut with("env", serde_json::json!({"GITHUB_TOKEN": "t"}))),
            Ok(())
        );
        assert_eq!(rule_of(check_rules(&rules, &mut codex("gpt-5.6-sol"))), Some(Rule::Model));
        for key in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
            "anthropic_model",
        ] {
            let err = check_rules(&rules, &mut with("env", serde_json::json!({key: "opus"})))
                .unwrap_err();
            assert_eq!(err.rule, Rule::Model, "{key}");
            assert!(err.to_string().contains(key), "names the key: {err}");
        }
        let overrides =
            serde_json::json!({"codex_model": "gpt", "env": {"ANTHROPIC_MODEL": "opus"}});
        assert_eq!(
            check_rules(&DispatchPolicyRules::default(), &mut req("d", overrides)),
            Ok(()),
            "an open model list leaves model overrides alone"
        );
    }

    #[test]
    fn any_policy_refuses_caller_base_urls_and_worker_net_env() {
        let rules = DispatchPolicyRules::default();
        let keys = ["FIREWORKS_BASE_URL", "anthropic_base_url", "WORKER_NET_ALLOW", "worker_net_x"];
        for key in keys {
            let payload = serde_json::json!({"env": {key: "https://x"}});
            let err = check_rules(&rules, &mut req("d", payload)).unwrap_err();
            assert_eq!(err.rule, Rule::Egress, "{key}");
            assert!(err.to_string().contains(key), "names the key: {err}");
        }
        let unrelated = serde_json::json!({"env": {"BASE_URL_NOTE": "x", "NET_ALLOW": "y"}});
        assert_eq!(check_rules(&rules, &mut req("d", unrelated)), Ok(()));
    }

    #[test]
    fn a_restricted_pack_refuses_ref_or_subdir_without_a_url() {
        let rules = DispatchPolicyRules {
            context_packs: vec![pack("p", "https://git/p.git", None, None)],
            ..Default::default()
        };
        for key in ["CONTEXT_PACK_REF", "CONTEXT_PACK_SUBDIR"] {
            let payload = serde_json::json!({"env": {key: "evil"}});
            assert_eq!(
                rule_of(check_rules(&rules, &mut req("d", payload))),
                Some(Rule::ContextPack),
                "{key}"
            );
        }
        let ref_only = serde_json::json!({"env": {"CONTEXT_PACK_REF": "evil"}});
        assert_eq!(
            check_rules(&DispatchPolicyRules::default(), &mut req("d", ref_only)),
            Ok(()),
            "unrestricted packs leave the ref alone"
        );
    }

    #[test]
    fn normalize_rejects_a_ref_or_subdir_embedded_in_a_pack_url() {
        for url in ["https://git/p.git@v1", "https://git/p.git#packs/a", "git@host:o/r.git@v1"] {
            let mut rules = DispatchPolicyRules {
                context_packs: vec![pack("p", url, None, None)],
                ..Default::default()
            };
            assert!(normalize(&mut rules).unwrap_err().contains("git_ref/subdir"), "{url}");
        }
        for url in ["https://user@git/p.git", "git@github.com:o/r.git", "https://git/p.git"] {
            let mut rules = DispatchPolicyRules {
                context_packs: vec![pack("p", url, None, None)],
                ..Default::default()
            };
            assert_eq!(normalize(&mut rules), Ok(()), "{url}");
        }
    }

    #[test]
    fn the_context_pack_rule_matches_url_and_any_pinned_ref_or_subdir() {
        let rules = DispatchPolicyRules {
            context_packs: vec![
                pack("loose", "https://git/loose.git", None, None),
                pack("pinned", "https://git/pinned.git", Some("main"), Some("packs/a")),
            ],
            ..Default::default()
        };
        let env = |url: &str, git_ref: &str, subdir: &str| {
            serde_json::json!({"env": {
                "CONTEXT_PACK_URL": url, "CONTEXT_PACK_REF": git_ref, "CONTEXT_PACK_SUBDIR": subdir
            }})
        };
        assert_eq!(check_rules(&rules, &mut req("d", serde_json::json!({}))), Ok(()), "no pack");
        assert_eq!(
            check_rules(&rules, &mut req("d", env("https://git/loose.git", "v9", "x"))),
            Ok(())
        );
        assert_eq!(
            check_rules(&rules, &mut req("d", env("https://git/pinned.git", "main", "packs/a"))),
            Ok(())
        );
        for bad in [
            env("https://git/other.git", "main", "packs/a"),
            env("https://git/pinned.git", "dev", "packs/a"),
            env("https://git/pinned.git", "main", "packs/b"),
            serde_json::json!({"env": {"CONTEXT_PACK_URL": "https://git/pinned.git"}}),
        ] {
            assert_eq!(rule_of(check_rules(&rules, &mut req("d", bad))), Some(Rule::ContextPack));
        }
    }

    #[test]
    fn the_timeout_ceiling_refuses_longer_runs_and_fills_in_a_missing_one() {
        let rules = DispatchPolicyRules { max_timeout_minutes: Some(30), ..Default::default() };
        let mut within = req("d", serde_json::json!({}));
        within.timeout = Some(30);
        assert_eq!(check_rules(&rules, &mut within), Ok(()));
        let mut over = req("d", serde_json::json!({}));
        over.timeout = Some(31);
        assert_eq!(rule_of(check_rules(&rules, &mut over)), Some(Rule::Timeout));
        let mut none = req("d", serde_json::json!({}));
        assert_eq!(check_rules(&rules, &mut none), Ok(()));
        assert_eq!(none.timeout, Some(30));
    }

    #[test]
    fn normalize_trims_dedups_and_rejects_nameless_packs_and_zero_limits() {
        let mut rules = DispatchPolicyRules {
            dispatchers: vec![" b ".into(), "a".into(), "b".into(), String::new()],
            context_packs: vec![pack("p", " https://x ", Some(" "), None)],
            ..Default::default()
        };
        assert_eq!(normalize(&mut rules), Ok(()));
        assert_eq!(rules.dispatchers, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(rules.context_packs[0].url, "https://x");
        assert_eq!(rules.context_packs[0].git_ref, None);

        let mut nameless = DispatchPolicyRules {
            context_packs: vec![pack("", "https://x", None, None)],
            ..Default::default()
        };
        assert!(normalize(&mut nameless).is_err());
        let mut dup = DispatchPolicyRules {
            context_packs: vec![
                pack("p", "https://x", None, None),
                pack("p", "https://y", None, None),
            ],
            ..Default::default()
        };
        assert!(normalize(&mut dup).is_err());
        let mut zero = DispatchPolicyRules { daily_cap: Some(0), ..Default::default() };
        assert!(normalize(&mut zero).is_err());
    }

    async fn seed_user(pool: &sqlx::PgPool, tag: &str) -> uuid::Uuid {
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    async fn seed_dispatcher(pool: &sqlx::PgPool, owner: uuid::Uuid, name: &str) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO dispatchers (id, user_id, name, kind, key_hash, key_preview) \
             VALUES ($1, $2, $3, 'kubernetes', $4, 'dk…')",
        )
        .bind(id)
        .bind(owner)
        .bind(name)
        .bind(format!("dk-{id}"))
        .execute(pool)
        .await
        .expect("seed dispatcher");
        id
    }

    async fn connect(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    /// Without a policy a user resolves only its own dispatchers; a policy
    /// naming another owner's dispatcher lets the name resolve, and the admin
    /// view is unchanged either way.
    #[tokio::test]
    async fn a_policy_grants_another_owners_dispatcher_by_name() {
        let Some(pool) = connect("a_policy_grants_another_owners_dispatcher_by_name").await else {
            return;
        };
        let state = crate::state::AppState::for_test(pool.clone());
        let owner = seed_user(&pool, "owner").await;
        let service = seed_user(&pool, "service").await;
        let name = format!("kube-{}", uuid::Uuid::new_v4());
        let dispatcher_id = seed_dispatcher(&pool, owner, &name).await;
        let resolve = |uid: Option<uuid::Uuid>| {
            crate::routes::dispatch::resolve_dispatcher(&state, uid, &name)
        };

        assert!(resolve(Some(owner)).await.unwrap().is_some(), "the owner resolves it");
        assert!(resolve(Some(service)).await.unwrap().is_none(), "no policy: not visible");
        assert!(resolve(None).await.unwrap().is_some(), "admin resolves across owners");

        let rules = DispatchPolicyRules { dispatchers: vec![name.clone()], ..Default::default() };
        crate::store::dispatch_policies::upsert(&pool, service, &rules).await.unwrap();
        assert!(resolve(Some(service)).await.unwrap().is_some(), "granted by policy");

        let other =
            DispatchPolicyRules { dispatchers: vec!["elsewhere".into()], ..Default::default() };
        crate::store::dispatch_policies::upsert(&pool, service, &other).await.unwrap();
        assert!(
            resolve(Some(service)).await.unwrap().is_none(),
            "a policy naming other dispatchers grants nothing"
        );

        sqlx::query("DELETE FROM dispatchers WHERE id = $1")
            .bind(dispatcher_id)
            .execute(&pool)
            .await
            .ok();
        for uid in [owner, service] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }

    /// The store round-trips every rule, `enforce` is a no-op without a row and
    /// refuses on the DB-backed caps once the row exists.
    #[tokio::test]
    async fn enforce_is_inert_without_a_policy_and_applies_the_caps_with_one() {
        let Some(pool) =
            connect("enforce_is_inert_without_a_policy_and_applies_the_caps_with_one").await
        else {
            return;
        };
        let state = crate::state::AppState::for_test(pool.clone());
        let uid = seed_user(&pool, "svc").await;
        let mut r = req("anything", serde_json::json!({"model": "whatever"}));
        assert!(super::enforce(&state, uid, &mut r).await.unwrap().is_none());
        assert_eq!(r.timeout, None);
        let mut unbound = req(
            "anything",
            serde_json::json!({"model": "opus", "codex_model": "gpt-5.6-sol", "env": {
                "FIREWORKS_BASE_URL": "https://x", "WORKER_NET_ALLOW": "*",
                "ANTHROPIC_MODEL": "opus", "CONTEXT_PACK_REF": "dev"
            }}),
        );
        let sent = unbound.payload.clone();
        assert!(super::enforce(&state, uid, &mut unbound).await.unwrap().is_none());
        assert_eq!(unbound.payload, sent, "no policy: the payload goes through untouched");

        let rules = DispatchPolicyRules {
            dispatchers: vec!["anything".into()],
            context_packs: vec![pack("p", "https://git/p.git", Some("main"), None)],
            max_timeout_minutes: Some(15),
            daily_cap: Some(1),
            max_concurrent: Some(1),
            ..Default::default()
        };
        crate::store::dispatch_policies::upsert(&pool, uid, &rules).await.unwrap();
        let stored = crate::store::dispatch_policies::get(&pool, uid).await.unwrap().unwrap();
        assert_eq!(stored.rules, rules);
        assert_eq!(stored.user_id, uid);

        let applied = super::enforce(&state, uid, &mut r).await.unwrap().expect("policy applied");
        assert_eq!(applied.rules.daily_cap, Some(1));
        assert_eq!(r.timeout, Some(15), "the ceiling fills in a missing timeout");

        crate::store::dispatch_policies::record_use(&pool, uid, &format!("s-1-{uid}"))
            .await
            .unwrap();
        let (code, body) = super::enforce(&state, uid, &mut r).await.unwrap_err();
        assert_eq!(code, axum::http::StatusCode::FORBIDDEN);
        assert!(body.error.contains("(daily_cap)"), "{}", body.error);

        let relaxed = DispatchPolicyRules { daily_cap: None, ..rules };
        crate::store::dispatch_policies::upsert(&pool, uid, &relaxed).await.unwrap();
        let live_sid = format!("s-live-{uid}");
        for sid in [live_sid.clone(), format!("s-refused-{uid}")] {
            sqlx::query(
                "INSERT INTO auth_keys (user_id, key_hash, kind, session_id, expires_at) \
                 VALUES ($1, $2, 'ephemeral', $3, now() + interval '1 hour')",
            )
            .bind(uid)
            .bind(format!("eph-{sid}"))
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(
            crate::store::dispatch_policies::live_dispatches(&pool, uid).await.unwrap(),
            0,
            "a key whose forward was refused holds no slot"
        );
        crate::store::dispatch_policies::record_use(&pool, uid, &live_sid).await.unwrap();
        assert_eq!(crate::store::dispatch_policies::live_dispatches(&pool, uid).await.unwrap(), 1);
        let (code, body) = super::enforce(&state, uid, &mut r).await.unwrap_err();
        assert_eq!(code, axum::http::StatusCode::FORBIDDEN);
        assert!(body.error.contains("(max_concurrent)"), "{}", body.error);

        assert!(crate::store::dispatch_policies::delete(&pool, uid).await.unwrap());
        assert!(super::enforce(&state, uid, &mut r).await.unwrap().is_none());

        sqlx::query("DELETE FROM auth_keys WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// A policy-bound service user reads only the sessions it dispatched: the
    /// session read guard is unchanged by the policy.
    #[tokio::test]
    async fn a_service_user_reads_only_its_own_sessions() {
        let Some(pool) = connect("a_service_user_reads_only_its_own_sessions").await else {
            return;
        };
        let service = seed_user(&pool, "svc").await;
        let other = seed_user(&pool, "other").await;
        let rules = DispatchPolicyRules { dispatchers: vec!["kube".into()], ..Default::default() };
        crate::store::dispatch_policies::upsert(&pool, service, &rules).await.unwrap();
        let mut sids = Vec::new();
        for uid in [service, other] {
            let machine = uuid::Uuid::new_v4();
            sqlx::query("INSERT INTO machines (id, user_id, name, key_hash, kind) VALUES ($1, $2, 'dispatch', $3, 'dispatch')")
                .bind(machine)
                .bind(uid)
                .bind(format!("mk-{machine}"))
                .execute(&pool)
                .await
                .unwrap();
            let sid = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
                 VALUES ($1, $2, $2, $3, '/w', 'active')",
            )
            .bind(&sid)
            .bind(machine)
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
            sids.push((sid, machine));
        }
        let ctx = crate::auth::AuthContext {
            user_id: service,
            key_id: uuid::Uuid::new_v4(),
            machine_id: None,
            scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
        };
        assert_eq!(crate::authz::authorize_session_read(&ctx, &sids[0].0, &pool).await, Ok(()));
        assert_eq!(
            crate::authz::authorize_session_read(&ctx, &sids[1].0, &pool).await,
            Err(axum::http::StatusCode::FORBIDDEN)
        );

        for (sid, machine) in &sids {
            sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(&pool).await.ok();
            sqlx::query("DELETE FROM machines WHERE id = $1")
                .bind(machine)
                .execute(&pool)
                .await
                .ok();
        }
        for uid in [service, other] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }
}
