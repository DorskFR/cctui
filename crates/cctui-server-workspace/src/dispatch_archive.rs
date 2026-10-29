//! Archives a dispatched session once its run ends. A session is dispatched
//! when it, or an ancestor it nests under (a `CctuiAgent` child), has a
//! dispatch record or handle.
//!
//! `daemon_lost` / `machine_offline` ends are left to the idle-TTL sweep: the
//! dispatch liveness sweep may still rewrite them, and the worker may
//! reconnect.

use std::collections::HashMap;

use cctui_proto::classifier::Bucket;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::live_sessions::live_sessions_predicate;
use crate::routes::sessions::ArchiveOutcome;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

pub const ENABLED_KEY: &str = "dispatchAutoArchive";
pub const GRACE_KEY: &str = "dispatchAutoArchiveGraceSecs";
pub const KEEP_PROFILES_KEY: &str = "dispatchAutoArchiveKeepProfiles";
pub const DEFAULT_GRACE_SECS: i64 = 180;
pub const MAX_GRACE_SECS: i64 = 3600;
const MAX_KEEP_PROFILES: usize = 64;
const MAX_PROFILE_LEN: usize = 128;

const TRANSPORT_ENDS: &[&str] = &["daemon_lost", "machine_offline"];
const TERMINAL: &[SessionRowStatus] = &[SessionRowStatus::Ended, SessionRowStatus::Failed];
/// Older ends predate the feature, or were un-archived and kept on purpose.
const LOOKBACK_HOURS: i32 = 24;
const MAX_ANCESTRY: i32 = 32;
const BATCH: i64 = 100;

/// The owner's setting, read from their `user_settings` blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub enabled: bool,
    pub grace_secs: i64,
    pub keep_profiles: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { enabled: true, grace_secs: DEFAULT_GRACE_SECS, keep_profiles: Vec::new() }
    }
}

fn grace_of(v: &Value) -> Option<i64> {
    v.as_i64().map(|secs| secs.clamp(0, MAX_GRACE_SECS))
}

fn profiles_of(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_PROFILE_LEN || out.iter().any(|p| p == name) {
            continue;
        }
        out.push(name.to_owned());
        if out.len() == MAX_KEEP_PROFILES {
            break;
        }
    }
    out
}

impl Policy {
    #[must_use]
    pub fn from_settings(data: Option<&Value>) -> Self {
        let defaults = Self::default();
        let Some(data) = data else { return defaults };
        Self {
            enabled: data.get(ENABLED_KEY).and_then(Value::as_bool).unwrap_or(defaults.enabled),
            grace_secs: data.get(GRACE_KEY).and_then(grace_of).unwrap_or(defaults.grace_secs),
            keep_profiles: data.get(KEEP_PROFILES_KEY).map(profiles_of).unwrap_or_default(),
        }
    }

    fn covers(&self, profile: Option<&str>) -> bool {
        self.enabled && profile.is_none_or(|p| !self.keep_profiles.iter().any(|k| k == p))
    }

    fn due(&self, ended_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        now >= ended_at + chrono::Duration::seconds(self.grace_secs)
    }
}

/// Normalize the setting keys in a settings blob being saved: a wrong-typed
/// value is dropped so the default applies.
pub fn clamp(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if obj.get(ENABLED_KEY).is_some_and(|v| !v.is_boolean()) {
        obj.remove(ENABLED_KEY);
    }
    match obj.get(GRACE_KEY).map(grace_of) {
        Some(Some(secs)) => {
            obj.insert(GRACE_KEY.to_owned(), Value::from(secs));
        }
        Some(None) => {
            obj.remove(GRACE_KEY);
        }
        None => {}
    }
    if let Some(v) = obj.get(KEEP_PROFILES_KEY) {
        let profiles = profiles_of(v);
        if profiles.is_empty() {
            obj.remove(KEEP_PROFILES_KEY);
        } else {
            obj.insert(KEEP_PROFILES_KEY.to_owned(), Value::from(profiles));
        }
    }
}

/// A finished turn that left a question, a plan or a permission open.
fn waits_on_user(tempo: Option<&str>, agent_state: Option<&str>, activity: Option<&str>) -> bool {
    crate::routes::sessions::bucket_from_signals(
        tempo,
        agent_state,
        activity,
        None,
        &[],
        &HashMap::new(),
    ) == Bucket::Blocked
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: String,
    profile: Option<String>,
    ended_at: DateTime<Utc>,
    tempo: Option<String>,
    agent_state: Option<String>,
    activity: Option<String>,
    settings: Option<Value>,
}

