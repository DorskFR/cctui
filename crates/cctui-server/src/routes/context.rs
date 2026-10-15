//! `/api/v1/context` — reusable per-user context attached to a session:
//! always-on rules, durable memory notes and prompt templates.
//!
//! Skills are deliberately absent: a skill bundle is a plugin, delivered per
//! session by the plugin platform. See `docs/adr/0002-session-context.md`.
//!
//! Resolution ([`resolve_auto`], [`resolve_rules`], [`resolve_for_spawn`]) is
//! pure over rows so it is unit-tested without a database, the way the prompt
//! resolver is.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

pub use cctui_proto::api::context::{
    AttachContextRequest, AttachContextResponse, AttachedContextRef, ContextItem, ContextItemSpec,
    ContextMatch, ResolvedContextResponse, SessionContextResponse, UpdateContextItemRequest,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

pub const KINDS: &[&str] = &["memory", "prompt", "rule"];
pub const SCOPES: &[&str] = &["user", "machine", "path", "label"];

/// A memory note is a note; anything larger belongs in the repo the agent can
/// already read.
const MAX_BODY: usize = 64 * 1024;
/// A rule rides inline into every matching launch, so it must stay short.
pub const MAX_RULE_BODY: usize = 4 * 1024;
/// Rule bytes one launch carries; the tail past it is dropped.
pub const MAX_RULES_PER_LAUNCH: usize = 16 * 1024;
const MAX_ITEMS_PER_SPAWN: usize = 32;

const COLS: &str = "id, user_id, kind, name, title, body, scope, scope_ref, filters, tags, \
                    enabled, version, pending, created_by_session, updated_by_session, \
                    created_at, updated_at";

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    name: String,
    title: String,
    body: String,
    scope: String,
    scope_ref: Option<String>,
    filters: Jsonb<ContextMatch>,
    tags: Vec<String>,
    enabled: bool,
    version: i32,
    pending: Option<Jsonb<ContextItemSpec>>,
    created_by_session: Option<String>,
    updated_by_session: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Row> for ContextItem {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            user_id: r.user_id,
            kind: r.kind,
            name: r.name,
            title: r.title,
            body: r.body,
            scope: r.scope,
            scope_ref: r.scope_ref,
            match_: r.filters.0,
            tags: r.tags,
            enabled: r.enabled,
            version: r.version,
            pending: r.pending.map(|p| p.0),
            created_by_session: r.created_by_session,
            updated_by_session: r.updated_by_session,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(dbe) = &e
        && dbe.code().as_deref() == Some("23505")
    {
        return AppError::new(StatusCode::CONFLICT, "an item of that kind and name already exists");
    }
    AppError::from(e)
}

/// A slug safe as both a wire reference and a staged filename.
pub(crate) fn clean_name(raw: &str) -> Result<String, AppError> {
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

fn clean_list(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> =
        raw.into_iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
    out.sort();
    out.dedup();
    out
}

fn clean_path(raw: &str) -> Result<String, AppError> {
    if !raw.starts_with('/') {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "a path filter must be absolute"));
    }
    let trimmed = raw.trim_end_matches('/');
    Ok(if trimmed.is_empty() { "/".to_owned() } else { trimmed.to_owned() })
}

fn clean_match(m: ContextMatch) -> Result<ContextMatch, AppError> {
    let paths = clean_list(m.paths).iter().map(|p| clean_path(p)).collect::<Result<_, _>>()?;
    Ok(ContextMatch {
        machines: clean_list(m.machines),
        paths: clean_list(paths),
        labels: clean_list(m.labels),
        adapters: clean_list(m.adapters.into_iter().map(|a| a.to_ascii_lowercase()).collect()),
    })
}

/// The legacy scope a filter maps onto: exactly one value under one of the
/// scoped keys, else `user`. Kept so `scope`/`scope_ref` stay readable.
#[must_use]
pub fn scope_of(m: &ContextMatch) -> (String, Option<String>) {
    let keys = [("machine", &m.machines), ("path", &m.paths), ("label", &m.labels)];
    let set: Vec<_> = keys.iter().filter(|(_, v)| !v.is_empty()).collect();
    match set.as_slice() {
        [(scope, values)] if values.len() == 1 && m.adapters.is_empty() => {
            ((*scope).to_owned(), Some(values[0].clone()))
        }
        _ => ("user".to_owned(), None),
    }
}

/// The filter a legacy scope stands for.
#[must_use]
pub fn match_of(scope: &str, scope_ref: Option<&str>) -> ContextMatch {
    let one = || scope_ref.map(|r| vec![r.to_owned()]).unwrap_or_default();
    match scope {
        "machine" => ContextMatch { machines: one(), ..ContextMatch::default() },
        "path" => ContextMatch { paths: one(), ..ContextMatch::default() },
        "label" => ContextMatch { labels: one(), ..ContextMatch::default() },
        _ => ContextMatch::default(),
    }
}

/// Validate and normalize a spec. A non-empty `match` wins over
/// `scope`/`scope_ref`; either way both are returned in step.
pub fn clean_spec(spec: ContextItemSpec) -> Result<ContextItemSpec, AppError> {
    let bad = |m: &'static str| AppError::new(StatusCode::BAD_REQUEST, m);
    let kind = spec.kind.trim().to_owned();
    if !KINDS.contains(&kind.as_str()) {
        return Err(bad("kind must be memory, prompt or rule"));
    }
    let title = spec.title.trim().to_owned();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(bad("title is required (max 120)"));
    }
    if kind == "rule" && spec.body.len() > MAX_RULE_BODY {
        return Err(bad("a rule body is too long (max 4KiB)"));
    }
    if spec.body.len() > MAX_BODY {
        return Err(bad("body is too long (max 64KiB)"));
    }
    let (scope, scope_ref, match_) = if spec.match_.is_empty() {
        let scope = Some(spec.scope.trim()).filter(|s| !s.is_empty()).unwrap_or("user").to_owned();
        if !SCOPES.contains(&scope.as_str()) {
            return Err(bad("unknown scope"));
        }
        let scope_ref = spec.scope_ref.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let scope_ref = match (scope.as_str(), scope_ref) {
            ("user", _) => None,
            (_, None) => return Err(bad("this scope needs a target")),
            ("path", Some(p)) if !p.starts_with('/') => {
                return Err(bad("a path scope must be absolute"));
            }
            ("path", Some(p)) => Some(clean_path(&p)?),
            (_, Some(v)) => Some(v),
        };
        let m = match_of(&scope, scope_ref.as_deref());
        (scope, scope_ref, m)
    } else {
        let m = clean_match(spec.match_)?;
        let (scope, scope_ref) = scope_of(&m);
        (scope, scope_ref, m)
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
        match_,
        tags,
        enabled: spec.enabled,
    })
}

