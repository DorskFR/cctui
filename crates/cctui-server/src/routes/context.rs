//! `/api/v1/context` — reusable per-user context attached to a session at
//! spawn: durable memory notes and prompt templates.
//!
//! Skills are deliberately absent: a skill bundle is a plugin, delivered per
//! session by the plugin platform. See `docs/adr/0002-session-context.md`.
//!
//! Scope resolution ([`resolve_auto`]) is a pure function over rows so it is
//! unit-tested without a database, the way the prompt resolver is.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

pub const KINDS: &[&str] = &["memory", "prompt"];
pub const SCOPES: &[&str] = &["user", "machine", "path", "label"];

/// Longest body we accept. A memory note is a note; anything larger belongs in
/// the repo the agent can already read.
const MAX_BODY: usize = 64 * 1024;
const MAX_ITEMS_PER_SPAWN: usize = 32;

#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    /// `memory` or `prompt`.
    pub kind: String,
    /// Slug, unique per `(user_id, kind)`; the stable reference a profile or a
    /// spawn request names.
    pub name: String,
    pub title: String,
    pub body: String,
    /// `user` | `machine` | `path` | `label`.
    pub scope: String,
    /// Machine id, working-dir prefix or label id. `None` for `user` scope.
    pub scope_ref: Option<String>,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub version: i32,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub updated_at: DateTime<Utc>,
}

/// The editable half of an item.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItemSpec {
    pub kind: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "user_scope")]
    pub scope: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub scope_ref: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn user_scope() -> String {
    "user".to_owned()
}

const fn yes() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdateContextItemRequest {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub spec: Option<ContextItemSpec>,
}

const COLS: &str = "id, user_id, kind, name, title, body, scope, scope_ref, tags, enabled, \
                    version, created_at, updated_at";

fn db_err(e: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(dbe) = &e
        && dbe.code().as_deref() == Some("23505")
    {
        return AppError::new(StatusCode::CONFLICT, "an item of that kind and name already exists");
    }
    AppError::from(e)
}

/// A slug safe as both a wire reference and a staged filename.
fn clean_name(raw: &str) -> Result<String, AppError> {
    let name = raw.trim().to_ascii_lowercase();
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !ok {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "name must be 1-64 chars of a-z, 0-9 and dashes, starting alphanumeric",
        ));
    }
    Ok(name)
}

/// Validate and normalize a spec. A `path` scope is canonicalized to an
/// absolute, slash-free-tail prefix so matching is a plain component compare.
pub fn clean_spec(spec: ContextItemSpec) -> Result<ContextItemSpec, AppError> {
    let bad = |m: &'static str| AppError::new(StatusCode::BAD_REQUEST, m);
    let kind = spec.kind.trim().to_owned();
    if !KINDS.contains(&kind.as_str()) {
        return Err(bad("kind must be memory or prompt"));
    }
    let scope = spec.scope.trim().to_owned();
    if !SCOPES.contains(&scope.as_str()) {
        return Err(bad("unknown scope"));
    }
    let title = spec.title.trim().to_owned();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(bad("title is required (max 120)"));
    }
    if spec.body.len() > MAX_BODY {
        return Err(bad("body is too long (max 64KiB)"));
    }
    let scope_ref = spec.scope_ref.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let scope_ref = match (scope.as_str(), scope_ref) {
        ("user", _) => None,
        (_, None) => return Err(bad("this scope needs a target")),
        ("path", Some(p)) if !p.starts_with('/') => {
            return Err(bad("a path scope must be absolute"));
        }
        ("path", Some(p)) => Some(p.trim_end_matches('/').to_owned()),
        (_, Some(v)) => Some(v),
    };
    let mut tags: Vec<String> = spec
        .tags
        .into_iter()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty() && t.len() <= 32)
        .collect();
    tags.sort();
    tags.dedup();
    Ok(ContextItemSpec {
        kind,
        name: clean_name(&spec.name)?,
        title,
        body: spec.body,
        scope,
        scope_ref,
        tags,
        enabled: spec.enabled,
    })
}