/// Ended, unpinned, never auto-archived sessions whose owner has the setting
/// on, each with the profile of the nearest dispatch at or above it.
const CANDIDATES_SQL: &str = concat!(
    "WITH RECURSIVE cand AS ( \
         SELECT s.id, s.parent_id, s.metadata->>'relation' AS relation \
           FROM sessions s \
           LEFT JOIN user_settings us ON us.user_id = s.user_id \
          WHERE ",
    live_sessions_predicate!("s"),
    "   AND s.status = ANY($1) AND s.pinned = false \
            AND COALESCE(s.end_reason, '') <> ALL($2) \
            AND COALESCE(s.ended_at, s.last_heartbeat) > now() - make_interval(hours => $3) \
            AND COALESCE(s.metadata->>'dispatch_auto_archived', '') <> 'true' \
            AND COALESCE(us.data->'dispatchAutoArchive', 'true'::jsonb) <> 'false'::jsonb \
            AND ($4::uuid IS NULL OR s.user_id = $4) \
     ), up AS ( \
         SELECT id AS leaf, id AS node, parent_id, relation, 0 AS depth FROM cand \
         UNION ALL \
         SELECT up.leaf, p.id, p.parent_id, p.metadata->>'relation', up.depth + 1 \
           FROM up JOIN sessions p ON p.id = up.parent_id \
          WHERE COALESCE(up.relation, '') NOT IN ('fork', 'followup') AND up.depth < $5 \
     ), origin AS ( \
         SELECT DISTINCT ON (up.leaf) up.leaf, r.profile \
           FROM up LEFT JOIN dispatch_records r ON r.session_id = up.node \
          WHERE r.session_id IS NOT NULL \
             OR EXISTS (SELECT 1 FROM dispatch_handles h WHERE h.session_id = up.node) \
          ORDER BY up.leaf, up.depth \
     ) \
     SELECT s.id, o.profile, COALESCE(s.ended_at, s.last_heartbeat) AS ended_at, \
            s.tempo, s.agent_state, s.activity, us.data AS settings \
       FROM origin o JOIN sessions s ON s.id = o.leaf \
       LEFT JOIN user_settings us ON us.user_id = s.user_id \
      ORDER BY ended_at \
      LIMIT $6"
);

async fn candidates(state: &AppState, owner: Option<Uuid>) -> sqlx::Result<Vec<Candidate>> {
    sqlx::query_as(CANDIDATES_SQL)
        .bind(SessionRowStatus::names(TERMINAL))
        .bind(TRANSPORT_ENDS)
        .bind(LOOKBACK_HOURS)
        .bind(owner)
        .bind(MAX_ANCESTRY)
        .bind(BATCH)
        .fetch_all(&state.pool)
        .await
}

async fn prompting(state: &AppState, tree: &[String]) -> bool {
    let prompts = state.permission_store.read().await;
    tree.iter().any(|s| prompts.pending_ask(s).is_some() || prompts.pending_plan(s).is_some())
        || prompts.list_pending().iter().any(|p| tree.contains(&p.session_id))
}

/// Whether the session or anything nested under it is open in a client,
/// carries an open user action, or holds a prompt on this replica.
async fn held(state: &AppState, id: &str) -> sqlx::Result<bool> {
    let mut tree = vec![id.to_owned()];
    tree.extend(
        crate::store::sessions::descendants(&state.pool, id).await?.into_iter().map(|c| c.id),
    );
    if prompting(state, &tree).await {
        return Ok(true);
    }
    let open_action: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_user_actions \
                        WHERE session_id = ANY($1) AND status = 'open')",
    )
    .bind(&tree)
    .fetch_one(&state.pool)
    .await?;
    if open_action {
        return Ok(true);
    }
    crate::session_viewers::any_viewed(&state.pool, &tree).await
}

/// Marks the session as handled; `false` when another replica got there first
/// or it was pinned or archived meanwhile.
async fn claim(state: &AppState, id: &str) -> sqlx::Result<bool> {
    let claimed = sqlx::query(
        "UPDATE sessions \
         SET metadata = COALESCE(metadata, '{}'::jsonb) \
                        || '{\"dispatch_auto_archived\": true}'::jsonb \
         WHERE id = $1 AND pinned = false AND status <> 'archived' \
           AND COALESCE(metadata->>'dispatch_auto_archived', '') <> 'true'",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(claimed.rows_affected() > 0)
}

async fn unclaim(state: &AppState, id: &str) {
    if let Err(err) = sqlx::query(
        "UPDATE sessions SET metadata = metadata - 'dispatch_auto_archived' \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%err, session_id = %id, "dispatch auto-archive flag not cleared");
    }
}

async fn archive(state: &AppState, id: &str) {
    match claim(state, id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(err) => {
            tracing::warn!(%err, session_id = %id, "dispatch auto-archive claim failed");
            return;
        }
    }
    match crate::routes::sessions::archive_one(
        state,
        id,
        false,
        cctui_proto::adapter::RemoveInitiator::Automatic,
        crate::events::Actor::Reaper,
    )
    .await
    {
        Ok(ArchiveOutcome::Archived) => {
            tracing::info!(session_id = %id, "auto-archived a finished dispatched session");
        }
        Ok(ArchiveOutcome::SkippedPinned) => {
            tracing::info!(session_id = %id, "dispatch auto-archive skipped: session is pinned");
        }
        Err(err) => {
            tracing::warn!(%err, session_id = %id, "dispatch auto-archive failed");
            unclaim(state, id).await;
        }
    }
}

async fn run(state: &AppState, owner: Option<Uuid>) {
    let rows = match candidates(state, owner).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "dispatch auto-archive sweep query failed");
            return;
        }
    };
    let now = Utc::now();
    for row in rows {
        let policy = Policy::from_settings(row.settings.as_ref());
        let (tempo, agent_state) = (row.tempo.as_deref(), row.agent_state.as_deref());
        let blocked = waits_on_user(tempo, agent_state, row.activity.as_deref());
        if blocked || !policy.covers(row.profile.as_deref()) || !policy.due(row.ended_at, now) {
            continue;
        }
        match held(state, &row.id).await {
            Ok(false) => archive(state, &row.id).await,
            Ok(true) => {}
            Err(err) => {
                tracing::warn!(%err, session_id = %row.id, "dispatch auto-archive check failed");
            }
        }
    }
}

/// Reaper tick.
pub async fn sweep(state: &AppState) {
    run(state, None).await;
}

#[cfg(test)]
mod tests;