/// The editable half of an existing item.
#[must_use]
pub fn spec_of(item: &ContextItem) -> ContextItemSpec {
    ContextItemSpec {
        kind: item.kind.clone(),
        name: item.name.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        scope: item.scope.clone(),
        scope_ref: item.scope_ref.clone(),
        match_: item.match_.clone(),
        tags: item.tags.clone(),
        enabled: item.enabled,
    }
}

/// What a launch is resolved against.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnScope {
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub adapter: Option<String>,
}

/// A launch's coordinates as recorded at spawn, so every later launch of the
/// session re-resolves its rules against the owner's current set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredScope {
    pub user_id: Uuid,
    #[serde(flatten)]
    pub coords: SpawnScope,
    /// No human is watching a dispatched worker, so everything it saves waits
    /// for approval.
    #[serde(default)]
    pub dispatched: bool,
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

/// AND across keys, OR within one key, an empty key matches anything. A
/// non-empty key never matches a launch that lacks that coordinate.
#[must_use]
pub fn filter_matches(m: &ContextMatch, scope: &SpawnScope) -> bool {
    let any = |list: &[String], hit: &dyn Fn(&str) -> bool| {
        list.is_empty() || list.iter().any(|v| hit(v.as_str()))
    };
    any(&m.machines, &|v| scope.machine_id.as_deref() == Some(v))
        && any(&m.paths, &|p| scope.working_dir.as_deref().is_some_and(|d| path_matches(p, d)))
        && any(&m.labels, &|l| scope.label_ids.iter().any(|id| id == l))
        && any(&m.adapters, &|a| scope.adapter.as_deref() == Some(a))
}

/// Whether an enabled item's filter matches this launch.
#[must_use]
pub fn scope_matches(item: &ContextItem, scope: &SpawnScope) -> bool {
    item.enabled && filter_matches(&item.match_, scope)
}

/// The memories a spawn pre-selects: every enabled `memory` whose filter
/// matches. Prompt templates are never auto-applied — a template replaces the
/// user's first turn, which must stay an explicit choice — and rules are
/// resolved by [`resolve_rules`].
#[must_use]
pub fn resolve_auto(items: &[ContextItem], scope: &SpawnScope) -> Vec<ContextItem> {
    let mut chosen: Vec<ContextItem> =
        items.iter().filter(|i| i.kind == "memory" && scope_matches(i, scope)).cloned().collect();
    sort_for_delivery(&mut chosen);
    chosen
}

/// Every enabled rule whose filter matches, in title order, capped at
/// [`MAX_RULES_PER_LAUNCH`] body bytes.
#[must_use]
pub fn resolve_rules(items: &[ContextItem], scope: &SpawnScope) -> Vec<ContextItem> {
    let mut rules: Vec<ContextItem> =
        items.iter().filter(|i| i.kind == "rule" && scope_matches(i, scope)).cloned().collect();
    rules.sort_by(|a, b| {
        (a.title.to_lowercase(), &a.name, a.id).cmp(&(b.title.to_lowercase(), &b.name, b.id))
    });
    cap_rules(rules)
}

/// Keep rules in order until the next one would cross the launch cap, then
/// drop it and everything after it, so the cut is the same on every launch.
fn cap_rules(rules: Vec<ContextItem>) -> Vec<ContextItem> {
    let mut used = 0;
    let Some(cut) = rules.iter().position(|r| {
        used += r.body.len();
        used > MAX_RULES_PER_LAUNCH
    }) else {
        return rules;
    };
    let mut rules = rules;
    let dropped: Vec<String> = rules.split_off(cut).into_iter().map(|r| r.name).collect();
    tracing::warn!(
        kept = rules.len(),
        ?dropped,
        cap = MAX_RULES_PER_LAUNCH,
        "context rules exceed the per-launch cap; dropped the tail"
    );
    rules
}

/// Memories first in title order, then the prompt template. The picker, the
/// preamble and the launch all show this set, so its order is fixed here rather
/// than inherited from whatever order the rows were read in.
fn sort_for_delivery(items: &mut [ContextItem]) {
    items.sort_by(|a, b| {
        (a.kind != "memory", a.title.to_lowercase(), a.id).cmp(&(
            b.kind != "memory",
            b.title.to_lowercase(),
            b.id,
        ))
    });
}

/// The matching rules, then explicit picks (by name; memories and prompts)
/// unioned with [`resolve_auto`] when `auto`. Rules attach whatever `auto`
/// says: enabling a rule is the consent. Picks are capped, so a runaway
/// filter cannot flood a launch.
#[must_use]
pub fn resolve_for_spawn(
    items: &[ContextItem],
    picks: &[String],
    auto: bool,
    scope: &SpawnScope,
) -> Vec<ContextItem> {
    let mut chosen: Vec<ContextItem> = if auto { resolve_auto(items, scope) } else { Vec::new() };
    for name in picks {
        if let Some(item) = items.iter().find(|i| &i.name == name && i.kind != "rule")
            && !chosen.iter().any(|c| c.id == item.id)
        {
            chosen.push(item.clone());
        }
    }
    sort_for_delivery(&mut chosen);
    chosen.truncate(MAX_ITEMS_PER_SPAWN);
    let mut out = resolve_rules(items, scope);
    out.extend(chosen);
    out
}

/// Expand a prompt template: `{{cwd}}`, `{{name}}`, `{{topic}}`. An unknown
/// placeholder is left verbatim rather than blanked, so a typo is visible in
/// the turn instead of silently deleting text.
#[must_use]
pub fn expand_template(body: &str, cwd: &str, name: &str, topic: &str) -> String {
    body.replace("{{cwd}}", cwd).replace("{{name}}", name).replace("{{topic}}", topic)
}

#[must_use]
pub fn attached_ref(item: &ContextItem) -> AttachedContextRef {
    AttachedContextRef {
        id: item.id.to_string(),
        kind: item.kind.clone(),
        name: item.name.clone(),
        title: item.title.clone(),
        version: item.version,
    }
}

/// The memories as one markdown document, the shape the daemon stages at
/// launch.
#[must_use]
pub fn render_memories(items: &[ContextItem]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("# Session context\n");
    for note in items {
        let _ = write!(out, "\n## {}\n\n{}\n", note.title.trim(), note.body.trim());
    }
    out
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<ContextItem>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM context_items WHERE user_id = $1 ORDER BY kind, lower(title), name"
    )))
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(ContextItem::from).collect())
}

pub async fn get_many(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<ContextItem>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM context_items WHERE id = ANY($1) ORDER BY kind, lower(title), name"
    )))
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(ContextItem::from).collect())
}

