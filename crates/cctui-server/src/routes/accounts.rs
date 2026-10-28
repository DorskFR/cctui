//! `/api/v1/accounts` — account identities + provider credentials.
//!
//! An **account** is an identity (e.g. `personal`, `enterprise`): a name, an
//! owner, optional extra environment, and sharing grants. Each account holds
//! zero or more **providers** (`account_providers`, née `oauth_accounts`):
//! one credential per provider family (anthropic | openai | fireworks | google) — a
//! native OAuth subscription, a compatible endpoint, or a static provider key.
//! OAuth refresh tokens
//! are encrypted at rest with the vault key (`crate::crypto`, same as
//! `api_keys`/`dispatchers`) and are **never** returned over the API —
//! list/get only ever surface provider/expiry/last-used + lightweight stats.
//! Accounts belong to the registering user and are visible/usable only by that
//! user (the `Human` route policy + `owner_filter`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::Utc;
use uuid::Uuid;

pub use cctui_proto::api::accounts::{
    AccountInfo, AccountModel, AccountUsage, AccountUsageEntry, CreateAccount, GrantShare,
    OAuthFinish, OAuthStartResponse, ProviderInfo, ProviderSpec, ShareInfo, UpdateAccount,
    UpdateProvider, UsageWindowView,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::routes::gateway;
use crate::state::AppState;

pub use cctui_server_core::accounts::*;
impl cctui_server_core::accounts::AccountsContext for AppState {
    fn account_core(&self) -> &cctui_server_core::state::CoreState {
        &self.core
    }
    fn oauth_logins(&self) -> &PendingOAuthLogins {
        &self.pending_oauth_logins
    }
}
/// Back-compat shim: if `CCTUI_CLAUDE_LITELLM_*` is set,
/// synthesize a server-owned **managed** anthropic-compatible provider per user
/// (under a dedicated `litellm (legacy)` account identity) so existing
/// deployments keep working after the env-var path is retired. Managed
/// providers are read-only over the API (edit/delete excluded). Idempotent:
/// re-upserted on every restart against the partial unique index
/// `(user_id, provider) WHERE managed`. A no-op unless both the endpoint and the
/// model list are configured. Retained for env-var deployments.
// Linear per-user upsert loop; the parent+child pair pushes it over the limit.
#[allow(clippy::cognitive_complexity)]
pub async fn sync_litellm_shim(pool: &sqlx::PgPool, config: &crate::config::Config) {
    let Some(endpoint) = config.claude_litellm_endpoint.as_deref() else { return };
    let models = config.claude_litellm_visible_models();
    if models.is_empty() {
        return;
    }
    let key = crate::crypto::vault_key();
    let cred = config.claude_litellm_token.as_deref().unwrap_or("sk-dummy");
    let enc_access = crate::crypto::encrypt(cred, &key);
    let models_json = serde_json::to_value(
        models
            .iter()
            .map(|m| AccountModel {
                model: m.model.clone(),
                label: m.label.clone(),
                ..AccountModel::default()
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or(serde_json::Value::Null);

    // One managed provider per user, keyed by (user_id, provider) WHERE managed,
    // parented under an idempotently upserted `litellm (legacy)` identity.
    let users: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE revoked_at IS NULL")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for uid in users {
        // Resolve the parent identity via the user's existing MANAGED provider
        // first — never adopt a same-named account the user made themselves:
        // the `ON CONFLICT (user_id, name) DO UPDATE … RETURNING id`
        // find-or-create hijacked any user account literally named
        // `litellm (legacy)`, making it read-only/undeletable (managed guard)
        // or failing the (account_id, family) unique index outright.
        let managed_parent: Option<Uuid> = sqlx::query_scalar(
            "SELECT account_id FROM account_providers \
             WHERE user_id = $1 AND managed AND provider = 'anthropic-compatible'",
        )
        .bind(uid)
        .fetch_optional(pool)
        .await
        .unwrap_or_default();
        let parent: Result<Option<Uuid>, sqlx::Error> = match managed_parent {
            Some(id) => Ok(Some(id)),
            None => {
                sqlx::query_scalar(
                    "INSERT INTO accounts (user_id, name) VALUES ($1, 'litellm (legacy)') \
                     ON CONFLICT (user_id, name) DO NOTHING \
                     RETURNING id",
                )
                .bind(uid)
                .fetch_optional(pool)
                .await
            }
        };
        let account_id = match parent {
            Ok(Some(id)) => id,
            Ok(None) => {
                tracing::warn!(
                    %uid,
                    "litellm shim skipped: user already has an unmanaged account named \
                     'litellm (legacy)'"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(%uid, "litellm shim parent upsert failed: {e}");
                continue;
            }
        };
        let res = sqlx::query(
            "INSERT INTO account_providers \
                (user_id, account_id, provider, encrypted_access_token, base_url, models, \
                 auth_scheme, managed) \
             VALUES ($1, $2, 'anthropic-compatible', $3, $4, $5, 'bearer', TRUE) \
             ON CONFLICT (user_id, provider) WHERE managed DO UPDATE \
               SET encrypted_access_token = EXCLUDED.encrypted_access_token, \
                   base_url = EXCLUDED.base_url, models = EXCLUDED.models, \
                   account_id = EXCLUDED.account_id",
        )
        .bind(uid)
        .bind(account_id)
        .bind(&enc_access)
        .bind(endpoint)
        .bind(&models_json)
        .execute(pool)
        .await;
        if let Err(e) = res {
            tracing::warn!(%uid, "litellm shim upsert failed: {e}");
        }
    }
    tracing::info!("CCTUI_CLAUDE_LITELLM_* shim: synced managed compatible providers");
}

pub async fn list_accounts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountInfo>>, AppError> {
    cctui_server_core::accounts::list_accounts(&state, &ctx).await
}

pub async fn get_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountInfo>, AppError> {
    cctui_server_core::accounts::get_account(&state, &ctx, id).await
}

pub async fn create_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateAccount>,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    cctui_server_core::accounts::create_account(&state, &ctx, req).await
}

pub async fn update_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAccount>,
) -> Result<Json<AccountInfo>, AppError> {
    cctui_server_core::accounts::update_account(&state, &ctx, id, req).await
}

pub async fn delete_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    cctui_server_core::accounts::delete_account(&state, &ctx, id).await
}

pub async fn add_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<ProviderSpec>,
) -> Result<(StatusCode, Json<ProviderInfo>), AppError> {
    cctui_server_core::accounts::add_provider(&state, &ctx, id, req).await
}

