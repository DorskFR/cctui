//! GitHub HTTP handlers.
//!
//! GH-CONN-1 lands real connector CRUD on `/api/v1/github/connectors`: create
//! (encrypts the credential at rest with the vault key, same as the OAuth-account
//! vault), list, and delete. The credential is **never** returned — list/get only
//! surface a masked preview + whether a webhook secret is set. The `pulls`
//! handler stays a `501` stub until a later GH-* story; the webhook ingress
//! (`triggers/github`) is implemented in [`crate::webhook`] (GH-CONN-2).
//!
//! Auth: the nested GitHub router is wrapped (in `cctui-server::main`) with the
//! same auth middleware as the rest of `/api/v1`, plus a thin layer that maps the
//! server's `AuthContext` into a [`CallerIdentity`] extension. A user acts as
//! itself; the admin token has no user identity and must name the owner.
#![allow(clippy::unused_async)]

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::github::{
    CallerIdentity, CheckSummary, ConnectorInfo, CreateConnector, CreateDraftComment,
    CreateReviewDraft, GithubCredentialKind, MarkViewedRequest, PublishReviewRequest,
    PublishReviewResult, PullDiff, PullInboxItem, ReviewDraftInfo, ReviewSummary, ReviewVerdict,
    UpdateDraftComment, UpdateReviewDraft, ViewedMarkInfo,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::attention::{self, Viewer};
use crate::diff::{self, HttpDiffClient};
use crate::drafts::{self, DraftError};
use crate::publish::{self, ReviewSubmitClient};
use crate::store;
use crate::viewed::{self, ViewedError};
use crate::{GithubState, crypto};

/// Query string for `GET .../threads`: `?sync=1` pulls fresh threads from GitHub
/// before reading back; otherwise the synced rows are served as-is.
#[derive(serde::Deserialize)]
pub struct ThreadsQuery {
    #[serde(default)]
    sync: Option<bool>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(code: StatusCode, msg: &str) -> ApiError {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn kind_str(k: GithubCredentialKind) -> &'static str {
    match k {
        GithubCredentialKind::Pat => "pat",
        GithubCredentialKind::AppInstallation => "app_installation",
    }
}

fn kind_from_str(s: &str) -> GithubCredentialKind {
    match s {
        "app_installation" => GithubCredentialKind::AppInstallation,
        _ => GithubCredentialKind::Pat,
    }
}

/// Resolve which user a connector operation targets. A user always acts as
/// itself; the admin token has no user identity, so it must name the owner
/// explicitly (mirrors the OAuth-account vault, CCT-251).
fn resolve_owner(ctx: &CallerIdentity, explicit: Option<Uuid>) -> Result<Uuid, ApiError> {
    if let Some(uid) = ctx.user_id {
        return Ok(uid);
    }
    if ctx.is_admin {
        return explicit.ok_or_else(|| {
            err(StatusCode::BAD_REQUEST, "user_id required when using the admin token")
        });
    }
    Err(err(StatusCode::FORBIDDEN, "user or admin token required"))
}

/// One connector row, as stored. The credential columns hold ciphertext only.
struct ConnectorRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    credential_kind: String,
    encrypted_credential: String,
    encrypted_webhook_secret: Option<String>,
    repos: Vec<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_polled_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<String>,
}

impl ConnectorRow {
    /// Project to the API view. Decrypts the credential **only** to derive a
    /// masked preview — the plaintext is dropped immediately and never sent.
    fn into_info(self) -> ConnectorInfo {
        let key = crypto::vault_key();
        let preview = crypto::deobfuscate(&self.encrypted_credential, &key)
            .as_deref()
            .map_or_else(|| "•••".to_string(), crypto::credential_preview);
        ConnectorInfo {
            id: self.id,
            name: self.name,
            credential_kind: kind_from_str(&self.credential_kind),
            credential_preview: preview,
            has_webhook_secret: self.encrypted_webhook_secret.is_some(),
            repos: self.repos,
            user_id: self.user_id,
            created_at: self.created_at.to_rfc3339(),
            last_polled_at: self.last_polled_at.map(|t| t.to_rfc3339()),
            last_error: self.last_error,
        }
    }
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for ConnectorRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            credential_kind: row.try_get("credential_kind")?,
            encrypted_credential: row.try_get("encrypted_credential")?,
            encrypted_webhook_secret: row.try_get("encrypted_webhook_secret")?,
            repos: row.try_get("repos")?,
            created_at: row.try_get("created_at")?,
            last_polled_at: row.try_get("last_polled_at")?,
            last_error: row.try_get("last_error")?,
        })
    }
}