pub async fn get_one(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<ContextItem>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM context_items WHERE id = $1 AND user_id = $2"
    )))
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ContextItem::from))
}

pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM context_items WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// `pending` is an agent's proposal awaiting approval; `by_session` records
/// the agent session that wrote the row.
pub async fn insert(
    pool: &PgPool,
    user_id: Uuid,
    spec: &ContextItemSpec,
    pending: Option<&ContextItemSpec>,
    by_session: Option<&str>,
) -> Result<ContextItem, sqlx::Error> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO context_items \
            (user_id, kind, name, title, body, scope, scope_ref, filters, tags, enabled, \
             pending, created_by_session, updated_by_session) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12) RETURNING {COLS}"
    )))
    .bind(user_id)
    .bind(&spec.kind)
    .bind(&spec.name)
    .bind(&spec.title)
    .bind(&spec.body)
    .bind(&spec.scope)
    .bind(spec.scope_ref.as_deref())
    .bind(Jsonb(spec.match_.clone()))
    .bind(&spec.tags)
    .bind(spec.enabled)
    .bind(pending.cloned().map(Jsonb))
    .bind(by_session)
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

/// Replace the item's live spec. `version` advances on every edit so a session
/// card can name the revision it received. A human edit (`by_session` `None`)
/// clears the agent provenance of the last edit.
pub async fn update(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    spec: &ContextItemSpec,
    by_session: Option<&str>,
) -> Result<Option<ContextItem>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE context_items SET kind = $3, name = $4, title = $5, body = $6, scope = $7, \
            scope_ref = $8, filters = $9, tags = $10, enabled = $11, updated_by_session = $12, \
            version = version + 1, updated_at = now() \
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
    .bind(Jsonb(spec.match_.clone()))
    .bind(&spec.tags)
    .bind(spec.enabled)
    .bind(by_session)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ContextItem::from))
}

/// Park an agent's proposed spec beside the live one, which stays untouched.
pub async fn propose(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    spec: &ContextItemSpec,
    by_session: &str,
) -> Result<Option<ContextItem>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE context_items SET pending = $3, updated_by_session = $4, updated_at = now() \
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(user_id)
    .bind(Jsonb(spec.clone()))
    .bind(by_session)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ContextItem::from))
}

/// Promote the pending spec to live (bumping `version`) or drop it. `None`
/// when the item is not the caller's or has nothing pending.
pub async fn settle_pending(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    approve: bool,
) -> Result<Option<ContextItem>, sqlx::Error> {
    let Some(item) = get_one(pool, user_id, id).await? else { return Ok(None) };
    let Some(pending) = item.pending else { return Ok(None) };
    let row: Option<Row> = if approve {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE context_items SET title = $3, body = $4, scope = $5, scope_ref = $6, \
                filters = $7, tags = $8, enabled = $9, pending = NULL, version = version + 1, \
                updated_at = now() \
             WHERE id = $1 AND user_id = $2 AND pending IS NOT NULL RETURNING {COLS}"
        )))
        .bind(id)
        .bind(user_id)
        .bind(&pending.title)
        .bind(&pending.body)
        .bind(&pending.scope)
        .bind(pending.scope_ref.as_deref())
        .bind(Jsonb(pending.match_.clone()))
        .bind(&pending.tags)
        .bind(pending.enabled)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE context_items SET pending = NULL, updated_at = now() \
             WHERE id = $1 AND user_id = $2 AND pending IS NOT NULL RETURNING {COLS}"
        )))
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
    };
    Ok(row.map(ContextItem::from))
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
/// its gateway env with: the memories and prompt it picked, frozen, and the
/// coordinates its rules are re-resolved against at every launch.
/// Best-effort: a failure costs the session its context, never its launch.
pub async fn remember_intent(
    pool: &PgPool,
    spawn_key: &str,
    items: &[ContextItem],
    scope: &StoredScope,
) {
    let ids: Vec<Uuid> = items.iter().filter(|i| i.kind != "rule").map(|i| i.id).collect();
    let done = sqlx::query(
        "INSERT INTO session_context (spawn_key, item_ids, scope) VALUES ($1, $2, $3) \
         ON CONFLICT (spawn_key) DO UPDATE SET item_ids = EXCLUDED.item_ids, \
             scope = EXCLUDED.scope",
    )
    .bind(spawn_key)
    .bind(&ids)
    .bind(Jsonb(scope))
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
        "INSERT INTO session_context (spawn_key, item_ids, scope, rules) \
         SELECT $1, item_ids, scope, rules FROM session_context WHERE spawn_key = $2 \
         ON CONFLICT (spawn_key) DO UPDATE SET item_ids = EXCLUDED.item_ids, \
             scope = EXCLUDED.scope, rules = EXCLUDED.rules",
    )
    .bind(session_id)
    .bind(spawn_key)
    .execute(pool)
    .await;
    if let Err(e) = done {
        tracing::warn!(%session_id, error = %e, "could not re-key the session context set");
    }
}

/// A fork carries on its parent's conversation, so it launches with the
/// parent's memories and re-resolves rules at the parent's coordinates.
pub async fn inherit_intent(pool: &PgPool, child_key: &str, parent_id: &str) {
    claim_intent(pool, child_key, Some(parent_id)).await;
}

type IntentRow = (Vec<Uuid>, Option<Jsonb<StoredScope>>, Jsonb<Vec<AttachedContextRef>>);

async fn intent_of(pool: &PgPool, session_id: &str) -> Result<Option<IntentRow>, sqlx::Error> {
    sqlx::query_as("SELECT item_ids, scope, rules FROM session_context WHERE spawn_key = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await
}

/// The coordinates the session was spawned with, if cctui spawned it.
pub async fn stored_scope(pool: &PgPool, session_id: &str) -> Option<StoredScope> {
    intent_of(pool, session_id).await.ok().flatten().and_then(|(_, scope, _)| scope.map(|s| s.0))
}

/// What one launch of `session_id` carries: its rules, re-resolved now against
/// the owner's current set, then the memories and prompt frozen at spawn. The
/// delivered rules are recorded for the session card.
pub async fn for_launch(pool: &PgPool, session_id: &str) -> Vec<ContextItem> {
    let row = match intent_of(pool, session_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Vec::new(),
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "context intent lookup failed");
            return Vec::new();
        }
    };
    let (ids, scope, _) = row;
    let mut out = Vec::new();
    if let Some(Jsonb(scope)) = scope {
        match list_for_user(pool, scope.user_id).await {
            Ok(items) => {
                let rules = resolve_rules(&items, &scope.coords);
                record_rules(pool, session_id, &rules).await;
                out.extend(rules);
            }
            Err(e) => tracing::warn!(%session_id, error = %e, "rule lookup failed"),
        }
    }
    match get_many(pool, &ids).await {
        Ok(items) => out.extend(items.into_iter().filter(|i| i.kind != "rule")),
        Err(e) => tracing::warn!(%session_id, error = %e, "context item lookup failed"),
    }
    out
}

