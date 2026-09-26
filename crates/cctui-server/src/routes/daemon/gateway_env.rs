use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;

// ---- /api/v1/daemon/sessions/{id}/gateway-env ----

/// Resolve a session's gateway-routing env for the daemon's launch chokepoint.
/// The daemon calls this at every worker (re)launch — spawn, resume,
/// cold-resume, fork — so the gateway credential comes from the server's durable
/// `sessions.account_id` binding rather than from whatever env the triggering
/// command happened to carry. This is what makes routing survive a daemon /
/// claude-daemon restart and session-id rotation: the env is re-derived from the
/// DB, not from volatile process/in-memory state.
///
/// Self-authenticating like [`auth`]/[`ws`]: the machine key is the Bearer.
/// Scoped to the machine's owning user so a daemon can't resolve another user's
/// account env.
///
/// Returns `{account_bound, env}`:
///   * no binding → `{false, {}}` (no gateway routing needed; launch as-is)
///   * bound + mintable → `{true, env}` (inject and launch)
///   * bound + unmintable (account gone) → `{true, {}}` (daemon fails closed)
///   * transient DB error → 500 (daemon falls back to the pushed env hint)
pub async fn session_gateway_env(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<cctui_proto::api::GatewayEnvResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };

    let allowed = gateway_env_allowed(&state.pool, ctx.user_id, machine_id, &session_id)
        .await
        .map_err(|e| {
            tracing::error!(%session_id, "daemon gateway-env ownership lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if !allowed {
        return Ok(Json(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
            whip_phrases: None,
            spawn_capability: None,
            plugins: Vec::new(),
        }));
    }

    // The machine user's whip stall-phrase override rides this pull to
    // reach the connectionless `whip-stop-hook`; per-user, so it applies whether
    // or not the session is account-bound.
    let user_settings = user_settings_of(&state, ctx.user_id).await;
    let whip_phrases =
        user_settings.as_ref().and_then(crate::routes::settings::whip_stop_phrases_of);
    let plugins = session_plugins(&state.plugins, user_settings.as_ref());

    // Resolve EVERY bound family (one account per family) and re-mint each, so a
    // worker carrying both claude + codex creds gets both restored on launch,
    // not just the last-minted family. The families emit disjoint env
    // keys, so the merge never collides.
    let accounts = crate::routes::gateway::resolve_session_accounts(&state, &session_id).await;
    if accounts.is_empty() {
        return Ok(Json(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
            whip_phrases,
            spawn_capability: spawn_capability_for(&state, &session_id).await,
            plugins,
        }));
    }
    let mut env = std::collections::BTreeMap::new();
    for account_id in accounts {
        match crate::routes::gateway::mint_session_env_for_account(&state, account_id, &session_id)
            .await
        {
            Ok(Some(e)) => env.extend(e),
            // This family's account row is gone — skip it; other families may
            // still mint. With every family gone, `env` stays empty and we report
            // bound + empty below so the daemon fails closed instead of launching
            // a worker that will 401.
            Ok(None) => {}
            // Transient DB failure: let the daemon fall back to its pushed env hint.
            Err(e) => {
                tracing::error!(%session_id, "daemon gateway-env mint failed: {e}");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    // Merge the bound account(s)' per-account `settings_json` so it
    // rides alongside the gateway env on this same pull. Re-served on every
    // (re)launch, it survives a daemon / claude-daemon restart; the daemon
    // deep-merges it UNDER its managed hook settings when writing the worker's
    // `--settings` file (that daemon-side merge is).
    let mut settings = crate::routes::gateway::resolve_session_settings(&state, &session_id).await;
    // Codex defaults to the `priority` tier, so a relaunch with no opinion is the
    // expensive one. Serve a concrete tier whenever an openai account is bound.
    if crate::routes::gateway::session_has_family(
        &state,
        &session_id,
        crate::routes::gateway::Family::Openai,
    )
    .await
    {
        settings = crate::settings_catalog::codex::overlay_service_tier(settings);
    }
    // This pull only happens when the daemon is actually (re)launching the
    // worker — a session marked `ended` (possibly by a spurious end)
    // is provably coming back to life, so un-stick the terminal status here.
    // `archived` stays parked: un-archiving is an explicit user action.
    let _ = sqlx::query(
        "UPDATE sessions SET status = 'active', ended_at = NULL, end_reason = NULL, end_detail = NULL \
         WHERE id = $1 AND status = 'ended'",
    )
    .bind(&session_id)
    .execute(&state.pool)
    .await;
    Ok(Json(cctui_proto::api::GatewayEnvResponse {
        account_bound: true,
        env,
        settings,
        whip_phrases,
        spawn_capability: spawn_capability_for(&state, &session_id).await,
        plugins,
    }))
}

/// Whether a daemon may resolve `session_id`'s gateway env: the row is missing
/// (spawn-time race before register; the account resolves via the freshly
/// minted token row) or owned by `user_id`, and by `machine_id` when the row
/// names a machine. A NULL owner is foreign.
async fn gateway_env_allowed(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<Uuid>)> =
        sqlx::query_as("SELECT user_id, machine_uuid FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_none_or(|(owner, machine)| {
        owner == Some(user_id) && machine.is_none_or(|m| m == machine_id)
    }))
}