/// What a spawn is resolved against.
#[derive(Debug, Default, Clone)]
pub struct SpawnScope {
    pub machine_id: Option<String>,
    pub working_dir: Option<String>,
    pub label_ids: Vec<String>,
}

/// Whether `dir` is at or under `prefix`, comparing whole path components so
/// `/src/foobar` does not match a `/src/foo` scope.
#[must_use]
pub fn path_matches(prefix: &str, dir: &str) -> bool {
    let norm = |p: &str| {
        p.trim_end_matches('/').split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect()
    };
    let (prefix, dir): (Vec<String>, Vec<String>) = (norm(prefix), norm(dir));
    prefix.len() <= dir.len() && prefix.iter().zip(&dir).all(|(a, b)| a == b)
}

/// Whether an enabled item's scope matches this spawn.
#[must_use]
pub fn scope_matches(item: &ContextItem, scope: &SpawnScope) -> bool {
    if !item.enabled {
        return false;
    }
    match (item.scope.as_str(), item.scope_ref.as_deref()) {
        ("user", _) => true,
        ("machine", Some(m)) => scope.machine_id.as_deref() == Some(m),
        ("path", Some(p)) => scope.working_dir.as_deref().is_some_and(|d| path_matches(p, d)),
        ("label", Some(l)) => scope.label_ids.iter().any(|id| id == l),
        _ => false,
    }
}

/// The items a spawn resolves automatically: every enabled `memory` whose
/// scope matches. Prompt templates are never auto-applied — a template
/// replaces the user's first turn, which must stay an explicit choice.
#[must_use]
pub fn resolve_auto(items: &[ContextItem], scope: &SpawnScope) -> Vec<ContextItem> {
    items.iter().filter(|i| i.kind == "memory" && scope_matches(i, scope)).cloned().collect()
}

/// Explicit picks (by name, any kind) unioned with [`resolve_auto`] when
/// `auto`, in a stable order: memories first in title order, then the prompt
/// template. Capped, so a runaway scope cannot flood a launch.
#[must_use]
pub fn resolve_for_spawn(
    items: &[ContextItem],
    picks: &[String],
    auto: bool,
    scope: &SpawnScope,
) -> Vec<ContextItem> {
    let mut chosen: Vec<ContextItem> = if auto { resolve_auto(items, scope) } else { Vec::new() };
    for name in picks {
        if let Some(item) = items.iter().find(|i| &i.name == name)
            && !chosen.iter().any(|c| c.id == item.id)
        {
            chosen.push(item.clone());
        }
    }
    chosen.sort_by(|a, b| {
        (a.kind != "memory", a.title.to_lowercase(), a.id).cmp(&(
            b.kind != "memory",
            b.title.to_lowercase(),
            b.id,
        ))
    });
    chosen.truncate(MAX_ITEMS_PER_SPAWN);
    chosen
}

/// Expand a prompt template: `{{cwd}}`, `{{name}}`, `{{topic}}`. An unknown
/// placeholder is left verbatim rather than blanked, so a typo is visible in
/// the turn instead of silently deleting text.
#[must_use]
pub fn expand_template(body: &str, cwd: &str, name: &str, topic: &str) -> String {
    body.replace("{{cwd}}", cwd).replace("{{name}}", name).replace("{{topic}}", topic)
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<ContextItem>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM context_items WHERE user_id = $1 ORDER BY kind, lower(title), name"
    )))
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn get_many(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<ContextItem>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM context_items WHERE id = ANY($1) ORDER BY kind, lower(title), name"
    )))
    .bind(ids)
    .fetch_all(pool)
    .await
}

pub async fn insert(
    pool: &PgPool,
    user_id: Uuid,
    spec: &ContextItemSpec,
) -> Result<ContextItem, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO context_items \
            (user_id, kind, name, title, body, scope, scope_ref, tags, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLS}"
    )))
    .bind(user_id)
    .bind(&spec.kind)
    .bind(&spec.name)
    .bind(&spec.title)
    .bind(&spec.body)
    .bind(&spec.scope)
    .bind(spec.scope_ref.as_deref())
    .bind(&spec.tags)
    .bind(spec.enabled)
    .fetch_one(pool)
    .await
}