pub async fn update_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateProvider>,
) -> Result<Json<ProviderInfo>, AppError> {
    cctui_server_core::accounts::update_provider(&state, &ctx, (id, provider_id), req).await
}

pub async fn delete_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    cctui_server_core::accounts::delete_provider(&state, &ctx, (id, provider_id)).await
}

pub async fn move_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<MoveProvider>,
) -> Result<Json<ProviderInfo>, AppError> {
    cctui_server_core::accounts::move_provider(&state, &ctx, (id, provider_id), req).await
}

pub async fn oauth_start(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthStart>,
) -> Result<Json<OAuthStartResponse>, AppError> {
    cctui_server_core::accounts::oauth_start(&state, &ctx, req).await
}

pub async fn oauth_finish(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthFinish>,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    cctui_server_core::accounts::oauth_finish(&state, &ctx, req).await
}
/// How long a cached usage fetch is served before we re-hit upstream.
/// Anthropic's usage endpoint rate-limits per access token (safe at ~180s); we
/// cache for a few minutes so a viewed accounts page + slow background poll never
/// spams it, and many clients share one entry per account.
pub use cctui_server_core::gateway::usage::USAGE_CACHE_TTL;

/// Rate `window` now, on the slope from `previous` when there is one and
/// on the window average otherwise.
fn usage_window_view(
    window: crate::soft_limit::UsageWindow,
    previous: Option<crate::pace::Sample>,
) -> UsageWindowView {
    let pace = crate::pace::for_window(Utc::now(), &window, previous);
    UsageWindowView { window, pace }
}

/// The earlier readings a provider's windows are rated against, or none when
/// the lookup fails: a slope refines the pace, it is never a precondition for
/// serving the gauge.
async fn previous_samples(
    state: &AppState,
    provider_id: Uuid,
    usage: Option<&serde_json::Value>,
) -> Vec<crate::store::usage_samples::PreviousSample> {
    let Some(usage) = usage else {
        return Vec::new();
    };
    let windows = crate::soft_limit::normalize_usage_windows(usage);
    match crate::store::usage_samples::previous_for(&state.pool, provider_id, &windows, Utc::now())
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(provider_id = %provider_id, error = %e, "reading usage samples failed");
            Vec::new()
        }
    }
}

/// Assemble the view; `previous` carries at most one earlier sample per
/// window key (see [`previous_samples`]).
fn build_account_usage(
    account_id: Uuid,
    provider: String,
    usage: Option<serde_json::Value>,
    age_secs: u64,
    previous: &[crate::store::usage_samples::PreviousSample],
) -> AccountUsage {
    let windows = usage
        .as_ref()
        .map(crate::soft_limit::normalize_usage_windows)
        .unwrap_or_default()
        .into_iter()
        .map(|w| {
            let prev = previous
                .iter()
                .find(|p| p.window_key == w.key)
                .map(crate::store::usage_samples::PreviousSample::sample);
            usage_window_view(w, prev)
        })
        .collect();
    let limit_reset =
        usage.as_ref().and_then(|u| crate::routes::limit_reset::limit_reset_status(&provider, u));
    let limit_resets = usage
        .as_ref()
        .map(|u| crate::routes::limit_reset::limit_resets(&provider, u))
        .unwrap_or_default();
    AccountUsage {
        account_id,
        provider,
        usage,
        windows,
        age_secs,
        limit_reset,
        limit_resets,
        provider_status: None,
    }
}

/// Attach the upstream incident reading for this credential's family, read
/// from the poller's cache — no network on the request path.
fn with_provider_status(mut row: AccountUsage, state: &AppState) -> AccountUsage {
    row.provider_status = state.provider_status.degraded_for_provider(&row.provider);
    row
}

/// Cache a fresh usage fetch and push it to every client.
///
/// Only ever reached after an upstream fetch that already happened, so the push
/// adds no upstream calls: a caller served from a fresh cache returns before
/// this and broadcasts nothing, which is what bounds pushes to one per account
/// per [`USAGE_CACHE_TTL`] and keeps the event from becoming a refresh storm.
/// An empty `provider` means the account no longer resolves: still cached, but
/// not published, since there is no row shape to patch a client cache with.
pub async fn store_and_broadcast_usage(
    state: &AppState,
    id: Uuid,
    provider: String,
    usage: Option<serde_json::Value>,
) -> AccountUsage {
    cctui_server_core::gateway::usage::cache_usage(&state.account_usage_cache, id, usage.clone());
    render_and_broadcast_usage(state, id, provider, usage).await
}