/// The session's `CctuiAgent` capability, as recorded by the spawn/dispatch that
/// launched it. Falls back to the durable table so a server restart does not
/// disarm a live session's spawn tool, and to the machine default so a session
/// nobody granted anything — an adopted native session, an undeclared dispatch —
/// still gets the relay. Only a lookup failure yields `None`.
async fn spawn_capability_for(
    state: &AppState,
    session_id: &str,
) -> Option<cctui_proto::api::SpawnCapability> {
    if let Some(cap) = state.spawn_capabilities.get(session_id) {
        return Some(cap.clone());
    }
    match crate::store::spawn_capabilities::get(&state.pool, session_id).await {
        Ok(Some(cap)) => {
            state.spawn_capabilities.insert(session_id.to_owned(), cap.clone());
            Some(cap)
        }
        Ok(None) => Some(grant_default(state, session_id).await),
        Err(e) => {
            tracing::error!(
                %session_id,
                error = %e,
                "spawn-capability lookup failed — CctuiAgent will be withheld from this session"
            );
            None
        }
    }
}

/// The default grant for a session launched without one. It carries no mode
/// ceiling: spawn-child then caps children at the session's live
/// `permission_mode`. A persist failure still serves the grant for this launch.
async fn grant_default(state: &AppState, session_id: &str) -> cctui_proto::api::SpawnCapability {
    let cap = crate::routes::server_settings::spawn_default_capability(state).await;
    if let Err(e) = crate::store::spawn_capabilities::upsert(&state.pool, session_id, &cap).await {
        tracing::error!(%session_id, error = %e, "default spawn-capability persist failed");
    }
    state.spawn_capabilities.insert(session_id.to_owned(), cap.clone());
    cap
}

/// The machine user's clamped `whipStopPhrases` block from
/// `user_settings.data`, or `None` when unset / reduced to the default. Read from
/// the DB on the same gateway-env pull that carries the account settings.
async fn user_settings_of(state: &AppState, user_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
}

/// The skill bundles and setting env of every installed plugin the user
/// enabled; plugins that contribute neither are skipped.
pub fn session_plugins(
    registry: &crate::plugins::PluginRegistry,
    settings: Option<&serde_json::Value>,
) -> Vec<cctui_proto::api::SessionPlugin> {
    crate::plugins::enabled_ids(settings)
        .iter()
        .filter_map(|id| registry.get(id))
        .map(|p| (crate::plugins::plugin_env(&p.manifest, settings), p))
        .filter(|(env, p)| !p.skill_files.is_empty() || !env.is_empty())
        .map(|(env, p)| cctui_proto::api::SessionPlugin {
            id: p.manifest.id,
            version: p.manifest.version,
            skills_hash: p.skills_hash,
            files: p.skill_files,
            env,
        })
        .collect()
}

// ---- /api/v1/daemon/sessions/{id}/token-valid ----

/// Query for [`session_token_valid`]: the sha256 hex of the session token the
/// daemon launched the worker with. Hash-only on purpose — no token material
/// on the wire (invariant).
#[derive(Deserialize)]
pub struct TokenValidQuery {
    pub hash: String,
}

