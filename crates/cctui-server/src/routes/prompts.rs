//! Prompt CRUD + repo-scoped resolution.
//!
//! Prompts can be scoped to a GitHub repo so the "Review with agent" entry
//! points can seed the *effective* review prompt for the PR's repo. Scoping is
//! richelieu-style most-specific-wins: a prompt scoped to `owner/repo` beats one
//! scoped to the whole `owner`, which beats a global (unscoped) prompt. The
//! resolution logic is a pure function ([`most_specific`]) so it is unit-tested
//! without a database.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct Prompt {
    pub id: Uuid,
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    /// Purpose tag: `general` (default) or `review` (a "Review with agent"
    /// prompt). The resolver filters on this so review-prompt scoping never
    /// collides with ordinary prompts.
    pub kind: String,
    /// GitHub owner this prompt is scoped to, or `None` for a global prompt.
    pub scope_owner: Option<String>,
    /// Repo name (within `scope_owner`) this prompt is scoped to. Requires
    /// `scope_owner`; `None` means owner-wide (or global, when owner is also
    /// `None`).
    pub scope_repo: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePrompt {
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub scope_owner: Option<String>,
    #[serde(default)]
    pub scope_repo: Option<String>,
}

/// Query for `GET /prompts/resolve`: pick the effective prompt for a repo.
#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// GitHub owner (e.g. `octocat`).
    pub owner: String,
    /// Repo name within the owner (e.g. `hello-world`).
    pub repo: String,
    /// Prompt purpose to resolve. Defaults to `review`.
    #[serde(default)]
    pub kind: Option<String>,
}

const SELECT_COLS: &str = "id, name, content, description, kind, scope_owner, scope_repo, \
     created_at, updated_at";

/// Owner-scoped list: a non-admin sees only the prompts they own; an admin sees
/// all (including legacy NULL-owner rows). The `$1::uuid IS NULL` god-view
/// binding collapses both cases.
pub async fn list_prompts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<Prompt>>, StatusCode> {
    let rows: Vec<Prompt> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SELECT_COLS} FROM prompts \
         WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name"
    )))
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

pub async fn create_prompt(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreatePrompt>,
) -> Result<(StatusCode, Json<Prompt>), StatusCode> {
    // A repo scope requires an owner (mirrors the DB CHECK) — reject early with a
    // clear status rather than surfacing a 500 from the constraint.
    if req.scope_repo.is_some() && req.scope_owner.is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kind = req.kind.as_deref().unwrap_or("general");
    let row: Prompt = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO prompts (name, content, description, kind, scope_owner, scope_repo, user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {SELECT_COLS}"
    )))
    .bind(&req.name)
    .bind(&req.content)
    .bind(&req.description)
    .bind(kind)
    .bind(&req.scope_owner)
    .bind(&req.scope_repo)
    .bind(ctx.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_prompt(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Prompt>, StatusCode> {
    let row: Option<Prompt> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SELECT_COLS} FROM prompts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)"
    )))
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Resolve the effective prompt of `kind` (default `review`) for `owner/repo`,
/// applying most-specific-wins. Returns 404 when no candidate matches.
pub async fn resolve_prompt(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<Prompt>, StatusCode> {
    let kind = q.kind.as_deref().unwrap_or("review");
    // Pull every candidate whose scope *could* apply to this repo: the exact
    // repo, the owner-wide, or the global one. The DB only narrows the search;
    // the precedence decision is made by the pure `most_specific` below so it is
    // testable and the rule lives in one place. Owner-scoped: a
    // non-admin only resolves their own prompts; an admin (or env token, NULL
    // god-view) resolves across all owners.
    let candidates: Vec<Prompt> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SELECT_COLS} FROM prompts \
         WHERE kind = $1 AND ($4::uuid IS NULL OR user_id = $4) AND ( \
             (scope_owner IS NULL AND scope_repo IS NULL) OR \
             (scope_owner = $2 AND scope_repo IS NULL) OR \
             (scope_owner = $2 AND scope_repo = $3) )"
    )))
    .bind(kind)
    .bind(&q.owner)
    .bind(&q.repo)
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    most_specific(candidates, &q.owner, &q.repo).map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Specificity of a prompt's scope for a given `owner/repo`. Higher wins.
/// `None` means the prompt's scope does not apply to this repo at all.
fn scope_rank(p: &Prompt, owner: &str, repo: &str) -> Option<u8> {
    match (p.scope_owner.as_deref(), p.scope_repo.as_deref()) {
        // Exact repo match — most specific.
        (Some(o), Some(r)) if o == owner && r == repo => Some(2),
        // Owner-wide match.
        (Some(o), None) if o == owner => Some(1),
        // Global (unscoped) fallback.
        (None, None) => Some(0),
        // Any other scope (different owner/repo, or an owner-scoped prompt with a
        // mismatched repo) does not apply.
        _ => None,
    }
}