async fn record_rules(pool: &PgPool, session_id: &str, rules: &[ContextItem]) {
    let refs: Vec<AttachedContextRef> = rules.iter().map(attached_ref).collect();
    let done = sqlx::query("UPDATE session_context SET rules = $2 WHERE spawn_key = $1")
        .bind(session_id)
        .bind(Jsonb(refs))
        .execute(pool)
        .await;
    if let Err(e) = done {
        tracing::warn!(%session_id, error = %e, "could not record the delivered rules");
    }
}

/// Add items to a live session's granted set, so a relaunch restages them.
pub async fn grant(pool: &PgPool, session_id: &str, ids: &[Uuid]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_context (spawn_key, item_ids) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET item_ids = ARRAY( \
             SELECT DISTINCT unnest(session_context.item_ids || EXCLUDED.item_ids))",
    )
    .bind(session_id)
    .bind(ids)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The context a `CctuiAgent` child inherits: the pinned set of the profile it
/// names, by name, the way the tool takes it, plus the rules its coordinates
/// match.
///
/// The same [`resolve_for_spawn`] the webui path uses, so a profile means the
/// same thing from a browser and from the tool. Best-effort throughout: a
/// child launches without context rather than not at all.
pub async fn resolve_for_child(
    pool: &PgPool,
    user_id: Uuid,
    profile_name: Option<&str>,
    scope: &SpawnScope,
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
    // A child inherits what its profile pins, never what its cwd happens to
    // match; its rules still apply, as on every launch.
    resolve_for_spawn(&items, &picks, false, scope)
}

/// Stage `names` (memories of `owner`) into a live session as one file and
/// grant them to it. Unknown names are reported, not fatal.
pub async fn attach_memories(
    state: &AppState,
    session_id: &str,
    owner: Uuid,
    names: &[String],
) -> Result<(Option<String>, AttachContextResponse), AppError> {
    let items = list_for_user(&state.pool, owner).await?;
    let mut picked: Vec<ContextItem> = Vec::new();
    let mut out = AttachContextResponse::default();
    for raw in names {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() || out.attached.contains(&name) || out.unknown.contains(&name) {
            continue;
        }
        match items.iter().find(|i| i.kind == "memory" && i.name == name) {
            Some(item) => {
                out.attached.push(name);
                picked.push(item.clone());
            }
            None => out.unknown.push(name),
        }
    }
    if picked.is_empty() {
        return Ok((None, out));
    }
    let file = cctui_proto::adapter::BootstrapFile {
        name: format!("context-{}.md", Utc::now().format("%Y%m%d%H%M%S")),
        content_b64: base64::engine::general_purpose::STANDARD.encode(render_memories(&picked)),
    };
    let paths =
        crate::bus::stage_files(state, session_id, vec![file]).await.map_err(staging_error)?;
    let ids: Vec<Uuid> = picked.iter().map(|i| i.id).collect();
    grant(&state.pool, session_id, &ids).await?;
    Ok((paths.into_iter().next(), out))
}

fn staging_error(err: crate::bus::BusError) -> AppError {
    use crate::bus::BusError;
    match err {
        BusError::NotFound => AppError::new(StatusCode::NOT_FOUND, "session not found"),
        err @ (BusError::NoDaemon(_) | BusError::Closed) => AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{err} — the session's machine is offline; try again"),
        ),
        BusError::Timeout => AppError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "timed out staging context on the session's machine",
        ),
        err @ BusError::Staging(_) => AppError::new(StatusCode::BAD_GATEWAY, err.to_string()),
        err => {
            tracing::error!(%err, "staging context failed");
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not stage context")
        }
    }
}

/// The title of the "needs you" card an agent's proposal files.
#[must_use]
pub fn approval_title(kind: &str, name: &str) -> String {
    format!("Approve {kind} `{name}`")
}

/// Close the proposal's card on the session that filed it. Best-effort.
async fn close_approval_card(state: &AppState, item: &ContextItem, approved: bool) {
    use cctui_proto::api::{UserActionResolver, UserActionStatus};
    let Some(session_id) = item.updated_by_session.as_deref() else { return };
    let title = approval_title(&item.kind, &item.name);
    let Ok(list) = crate::routes::user_actions::list(&state.pool, session_id).await else {
        return;
    };
    for open in list.iter().filter(|a| a.status.is_open() && a.title == title) {
        let note = if approved { "approved" } else { "rejected" };
        let status = if approved { UserActionStatus::Done } else { UserActionStatus::Dropped };
        if let Err(e) = crate::routes::user_actions::tick(
            state,
            session_id,
            open.id,
            status,
            Some(note),
            UserActionResolver::User,
        )
        .await
        {
            tracing::warn!(%session_id, error = ?e, "could not close the approval card");
        }
    }
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
    let row = insert(&state.pool, ctx.user_id, &spec, None, None).await.map_err(db_err)?;
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
    let row = update(&state.pool, ctx.user_id, id, &spec, None).await.map_err(db_err)?;
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

async fn settle(
    state: &AppState,
    ctx: &AuthContext,
    id: Uuid,
    approve: bool,
) -> Result<Json<ContextItem>, AppError> {
    let before = get_one(&state.pool, ctx.user_id, id).await?;
    let Some(before) = before else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "context item not found"));
    };
    let Some(item) = settle_pending(&state.pool, ctx.user_id, id, approve).await.map_err(db_err)?
    else {
        return Err(AppError::new(StatusCode::CONFLICT, "this item has no pending change"));
    };
    close_approval_card(state, &before, approve).await;
    Ok(Json(item))
}