/// Replace the item's spec. `version` advances on every edit so a session card
/// can name the revision it received.
pub async fn update(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    spec: &ContextItemSpec,
) -> Result<Option<ContextItem>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE context_items SET kind = $3, name = $4, title = $5, body = $6, scope = $7, \
            scope_ref = $8, tags = $9, enabled = $10, version = version + 1, updated_at = now() \
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(user_id)
    .bind(&spec.kind)
    .bind(&spec.name)
    .bind(&spec.title)
    .bind(&spec.body)
    .bind(&spec.scope)
    .bind(spec.scope_ref.as_deref())
    .bind(&spec.tags)
    .bind(spec.enabled)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM context_items WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Record what a spawn resolved, keyed by the launch key the daemon will pull
/// its gateway env with. Best-effort: a failure costs the session its context,
/// never its launch.
pub async fn remember_intent(pool: &PgPool, spawn_key: &str, items: &[ContextItem]) {
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    if ids.is_empty() {
        return;
    }
    let done = sqlx::query(
        "INSERT INTO session_context (spawn_key, item_ids) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET item_ids = EXCLUDED.item_ids",
    )
    .bind(spawn_key)
    .bind(&ids)
    .execute(pool)
    .await;
    if let Err(e) = done {
        tracing::warn!(%spawn_key, error = %e, "could not record the session context set");
    }
}

/// Re-key the spawn's context set onto the id the harness actually registered
/// under, so a later resume — which pulls gateway env by the real session id,
/// not the launch key — still resolves the same kit. Idempotent.
pub async fn claim_intent(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    let Some(spawn_key) = spawn_key.filter(|k| *k != session_id) else { return };
    let done = sqlx::query(
        "INSERT INTO session_context (spawn_key, item_ids)          SELECT $1, item_ids FROM session_context WHERE spawn_key = $2          ON CONFLICT (spawn_key) DO UPDATE SET item_ids = EXCLUDED.item_ids",
    )
    .bind(session_id)
    .bind(spawn_key)
    .execute(pool)
    .await;
    if let Err(e) = done {
        tracing::warn!(%session_id, error = %e, "could not re-key the session context set");
    }
}

/// The items a launching session was granted, resolved through the spawn key
/// the server recorded and, failing that, the session's own id.
pub async fn for_session(pool: &PgPool, session_id: &str) -> Vec<ContextItem> {
    let ids: Option<Vec<Uuid>> =
        sqlx::query_scalar("SELECT item_ids FROM session_context WHERE spawn_key = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await
            .unwrap_or_default();
    let Some(ids) = ids.filter(|ids| !ids.is_empty()) else { return Vec::new() };
    get_many(pool, &ids).await.unwrap_or_else(|e| {
        tracing::warn!(%session_id, error = %e, "context item lookup failed");
        Vec::new()
    })
}

/// The context a `CctuiAgent` child inherits: the pinned set of the profile it
/// names, by name, the way the tool takes it.
///
/// The same [`resolve_for_spawn`] the webui path uses, so a profile means the
/// same thing from a browser and from the tool. The scope is still resolved
/// so the two paths agree once auto-attach is switched on. Best-effort
/// throughout: a child launches without context rather than not at all.
pub async fn resolve_for_child(
    pool: &PgPool,
    user_id: Uuid,
    profile_name: Option<&str>,
    machine_id: Option<&str>,
    working_dir: Option<&str>,
) -> Vec<ContextItem> {
    let items = match list_for_user(pool, user_id).await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "child context lookup failed; spawning without it");
            return Vec::new();
        }
    };
    let mut picks = Vec::new();
    if let Some(name) = profile_name.map(str::trim).filter(|n| !n.is_empty()) {
        let pinned: Option<Vec<Uuid>> = sqlx::query_scalar(
            "SELECT context_items FROM session_profiles WHERE user_id = $1 AND name = $2",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap_or_default();
        picks.extend(
            pinned
                .unwrap_or_default()
                .iter()
                .filter_map(|id| items.iter().find(|i| &i.id == id))
                .map(|i| i.name.clone()),
        );
    }
    let scope = SpawnScope {
        machine_id: machine_id.map(str::to_owned),
        working_dir: working_dir.map(str::to_owned),
        label_ids: Vec::new(),
    };
    // Auto off, like every other spawn path: a child inherits what its
    // profile pins, never what its cwd happens to match.
    resolve_for_spawn(&items, &picks, false, &scope)
}