/// Render and publish a reading already stored in the shared quota cache.
pub async fn render_and_broadcast_usage(
    state: &AppState,
    id: Uuid,
    provider: String,
    usage: Option<serde_json::Value>,
) -> AccountUsage {
    let previous = previous_samples(state, id, usage.as_ref()).await;
    let row = with_provider_status(build_account_usage(id, provider, usage, 0, &previous), state);
    if row.provider.is_empty() {
        return row;
    }
    if let Ok(usage) = serde_json::to_value(&row) {
        state
            .bus
            .publish_server(cctui_proto::ws::ServerEvent::AccountUsage { account_id: id, usage });
    }
    row
}

/// `GET /api/v1/accounts/{id}/usage` — current subscription usage for a
/// provider credential. `{id}` is the provider-row id (the pre-
/// account id — migrated rows share the uuid, so old callers keep working).
/// Free + tokenless: for anthropic providers this hits Anthropic's OAuth usage
/// endpoint (5h/7d window utilization), served from a slow-refresh per-provider
/// cache so we never spam the rate-limited upstream. OpenAI/codex providers
/// have no such API, so the 5h/7d windows are metered locally from recorded
/// token usage — same shape, same cache, same UI chip.
/// Ownership: a user may only read their own providers; admin may read any.
pub async fn account_usage(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountUsage>, AppError> {
    // Authorize + resolve provider in one go. Admin (`ctx.user_id` = NULL) may
    // read any provider; a user only its own.
    let provider: Option<String> = sqlx::query_scalar(
        "SELECT provider FROM account_providers \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await?;
    let Some(provider) = provider else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    };

    // Serve a fresh-enough cached value without touching upstream.
    if let Some(hit) = state.account_usage_cache.get(&id)
        && hit.fetched_at.elapsed() < USAGE_CACHE_TTL.to_std().unwrap_or_default()
    {
        let age_secs = hit.fetched_at.elapsed().as_secs();
        let usage = hit.usage.clone();
        drop(hit);
        let previous = previous_samples(&state, id, usage.as_ref()).await;
        return Ok(Json(with_provider_status(
            build_account_usage(id, provider, usage, age_secs, &previous),
            &state,
        )));
    }

    // Stale or absent → fetch upstream (anthropic only; Codex returns None).
    let fetched = gateway::fetch_account_usage(&state, id).await;
    let usage = if let Ok(u) = fetched {
        gateway::record_usage_samples(&state, id, u.as_ref());
        u
    } else {
        // Upstream hiccup (e.g. 429/refresh fail): fall back to the last
        // cached value if we have one rather than erroring the whole row.
        let cached = state
            .account_usage_cache
            .get(&id)
            .map(|hit| (hit.usage.clone(), hit.fetched_at.elapsed().as_secs()));
        if let Some((usage, age_secs)) = cached {
            let previous = previous_samples(&state, id, usage.as_ref()).await;
            return Ok(Json(with_provider_status(
                build_account_usage(id, provider, usage, age_secs, &previous),
                &state,
            )));
        }
        // No prior value — surface as "no usage" so the UI just hides the chip.
        None
    };
    Ok(Json(store_and_broadcast_usage(&state, id, provider, usage).await))
}

#[derive(sqlx::FromRow)]
struct UsageProviderRow {
    id: Uuid,
    provider: String,
    account_id: Uuid,
    account_name: String,
    account_emoji: Option<String>,
    header_pin: bool,
}

/// `GET /api/v1/accounts/usage` — usage windows of every provider credential
/// the caller owns (admin: all), in one round trip. Each row is served by the
/// same per-provider cache as the single-provider route, refreshed upstream at
/// most once per [`USAGE_CACHE_TTL`]; a provider whose fetch fails and has no
/// cached value reports empty windows rather than failing the whole list.
pub async fn all_accounts_usage(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountUsageEntry>>, AppError> {
    let rows: Vec<UsageProviderRow> = sqlx::query_as(
        "SELECT p.id, p.provider, p.account_id, p.header_pin, a.name AS account_name, a.emoji AS account_emoji          FROM account_providers p JOIN accounts a ON a.id = p.account_id          WHERE ($1::uuid IS NULL OR p.user_id = $1)            AND (p.provider IN ('anthropic', 'openai', 'fireworks', 'google') OR p.usage_probe IS NOT NULL)          ORDER BY a.name, p.family",
    )
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await?;

    let fetches = rows.iter().map(|r| gateway::usage_for_soft_limit(&state, r.id));
    let usages = futures_util::future::join_all(fetches).await;
    let previous = futures_util::future::join_all(
        rows.iter().zip(&usages).map(|(r, usage)| previous_samples(&state, r.id, usage.as_ref())),
    )
    .await;
    let out = rows
        .into_iter()
        .zip(usages)
        .zip(previous)
        .map(|((r, usage), previous)| {
            let age_secs = state
                .account_usage_cache
                .get(&r.id)
                .map_or(0, |hit| hit.fetched_at.elapsed().as_secs());
            AccountUsageEntry {
                usage: with_provider_status(
                    build_account_usage(r.id, r.provider, usage, age_secs, &previous),
                    &state,
                ),
                account: r.account_id,
                account_name: r.account_name,
                account_emoji: r.account_emoji,
                header_pin: r.header_pin,
            }
        })
        .collect();
    Ok(Json(out))
}

// ----------------------------------------------------------------------------
// Account sharing management
//
// `account_shares` is the sharing seam: a live grant row lets
// a NON-owner resolve/use an account on the gateway + dispatch path, without
// transferring ownership. Grants key on the account IDENTITY: sharing
// an account shares all its provider credentials. Owner-scoped: only the
// account's owner (or an admin) may manage its shares — a grant confers `use`,
// never share management.
// ----------------------------------------------------------------------------

/// `GET /api/v1/accounts/{id}/shares` — who the account is shared with (owner-
/// scoped). Lists only live grants (`revoked_at IS NULL`).
pub async fn list_shares(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ShareInfo>>, AppError> {
    require_account_owner(&state, &ctx, id).await?;
    let rows: Vec<ShareInfo> = sqlx::query_as(
        "SELECT s.resource_id AS account_id, s.grantee_id AS user_id, u.name AS user_name, \
                s.action, s.granted_at \
         FROM resource_shares s JOIN users u ON u.id = s.grantee_id \
         WHERE s.resource_type = 'account' AND s.resource_id = $1 AND s.revoked_at IS NULL \
         ORDER BY u.name",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// `POST /api/v1/accounts/{id}/shares` — grant `use` to another user (owner-
/// scoped). `user` is a UUID or login. Idempotent: re-granting a
/// revoked share un-revokes it in place rather than 409ing on the primary key.
pub async fn grant_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<GrantShare>,
) -> Result<(StatusCode, Json<ShareInfo>), AppError> {
    require_account_owner(&state, &ctx, id).await?;

    // Only `use` today (schema default); reject anything else so a typo doesn't
    // silently store a dead action that no code path honours.
    let action = req.action.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("use");
    if action != "use" {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "action must be 'use'"));
    }

    // Resolve the grantee by UUID or login (`users.name`), active users only.
    let ident = req.user.trim();
    if ident.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "user required"));
    }
    let target: Option<Uuid> = Uuid::parse_str(ident)
        .map_or_else(
            |_| {
                sqlx::query_scalar("SELECT id FROM users WHERE name = $1 AND revoked_at IS NULL")
                    .bind(ident)
            },
            |uuid| {
                sqlx::query_scalar("SELECT id FROM users WHERE id = $1 AND revoked_at IS NULL")
                    .bind(uuid)
            },
        )
        .fetch_optional(&state.pool)
        .await?;
    let Some(target) = target else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such user"));
    };

    sqlx::query(
        "INSERT INTO resource_shares (resource_type, resource_id, grantee_id, action) \
         VALUES ('account', $1, $2, $3) \
         ON CONFLICT (resource_type, resource_id, grantee_id, action) \
         DO UPDATE SET revoked_at = NULL, granted_at = now()",
    )
    .bind(id)
    .bind(target)
    .bind(action)
    .execute(&state.pool)
    .await?;

    let info: ShareInfo = sqlx::query_as(
        "SELECT s.resource_id AS account_id, s.grantee_id AS user_id, u.name AS user_name, \
                s.action, s.granted_at \
         FROM resource_shares s JOIN users u ON u.id = s.grantee_id \
         WHERE s.resource_type = 'account' AND s.resource_id = $1 \
           AND s.grantee_id = $2 AND s.action = $3",
    )
    .bind(id)
    .bind(target)
    .bind(action)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(info)))
}