/// Does this session's minted token still resolve at the gateway?
///
/// The daemon's validity sweep calls this for TRUSTED workers (ones it
/// launched with gateway env) so a worker whose `session_tokens` row got
/// unbound/deleted (401s forever at the gateway) is observable and healable.
/// `valid` = a `session_tokens` row with this hash exists
/// FOR THIS SESSION, is not revoked, and joins a live `account_providers` row
/// (the same join [`resolve_account`](crate::routes::gateway) applies, but by
/// hash equality).
///
/// Self-authenticating like [`session_gateway_env`]: machine-key Bearer.
/// User-scoped the same way, except a session owned by another user answers
/// 404 rather than `{valid: false}` — a false `valid` triggers a destructive
/// kill + cold-resume daemon-side, and the daemon treats any non-200 as
/// "unknown" (no heal). Transient DB error → 500 for the same reason
/// (fail-open; the heal kill is destructive).
pub async fn session_token_valid(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<TokenValidQuery>,
) -> Result<Json<cctui_proto::api::TokenValidResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    if ctx.machine_id.is_none() {
        return Err(StatusCode::FORBIDDEN);
    }

    // User-scope (mirrors `session_gateway_env`): only answer for sessions
    // owned by the machine's user. A foreign session 404s — NOT `valid:false`,
    // which would trigger a destructive kill + cold-resume daemon-side.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!(%session_id, "token-valid owner lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if owner.is_some_and(|o| o != ctx.user_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // Valid = a live token row with this hash, scoped to this session, that
    // still joins a live account (same join semantics as the gateway's
    // `resolve_account`, but by hash equality — the daemon sends the sha256
    // hex of the token it launched the worker with, never the token itself).
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM session_tokens t JOIN account_providers a ON a.id = t.account_id
            WHERE t.token_hash = $1 AND t.session_id = $2 AND t.revoked_at IS NULL
              AND (t.expires_at IS NULL OR t.expires_at > now())
         )",
    )
    .bind(&q.hash)
    .bind(&session_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!(%session_id, "token-valid lookup failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(cctui_proto::api::TokenValidResponse { valid }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::daemon::test_support::{
        drop_machines, seed_machine, seed_owned_session, seed_session,
    };

    #[test]
    fn ungranted_sessions_spawn_up_to_their_live_mode() {
        use crate::routes::spawn_child::{Usage, authorize};
        use cctui_proto::adapter::PermissionMode;
        let cap = cctui_proto::api::SpawnCapability::machine_default();
        let req = |m| cctui_proto::api::SpawnChildRequest {
            adapter: "codex".into(),
            prompt: "go".into(),
            permission_mode: Some(m),
            ..Default::default()
        };
        let usage = |p| Usage { parent_mode: Some(p), ..Usage::default() };

        let a = authorize(Some(&cap), &req(PermissionMode::Yolo), &usage(PermissionMode::Yolo))
            .expect("yolo parent spawns yolo");
        assert_eq!(a.permission_mode, PermissionMode::Yolo);

        assert!(
            authorize(Some(&cap), &req(PermissionMode::Yolo), &usage(PermissionMode::Ask)).is_err()
        );
        let a = authorize(Some(&cap), &req(PermissionMode::Ask), &usage(PermissionMode::Ask))
            .expect("ask parent spawns ask");
        assert_eq!(a.permission_mode, PermissionMode::Ask);
    }

    #[tokio::test]
    async fn gateway_env_is_refused_for_sessions_the_machine_does_not_own() {
        let Some(url) = crate::routes::gateway::test_db_url("gateway_env_owner") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (ua, ma) = seed_machine(&pool, "owner").await;
        let (ub, mb) = seed_machine(&pool, "intruder").await;
        let ma2 = {
            let mid = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(mid)
            .bind(ua)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed second machine");
            mid
        };
        let owned = seed_owned_session(&pool, ua, ma).await;
        let (ownerless_user, ownerless) = seed_session(&pool, "claude-code", "anthropic").await;
        sqlx::query("UPDATE sessions SET user_id = NULL WHERE id = $1")
            .bind(&ownerless)
            .execute(&pool)
            .await
            .expect("clear owner");

        let allowed = async |user, machine, sid: &str| {
            super::gateway_env_allowed(&pool, user, machine, sid).await.expect("lookup")
        };
        assert!(allowed(ua, ma, &owned).await);
        assert!(allowed(ua, ma, "not-registered-yet").await);
        assert!(!allowed(ub, mb, &owned).await, "another user's session is refused");
        assert!(!allowed(ua, ma2, &owned).await, "another machine's session is refused");
        assert!(!allowed(ub, mb, &ownerless).await, "a NULL-owner session is refused");
        assert!(!allowed(ua, ma, &ownerless).await);

        sqlx::query("DELETE FROM machines WHERE id = $1").bind(ma2).execute(&pool).await.ok();
        drop_machines(&pool, &[owned, ownerless], &[(ua, ma), (ub, mb)]).await;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(ownerless_user)
            .execute(&pool)
            .await
            .ok();

        pool.close().await;
        assert!(
            super::gateway_env_allowed(&pool, ua, ma, "any").await.is_err(),
            "a lookup failure surfaces as an error (500), never as allowed",
        );
    }

    #[test]
    fn session_plugins_follow_the_users_enabled_flags() {
        use crate::plugins::test_support::write_plugin;
        let root = tempfile::tempdir().unwrap();
        write_plugin(
            root.path(),
            "on",
            r#","settings":[{"key":"host","label":"Host","env":"ON_HOST","type":"string"}]"#,
        );
        write_plugin(root.path(), "off", "");
        let noskill = root.path().join("bare");
        std::fs::create_dir_all(&noskill).unwrap();
        std::fs::write(
            noskill.join("plugin.json"),
            br#"{"id":"bare","name":"b","version":"1","cctuiApi":1}"#,
        )
        .unwrap();
        let registry = crate::plugins::PluginRegistry::from_dir(root.path().to_path_buf());
        let settings = serde_json::json!({
            "plugins": {
                "enabled": { "on": true, "off": false, "bare": true, "ghost": true },
                "config": { "on": { "host": "10.0.0.5" }, "off": { "host": "x" } }
            }
        });
        let plugins = super::session_plugins(&registry, Some(&settings));
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "on");
        assert_eq!(plugins[0].version, "1.2.3");
        assert_eq!(plugins[0].files, vec!["on/SKILL.md", "on/notes.txt"]);
        assert_eq!(plugins[0].skills_hash.len(), 16);
        assert_eq!(plugins[0].env.get("ON_HOST").map(String::as_str), Some("10.0.0.5"));
        assert!(super::session_plugins(&registry, None).is_empty());
    }
}