/// Most-specific-wins resolution: among `candidates`, return the one whose scope
/// best matches `owner/repo` (exact repo > owner-wide > global). Candidates that
/// don't apply are ignored. Deterministic on ties via prompt id (the partial
/// unique indexes make real ties impossible, but a defensive tiebreak keeps the
/// pure function total).
fn most_specific(candidates: Vec<Prompt>, owner: &str, repo: &str) -> Option<Prompt> {
    candidates
        .into_iter()
        .filter_map(|p| scope_rank(&p, owner, repo).map(|r| (r, p)))
        .max_by(|(ra, pa), (rb, pb)| ra.cmp(rb).then_with(|| pa.id.cmp(&pb.id)))
        .map(|(_, p)| p)
}

/// `DELETE /prompts/{id}` — owner-or-admin only. The god-view binding scopes the
/// `DELETE` so a non-admin can never remove another user's (or a legacy
/// NULL-owner) prompt; 0 rows affected → 404.
pub async fn delete_prompt(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let res =
        sqlx::query("DELETE FROM prompts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
            .bind(id)
            .bind(ctx.owner_filter())
            .execute(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    if res.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn db_err(e: &sqlx::Error) -> StatusCode {
    tracing::error!("db error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(name: &str, owner: Option<&str>, repo: Option<&str>) -> Prompt {
        Prompt {
            id: Uuid::new_v4(),
            name: name.into(),
            content: format!("content of {name}"),
            description: None,
            kind: "review".into(),
            scope_owner: owner.map(Into::into),
            scope_repo: repo.map(Into::into),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn exact_repo_beats_owner_and_global() {
        let cands = vec![
            prompt("global", None, None),
            prompt("owner", Some("acme"), None),
            prompt("repo", Some("acme"), Some("widgets")),
        ];
        let got = most_specific(cands, "acme", "widgets").unwrap();
        assert_eq!(got.name, "repo");
    }

    #[test]
    fn owner_beats_global_when_no_repo_match() {
        let cands = vec![
            prompt("global", None, None),
            prompt("owner", Some("acme"), None),
            // a repo-scoped prompt for a *different* repo must not apply
            prompt("other-repo", Some("acme"), Some("gadgets")),
        ];
        let got = most_specific(cands, "acme", "widgets").unwrap();
        assert_eq!(got.name, "owner");
    }

    #[test]
    fn global_used_when_nothing_more_specific() {
        let cands = vec![prompt("global", None, None), prompt("other-owner", Some("other"), None)];
        let got = most_specific(cands, "acme", "widgets").unwrap();
        assert_eq!(got.name, "global");
    }

    #[test]
    fn no_candidate_applies_returns_none() {
        let cands = vec![
            prompt("other-owner", Some("other"), None),
            prompt("other-repo", Some("acme"), Some("gadgets")),
        ];
        assert!(most_specific(cands, "acme", "widgets").is_none());
    }

    #[test]
    fn empty_candidates_returns_none() {
        assert!(most_specific(vec![], "acme", "widgets").is_none());
    }

    #[test]
    fn owner_scope_with_mismatched_repo_does_not_leak() {
        // An owner-wide prompt for a different owner must never resolve.
        let cands = vec![prompt("owner", Some("acme"), None)];
        assert!(most_specific(cands, "different", "widgets").is_none());
    }
}