/// `GET /context` — every item the caller owns.
pub async fn list_items(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<ContextItem>>, AppError> {
    Ok(Json(list_for_user(&state.pool, ctx.user_id).await?))
}

/// `POST /context` — create; 409 on a duplicate `(kind, name)`.
pub async fn create_item(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(spec): Json<ContextItemSpec>,
) -> Result<(StatusCode, Json<ContextItem>), AppError> {
    let spec = clean_spec(spec)?;
    let row = insert(&state.pool, ctx.user_id, &spec).await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `PATCH /context/{id}` — replace the spec of the caller's own item.
pub async fn update_item(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateContextItemRequest>,
) -> Result<Json<ContextItem>, AppError> {
    let spec = req
        .spec
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "spec is required"))
        .and_then(clean_spec)?;
    let row = update(&state.pool, ctx.user_id, id, &spec).await.map_err(db_err)?;
    row.map(Json).ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "context item not found"))
}

/// `DELETE /context/{id}` — the caller's own item only. Sessions already
/// launched with it keep the copy they were staged.
pub async fn delete_item(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    if delete(&state.pool, ctx.user_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::new(StatusCode::NOT_FOUND, "context item not found"))
    }
}

/// Query for `GET /context/resolve`: what a spawn into these coordinates would
/// attach on its own. Backs the spawn modal's pre-checked set.
#[derive(Debug, serde::Deserialize)]
pub struct ResolveQuery {
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Comma-separated label ids.
    #[serde(default)]
    pub labels: Option<String>,
}

