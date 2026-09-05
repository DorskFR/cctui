//! `/api/v1/profiles` — per-user spawn profiles: a named harness / account /
//! model / effort / permission-mode kit the spawn panel applies in one click.
//! Profiles only shape the *next* spawn; deleting one never touches a session.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::accounts::{err, require_human};
use crate::auth::AuthContext;
use crate::state::AppState;

type ApiErr = (StatusCode, Json<serde_json::Value>);

const HARNESSES: &[&str] = &["claude-code", "codex"];
const PERMISSION_MODES: &[&str] = &["ask", "auto", "yolo", "whip"];

#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SessionProfile {
    #[ts(type = "string")]
    pub id: Uuid,
    #[ts(type = "string")]
    pub user_id: Uuid,
    pub name: String,
    pub harness: String,
    #[ts(type = "string | null")]
    pub account_id: Option<Uuid>,
    #[ts(type = "string | null")]
    pub pool_id: Option<Uuid>,
    pub no_account: bool,
    pub model_alias: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
}

/// The knobs a profile carries. The account pick is at most one of
/// `account_id` / `pool_id` / `no_account`; none = Auto (the server elects one).
/// `None` model / effort / permission mode = the harness or account default.
#[derive(Clone, Debug, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ProfileSpec {
    pub harness: String,
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub pool_id: Option<Uuid>,
    #[serde(default)]
    #[ts(type = "boolean", optional)]
    pub no_account: bool,
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub model_alias: Option<String>,
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub effort: Option<String>,
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub permission_mode: Option<String>,
}

#[derive(Debug, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(flatten)]
    pub spec: ProfileSpec,
}

/// Every field optional; the spec, when present, replaces the whole kit (the
/// panel always holds the full one), so a cleared knob really clears.
#[derive(Debug, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub spec: Option<ProfileSpec>,
}

const COLS: &str = "id, user_id, name, harness, account_id, pool_id, no_account, model_alias, \
                    effort, permission_mode, created_at, updated_at";

fn db_err(e: &sqlx::Error) -> ApiErr {
    if let sqlx::Error::Database(dbe) = e
        && dbe.code().as_deref() == Some("23505")
    {
        return err(StatusCode::CONFLICT, "a profile with that name already exists");
    }
    tracing::error!("db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn clean_name(raw: &str) -> Result<String, ApiErr> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "profile name is required"));
    }
    if name.chars().count() > 64 {
        return Err(err(StatusCode::BAD_REQUEST, "profile name is too long (max 64)"));
    }
    Ok(name.to_string())
}

/// Trim the knobs, drop empty ones, and reject values outside the vocabulary
/// the spawn path understands.
fn clean_spec(spec: ProfileSpec) -> Result<ProfileSpec, ApiErr> {
    let harness = spec.harness.trim().to_string();
    if !HARNESSES.contains(&harness.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "unknown harness"));
    }
    let opt = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let permission_mode = opt(spec.permission_mode);
    if let Some(mode) = &permission_mode
        && !PERMISSION_MODES.contains(&mode.as_str())
    {
        return Err(err(StatusCode::BAD_REQUEST, "unknown permission mode"));
    }
    let picks = [spec.account_id.is_some(), spec.pool_id.is_some(), spec.no_account];
    if picks.iter().filter(|p| **p).count() > 1 {
        return Err(err(StatusCode::BAD_REQUEST, "pick one of account, pool or no account"));
    }
    Ok(ProfileSpec {
        harness,
        account_id: spec.account_id,
        pool_id: spec.pool_id,
        no_account: spec.no_account,
        model_alias: opt(spec.model_alias),
        effort: opt(spec.effort),
        permission_mode,
    })
}

/// The account must be one the caller can launch on: owned, or shared to them.
async fn account_usable(
    pool: &PgPool,
    user_id: Option<Uuid>,
    account_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM accounts a \
          WHERE a.id = $2 \
            AND ($1::uuid IS NULL OR a.user_id = $1 OR EXISTS ( \
                SELECT 1 FROM resource_shares s \
                 WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                   AND s.grantee_id = $1 AND s.revoked_at IS NULL)))",
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_one(pool)
    .await
}

async fn pool_owned(
    pool: &PgPool,
    user_id: Option<Uuid>,
    pool_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_pools \
          WHERE id = $2 AND ($1::uuid IS NULL OR user_id = $1))",
    )
    .bind(user_id)
    .bind(pool_id)
    .fetch_one(pool)
    .await
}

/// `user_id` is the caller's owner filter: `None` (admin) matches the account
/// list's own scoping, so the picker cannot offer what this then rejects.
async fn check_account(
    pool: &PgPool,
    user_id: Option<Uuid>,
    spec: &ProfileSpec,
) -> Result<(), ApiErr> {
    if let Some(account) = spec.account_id
        && !account_usable(pool, user_id, account).await.map_err(|e| db_err(&e))?
    {
        return Err(err(StatusCode::BAD_REQUEST, "unknown account"));
    }
    if let Some(id) = spec.pool_id
        && !pool_owned(pool, user_id, id).await.map_err(|e| db_err(&e))?
    {
        return Err(err(StatusCode::BAD_REQUEST, "unknown pool"));
    }
    Ok(())
}

pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<SessionProfile>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM session_profiles WHERE user_id = $1 ORDER BY created_at, lower(name)"
    )))
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn insert(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    spec: &ProfileSpec,
) -> Result<SessionProfile, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO session_profiles \
            (user_id, name, harness, account_id, pool_id, no_account, model_alias, effort, \
             permission_mode) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLS}"
    )))
    .bind(user_id)
    .bind(name)
    .bind(&spec.harness)
    .bind(spec.account_id)
    .bind(spec.pool_id)
    .bind(spec.no_account)
    .bind(spec.model_alias.as_deref())
    .bind(spec.effort.as_deref())
    .bind(spec.permission_mode.as_deref())
    .fetch_one(pool)
    .await
}

pub async fn update(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    name: Option<&str>,
    spec: Option<&ProfileSpec>,
) -> Result<Option<SessionProfile>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE session_profiles SET \
            name = COALESCE($3, name), \
            harness = CASE WHEN $4 THEN $5 ELSE harness END, \
            account_id = CASE WHEN $4 THEN $6 ELSE account_id END, \
            pool_id = CASE WHEN $4 THEN $7 ELSE pool_id END, \
            no_account = CASE WHEN $4 THEN $8 ELSE no_account END, \
            model_alias = CASE WHEN $4 THEN $9 ELSE model_alias END, \
            effort = CASE WHEN $4 THEN $10 ELSE effort END, \
            permission_mode = CASE WHEN $4 THEN $11 ELSE permission_mode END, \
            updated_at = now() \
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(spec.is_some())
    .bind(spec.map(|s| s.harness.as_str()))
    .bind(spec.and_then(|s| s.account_id))
    .bind(spec.and_then(|s| s.pool_id))
    .bind(spec.is_some_and(|s| s.no_account))
    .bind(spec.and_then(|s| s.model_alias.as_deref()))
    .bind(spec.and_then(|s| s.effort.as_deref()))
    .bind(spec.and_then(|s| s.permission_mode.as_deref()))
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM session_profiles WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// `GET /profiles` — the caller's profiles, oldest first (stable panel order).
pub async fn list_profiles(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<SessionProfile>>, ApiErr> {
    require_human(&ctx)?;
    let rows = list_for_user(&state.pool, ctx.user_id).await.map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

/// `POST /profiles` — create; 409 when the caller already has that name.
pub async fn create_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<SessionProfile>), ApiErr> {
    require_human(&ctx)?;
    let name = clean_name(&req.name)?;
    let spec = clean_spec(req.spec)?;
    check_account(&state.pool, ctx.owner_filter(), &spec).await?;
    let row = insert(&state.pool, ctx.user_id, &name, &spec).await.map_err(|e| db_err(&e))?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `PATCH /profiles/{id}` — rename and/or replace the kit.
pub async fn update_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<SessionProfile>, ApiErr> {
    require_human(&ctx)?;
    let name = req.name.as_deref().map(clean_name).transpose()?;
    let spec = req.spec.map(clean_spec).transpose()?;
    if let Some(spec) = &spec {
        check_account(&state.pool, ctx.owner_filter(), spec).await?;
    }
    let row = update(&state.pool, ctx.user_id, id, name.as_deref(), spec.as_ref())
        .await
        .map_err(|e| db_err(&e))?;
    row.map(Json).ok_or_else(|| err(StatusCode::NOT_FOUND, "profile not found"))
}

/// `DELETE /profiles/{id}` — the caller's own profile only; sessions spawned
/// from it are untouched.
pub async fn delete_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiErr> {
    require_human(&ctx)?;
    if delete(&state.pool, ctx.user_id, id).await.map_err(|e| db_err(&e))? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "profile not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(harness: &str, mode: Option<&str>) -> ProfileSpec {
        ProfileSpec {
            harness: harness.into(),
            account_id: None,
            pool_id: None,
            no_account: false,
            model_alias: Some(" fable ".into()),
            effort: Some(String::new()),
            permission_mode: mode.map(str::to_string),
        }
    }

    #[test]
    fn clean_spec_trims_and_drops_empty_knobs() {
        let s = clean_spec(spec(" codex ", Some("yolo"))).expect("valid");
        assert_eq!(s.harness, "codex");
        assert_eq!(s.model_alias.as_deref(), Some("fable"));
        assert_eq!(s.effort, None);
        assert_eq!(s.permission_mode.as_deref(), Some("yolo"));
    }

    #[test]
    fn clean_spec_rejects_unknown_vocabulary() {
        assert_eq!(clean_spec(spec("opencode", None)).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(
            clean_spec(spec("claude-code", Some("sudo"))).unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        assert!(clean_spec(spec("claude-code", Some(" "))).unwrap().permission_mode.is_none());
        let both =
            ProfileSpec { pool_id: Some(Uuid::new_v4()), no_account: true, ..spec("codex", None) };
        assert_eq!(clean_spec(both).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn clean_name_requires_something_short() {
        assert_eq!(clean_name("  Orchestrator ").unwrap(), "Orchestrator");
        assert_eq!(clean_name("   ").unwrap_err().0, StatusCode::BAD_REQUEST);
        assert_eq!(clean_name(&"x".repeat(65)).unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn profiles_crud_over_db() {
        let Some(url) = crate::routes::gateway::test_db_url("profiles_crud_over_db") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let suffix = Uuid::new_v4();
        let mk_user = |tag: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO users (id, name, key_hash) \
                     VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
                )
                .bind(format!("profiles-{tag}-{suffix}"))
                .fetch_one(&pool)
                .await
                .expect("insert user")
            }
        };
        let owner = mk_user("owner").await;
        let other = mk_user("other").await;
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(owner)
                .bind(format!("profiles-{suffix}"))
                .fetch_one(&pool)
                .await
                .expect("insert account");

        let pool_id: Uuid = sqlx::query_scalar(
            "INSERT INTO account_pools (user_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(owner)
        .bind(format!("profiles-pool-{suffix}"))
        .fetch_one(&pool)
        .await
        .expect("insert pool");

        let kit = ProfileSpec {
            harness: "claude-code".into(),
            account_id: Some(account),
            pool_id: None,
            no_account: false,
            model_alias: Some("fable".into()),
            effort: Some("medium".into()),
            permission_mode: Some("yolo".into()),
        };
        let created = insert(&pool, owner, "Orchestrator", &kit).await.expect("insert");
        assert_eq!(created.account_id, Some(account));
        assert_eq!(created.permission_mode.as_deref(), Some("yolo"));

        let dup = insert(&pool, owner, "Orchestrator", &kit).await.unwrap_err();
        assert_eq!(db_err(&dup).0, StatusCode::CONFLICT);
        assert!(insert(&pool, other, "Orchestrator", &kit).await.is_ok());

        assert!(account_usable(&pool, Some(owner), account).await.unwrap());
        assert!(!account_usable(&pool, Some(other), account).await.unwrap());
        assert!(pool_owned(&pool, Some(owner), pool_id).await.unwrap());
        assert!(!pool_owned(&pool, Some(other), pool_id).await.unwrap());
        // Admin (no owner filter) reaches any account or pool, matching the
        // scoping the account list itself uses to populate the picker.
        assert!(account_usable(&pool, None, account).await.unwrap());
        assert!(pool_owned(&pool, None, pool_id).await.unwrap());

        let pooled = ProfileSpec { account_id: None, pool_id: Some(pool_id), ..kit.clone() };
        let with_pool = insert(&pool, owner, "Pooled", &pooled).await.expect("insert pooled");
        assert_eq!(with_pool.pool_id, Some(pool_id));
        assert_eq!(with_pool.account_id, None);
        let clash = ProfileSpec { no_account: true, ..pooled.clone() };
        assert!(insert(&pool, owner, "Clash", &clash).await.is_err());

        let mine = list_for_user(&pool, owner).await.expect("list");
        assert_eq!(mine.iter().map(|p| p.id).collect::<Vec<_>>(), vec![created.id, with_pool.id]);

        let renamed = update(&pool, owner, created.id, Some("Deep review"), None)
            .await
            .expect("rename")
            .expect("found");
        assert_eq!(renamed.name, "Deep review");
        assert_eq!(renamed.model_alias.as_deref(), Some("fable"));

        let cleared =
            ProfileSpec { harness: "codex".into(), no_account: true, ..ProfileSpec::default() };
        let replaced = update(&pool, owner, created.id, None, Some(&cleared))
            .await
            .expect("replace")
            .expect("found");
        assert_eq!(replaced.name, "Deep review");
        assert_eq!(replaced.harness, "codex");
        assert_eq!(replaced.account_id, None);
        assert!(replaced.no_account);
        assert_eq!(replaced.effort, None);
        assert!(replaced.updated_at >= created.updated_at);

        assert!(update(&pool, other, created.id, Some("hijack"), None).await.unwrap().is_none());
        assert!(!delete(&pool, other, created.id).await.unwrap());
        assert!(delete(&pool, owner, created.id).await.unwrap());
        assert!(delete(&pool, owner, with_pool.id).await.unwrap());
        assert!(list_for_user(&pool, owner).await.unwrap().is_empty());

        sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
            .bind(owner)
            .bind(other)
            .execute(&pool)
            .await
            .expect("cleanup");
    }
}