const SELECT_COLS: &str = "id, user_id, name, credential_kind, encrypted_credential, \
                           encrypted_webhook_secret, repos, created_at, last_polled_at, \
                           last_error";

/// `GET /api/v1/github/connectors` — the caller's connectors (credential masked).
/// Admin sees every connector.
pub async fn list_connectors(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
) -> Result<Json<Vec<ConnectorInfo>>, ApiError> {
    // Admin (user_id = None) sees all; a user only its own.
    let rows: Vec<ConnectorRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM github.connectors \
         WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name"
    ))
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connectors list db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    Ok(Json(rows.into_iter().map(ConnectorRow::into_info).collect()))
}

/// `POST /api/v1/github/connectors` — register a connector with an encrypted
/// credential. The plaintext credential and webhook secret are encrypted at rest
/// and never returned.
pub async fn create_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Json(req): Json<CreateConnector>,
) -> Result<(StatusCode, Json<ConnectorInfo>), ApiError> {
    let uid = resolve_owner(&ctx, req.user_id)?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    if req.credential.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "credential required"));
    }

    let key = crypto::vault_key();
    let enc_credential = crypto::obfuscate(req.credential.trim(), &key);
    let enc_webhook = req
        .webhook_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crypto::obfuscate(s, &key));
    let repos: Vec<String> =
        req.repos.iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();

    let row: Result<ConnectorRow, sqlx::Error> = sqlx::query_as(&format!(
        "INSERT INTO github.connectors \
            (user_id, name, credential_kind, encrypted_credential, \
             encrypted_webhook_secret, repos) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {SELECT_COLS}"
    ))
    .bind(uid)
    .bind(req.name.trim())
    .bind(kind_str(req.credential_kind))
    .bind(&enc_credential)
    .bind(&enc_webhook)
    .bind(&repos)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(r) => Ok((StatusCode::CREATED, Json(r.into_info()))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "a connector with that name already exists"))
        }
        Err(e) => {
            tracing::error!("github connector create db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `PATCH /api/v1/github/connectors/{id}` — update a connector's name, tracked
/// repos, credential, and/or webhook secret. Only the fields present in the body
/// change. Rotating the credential clears the cached `viewer_login` (and the
/// stale `last_error`) so the next poll re-resolves against the new token. A
/// user may edit only its own connectors; admin may edit any.
pub async fn update_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
    Json(req): Json<cctui_proto::github::UpdateConnector>,
) -> Result<Json<ConnectorInfo>, ApiError> {
    scope_connector(&state, &ctx, id).await?;

    if let Some(name) = req.name.as_deref()
        && name.trim().is_empty()
    {
        return Err(err(StatusCode::BAD_REQUEST, "name cannot be empty"));
    }

    let key = crypto::vault_key();
    // Build the SET clause from the present fields only. `COALESCE($n, col)`
    // leaves a column unchanged when the bind is NULL.
    let new_name = req.name.as_deref().map(str::trim).map(str::to_string);
    let new_repos = req.repos.as_ref().map(|rs| {
        rs.iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect::<Vec<_>>()
    });
    // Only rotate the credential when a non-empty one is supplied.
    let new_credential = req
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crypto::obfuscate(s, &key));
    let rotating = new_credential.is_some();
    // Webhook secret: Some("") clears, Some(non-empty) sets, None leaves as-is.
    // `set_webhook` flags whether to touch the column at all.
    let (set_webhook, new_webhook) = match req.webhook_secret.as_deref().map(str::trim) {
        None => (false, None),
        Some("") => (true, None),
        Some(s) => (true, Some(crypto::obfuscate(s, &key))),
    };

    let row: Result<ConnectorRow, sqlx::Error> = sqlx::query_as(&format!(
        "UPDATE github.connectors SET \
            name = COALESCE($2, name), \
            repos = COALESCE($3, repos), \
            encrypted_credential = COALESCE($4, encrypted_credential), \
            encrypted_webhook_secret = CASE WHEN $5 THEN $6 ELSE encrypted_webhook_secret END, \
            viewer_login = CASE WHEN $7 THEN NULL ELSE viewer_login END, \
            last_error = CASE WHEN $7 THEN NULL ELSE last_error END, \
            updated_at = now() \
         WHERE id = $1 RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(new_name)
    .bind(new_repos)
    .bind(new_credential)
    .bind(set_webhook)
    .bind(new_webhook)
    .bind(rotating)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(r) => Ok(Json(r.into_info())),
        Err(sqlx::Error::RowNotFound) => Err(err(StatusCode::NOT_FOUND, "no such connector")),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "a connector with that name already exists"))
        }
        Err(e) => {
            tracing::error!("github connector update db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `DELETE /api/v1/github/connectors/{id}` — delete a connector and its
/// encrypted credential. A user may delete only its own; admin may delete any.
pub async fn delete_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if ctx.user_id.is_none() && !ctx.is_admin {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    let res = sqlx::query(
        "DELETE FROM github.connectors WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.user_id)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connector delete db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    if res.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such connector"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/github/connectors/{id}/sync` — run the reconcile poll for one
/// connector immediately, instead of waiting for the next scheduled tick
/// (CCT-396). Scoped to the caller (admin may sync any). Returns the updated
/// connector view, whose `last_polled_at`/`last_error` reflect this attempt — so
/// a bad credential surfaces right away rather than only in the server log.
pub async fn sync_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectorInfo>, ApiError> {
    scope_connector(&state, &ctx, id).await?;
    crate::reconcile::sync_now(&state, id).await;
    let row: Option<ConnectorRow> =
        sqlx::query_as(&format!("SELECT {SELECT_COLS} FROM github.connectors WHERE id = $1"))
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("github connector sync reload db error: {e}");
                err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;
    row.map(|r| Json(r.into_info())).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such connector"))
}

/// One synced PR row plus the owning connector's cached login, scoped to the
/// caller. `viewer_login` is `None` until the connector's first reconcile pass.
struct PullRow {
    connector_id: Uuid,
    viewer_login: Option<String>,
    repo: String,
    number: i64,
    title: String,
    state: String,
    merged: bool,
    draft: bool,
    author: String,
    head_sha: String,
    head_ref: String,
    base_ref: String,
    mergeable_state: Option<String>,
    gh_updated_at: chrono::DateTime<chrono::Utc>,
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for PullRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            connector_id: row.try_get("connector_id")?,
            viewer_login: row.try_get("viewer_login")?,
            repo: row.try_get("repo")?,
            number: row.try_get("number")?,
            title: row.try_get("title")?,
            state: row.try_get("state")?,
            merged: row.try_get("merged")?,
            draft: row.try_get("draft")?,
            author: row.try_get("author")?,
            head_sha: row.try_get("head_sha")?,
            head_ref: row.try_get("head_ref")?,
            base_ref: row.try_get("base_ref")?,
            mergeable_state: row.try_get("mergeable_state")?,
            gh_updated_at: row.try_get("gh_updated_at")?,
        })
    }
}

/// `GET /api/v1/github/pulls` — the live PR inbox (GH-UI-1).
///
/// Reads the synced CONN-3 rows back, scoped to the caller's connectors (admin
/// sees all), derives each PR's attention bucket (GH-CONN-6) and a small
/// CI/review summary, and returns one flat list the webui groups by `bucket`.
/// No GitHub call is made — the viewer's login is read from the cached
/// `viewer_login` the reconcile poll backfills.
pub async fn list_pulls(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
) -> Result<Json<Vec<PullInboxItem>>, ApiError> {
    let pulls: Vec<PullRow> = sqlx::query_as(
        "SELECT p.connector_id, c.viewer_login, p.repo, p.number, p.title, \
                p.state, p.merged, p.draft, p.author, p.head_sha, p.head_ref, \
                p.base_ref, p.mergeable_state, p.gh_updated_at \
         FROM github.pulls p \
         JOIN github.connectors c ON c.id = p.connector_id \
         WHERE $1::uuid IS NULL OR c.user_id = $1 \
         ORDER BY p.gh_updated_at DESC",
    )
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github pulls list db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let mut items = Vec::with_capacity(pulls.len());
    for p in pulls {
        // Checks key off the head SHA; reviews off the PR number. Both scoped to
        // the same connector so multi-account inboxes don't cross-contaminate.
        let checks: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT status, conclusion FROM github.checks \
             WHERE connector_id = $1 AND repo = $2 AND head_sha = $3",
        )
        .bind(p.connector_id)
        .bind(&p.repo)
        .bind(&p.head_sha)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        let reviews: Vec<(String,)> = sqlx::query_as(
            "SELECT state FROM github.reviews \
             WHERE connector_id = $1 AND repo = $2 AND pull_number = $3",
        )
        .bind(p.connector_id)
        .bind(&p.repo)
        .bind(p.number)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        // Viewer relationship: authored when the PR's author is the connector's
        // own login. The reconcile scope is "author:@me OR review-requested:@me",
        // so a tracked PR the viewer didn't author is one they owe a review on.
        let viewer = match p.viewer_login.as_deref() {
            Some(login) if login.eq_ignore_ascii_case(&p.author) => {
                Viewer { authored: true, review_requested: false }
            }
            Some(_) => Viewer { authored: false, review_requested: true },
            None => Viewer::default(),
        };
        let bucket = attention::derive_bucket_from_rows(
            &p.state, p.merged, p.draft, &checks, &reviews, viewer,
        );

        items.push(PullInboxItem {
            connector_id: p.connector_id,
            repo: p.repo,
            number: p.number,
            title: p.title,
            state: p.state,
            merged: p.merged,
            draft: p.draft,
            author: p.author,
            head_ref: p.head_ref,
            base_ref: p.base_ref,
            mergeable_state: p.mergeable_state,
            gh_updated_at: p.gh_updated_at.to_rfc3339(),
            bucket,
            checks: summarize_checks(&checks),
            reviews: summarize_reviews(&reviews),
        });
    }
    Ok(Json(items))
}

/// `GET /api/v1/github/pulls/{connector_id}/{owner}/{name}/{number}/diff` —
/// the server-side diff proxy (GH-VIEW-1, docs §6.2).
///
/// `{ref}` is the same `(connector_id, repo, number)` locator the inbox uses
/// (GH-UI-1). `repo` is `owner/name`, so it spans two path segments. Resolves
/// the stored pull to its `head_sha` + the owning connector's credential, serves
/// the per-head-SHA cache if warm, else fetches from GitHub (paginated files +
/// truncated-file blob fallback), caches, and returns the structured [`PullDiff`].
/// GitHub-only: no daemon, no checkout.
pub async fn pull_diff(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<(Uuid, String, String, i64)>,
) -> Result<Json<Arc<PullDiff>>, ApiError> {
    let repo = format!("{owner}/{name}");

    // Resolve the stored pull, scoped to the caller's connector (admin sees all).
    // We need the head SHA (cache key) and the connector's encrypted credential.
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT p.head_sha, c.encrypted_credential \
         FROM github.pulls p JOIN github.connectors c ON c.id = p.connector_id \
         WHERE p.connector_id = $1 AND p.repo = $2 AND p.number = $3 \
           AND ($4::uuid IS NULL OR c.user_id = $4)",
    )
    .bind(connector_id)
    .bind(&repo)
    .bind(number)
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github pull diff db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let Some((head_sha, enc_credential)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "no such pull"));
    };

    // Per-head-SHA cache: a repeated load of an unchanged PR skips GitHub.
    if let Some(cached) = state.diff_cache.get(&head_sha) {
        return Ok(Json(cached));
    }

    let key = crypto::vault_key();
    let Some(credential) = crypto::deobfuscate(&enc_credential, &key) else {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "connector credential unavailable"));
    };

    let client = HttpDiffClient::new();
    let built = diff::build_pull_diff(&client, &credential, &repo, number, &head_sha)
        .await
        .map_err(|e| {
            // Never log the credential; the GitHub error text is safe.
            tracing::warn!(%connector_id, repo, number, "github diff fetch failed: {e}");
            err(StatusCode::BAD_GATEWAY, "failed to fetch diff from github")
        })?;
    let arc = Arc::new(built);
    state.diff_cache.put(arc.clone());
    Ok(Json(arc))
}

/// Aggregate CI check rows into the inbox's [`CheckSummary`]. Lockstep with
/// [`crate::classifier_feed`]'s failing-conclusion set.
fn summarize_checks(checks: &[(String, Option<String>)]) -> CheckSummary {
    let mut s = CheckSummary::default();
    for (status, conclusion) in checks {
        if status != "completed" {
            s.pending += 1;
            continue;
        }
        match conclusion.as_deref() {
            Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure") => {
                s.failed += 1;
            }
            _ => s.passed += 1,
        }
    }
    s
}

// ---------------------------------------------------------------------------
// GH-VIEW-4: review-draft CRUD.
//
// All draft routes share the `(connector_id, owner, name, number)` PR locator
// the diff proxy uses. Every handler first checks the caller owns the connector
// (admin sees all) so a user can never read or mutate another user's drafts,
// then delegates to `crate::drafts`. Draft comments are added *instantly* — no
// GitHub round-trip — which is exactly why a draft store exists (docs §6.2).
// ---------------------------------------------------------------------------

/// Resolve the PR locator from path parts + assert the caller may act on the
/// connector. Returns the `owner/name` repo slug. A user may act only on its own
/// connectors; the admin token may act on any. A connector the caller can't see
/// is reported as `404` (not `403`) so its existence isn't leaked.
async fn scope_connector(
    state: &GithubState,
    ctx: &CallerIdentity,
    connector_id: Uuid,
) -> Result<(), ApiError> {
    if ctx.user_id.is_none() && !ctx.is_admin {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    let owned: Option<bool> = sqlx::query_scalar(
        "SELECT true FROM github.connectors \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(connector_id)
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connector scope db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    if owned.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "no such connector"));
    }
    Ok(())
}

/// Map a [`DraftError`] to an HTTP response.
fn draft_err(e: DraftError) -> ApiError {
    match e {
        DraftError::NotFound => err(StatusCode::NOT_FOUND, "no such draft"),
        DraftError::Db => err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    }
}

type PullRef = (Uuid, String, String, i64);
type DraftRef = (Uuid, String, String, i64, Uuid);
type CommentRef = (Uuid, String, String, i64, Uuid, Uuid);

/// `GET .../{connector_id}/{owner}/{name}/{number}/drafts` — the caller's drafts
/// for a PR (each with its inline comments).
pub async fn list_drafts(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
) -> Result<Json<Vec<ReviewDraftInfo>>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");
    drafts::list_drafts(&state.pool, connector_id, &repo, number, ctx.user_id)
        .await
        .map(Json)
        .map_err(draft_err)
}

/// `POST .../{number}/drafts` — open (or reuse) the caller's open draft for a PR.
/// The admin token has no user identity, so it cannot author a user draft.
pub async fn create_draft(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
    Json(req): Json<CreateReviewDraft>,
) -> Result<(StatusCode, Json<ReviewDraftInfo>), ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let Some(uid) = ctx.user_id else {
        return Err(err(StatusCode::BAD_REQUEST, "a user identity is required to author a draft"));
    };
    let repo = format!("{owner}/{name}");
    let verdict = req.verdict.unwrap_or(ReviewVerdict::Comment);
    drafts::open_user_draft(&state.pool, connector_id, &repo, number, uid, verdict)
        .await
        .map(|d| (StatusCode::CREATED, Json(d)))
        .map_err(draft_err)
}

/// `PATCH .../drafts/{draft_id}` — change the draft's verdict.
pub async fn update_draft(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number, draft_id)): Path<DraftRef>,
    Json(req): Json<UpdateReviewDraft>,
) -> Result<Json<ReviewDraftInfo>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");
    drafts::update_verdict(&state.pool, connector_id, &repo, number, draft_id, req.verdict)
        .await
        .map(Json)
        .map_err(draft_err)
}

/// `DELETE .../drafts/{draft_id}` — discard a draft (and its comments).
pub async fn delete_draft(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number, draft_id)): Path<DraftRef>,
) -> Result<StatusCode, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");
    drafts::delete_draft(&state.pool, connector_id, &repo, number, draft_id)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(draft_err)
}

/// `POST .../drafts/{draft_id}/comments` — add an inline draft comment anchored
/// on the reviewer's diff selection. Instant: no GitHub round-trip.
pub async fn create_draft_comment(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number, draft_id)): Path<DraftRef>,
    Json(req): Json<CreateDraftComment>,
) -> Result<(StatusCode, Json<ReviewDraftInfo>), ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    if req.body.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "comment body required"));
    }
    let repo = format!("{owner}/{name}");
    drafts::add_comment(&state.pool, connector_id, &repo, number, draft_id, &req)
        .await
        .map(|d| (StatusCode::CREATED, Json(d)))
        .map_err(draft_err)
}

/// `PATCH .../drafts/{draft_id}/comments/{comment_id}` — edit a comment's body.
pub async fn update_draft_comment(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number, draft_id, comment_id)): Path<CommentRef>,
    Json(req): Json<UpdateDraftComment>,
) -> Result<Json<ReviewDraftInfo>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    if req.body.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "comment body required"));
    }
    let repo = format!("{owner}/{name}");
    drafts::update_comment(
        &state.pool,
        connector_id,
        &repo,
        number,
        draft_id,
        comment_id,
        &req.body,
    )
    .await
    .map(Json)
    .map_err(draft_err)
}

/// `DELETE .../drafts/{draft_id}/comments/{comment_id}` — remove a draft comment.
pub async fn delete_draft_comment(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number, draft_id, comment_id)): Path<CommentRef>,
) -> Result<Json<ReviewDraftInfo>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");
    drafts::delete_comment(&state.pool, connector_id, &repo, number, draft_id, comment_id)
        .await
        .map(Json)
        .map_err(draft_err)
}

// ---------------------------------------------------------------------------
// GH-VIEW-5: publish a draft as one batched GitHub review + pull-down of
// existing open GitHub review threads.
// ---------------------------------------------------------------------------

/// `POST .../{number}/publish-review` — submit the named draft as ONE batched
/// `POST /repos/{o}/{r}/pulls/{n}/reviews` (docs §6.2). Resolves each draft
/// comment's GH-VIEW-2 anchor against the PR's current head SHA, refuses if the
/// reviewer's `expected_head_sha` no longer matches (force-push), skips
/// un-anchorable comments (reporting them), submits the rest with the draft's
/// verdict, then marks the draft published and stores the returned GitHub ids.
pub async fn publish_review(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
    Json(req): Json<PublishReviewRequest>,
) -> Result<Json<PublishReviewResult>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");

    // Load the draft (header + comments), scoped to the caller.
    let drafts = drafts::list_drafts(&state.pool, connector_id, &repo, number, ctx.user_id)
        .await
        .map_err(draft_err)?;
    let Some(draft) = drafts.into_iter().find(|d| d.id == req.draft_id) else {
        return Err(err(StatusCode::NOT_FOUND, "no such draft"));
    };
    if draft.status != cctui_proto::github::DraftStatus::Draft {
        return Err(err(StatusCode::CONFLICT, "draft already published"));
    }

    // Resolve the pull's current head SHA + the connector credential (same query
    // shape as the diff proxy).
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT p.head_sha, c.encrypted_credential \
         FROM github.pulls p JOIN github.connectors c ON c.id = p.connector_id \
         WHERE p.connector_id = $1 AND p.repo = $2 AND p.number = $3 \
           AND ($4::uuid IS NULL OR c.user_id = $4)",
    )
    .bind(connector_id)
    .bind(&repo)
    .bind(number)
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github publish-review db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    let Some((head_sha, enc_credential)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "no such pull"));
    };

    let key = crypto::vault_key();
    let Some(credential) = crypto::deobfuscate(&enc_credential, &key) else {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "connector credential unavailable"));
    };

    // Fetch the current diff (cache-warm or from GitHub) to anchor comments.
    let diff = if let Some(cached) = state.diff_cache.get(&head_sha) {
        cached
    } else {
        let client = HttpDiffClient::new();
        let built = diff::build_pull_diff(&client, &credential, &repo, number, &head_sha)
            .await
            .map_err(|e| {
                tracing::warn!(%connector_id, repo, number, "github diff fetch failed: {e}");
                err(StatusCode::BAD_GATEWAY, "failed to fetch diff from github")
            })?;
        let arc = std::sync::Arc::new(built);
        state.diff_cache.put(arc.clone());
        arc
    };

    // Assemble the single batched payload, refusing on a stale head SHA.
    let payload = publish::assemble_review_payload(
        &draft,
        &diff,
        req.summary.clone(),
        req.expected_head_sha.as_deref(),
    )
    .map_err(|e| match e {
        publish::PublishError::StaleHeadSha { selection_sha, diff_sha } => err(
            StatusCode::CONFLICT,
            &format!(
                "pull was updated since this draft (drafted against {selection_sha}, now {diff_sha}); \
                 re-review against the current head"
            ),
        ),
        publish::PublishError::EmptyReview => {
            err(StatusCode::BAD_REQUEST, "nothing to publish: add a comment or a summary")
        }
    })?;

    // Submit ONE batched review.
    let client = publish::HttpReviewClient::new();
    let body = publish::review_request_json(&payload, &head_sha);
    let submitted = client.submit_review(&credential, &repo, number, &body).await.map_err(|e| {
        tracing::warn!(%connector_id, repo, number, "github submit review failed: {e}");
        err(StatusCode::BAD_GATEWAY, "failed to submit review to github")
    })?;

    // Pair returned GitHub comment ids with the source draft comments (same order
    // as the submitted comments array). A short/empty id list just leaves those
    // comments without a backfilled github_comment_id — non-fatal.
    let backfill: Vec<(Uuid, i64)> = payload
        .comments
        .iter()
        .zip(submitted.comment_ids.iter())
        .map(|(c, gid)| (c.draft_comment_id, *gid))
        .collect();

    drafts::mark_published(&state.pool, connector_id, &repo, number, draft.id, &backfill)
        .await
        .map_err(draft_err)?;

    #[allow(clippy::cast_possible_truncation)]
    let submitted_count = payload.comments.len() as u32;
    Ok(Json(PublishReviewResult {
        review_id: submitted.review_id,
        submitted: submitted_count,
        skipped: payload.skipped,
    }))
}

/// `GET .../{number}/threads` — the PR's pulled-down OPEN GitHub review threads
/// (CONN-3 rows), so the viewer renders them inline alongside local drafts. If
/// `?sync=1`, first pull the latest threads from GitHub and upsert them, then
/// read back; otherwise serve the synced rows directly.
pub async fn list_threads(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
    axum::extract::Query(q): axum::extract::Query<ThreadsQuery>,
) -> Result<Json<Vec<cctui_proto::github::ReviewThreadInfo>>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let repo = format!("{owner}/{name}");

    if q.sync.unwrap_or(false) {
        if let Err(e) = sync_threads(&state, &ctx, connector_id, &repo, number).await {
            // A sync failure (network, rate limit) is non-fatal: fall through and
            // serve whatever is already synced rather than failing the read.
            tracing::warn!(%connector_id, repo, number, "github thread pull-down failed: {e}");
        }
    }

    store::list_open_threads(&state.pool, connector_id, &repo, number).await.map(Json).map_err(
        |e| {
            tracing::error!("github list threads db error: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        },
    )
}

/// Pull the PR's open GitHub review threads/comments and upsert them into
/// `github.review_threads` / `review_comments` (CONN-3 fns).
async fn sync_threads(
    state: &GithubState,
    ctx: &CallerIdentity,
    connector_id: Uuid,
    repo: &str,
    number: i64,
) -> anyhow::Result<()> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT c.encrypted_credential FROM github.connectors c \
         WHERE c.id = $1 AND ($2::uuid IS NULL OR c.user_id = $2)",
    )
    .bind(connector_id)
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((enc_credential,)) = row else {
        anyhow::bail!("connector not found");
    };
    let key = crypto::vault_key();
    let Some(credential) = crypto::deobfuscate(&enc_credential, &key) else {
        anyhow::bail!("connector credential unavailable");
    };

    let client = publish::HttpReviewClient::new();
    let threads = client.fetch_review_threads(&credential, repo, number).await?;
    for t in threads {
        let thread = cctui_proto::github::ReviewThreadUpsert {
            repo: repo.to_string(),
            pull_number: number,
            thread_node_id: t.thread_node_id.clone(),
            // The threads table stores `path` NOT NULL; an unanchored thread
            // (rare for inline review comments) stores an empty path.
            path: t.path.clone().unwrap_or_default(),
            side: t.side.clone(),
            line: t.line,
            resolved: t.resolved,
        };
        store::upsert_review_thread(&state.pool, &state.events, connector_id, &thread).await?;
        for c in &t.comments {
            let comment = cctui_proto::github::ReviewCommentUpsert {
                repo: repo.to_string(),
                pull_number: number,
                comment_id: c.comment_id,
                thread_node_id: Some(t.thread_node_id.clone()),
                author: c.author.clone(),
                body: c.body.clone(),
                path: t.path.clone(),
                side: t.side.clone(),
                line: t.line,
                gh_created_at: c.created_at.clone(),
                gh_updated_at: c.created_at.clone(),
            };
            store::upsert_review_comment(&state.pool, &state.events, connector_id, &comment)
                .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GH-VIEW-6: blob-keyed "reviewed" marks.
//
// A reviewer marks a file reviewed keyed to its blob SHA (`DiffFile.blob_sha`).
// Marks are scoped to the caller (user identity required — the admin token has
// no user, so it cannot own marks) + the connector + the PR ref. The webui pairs
// each mark's blob SHA with the current diff: a file stays reviewed only while
// its current blob SHA still matches, so a push re-flags only changed files.
// All three handlers share the `(connector_id, owner, name, number)` locator.
// ---------------------------------------------------------------------------

/// Map a [`ViewedError`] to an HTTP response.
fn viewed_err(e: ViewedError) -> ApiError {
    match e {
        ViewedError::NotFound => err(StatusCode::NOT_FOUND, "file was not marked reviewed"),
        ViewedError::Db => err(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    }
}

/// `GET .../{number}/viewed` — the caller's blob-keyed reviewed marks for a PR.
pub async fn list_viewed(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
) -> Result<Json<Vec<ViewedMarkInfo>>, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let Some(uid) = ctx.user_id else {
        // No user identity (admin token) → no marks to own; an empty list is the
        // honest answer rather than an error.
        return Ok(Json(Vec::new()));
    };
    let repo = format!("{owner}/{name}");
    viewed::list(&state.pool, uid, connector_id, &repo, number).await.map(Json).map_err(viewed_err)
}

/// `POST .../{number}/mark-viewed` — mark a file reviewed keyed to its blob SHA.
/// Idempotent: re-marking the same path updates the stored blob SHA in place.
pub async fn mark_viewed(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
    Json(req): Json<MarkViewedRequest>,
) -> Result<StatusCode, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let Some(uid) = ctx.user_id else {
        return Err(err(StatusCode::BAD_REQUEST, "a user identity is required to mark a file"));
    };
    if req.path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path required"));
    }
    let Some(blob_sha) = req.blob_sha.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "blob_sha required to mark a file reviewed"));
    };
    let repo = format!("{owner}/{name}");
    viewed::mark(&state.pool, uid, connector_id, &repo, number, req.path.trim(), blob_sha)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(viewed_err)
}

/// `POST .../{number}/unmark-viewed` — clear a file's reviewed mark.
pub async fn unmark_viewed(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path((connector_id, owner, name, number)): Path<PullRef>,
    Json(req): Json<MarkViewedRequest>,
) -> Result<StatusCode, ApiError> {
    scope_connector(&state, &ctx, connector_id).await?;
    let Some(uid) = ctx.user_id else {
        return Err(err(StatusCode::BAD_REQUEST, "a user identity is required to unmark a file"));
    };
    if req.path.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "path required"));
    }
    let repo = format!("{owner}/{name}");
    viewed::unmark(&state.pool, uid, connector_id, &repo, number, req.path.trim())
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(viewed_err)
}

/// Aggregate review rows into the inbox's [`ReviewSummary`].
fn summarize_reviews(reviews: &[(String,)]) -> ReviewSummary {
    let mut s = ReviewSummary::default();
    for (st,) in reviews {
        if st.eq_ignore_ascii_case("changes_requested") {
            s.changes_requested += 1;
        } else if st.eq_ignore_ascii_case("approved") {
            s.approved += 1;
        } else if st.eq_ignore_ascii_case("commented") {
            s.commented += 1;
        }
    }
    s
}