/// `GET /context/resolve` — the auto-resolved set for a prospective spawn.
pub async fn resolve_items(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<Vec<ContextItem>>, AppError> {
    let items = list_for_user(&state.pool, ctx.user_id).await?;
    let scope = SpawnScope {
        machine_id: q.machine_id,
        working_dir: q.working_dir,
        label_ids: q
            .labels
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    Ok(Json(resolve_auto(&items, &scope)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, name: &str, scope: &str, scope_ref: Option<&str>) -> ContextItem {
        ContextItem {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            title: name.to_owned(),
            body: format!("body of {name}"),
            scope: scope.to_owned(),
            scope_ref: scope_ref.map(str::to_owned),
            tags: Vec::new(),
            enabled: true,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn names(items: &[ContextItem]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn a_path_scope_matches_whole_components_only() {
        assert!(path_matches("/src/foo", "/src/foo"));
        assert!(path_matches("/src/foo", "/src/foo/bar"));
        assert!(path_matches("/src/foo/", "/src/foo/bar"));
        assert!(!path_matches("/src/foo", "/src/foobar"), "a prefix is not a parent");
        assert!(!path_matches("/src/foo", "/src"));
        assert!(!path_matches("/src/foo", "/other/foo"));
        assert!(path_matches("/", "/anything/at/all"));
    }

    #[test]
    fn every_scope_resolves_against_the_spawn_coordinates() {
        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "this-box", "machine", Some("m-1")),
            item("memory", "other-box", "machine", Some("m-2")),
            item("memory", "in-repo", "path", Some("/w/repo")),
            item("memory", "elsewhere", "path", Some("/w/other")),
            item("memory", "tagged", "label", Some("lab-1")),
            item("memory", "untagged", "label", Some("lab-9")),
        ];
        let scope = SpawnScope {
            machine_id: Some("m-1".into()),
            working_dir: Some("/w/repo/crates".into()),
            label_ids: vec!["lab-1".into()],
        };
        assert_eq!(
            names(&resolve_auto(&items, &scope)),
            ["always", "in-repo", "this-box", "tagged"]
        );
    }

    #[test]
    fn a_disabled_item_is_never_resolved() {
        let mut off = item("memory", "off", "user", None);
        off.enabled = false;
        assert!(resolve_auto(&[off], &SpawnScope::default()).is_empty());
    }

    /// A prompt template replaces the first turn, so it only ever arrives by
    /// an explicit pick.
    #[test]
    fn prompt_templates_are_never_auto_resolved() {
        let items = vec![item("prompt", "reviewer", "user", None)];
        assert!(resolve_auto(&items, &SpawnScope::default()).is_empty());
        let picked =
            resolve_for_spawn(&items, &["reviewer".to_owned()], true, &SpawnScope::default());
        assert_eq!(names(&picked), ["reviewer"]);
    }

    #[test]
    fn explicit_picks_union_with_the_auto_set_without_duplicating() {
        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "hand-picked", "machine", Some("elsewhere")),
            item("prompt", "reviewer", "user", None),
        ];
        let picks = ["always".to_owned(), "hand-picked".to_owned(), "reviewer".to_owned()];
        let got = resolve_for_spawn(&items, &picks, true, &SpawnScope::default());
        assert_eq!(
            names(&got),
            ["always", "hand-picked", "reviewer"],
            "memories first, prompt last"
        );

        let manual =
            resolve_for_spawn(&items, &["hand-picked".to_owned()], false, &SpawnScope::default());
        assert_eq!(names(&manual), ["hand-picked"], "auto off means only the picks");
        assert!(resolve_for_spawn(&items, &[], false, &SpawnScope::default()).is_empty());
    }

    /// Profile parity: both spawn paths funnel into `resolve_for_spawn` with
    /// the profile's pinned names, so a webui spawn and a `CctuiAgent` child
    /// of the same profile resolve the same kit. They differ only in how they
    /// look the profile up — by id from the form, by name from the tool — and
    /// both key the machine scope on the machine's uuid, never on the name a
    /// request happened to use.
    #[test]
    fn a_profile_resolves_the_same_kit_from_the_webui_and_from_the_tool() {
        let machine = Uuid::new_v4().to_string();
        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "this-box", "machine", Some(&machine)),
            item("memory", "in-repo", "path", Some("/w/repo")),
            item("memory", "pinned", "machine", Some("somewhere-else")),
            item("prompt", "reviewer", "user", None),
        ];
        // What a profile pins, named the same way on both paths.
        let pinned = ["pinned".to_owned(), "reviewer".to_owned()];
        let scope = SpawnScope {
            machine_id: Some(machine),
            working_dir: Some("/w/repo/crates".into()),
            label_ids: Vec::new(),
        };

        // Both paths run with auto off today: the pinned set, nothing else.
        let from_webui = resolve_for_spawn(&items, &pinned, false, &scope);
        let from_tool = resolve_for_spawn(&items, &pinned, false, &scope);
        assert_eq!(names(&from_webui), names(&from_tool));
        assert_eq!(
            names(&from_webui),
            ["pinned", "reviewer"],
            "a scope match the profile did not pin must not ride along"
        );

        // And they still agree once auto-attach is switched on.
        assert_eq!(
            names(&resolve_for_spawn(&items, &pinned, true, &scope)),
            ["always", "in-repo", "pinned", "this-box", "reviewer"],
        );

        // A machine-scoped item keyed on a different machine must not follow.
        let elsewhere = SpawnScope { machine_id: Some(Uuid::new_v4().to_string()), ..scope };
        assert!(!names(&resolve_for_spawn(&items, &[], true, &elsewhere)).contains(&"this-box"));
    }

    /// The shipped default: a spawn that says nothing about context gets
    /// none. Auto-attach stays off until the spawn panel can show what a
    /// scope would pull in.
    #[test]
    fn a_spawn_that_asks_for_nothing_attaches_nothing() {
        let asked = cctui_proto::api::SpawnContext::default();
        assert!(!asked.auto, "auto-attach is opt-in");
        assert!(asked.items.is_empty());

        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "in-repo", "path", Some("/w/repo")),
        ];
        let scope = SpawnScope {
            machine_id: Some("m-1".into()),
            working_dir: Some("/w/repo".into()),
            label_ids: Vec::new(),
        };
        assert!(
            resolve_for_spawn(&items, &asked.items, asked.auto, &scope).is_empty(),
            "a matching scope is not consent"
        );
        // A body that omits `auto` decodes to the same opt-in default.
        let decoded: cctui_proto::api::SpawnContext =
            serde_json::from_str(r#"{"items":["always"]}"#).expect("decodes");
        assert!(!decoded.auto);
        assert_eq!(
            names(&resolve_for_spawn(&items, &decoded.items, decoded.auto, &scope)),
            ["always"]
        );
    }

    #[test]
    fn an_unknown_pick_is_ignored_rather_than_failing_the_spawn() {
        let items = vec![item("memory", "known", "user", None)];
        let got = resolve_for_spawn(&items, &["ghost".to_owned()], false, &SpawnScope::default());
        assert!(got.is_empty());
    }

    #[test]
    fn the_resolved_set_is_capped() {
        let items: Vec<ContextItem> =
            (0..50).map(|i| item("memory", &format!("m{i:02}"), "user", None)).collect();
        assert_eq!(resolve_auto(&items, &SpawnScope::default()).len(), 50);
        assert_eq!(
            resolve_for_spawn(&items, &[], true, &SpawnScope::default()).len(),
            MAX_ITEMS_PER_SPAWN
        );
    }

    #[test]
    fn templates_expand_known_placeholders_and_leave_typos_visible() {
        let out = expand_template("in {{cwd}} as {{name}} on {{topic}}", "/w", "rev", "auth");
        assert_eq!(out, "in /w as rev on auth");
        assert_eq!(expand_template("{{nope}}", "/w", "n", "t"), "{{nope}}");
    }

    #[test]
    fn names_are_slugs() {
        assert_eq!(clean_name(" House-Style ").unwrap(), "house-style");
        for bad in ["", "-leading", "has space", "has_underscore", "héllo", &"x".repeat(65)] {
            assert!(clean_name(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    fn spec(kind: &str, scope: &str, scope_ref: Option<&str>) -> ContextItemSpec {
        ContextItemSpec {
            kind: kind.to_owned(),
            name: "note".to_owned(),
            title: " House style ".to_owned(),
            body: "be terse".to_owned(),
            scope: scope.to_owned(),
            scope_ref: scope_ref.map(str::to_owned),
            tags: vec![" Rust ".to_owned(), "rust".to_owned(), String::new()],
            enabled: true,
        }
    }

    #[test]
    fn a_spec_is_normalized_and_validated() {
        let ok = clean_spec(spec("memory", "path", Some("/w/repo/"))).expect("valid");
        assert_eq!(ok.title, "House style");
        assert_eq!(ok.scope_ref.as_deref(), Some("/w/repo"), "trailing slash normalized away");
        assert_eq!(ok.tags, ["rust"], "tags lowercased, deduped, blanks dropped");

        let user_scoped = clean_spec(spec("memory", "user", Some("ignored"))).expect("valid");
        assert!(user_scoped.scope_ref.is_none(), "user scope carries no target");

        assert!(clean_spec(spec("skill", "user", None)).is_err(), "skills are plugins");
        assert!(clean_spec(spec("memory", "nonsense", None)).is_err());
        assert!(clean_spec(spec("memory", "machine", None)).is_err(), "needs a target");
        assert!(clean_spec(spec("memory", "path", Some("relative"))).is_err());

        let mut huge = spec("memory", "user", None);
        huge.body = "x".repeat(MAX_BODY + 1);
        assert!(clean_spec(huge).is_err());

        let mut blank = spec("memory", "user", None);
        blank.title = "   ".to_owned();
        assert!(clean_spec(blank).is_err());
    }
}