/// `POST /context/{id}/pending/approve` — make an agent's proposal live.
pub async fn approve_pending(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<ContextItem>, AppError> {
    settle(&state, &ctx, id, true).await
}

/// `POST /context/{id}/pending/reject` — drop an agent's proposal; the live
/// spec is untouched.
pub async fn reject_pending(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<ContextItem>, AppError> {
    settle(&state, &ctx, id, false).await
}

/// Query for `GET /context/resolve`: what a launch into these coordinates
/// would carry. Backs the spawn modal's rule row and pre-checked memories.
#[derive(Debug, serde::Deserialize)]
pub struct ResolveQuery {
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Comma-separated label ids.
    #[serde(default)]
    pub labels: Option<String>,
    #[serde(default)]
    pub adapter: Option<String>,
}

/// `GET /context/resolve`.
pub async fn resolve_items(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<ResolvedContextResponse>, AppError> {
    let items = list_for_user(&state.pool, ctx.user_id).await?;
    let blank = |s: Option<String>| s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let scope = SpawnScope {
        machine_id: blank(q.machine_id),
        working_dir: blank(q.working_dir),
        label_ids: q
            .labels
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        adapter: blank(q.adapter),
    };
    Ok(Json(ResolvedContextResponse {
        rules: resolve_rules(&items, &scope),
        memories: resolve_auto(&items, &scope),
    }))
}

async fn require_session_owner(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<Uuid, AppError> {
    match crate::authz::session_owner(session_id, &state.pool).await? {
        Some(owner) if owner == ctx.user_id => Ok(owner),
        Some(_) => Err(AppError::new(StatusCode::FORBIDDEN, "not your session")),
        None => Err(AppError::new(StatusCode::NOT_FOUND, "no such session")),
    }
}

/// `GET /sessions/{id}/context` — the rules the latest launch delivered and
/// the memories the session holds.
pub async fn session_context(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionContextResponse>, AppError> {
    let Some((ids, _, Jsonb(rules))) = intent_of(&state.pool, &session_id).await? else {
        return Ok(Json(SessionContextResponse::default()));
    };
    let memories = get_many(&state.pool, &ids)
        .await?
        .iter()
        .filter(|i| i.kind == "memory")
        .map(attached_ref)
        .collect();
    Ok(Json(SessionContextResponse { rules, memories }))
}

/// `POST /sessions/{id}/context` — stage memories into a live session and tell
/// it, in one turn, where they are.
pub async fn attach_to_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<AttachContextRequest>,
) -> Result<Json<AttachContextResponse>, AppError> {
    let owner = require_session_owner(&state, &ctx, &session_id).await?;
    if req.names.iter().all(|n| n.trim().is_empty()) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "names is required"));
    }
    let (path, out) = attach_memories(&state, &session_id, owner, &req.names).await?;
    if let Some(path) = path {
        announce_attached(&state, &session_id, &out.attached, &path).await;
    }
    Ok(Json(out))
}

/// The one turn naming the staged file. A failure leaves the file staged and
/// granted, so it is logged rather than unwinding the attach.
async fn announce_attached(state: &AppState, session_id: &str, names: &[String], path: &str) {
    let text = format!(
        "[cctui] context attached: {}. Read `{path}` now and follow it for the rest of this \
         session.",
        names.join(", ")
    );
    let env = crate::routes::gateway::resume_env_for_session(state, session_id).await;
    let command_id = Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.to_owned()),
        None,
    );
    let sent = crate::bus::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.to_owned(),
            recover_auth: false,
            text,
            ask_picks: None,
            env,
            command_id: Some(command_id),
            turn_id: None,
        },
    )
    .await;
    if let Err(err) = sent {
        tracing::warn!(%session_id, %err, "context staged but the announcing turn was not sent");
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

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
            match_: match_of(scope, scope_ref),
            tags: Vec::new(),
            enabled: true,
            version: 1,
            pending: None,
            created_by_session: None,
            updated_by_session: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn rule(name: &str, m: ContextMatch) -> ContextItem {
        let mut r = item("rule", name, "user", None);
        r.match_ = m;
        r
    }

    fn names(items: &[ContextItem]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    fn at(adapter: &str, dir: &str) -> SpawnScope {
        SpawnScope {
            machine_id: Some("m-1".into()),
            working_dir: Some(dir.into()),
            label_ids: Vec::new(),
            adapter: Some(adapter.into()),
        }
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
            adapter: None,
        };
        assert_eq!(
            names(&resolve_auto(&items, &scope)),
            ["always", "in-repo", "tagged", "this-box"],
            "one per scope kind, in title order and never in row order"
        );
    }

    #[test]
    fn filter_keys_are_anded_values_ored_and_empty_means_any() {
        let m = ContextMatch {
            machines: vec!["m-1".into(), "m-2".into()],
            paths: vec!["/w/repo".into()],
            labels: Vec::new(),
            adapters: Vec::new(),
        };
        assert!(filter_matches(&m, &at("codex", "/w/repo")));
        let other_box = SpawnScope { machine_id: Some("m-2".into()), ..at("codex", "/w/repo/x") };
        assert!(filter_matches(&m, &other_box), "either machine");
        let third_box = SpawnScope { machine_id: Some("m-3".into()), ..at("codex", "/w/repo") };
        assert!(!filter_matches(&m, &third_box), "machine AND path");
        assert!(!filter_matches(&m, &at("codex", "/w/elsewhere")));
        assert!(filter_matches(&ContextMatch::default(), &SpawnScope::default()));

        let labelled = ContextMatch { labels: vec!["lab-1".into()], ..ContextMatch::default() };
        assert!(!filter_matches(&labelled, &SpawnScope::default()), "a missing coordinate");
        let with_label = SpawnScope { label_ids: vec!["lab-1".into()], ..SpawnScope::default() };
        assert!(filter_matches(&labelled, &with_label));
    }

    #[test]
    fn a_path_and_adapter_rule_attaches_only_to_that_adapter_under_that_path() {
        let items = vec![rule(
            "codex-repo",
            ContextMatch {
                paths: vec!["/w/repo".into()],
                adapters: vec!["codex".into()],
                ..ContextMatch::default()
            },
        )];
        assert_eq!(names(&resolve_rules(&items, &at("codex", "/w/repo/sub"))), ["codex-repo"]);
        assert!(resolve_rules(&items, &at("claude-code", "/w/repo/sub")).is_empty());
        assert!(resolve_rules(&items, &at("codex", "/w/repo2")).is_empty());
        assert!(
            resolve_rules(&items, &SpawnScope { adapter: None, ..at("codex", "/w/repo") })
                .is_empty(),
            "an adapter filter never matches a launch with no adapter"
        );
    }

    #[test]
    fn a_disabled_item_is_never_resolved() {
        let mut off = item("memory", "off", "user", None);
        off.enabled = false;
        assert!(resolve_auto(&[off], &SpawnScope::default()).is_empty());

        let mut off_rule = rule("off-rule", ContextMatch::default());
        off_rule.enabled = false;
        assert!(resolve_rules(std::slice::from_ref(&off_rule), &at("codex", "/w")).is_empty());
        assert!(
            resolve_for_spawn(&[off_rule], &["off-rule".to_owned()], true, &at("codex", "/w"))
                .is_empty(),
            "neither auto nor a pick brings back a disabled rule"
        );
    }

    /// An agent's new rule is created disabled with its spec parked in
    /// `pending`, so it reaches no launch until the owner approves it.
    #[test]
    fn a_pending_rule_is_not_resolved_until_approved() {
        let mut proposed = rule("proposed", ContextMatch::default());
        proposed.enabled = false;
        proposed.body = String::new();
        proposed.pending = Some(ContextItemSpec {
            kind: "rule".into(),
            name: "proposed".into(),
            title: "proposed".into(),
            body: "do the thing".into(),
            enabled: true,
            ..ContextItemSpec::default()
        });
        assert!(resolve_rules(&[proposed.clone()], &at("codex", "/w")).is_empty());

        let pending = proposed.pending.take().expect("pending");
        proposed.body = pending.body;
        proposed.enabled = pending.enabled;
        assert_eq!(names(&resolve_rules(&[proposed], &at("codex", "/w"))), ["proposed"]);
    }

    #[test]
    fn rules_attach_whatever_auto_says_and_come_first() {
        let items = vec![
            item("memory", "note", "user", None),
            rule("zeta", ContextMatch::default()),
            rule("alpha", ContextMatch::default()),
            item("prompt", "reviewer", "user", None),
        ];
        let scope = at("claude-code", "/w");
        assert_eq!(names(&resolve_for_spawn(&items, &[], false, &scope)), ["alpha", "zeta"]);
        assert_eq!(
            names(&resolve_for_spawn(&items, &["reviewer".to_owned()], true, &scope)),
            ["alpha", "zeta", "note", "reviewer"],
            "rules, then memories, then the prompt"
        );
        assert_eq!(
            names(&resolve_for_spawn(&items, &["alpha".to_owned()], false, &scope)),
            ["alpha", "zeta"],
            "naming a rule never duplicates it"
        );
    }

    #[test]
    fn rules_past_the_launch_cap_are_dropped_from_the_tail() {
        let big = |name: &str| {
            let mut r = rule(name, ContextMatch::default());
            r.body = "x".repeat(MAX_RULE_BODY);
            r
        };
        let items: Vec<ContextItem> = ["a", "b", "c", "d", "e"].into_iter().map(big).collect();
        let kept = resolve_rules(&items, &at("codex", "/w"));
        assert_eq!(names(&kept), ["a", "b", "c", "d"], "16KiB holds four 4KiB rules");

        let mut small = rule("f", ContextMatch::default());
        small.body = "y".into();
        let mut items = items;
        items.push(small);
        assert_eq!(
            names(&resolve_rules(&items, &at("codex", "/w"))),
            ["a", "b", "c", "d"],
            "the cut is the tail in order, not a best fit"
        );
        let mut reversed = items.clone();
        reversed.reverse();
        assert_eq!(
            names(&resolve_rules(&reversed, &at("codex", "/w"))),
            ["a", "b", "c", "d"],
            "row order never changes the cut"
        );
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
    /// the profile's pinned names and the same coordinates, adapter included,
    /// so a webui spawn and a `CctuiAgent` child resolve the same kit — rules
    /// too. Both key the machine filter on the machine's uuid, never on the
    /// name a request happened to use.
    #[test]
    fn a_profile_resolves_the_same_kit_from_the_webui_and_from_the_tool() {
        let machine = Uuid::new_v4().to_string();
        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "this-box", "machine", Some(&machine)),
            item("memory", "in-repo", "path", Some("/w/repo")),
            item("memory", "pinned", "machine", Some("somewhere-else")),
            item("prompt", "reviewer", "user", None),
            rule("everywhere", ContextMatch::default()),
            rule(
                "codex-here",
                ContextMatch {
                    machines: vec![machine.clone()],
                    adapters: vec!["codex".into()],
                    ..ContextMatch::default()
                },
            ),
            rule(
                "claude-only",
                ContextMatch { adapters: vec!["claude-code".into()], ..ContextMatch::default() },
            ),
        ];
        let pinned = ["pinned".to_owned(), "reviewer".to_owned()];
        let scope = SpawnScope {
            machine_id: Some(machine),
            working_dir: Some("/w/repo/crates".into()),
            label_ids: Vec::new(),
            adapter: Some("codex".into()),
        };

        let from_webui = resolve_for_spawn(&items, &pinned, false, &scope);
        let from_tool = resolve_for_spawn(&items, &pinned, false, &scope);
        assert_eq!(names(&from_webui), names(&from_tool));
        assert_eq!(
            names(&from_webui),
            ["codex-here", "everywhere", "pinned", "reviewer"],
            "the matching rules plus the pins; a memory the profile did not pin stays out"
        );

        assert_eq!(
            names(&resolve_for_spawn(&items, &pinned, true, &scope)),
            ["codex-here", "everywhere", "always", "in-repo", "pinned", "this-box", "reviewer"],
        );

        let elsewhere = SpawnScope { machine_id: Some(Uuid::new_v4().to_string()), ..scope };
        let got = resolve_for_spawn(&items, &[], true, &elsewhere);
        assert!(!names(&got).contains(&"this-box"));
        assert!(!names(&got).contains(&"codex-here"));
    }

    /// A body that says nothing about context decodes to no picks and no
    /// auto; only the rules, which need no consent per spawn, still attach.
    #[test]
    fn a_spawn_that_asks_for_nothing_gets_only_its_rules() {
        let asked = cctui_proto::api::SpawnContext::default();
        assert!(!asked.auto);
        assert!(asked.items.is_empty());

        let items = vec![
            item("memory", "always", "user", None),
            item("memory", "in-repo", "path", Some("/w/repo")),
            rule("house", ContextMatch::default()),
        ];
        let scope = at("claude-code", "/w/repo");
        assert_eq!(
            names(&resolve_for_spawn(&items, &asked.items, asked.auto, &scope)),
            ["house"],
            "a matching memory is not consent; a rule is"
        );
        let decoded: cctui_proto::api::SpawnContext =
            serde_json::from_str(r#"{"items":["always"]}"#).expect("decodes");
        assert!(!decoded.auto);
        assert_eq!(
            names(&resolve_for_spawn(&items, &decoded.items, decoded.auto, &scope)),
            ["house", "always"]
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
            match_: ContextMatch::default(),
            tags: vec![" Rust ".to_owned(), "rust".to_owned(), String::new()],
            enabled: true,
        }
    }

    #[test]
    fn a_spec_is_normalized_and_validated() {
        let ok = clean_spec(spec("memory", "path", Some("/w/repo/"))).expect("valid");
        assert_eq!(ok.title, "House style");
        assert_eq!(ok.scope_ref.as_deref(), Some("/w/repo"), "trailing slash normalized away");
        assert_eq!(ok.match_.paths, ["/w/repo"], "a legacy scope fills the filter");
        assert_eq!(ok.tags, ["rust"], "tags lowercased, deduped, blanks dropped");

        let user_scoped = clean_spec(spec("memory", "user", Some("ignored"))).expect("valid");
        assert!(user_scoped.scope_ref.is_none(), "user scope carries no target");
        assert!(user_scoped.match_.is_empty());

        assert!(clean_spec(spec("skill", "user", None)).is_err(), "skills are plugins");
        assert!(clean_spec(spec("rule", "user", None)).is_ok());
        assert!(clean_spec(spec("memory", "nonsense", None)).is_err());
        assert_eq!(clean_spec(spec("memory", " ", None)).expect("blank is user").scope, "user");
        assert!(clean_spec(spec("memory", "machine", None)).is_err(), "needs a target");
        assert!(clean_spec(spec("memory", "path", Some("relative"))).is_err());

        let mut huge = spec("memory", "user", None);
        huge.body = "x".repeat(MAX_BODY + 1);
        assert!(clean_spec(huge).is_err());

        let mut blank = spec("memory", "user", None);
        blank.title = "   ".to_owned();
        assert!(clean_spec(blank).is_err());
    }

    #[test]
    fn a_rule_body_is_capped_at_4kib() {
        let mut at_cap = spec("rule", "user", None);
        at_cap.body = "x".repeat(MAX_RULE_BODY);
        assert!(clean_spec(at_cap).is_ok());

        let mut over = spec("rule", "user", None);
        over.body = "x".repeat(MAX_RULE_BODY + 1);
        let err = clean_spec(over).expect_err("over the rule cap");
        assert!(format!("{err:?}").contains("4KiB"), "{err:?}");

        let mut memory = spec("memory", "user", None);
        memory.body = "x".repeat(MAX_RULE_BODY + 1);
        assert!(clean_spec(memory).is_ok(), "the rule cap is for rules only");
    }

    #[test]
    fn a_match_wins_over_the_legacy_scope_and_keeps_it_in_step() {
        let mut s = spec("rule", "machine", Some("m-ignored"));
        s.match_ = ContextMatch {
            paths: vec!["/w/repo/".into(), " /w/repo".into()],
            adapters: vec![" Codex ".into()],
            ..ContextMatch::default()
        };
        let ok = clean_spec(s).expect("valid");
        assert_eq!(ok.match_.paths, ["/w/repo"], "normalized and deduped");
        assert_eq!(ok.match_.adapters, ["codex"]);
        assert_eq!((ok.scope.as_str(), ok.scope_ref), ("user", None), "two keys are no scope");

        let mut one = spec("memory", "user", None);
        one.match_ = ContextMatch { machines: vec!["m-1".into()], ..ContextMatch::default() };
        let ok = clean_spec(one).expect("valid");
        assert_eq!((ok.scope.as_str(), ok.scope_ref.as_deref()), ("machine", Some("m-1")));

        let mut relative = spec("memory", "user", None);
        relative.match_ = ContextMatch { paths: vec!["w/repo".into()], ..ContextMatch::default() };
        assert!(clean_spec(relative).is_err());

        let mut root = spec("memory", "user", None);
        root.match_ = ContextMatch { paths: vec!["/".into()], ..ContextMatch::default() };
        assert_eq!(clean_spec(root).expect("valid").match_.paths, ["/"]);
    }

    #[test]
    fn a_stored_scope_round_trips_through_its_column() {
        let stored = StoredScope {
            user_id: Uuid::new_v4(),
            coords: at("codex", "/w/repo"),
            dispatched: true,
        };
        let json = serde_json::to_value(&stored).expect("encodes");
        assert_eq!(json["adapter"], "codex", "coordinates are flat: {json}");
        let back: StoredScope = serde_json::from_value(json).expect("decodes");
        assert_eq!(back, stored);
        let legacy: StoredScope =
            serde_json::from_value(serde_json::json!({ "user_id": Uuid::nil() })).expect("decodes");
        assert!(!legacy.dispatched);
        assert_eq!(legacy.coords, SpawnScope::default());
    }

    #[test]
    fn attached_memories_render_as_one_titled_document() {
        let doc = render_memories(&[item("memory", "ops", "user", None)]);
        assert!(doc.starts_with("# Session context\n"), "{doc}");
        assert!(doc.contains("## ops\n\nbody of ops\n"), "{doc}");
    }

    async fn test_pool(tag: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed_user(pool: &PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("ctx-{uid}"))
            .bind(format!("hctx-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    fn rule_spec(name: &str, body: &str, m: ContextMatch) -> ContextItemSpec {
        clean_spec(ContextItemSpec {
            kind: "rule".into(),
            name: name.into(),
            title: name.into(),
            body: body.into(),
            match_: m,
            ..ContextItemSpec::default()
        })
        .expect("valid rule")
    }

    /// Rules are re-resolved at every launch against the owner's current set,
    /// so an edit reaches the next launch of a running session and a resume
    /// under the re-keyed id; memories stay as the spawn froze them.
    #[tokio::test]
    async fn every_launch_re_resolves_rules_and_records_what_it_delivered() {
        let Some(pool) = test_pool("context_rules_per_launch").await else { return };
        let uid = seed_user(&pool).await;
        let codex = ContextMatch { adapters: vec!["codex".into()], ..ContextMatch::default() };
        let r = insert(&pool, uid, &rule_spec("house", "v1 body", codex), None, None)
            .await
            .expect("rule");
        let memory = clean_spec(ContextItemSpec {
            kind: "memory".into(),
            name: "note".into(),
            title: "Note".into(),
            body: "remember".into(),
            ..ContextItemSpec::default()
        })
        .expect("valid memory");
        let m = insert(&pool, uid, &memory, None, None).await.expect("memory");

        let key = format!("ctx-key-{}", Uuid::new_v4());
        let stored = StoredScope { user_id: uid, coords: at("codex", "/w"), dispatched: false };
        remember_intent(&pool, &key, &[r.clone(), m.clone()], &stored).await;

        let first = for_launch(&pool, &key).await;
        assert_eq!(names(&first), ["house", "note"]);
        assert_eq!(first[0].body, "v1 body");

        let mut edited = spec_of(&r);
        edited.body = "v2 body".into();
        update(&pool, uid, r.id, &edited, None).await.expect("edit").expect("found");

        let session = format!("ctx-session-{}", Uuid::new_v4());
        claim_intent(&pool, &session, Some(&key)).await;
        let resumed = for_launch(&pool, &session).await;
        assert_eq!(names(&resumed), ["house", "note"], "the resume keeps the kit");
        assert_eq!(resumed[0].body, "v2 body", "the edit reaches the next launch");

        let Some((_, _, Jsonb(rules))) = intent_of(&pool, &session).await.expect("row") else {
            panic!("the re-keyed row exists");
        };
        assert_eq!(rules.len(), 1);
        assert_eq!((rules[0].name.as_str(), rules[0].version), ("house", 2));

        let claude = StoredScope { coords: at("claude-code", "/w"), ..stored };
        let other = format!("ctx-key-{}", Uuid::new_v4());
        remember_intent(&pool, &other, &[], &claude).await;
        assert!(for_launch(&pool, &other).await.is_empty(), "a codex rule skips claude");

        sqlx::query("DELETE FROM session_context WHERE spawn_key = ANY($1)")
            .bind(vec![key, session, other])
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[tokio::test]
    async fn a_proposal_leaves_the_live_rule_until_approved() {
        let Some(pool) = test_pool("context_rule_proposal").await else { return };
        let uid = seed_user(&pool).await;
        let live = rule_spec("house", "live", ContextMatch::default());
        let r = insert(&pool, uid, &live, None, None).await.expect("rule");

        let mut proposal = live.clone();
        proposal.body = "proposed".into();
        let parked =
            propose(&pool, uid, r.id, &proposal, "agent-1").await.expect("propose").expect("row");
        assert_eq!(parked.body, "live");
        assert_eq!(parked.version, r.version, "a proposal is not an edit");
        assert_eq!(parked.updated_by_session.as_deref(), Some("agent-1"));
        assert_eq!(parked.pending.as_ref().map(|p| p.body.as_str()), Some("proposed"));

        let approved = settle_pending(&pool, uid, r.id, true).await.expect("approve").expect("row");
        assert_eq!(approved.body, "proposed");
        assert_eq!(approved.version, r.version + 1);
        assert!(approved.pending.is_none());
        assert!(settle_pending(&pool, uid, r.id, true).await.expect("again").is_none());

        propose(&pool, uid, r.id, &live, "agent-1").await.expect("propose").expect("row");
        let rejected = settle_pending(&pool, uid, r.id, false).await.expect("reject").expect("row");
        assert_eq!(rejected.body, "proposed", "a rejection leaves the live body");
        assert!(rejected.pending.is_none());

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// Migration 174 runs against temp tables that shadow `context_items`,
    /// `session_context` and `context_tool_saves` on this connection, shaped as
    /// before 174.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // one migration replayed up and down
    async fn migration_174_backfills_match_and_its_down_restores_the_old_shape() {
        let Some(pool) = test_pool("migration_174").await else { return };
        let mut conn = pool.acquire().await.expect("acquire");
        sqlx::raw_sql(
            "CREATE TEMP TABLE context_items ( \
                 id UUID PRIMARY KEY DEFAULT gen_random_uuid(), user_id UUID NOT NULL, \
                 kind TEXT NOT NULL, name TEXT NOT NULL, title TEXT NOT NULL, \
                 body TEXT NOT NULL DEFAULT '', scope TEXT NOT NULL DEFAULT 'user', \
                 scope_ref TEXT, tags TEXT[] NOT NULL DEFAULT '{}', \
                 enabled BOOLEAN NOT NULL DEFAULT TRUE, version INTEGER NOT NULL DEFAULT 1, \
                 created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                 updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                 UNIQUE (user_id, kind, name), \
                 CHECK (kind IN ('memory', 'prompt')), \
                 CHECK (scope IN ('user', 'machine', 'path', 'label')), \
                 CHECK ((scope = 'user') = (scope_ref IS NULL))); \
             CREATE TEMP TABLE session_context ( \
                 spawn_key TEXT PRIMARY KEY, item_ids UUID[] NOT NULL DEFAULT '{}', \
                 created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
             CREATE TEMP TABLE context_tool_saves ( \
                 id BIGSERIAL PRIMARY KEY, session_id TEXT NOT NULL, \
                 created_at TIMESTAMPTZ NOT NULL DEFAULT now());",
        )
        .execute(&mut *conn)
        .await
        .expect("shadow tables");

        let uid = Uuid::new_v4();
        for (name, scope, scope_ref) in [
            ("u", "user", None),
            ("m", "machine", Some("m-1")),
            ("p", "path", Some("/w/repo")),
            ("l", "label", Some("lab-1")),
        ] {
            sqlx::query(
                "INSERT INTO context_items (user_id, kind, name, title, scope, scope_ref) \
                 VALUES ($1, 'memory', $2, $2, $3, $4)",
            )
            .bind(uid)
            .bind(name)
            .bind(scope)
            .bind(scope_ref)
            .execute(&mut *conn)
            .await
            .expect("seed");
        }

        let up = include_str!("../../../../migrations/174_context_rules.up.sql");
        let down = include_str!("../../../../migrations/174_context_rules.down.sql");
        sqlx::raw_sql(up).execute(&mut *conn).await.expect("174 up");

        let filters: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT name, filters FROM context_items ORDER BY name")
                .fetch_all(&mut *conn)
                .await
                .expect("filters");
        let by = |n: &str| filters.iter().find(|(name, _)| name == n).expect("row").1.clone();
        assert_eq!(by("u"), serde_json::json!({}));
        assert_eq!(by("m"), serde_json::json!({ "machines": ["m-1"] }));
        assert_eq!(by("p"), serde_json::json!({ "paths": ["/w/repo"] }));
        assert_eq!(by("l"), serde_json::json!({ "labels": ["lab-1"] }));

        sqlx::query(
            "INSERT INTO context_items (user_id, kind, name, title) VALUES ($1, 'rule', 'r', 'R')",
        )
        .bind(uid)
        .execute(&mut *conn)
        .await
        .expect("a rule fits after 174");
        sqlx::query("INSERT INTO session_context (spawn_key, scope) VALUES ('k', '{}')")
            .execute(&mut *conn)
            .await
            .expect("session_context gained scope");

        sqlx::raw_sql(down).execute(&mut *conn).await.expect("174 down");
        let kinds: Vec<String> = sqlx::query_scalar("SELECT DISTINCT kind FROM context_items")
            .fetch_all(&mut *conn)
            .await
            .expect("kinds");
        assert_eq!(kinds, ["memory"], "rules do not survive the down");
        let scopes: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT scope, scope_ref FROM context_items WHERE name = 'p'")
                .fetch_all(&mut *conn)
                .await
                .expect("scope readable");
        assert_eq!(scopes, [("path".to_owned(), Some("/w/repo".to_owned()))]);
        let has_filters: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
              WHERE table_schema LIKE 'pg_temp%' AND table_name = 'context_items' \
                AND column_name = 'filters')",
        )
        .fetch_one(&mut *conn)
        .await
        .expect("columns");
        assert!(!has_filters);
        let rule_after_down = sqlx::query(
            "INSERT INTO context_items (user_id, kind, name, title) VALUES ($1, 'rule', 'r2', 'R')",
        )
        .bind(uid)
        .execute(&mut *conn)
        .await;
        assert!(rule_after_down.is_err(), "the kind check is restored");
    }
}