/// `DELETE /api/v1/accounts/{id}/shares/{user_id}` — revoke a grant (owner-
/// scoped) by setting `revoked_at`. Idempotent-ish: 404 if there was no live
/// share to revoke.
pub async fn revoke_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_account_owner(&state, &ctx, id).await?;
    let res = sqlx::query(
        "UPDATE resource_shares SET revoked_at = now() \
         WHERE resource_type = 'account' AND resource_id = $1 \
           AND grantee_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such share"));
    }
    revoke_grantee_session_tokens(&state.pool, id, user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub use cctui_server_core::gateway::mint::revoke_grantee_session_tokens;

#[cfg(test)]
async fn prepare_provider_write(spec: &ProviderSpec) -> Result<ProviderWrite, AppError> {
    cctui_server_core::accounts::prepare_provider_write(spec, &crate::crypto::vault_key()).await
}

pub async fn settings_catalog(
    axum::extract::Query(q): axum::extract::Query<SettingsCatalogQuery>,
) -> Json<cctui_proto::api::accounts::SettingsCatalogResponse> {
    cctui_server_core::accounts::settings_catalog(&q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use chrono::{DateTime, Duration};
    use dashmap::DashMap;
    use std::sync::Arc;

    #[test]
    fn provider_token_totals_stay_correlated_to_one_provider() {
        assert!(
            PROVIDER_SELECT.contains("WHERE st.account_id = p.id ) t ON true"),
            "the token-total subquery must stay correlated to p.id"
        );
        assert!(
            !PROVIDER_SELECT.contains("GROUP BY st.account_id"),
            "a grouped subquery aggregates every provider's rows on a single-id lookup"
        );
    }

    #[test]
    fn provider_usage_reads_running_totals_not_every_turn() {
        assert!(!PROVIDER_SELECT.contains("session_token_usage"));
        assert_eq!(PROVIDER_SELECT.matches("JOIN session_usage_totals stu").count(), 2);
    }

    #[test]
    fn provider_settings_drop_the_inert_thinking_display() {
        let v = serde_json::json!({ "thinking_display": "summarized", "other": 1 });
        assert_eq!(validate_provider_settings(&v).unwrap(), serde_json::json!({ "other": 1 }));
        assert!(validate_provider_settings(&serde_json::json!("x")).is_err());
    }

    fn soft_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-19T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn hot_usage() -> serde_json::Value {
        serde_json::json!({
            "five_hour": { "utilization": 90.0, "resets_at": "2026-06-19T16:00:00Z" },
            "seven_day": { "utilization": 10.0, "resets_at": "2026-06-26T00:00:00Z" },
        })
    }

    fn update_provider(json: serde_json::Value) -> UpdateProvider {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_usage_probe_must_name_a_registered_probe_on_a_compatible_credential() {
        assert_eq!(validate_usage_probe(None, true).unwrap(), None);
        assert_eq!(validate_usage_probe(Some(""), true).unwrap(), None);
        assert_eq!(validate_usage_probe(Some("  "), true).unwrap(), None);
        assert_eq!(
            validate_usage_probe(Some("openrouter"), true).unwrap(),
            Some("openrouter".to_owned())
        );
        assert!(validate_usage_probe(Some("zai"), true).is_err());
        // A native subscription has a real usage API; a probe there would shadow it.
        assert!(validate_usage_probe(Some("openrouter"), false).is_err());
        // Clearing it is fine on any provider — there is nothing to shadow.
        assert_eq!(validate_usage_probe(Some(""), false).unwrap(), None);
    }

    #[test]
    fn a_declared_model_list_is_not_an_endpoint_field() {
        let models = update_provider(serde_json::json!({
            "models": [{"model": "claude-opus-5", "label": "Opus 5"}]
        }));
        assert!(!endpoint_fields_present(&models));
        assert!(!endpoint_fields_present(&update_provider(serde_json::json!({"models": []}))));
        assert!(endpoint_fields_present(&update_provider(
            serde_json::json!({"base_url": "https://x"})
        )));
        assert!(endpoint_fields_present(&update_provider(
            serde_json::json!({"auth_scheme": "bearer"})
        )));
        assert!(endpoint_fields_present(&update_provider(
            serde_json::json!({"access_token": "sk-x"})
        )));
    }

    #[test]
    fn usage_window_view_flattens_and_carries_pace() {
        let resets = (Utc::now() + Duration::hours(2)).to_rfc3339();
        let usage = serde_json::json!({
            "five_hour": { "utilization": 60.0, "resets_at": resets },
        });
        let built = build_account_usage(Uuid::nil(), "anthropic".into(), Some(usage), 7, &[]);
        let json = serde_json::to_value(&built).unwrap();
        let w = &json["windows"][0];
        assert_eq!(w["key"], "session");
        assert_eq!(w["utilization"], 60.0);
        let pace = &w["pace"];
        assert!(pace["ratio"].as_f64().unwrap() > 0.0);
        assert!(pace["expected_pct"].as_f64().unwrap() > 50.0);
        assert!(pace["projected_wall_at"].is_string());
        assert_eq!(json["age_secs"], 7);
    }

    /// The push has to be interchangeable with a poll response: the client
    /// patches its query cache with it instead of refetching, so any field the
    /// route carries and the event drops would silently blank in the UI.
    #[test]
    fn a_usage_push_carries_the_same_row_shape_as_the_route() {
        let resets = (Utc::now() + Duration::hours(2)).to_rfc3339();
        let usage = serde_json::json!({
            "five_hour": { "utilization": 60.0, "resets_at": resets },
        });
        let row = build_account_usage(Uuid::nil(), "anthropic".into(), Some(usage), 0, &[]);
        let payload = serde_json::to_value(&row).expect("row serializes");
        let event =
            cctui_proto::ws::ServerEvent::AccountUsage { account_id: Uuid::nil(), usage: payload };
        let json = serde_json::to_value(&event).expect("event serializes");
        assert_eq!(json["type"], "account_usage");
        assert_eq!(json["account_id"], serde_json::json!(Uuid::nil()));
        for field in ["account_id", "provider", "usage", "windows", "age_secs"] {
            assert!(json["usage"].get(field).is_some(), "push must carry {field}");
        }
        assert_eq!(json["usage"]["windows"][0]["utilization"], 60.0);
        assert_eq!(json["usage"]["age_secs"], 0, "a push is by definition just-fetched");
    }

    #[test]
    fn a_pace_cap_persists_on_percent_windows_and_is_range_checked() {
        let built = build_soft_limits_json(
            Some(&serde_json::json!({
                "session": {"pace_cap": 1.5},
                "usd_5h": {"cap_usd": 2.0, "pace_cap": 1.5},
            })),
            None,
            None,
            None,
            None,
        )
        .expect("valid")
        .expect("non-empty");
        assert_eq!(built["session"]["pace_cap"], 1.5);
        assert!(
            built["usd_5h"].get("pace_cap").is_none(),
            "a dollar window reports no utilization, so a pace cap there is unenforceable"
        );
        for bad in [0.5, 0.0, 1000.0] {
            assert!(
                build_soft_limits_json(
                    Some(&serde_json::json!({ "session": {"pace_cap": bad} })),
                    None,
                    None,
                    None,
                    None,
                )
                .is_err(),
                "pace_cap {bad} must be rejected"
            );
        }
    }

    #[test]
    fn raising_cap_over_usage_clears_every_blocked_session() {
        let caps = crate::soft_limit::SoftLimits::from_json(Some(&serde_json::json!({
            "session": {"cap_pct": 95}
        })));
        // The candidates come from `soft_limit_reason IS NOT NULL`, so a block
        // this process never saw (other replica, or set before a rollout) clears.
        let candidates = vec![
            ("s-other-replica".to_owned(), Some("session".to_owned())),
            ("s-pre-rollout".to_owned(), None),
        ];
        let windows = crate::soft_limit::normalize_usage_windows(&hot_usage());
        let cleared = soft_limit_blocks_to_clear(&candidates, &windows, &caps, soft_now());
        assert_eq!(cleared, ["s-other-replica", "s-pre-rollout"]);
    }

    #[test]
    fn still_over_new_cap_clears_nothing() {
        let caps = crate::soft_limit::SoftLimits::from_json(Some(&serde_json::json!({
            "session": {"cap_pct": 85}
        })));
        let windows = crate::soft_limit::normalize_usage_windows(&hot_usage());
        let cleared = soft_limit_blocks_to_clear(
            &[("s-blocked".to_owned(), Some("session".to_owned()))],
            &windows,
            &caps,
            soft_now(),
        );
        assert!(cleared.is_empty());
    }

    /// The account cap only lifts the blocks it owns, in both directions.
    #[test]
    fn raising_an_account_cap_spares_a_session_over_its_own_budget() {
        let caps = crate::soft_limit::SoftLimits::from_json(Some(&serde_json::json!({
            "session": {"cap_pct": 95}
        })));
        let mut windows = crate::soft_limit::normalize_usage_windows(&hot_usage());
        windows.push(crate::soft_limit::usd_window(crate::soft_limit::KEY_SESSION_USD, 5.0, None));
        let over_budget = format!(
            "{}{}",
            crate::soft_limit::SESSION_SCOPE_PREFIX,
            crate::soft_limit::KEY_SESSION_USD
        );
        let candidates = vec![
            ("s-own-budget".to_owned(), Some(over_budget)),
            ("s-account-cap".to_owned(), Some("session".to_owned())),
        ];
        let cleared = soft_limit_blocks_to_clear(&candidates, &windows, &caps, soft_now());
        assert_eq!(
            cleared,
            ["s-account-cap"],
            "the account cap clears its own block and never the session budget's"
        );
    }

    /// DB-gated: the re-evaluation picks its candidates from the session rows,
    /// so raising a cap lifts a block set by the other replica (or before a
    /// rollout) that this process has no memory of.
    #[tokio::test]
    async fn blocked_candidates_come_from_the_session_rows() {
        let Some(url) =
            crate::routes::gateway::test_db_url("blocked_candidates_come_from_the_session_rows")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let acct = Uuid::new_v4();
        let prov = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("reeval-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(acct)
            .bind(uid)
            .bind(format!("reeval-acct-{uid}"))
            .execute(&pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'anthropic', 'x', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(&pool)
        .await
        .expect("seed provider");

        let blocked = format!("reeval-blocked-{uid}");
        let running = format!("reeval-running-{uid}");
        for (id, reason) in
            [(&blocked, Some("switch account: personal rate-limited")), (&running, None)]
        {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, status, soft_limit_reason) \
                 VALUES ($1, 'm1', '/w', $2, 'active', $3)",
            )
            .bind(id)
            .bind(uid)
            .bind(reason)
            .execute(&pool)
            .await
            .expect("seed session");
            sqlx::query(
                "INSERT INTO session_tokens (token_hash, session_id, account_id) \
                 VALUES ($1, $2, $3)",
            )
            .bind(format!("th-{id}"))
            .bind(id)
            .bind(prov)
            .execute(&pool)
            .await
            .expect("seed token");
        }

        let candidates = soft_limit_blocked_sessions(&pool, prov).await.expect("candidates");
        assert_eq!(candidates, vec![(blocked.clone(), None)]);

        sqlx::query("DELETE FROM session_tokens WHERE account_id = $1")
            .bind(prov)
            .execute(&pool)
            .await
            .expect("cleanup tokens");
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .expect("cleanup sessions");
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .expect("cleanup");
    }

    #[test]
    fn accepts_a_single_emoji_grapheme() {
        for one in [
            "\u{1F419}",
            "\u{2764}\u{FE0F}",
            "\u{1F44D}\u{1F3FD}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{1F1EB}\u{1F1F7}",
            "\u{2600}",
        ] {
            assert_eq!(normalize_emoji(one), Ok(Some(one.to_string())), "{one} should be accepted");
        }
        assert_eq!(normalize_emoji("  \u{1F419}  "), Ok(Some("\u{1F419}".to_string())));
    }

    #[test]
    fn blank_emoji_clears_the_glyph() {
        assert_eq!(normalize_emoji(""), Ok(None));
        assert_eq!(normalize_emoji("   "), Ok(None));
    }

    #[test]
    fn rejects_text_and_multiple_glyphs() {
        for bad in [
            "\u{1F419}\u{1F419}",
            "hi",
            "a",
            "\u{1F419} \u{1F419}",
            "\u{1F1EB}\u{1F1F7}\u{1F1EB}\u{1F1F7}",
            "\u{1F419}\u{200D}",
            "\u{1F419}x",
            "\u{1F419}\u{1F419}\u{1F419}\u{1F419}\u{1F419}\u{1F419}\u{1F419}\u{1F419}\u{1F419}",
        ] {
            assert!(normalize_emoji(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    /// The PATCH's own UPDATE against a migrated database: a provider is pinned
    /// to the header by default, `header_pin: false` unpins it, and an absent
    /// field leaves the column alone.
    #[tokio::test]
    async fn header_pin_round_trips_through_the_provider_patch() {
        let Some(url) = crate::routes::gateway::test_db_url("header_pin_round_trips") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let suffix = Uuid::new_v4();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("cct922-{suffix}"))
        .fetch_one(&pool)
        .await
        .expect("insert user");
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(user)
                .bind(format!("cct922-{suffix}"))
                .fetch_one(&pool)
                .await
                .expect("insert account");
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, 'anthropic') RETURNING id",
        )
        .bind(user)
        .bind(account)
        .fetch_one(&pool)
        .await
        .expect("insert provider");

        let patch = |header_pin: Option<bool>| {
            let pool = pool.clone();
            async move {
                let updated: Option<Uuid> = sqlx::query_scalar(UPDATE_PROVIDER_SQL)
                    .bind(provider)
                    .bind(account)
                    .bind(Some(user))
                    .bind(None::<String>)
                    .bind(None::<String>)
                    .bind(None::<serde_json::Value>)
                    .bind(None::<String>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(false)
                    .bind(None::<serde_json::Value>)
                    .bind(header_pin)
                    .bind(false)
                    .bind(None::<String>)
                    .fetch_optional(&pool)
                    .await
                    .expect("patch provider");
                assert_eq!(updated, Some(provider));
                fetch_provider_info(&pool, provider)
                    .await
                    .expect("fetch provider")
                    .expect("provider exists")
                    .header_pin
            }
        };

        assert!(patch(None).await, "a new credential is pinned to the header");
        assert!(!patch(Some(false)).await, "header_pin: false unpins it");
        assert!(!patch(None).await, "an absent header_pin leaves the column unchanged");
        assert!(patch(Some(true)).await, "header_pin: true pins it again");

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user)
            .execute(&pool)
            .await
            .expect("cleanup");
    }

    #[test]
    fn missing_or_blank_account_name_is_rejected() {
        assert!(account_name_missing(None));
        assert!(account_name_missing(Some("")));
        assert!(account_name_missing(Some("   \t")));
        assert!(!account_name_missing(Some(" work ")));
    }

    #[test]
    fn splits_code_and_state() {
        let (code, state) = split_code_state("abc123#xyz789");
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("xyz789"));
    }

    #[test]
    fn splits_code_without_state() {
        let (code, state) = split_code_state("  abc123  ");
        assert_eq!(code, "abc123");
        assert_eq!(state, None);
    }

    #[test]
    fn pkce_challenge_is_url_safe_b64_of_sha256() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(pkce_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn sweep_drops_only_expired() {
        let store: PendingOAuthLogins = Arc::new(DashMap::new());
        store.insert(
            "fresh".into(),
            PendingOAuthLogin {
                user_id: Uuid::new_v4(),
                provider: "anthropic".into(),
                code_verifier: "v".into(),
                created_at: Utc::now(),
                account_id: None,
            },
        );
        store.insert(
            "stale".into(),
            PendingOAuthLogin {
                user_id: Uuid::new_v4(),
                provider: "anthropic".into(),
                code_verifier: "v".into(),
                created_at: Utc::now() - Duration::minutes(20),
                account_id: None,
            },
        );
        sweep_expired(&store);
        assert!(store.contains_key("fresh"));
        assert!(!store.contains_key("stale"));
    }

    #[test]
    fn extracts_code_from_callback_url() {
        assert_eq!(
            code_from_callback("http://localhost:1455/auth/callback?code=ABC123&state=xyz")
                .as_deref(),
            Some("ABC123")
        );
        // code may be url-encoded and carry a #state suffix.
        assert_eq!(
            code_from_callback("http://localhost:1455/auth/callback?code=a%2Bb%3Dc#st").as_deref(),
            Some("a+b=c")
        );
        // bare code (no URL).
        assert_eq!(code_from_callback("  rawcode#state ").as_deref(), Some("rawcode"));
        assert_eq!(code_from_callback(""), None);
        // query present but no code param.
        assert_eq!(code_from_callback("http://localhost:1455/auth/callback?state=x"), None);
    }

    #[test]
    fn parses_chatgpt_account_id_from_id_token() {
        // header.payload.signature — only the payload (claims) is read.
        let payload = serde_json::json!({
            "sub": "user-1",
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-abc-123" }
        });
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("header.{payload_b64}.sig");
        assert_eq!(chatgpt_account_id_from_id_token(&jwt).as_deref(), Some("acct-abc-123"));
        // missing claim → None.
        let empty = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        assert_eq!(chatgpt_account_id_from_id_token(&format!("h.{empty}.s")), None);
        assert_eq!(chatgpt_account_id_from_id_token("not-a-jwt"), None);
    }

    #[test]
    fn encodes_query_components() {
        assert_eq!(
            urlencoding("org:create_api_key user:profile"),
            "org%3Acreate_api_key%20user%3Aprofile"
        );
        assert_eq!(
            urlencoding("https://console.anthropic.com/oauth/code/callback"),
            "https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
        );
    }

    #[test]
    fn create_account_body_flattens_provider_spec() {
        // Legacy one-shot shape still deserializes: provider fields flat.
        let body: CreateAccount =
            serde_json::from_str(r#"{"name":"work","provider":"anthropic","refresh_token":"rt"}"#)
                .unwrap();
        assert_eq!(body.provider.provider.as_deref(), Some("anthropic"));
        assert_eq!(body.provider.refresh_token.as_deref(), Some("rt"));
        // Identity-only create: no provider block.
        let body: CreateAccount = serde_json::from_str(r#"{"name":"work"}"#).unwrap();
        assert!(body.provider.provider.is_none());
    }

    #[test]
    fn free_form_env_json_denylist_and_acceptance() {
        // Free-form names accepted.
        let mut ok = std::collections::HashMap::new();
        ok.insert("MY_TOKEN".to_string(), "secret".to_string());
        ok.insert("HTTP_PROXY".to_string(), "http://p".to_string());
        assert!(validate_env_json(&ok).is_ok());

        // Denylisted gateway/session vars rejected with a clear per-name reason.
        for denied in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_BG_CLAIM_AUTH"] {
            let mut bad = std::collections::HashMap::new();
            bad.insert(denied.to_string(), "x".to_string());
            let e = validate_env_json(&bad).expect_err("denylisted name must be rejected");
            assert!(matches!(e, AppError::Status(StatusCode::BAD_REQUEST, _)));
        }

        // Malformed name rejected.
        let mut malformed = std::collections::HashMap::new();
        malformed.insert("1BAD".to_string(), "x".to_string());
        assert!(validate_env_json(&malformed).is_err());
    }

    #[test]
    fn env_names_round_trip_names_only_sorted() {
        // A fixed key exercises the encrypt→decrypt-names path without a vault env.
        let key = b"test-key-32-bytes-test-key-32byt".to_vec();
        let mut map = std::collections::BTreeMap::new();
        map.insert("ZED".to_string(), "z".to_string());
        map.insert("ALPHA".to_string(), "a".to_string());
        let enc = crate::crypto::encrypt(&serde_json::to_string(&map).unwrap(), &key);

        let names = env_names_from_enc(Some(&enc), &key);
        assert_eq!(names, vec!["ALPHA".to_string(), "ZED".to_string()], "sorted names only");
        // No blob ⇒ empty; a garbage blob degrades to empty, never panics.
        assert!(env_names_from_enc(None, &key).is_empty());
        assert!(env_names_from_enc(Some("not-hex-zz"), &key).is_empty());
    }

    #[test]
    fn env_remove_drops_names_and_reencrypts() {
        let key = b"test-key-32-bytes-test-key-32byt".to_vec();
        let mut map = std::collections::BTreeMap::new();
        map.insert("KEEP".to_string(), "k".to_string());
        map.insert("DROP".to_string(), "d".to_string());
        let enc = crate::crypto::encrypt(&serde_json::to_string(&map).unwrap(), &key);

        let mut decrypted = env_map_from_enc(Some(&enc), &key);
        decrypted.retain(|name, _| name != "DROP");
        assert_eq!(decrypted.get("KEEP").map(String::as_str), Some("k"), "values survive");
        let reenc = crate::crypto::encrypt(&serde_json::to_string(&decrypted).unwrap(), &key);
        assert_eq!(env_names_from_enc(Some(&reenc), &key), vec!["KEEP".to_string()]);

        decrypted.retain(|name, _| name != "KEEP");
        assert!(decrypted.is_empty(), "removing every name clears the blob");
    }

    #[tokio::test]
    async fn provider_write_validates_provider() {
        let spec = ProviderSpec { provider: Some("bogus".into()), ..Default::default() };
        assert!(prepare_provider_write(&spec).await.is_err());
        let spec = ProviderSpec::default();
        assert!(prepare_provider_write(&spec).await.is_err());
        // native without refresh token → 400.
        let spec = ProviderSpec { provider: Some("anthropic".into()), ..Default::default() };
        assert!(prepare_provider_write(&spec).await.is_err());
    }

    #[tokio::test]
    async fn provider_write_rejects_an_internal_base_url() {
        for base in [
            "http://127.0.0.1:9000",
            "https://10.0.0.5",
            "https://169.254.169.254",
            "https://minio.storage.svc:9000",
            "https://localhost",
        ] {
            let spec = ProviderSpec {
                provider: Some("openai-compatible".into()),
                base_url: Some(base.into()),
                ..Default::default()
            };
            assert!(prepare_provider_write(&spec).await.is_err(), "{base}");
        }
        if std::env::var_os("CCTUI_VAULT_KEY").is_some() {
            let spec = ProviderSpec {
                provider: Some("openai-compatible".into()),
                base_url: Some("https://1.1.1.1/v1".into()),
                ..Default::default()
            };
            assert!(prepare_provider_write(&spec).await.is_ok());
        }
        assert!(check_base_url("https://192.168.1.10").await.is_err());
    }

    #[test]
    fn rate_limits_json_validates_and_clears() {
        let out =
            build_rate_limits_json(Some(&serde_json::json!({ "rpm": 30, "tpm": 90_000 }))).unwrap();
        assert_eq!(out, Some(serde_json::json!({ "rpm": 30, "tpm": 90_000 })));

        // Absent, null, empty object, and zeros all clear the column.
        assert_eq!(build_rate_limits_json(None).unwrap(), None);
        assert_eq!(build_rate_limits_json(Some(&serde_json::Value::Null)).unwrap(), None);
        assert_eq!(build_rate_limits_json(Some(&serde_json::json!({}))).unwrap(), None);
        assert_eq!(
            build_rate_limits_json(Some(&serde_json::json!({ "rpm": 0, "tpm": 0 }))).unwrap(),
            None
        );

        assert_eq!(
            build_rate_limits_json(Some(&serde_json::json!({ "rpm": 5 }))).unwrap(),
            Some(serde_json::json!({ "rpm": 5 }))
        );

        for bad in [
            serde_json::json!({ "rpm": -1 }),
            serde_json::json!({ "tpm": 1.5 }),
            serde_json::json!([1, 2]),
        ] {
            let e = build_rate_limits_json(Some(&bad)).expect_err("invalid rate_limits must 400");
            assert!(matches!(e, AppError::Status(StatusCode::BAD_REQUEST, _)));
        }
    }
}
