//! What each dispatch asked for, kept so a worker that never registers can be identified and
//! re-run.
//!
//! Env values, callback URLs, the callback secret and the worker key are never stored; the
//! prompt is kept to its first 2 KiB.

use std::sync::LazyLock;

use cctui_crypto::redact::CompiledPatterns;
use cctui_proto::api::{
    DispatchRequest, FailedDispatch, FailedDispatchReason, RerunBlocker, WorkerExit,
};
use sqlx::PgExecutor;
use sqlx::types::Json;
use uuid::Uuid;

pub const PROMPT_HEAD_BYTES: usize = 2048;
pub const REPLAY_MAX_BYTES: usize = 16 * 1024;
pub const REGISTER_TIMEOUT_MINUTES: i32 = 15;
const WORKER_LOG_MAX_BYTES: usize = 8 * 1024;
const WORKER_TEXT_MAX_BYTES: usize = 1024;
const LIST_LIMIT: i64 = 100;
const SESSION_FAILURES: &[&str] = &["oom_killed", "crashed", "spawn_failed"];
const STRIPPED_PAYLOAD_KEYS: &[&str] = &[
    "env",
    "prompt",
    "cctui_machine_key",
    "reply_url",
    "notify_url",
    "notify_secret",
    cctui_proto::worker_env::SERVER_ENV_KEYS_FIELD,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRecord {
    pub session_id: String,
    pub user_id: Option<Uuid>,
    pub dispatcher: String,
    pub name: Option<String>,
    pub profile: Option<String>,
    pub repo: Option<String>,
    pub model: Option<String>,
    pub adapter: Option<String>,
    pub prompt_head: Option<String>,
    pub prompt_file: Option<String>,
    pub prompt_truncated: bool,
    pub env_keys: Vec<String>,
    pub replay: Option<serde_json::Value>,
}

fn head(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

fn text(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// No session id, so a re-run is a new dispatch.
fn replay_of(req: &DispatchRequest, name: Option<&str>) -> Option<serde_json::Value> {
    let mut payload = req.payload.clone();
    if let Some(obj) = payload.as_object_mut() {
        for key in STRIPPED_PAYLOAD_KEYS {
            obj.remove(*key);
        }
        if let Some(name) = name {
            obj.entry("name").or_insert_with(|| serde_json::Value::String(name.to_owned()));
        }
    }
    let replay = DispatchRequest {
        dispatcher: req.dispatcher.clone(),
        session_id: None,
        timeout: req.timeout,
        reply_url: None,
        notify_url: None,
        notify_secret: None,
        payload,
        account: req.account.clone(),
        provider: req.provider.clone(),
        accounts: req.accounts.clone(),
    };
    let value = serde_json::to_value(&replay).ok()?;
    (value.to_string().len() <= REPLAY_MAX_BYTES).then_some(value)
}

impl NewRecord {
    #[must_use]
    pub fn from_request(
        session_id: &str,
        user_id: Option<Uuid>,
        req: &DispatchRequest,
        name: Option<&str>,
        adapter: Option<&str>,
    ) -> Self {
        let p = &req.payload;
        let (prompt_head, prompt_truncated) = p
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or((None, false), |prompt| {
                let (h, cut) = head(prompt, PROMPT_HEAD_BYTES);
                (Some(h.to_owned()), cut)
            });
        let mut env_keys: Vec<String> = p
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map(|env| env.keys().cloned().collect())
            .unwrap_or_default();
        env_keys.sort_unstable();
        Self {
            session_id: session_id.to_owned(),
            user_id,
            dispatcher: req.dispatcher.clone(),
            name: name.map(str::to_owned).or_else(|| text(p, "name")),
            profile: text(p, "profile"),
            repo: text(p, "repo"),
            model: text(p, "model"),
            adapter: adapter.map(str::to_owned),
            prompt_head,
            prompt_file: text(p, "prompt_file"),
            prompt_truncated,
            env_keys,
            replay: replay_of(req, name),
        }
    }
}

/// Upsert a fresh record. A retry naming another user's session id leaves
/// that user's record alone.
pub async fn insert(exec: impl PgExecutor<'_>, r: &NewRecord) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dispatch_records \
           (session_id, user_id, dispatcher_name, name, profile, repo, model, adapter, \
            prompt_head, prompt_file, prompt_truncated, env_keys, replay) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (session_id) DO UPDATE SET \
           dispatcher_name = EXCLUDED.dispatcher_name, name = EXCLUDED.name, \
           profile = EXCLUDED.profile, repo = EXCLUDED.repo, model = EXCLUDED.model, \
           adapter = EXCLUDED.adapter, prompt_head = EXCLUDED.prompt_head, \
           prompt_file = EXCLUDED.prompt_file, prompt_truncated = EXCLUDED.prompt_truncated, \
           env_keys = EXCLUDED.env_keys, replay = EXCLUDED.replay, outcome = 'pending', \
           end_reason = NULL, detail = NULL, redispatched_as = NULL, created_at = now() \
         WHERE dispatch_records.user_id IS NOT DISTINCT FROM EXCLUDED.user_id",
    )
    .bind(&r.session_id)
    .bind(r.user_id)
    .bind(&r.dispatcher)
    .bind(&r.name)
    .bind(&r.profile)
    .bind(&r.repo)
    .bind(&r.model)
    .bind(&r.adapter)
    .bind(&r.prompt_head)
    .bind(&r.prompt_file)
    .bind(r.prompt_truncated)
    .bind(&r.env_keys)
    .bind(&r.replay)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_outcome(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    outcome: &str,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dispatch_records SET outcome = $2, detail = $3 WHERE session_id = $1")
        .bind(session_id)
        .bind(outcome)
        .bind(detail)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn delete(exec: impl PgExecutor<'_>, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM dispatch_records WHERE session_id = $1")
        .bind(session_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Whether a forwarded, unregistered worker waits in its dispatcher's queue.
///
/// Leaving the queue starts its registration clock.
pub async fn set_queued(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    queued: bool,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE dispatch_records \
         SET queued = $2, started_at = CASE WHEN $2 THEN started_at ELSE now() END \
         WHERE session_id = $1 AND outcome = 'forwarded' AND queued <> $2",
    )
    .bind(session_id)
    .bind(queued)
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// A forwarded worker that never registered and whose dispatcher reports the
/// workload dead.
pub async fn mark_failed(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    end_reason: &str,
    detail: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE dispatch_records SET outcome = 'failed', end_reason = $2, detail = $3 \
         WHERE session_id = $1 AND outcome = 'forwarded'",
    )
    .bind(session_id)
    .bind(end_reason)
    .bind(detail)
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn scrubbed(text: Option<&str>, max: usize, keep_end: bool) -> Option<String> {
    static PATTERNS: LazyLock<CompiledPatterns> =
        LazyLock::new(|| cctui_crypto::redact::compile(true, &[], &cctui_crypto::vault_key()));
    let text = text?;
    let cut = if keep_end { tail(text, max) } else { head(text, max).0 };
    let mut value = serde_json::Value::String(cut.to_owned());
    cctui_crypto::redact::redact_json(&mut value, &PATTERNS);
    match value {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Bounded, with secret-looking values masked.
#[must_use]
pub fn scrub_worker_exit(exit: &WorkerExit) -> WorkerExit {
    WorkerExit {
        container: scrubbed(exit.container.as_deref(), WORKER_TEXT_MAX_BYTES, false),
        reason: scrubbed(exit.reason.as_deref(), WORKER_TEXT_MAX_BYTES, false),
        log_tail: scrubbed(exit.log_tail.as_deref(), WORKER_LOG_MAX_BYTES, true),
        logs_error: scrubbed(exit.logs_error.as_deref(), WORKER_TEXT_MAX_BYTES, false),
        exit_code: exit.exit_code,
        signal: exit.signal,
    }
}

pub async fn set_worker_exit(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    exit: &WorkerExit,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE dispatch_records SET worker_exit = $2 WHERE session_id = $1")
        .bind(session_id)
        .bind(Json(exit))
        .execute(exec)
        .await?;
    Ok(())
}

/// Marks records whose session registered, so a later session purge does not
/// read as a worker that never came up.
pub async fn mark_registered(exec: impl PgExecutor<'_>) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE dispatch_records r SET outcome = 'registered' FROM sessions s \
         WHERE s.id = r.session_id AND r.outcome IN ('pending', 'forwarded')",
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected())
}

pub async fn prune(exec: impl PgExecutor<'_>) -> Result<u64, sqlx::Error> {
    let done =
        sqlx::query("DELETE FROM dispatch_records WHERE created_at < now() - interval '7 days'")
            .execute(exec)
            .await?;
    Ok(done.rows_affected())
}

#[derive(sqlx::FromRow)]
pub struct UnregisteredHandle {
    pub session_id: String,
    pub user_id: Option<Uuid>,
    pub dispatcher_name: String,
    pub handle: String,
}

/// Forwarded workers with no session yet, for the liveness probe.
pub async fn unregistered_handles(
    exec: impl PgExecutor<'_>,
    max_age_hours: i64,
    limit: i64,
) -> Result<Vec<UnregisteredHandle>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.session_id, r.user_id, dh.dispatcher_name, dh.handle \
         FROM dispatch_records r \
         JOIN dispatch_handles dh ON dh.session_id = r.session_id \
         WHERE r.outcome = 'forwarded' \
           AND r.created_at > now() - ($1 || ' hours')::interval \
           AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = r.session_id) \
         ORDER BY r.created_at \
         LIMIT $2",
    )
    .bind(max_age_hours.to_string())
    .bind(limit)
    .fetch_all(exec)
    .await
}

#[derive(sqlx::FromRow)]
struct FailedRow {
    session_id: String,
    dispatcher_name: String,
    name: Option<String>,
    profile: Option<String>,
    repo: Option<String>,
    model: Option<String>,
    adapter: Option<String>,
    prompt_head: Option<String>,
    prompt_file: Option<String>,
    prompt_truncated: bool,
    env_keys: Vec<String>,
    has_replay: bool,
    outcome: String,
    end_reason: Option<String>,
    detail: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    worker_exit: Option<Json<WorkerExit>>,
    superseded_by: Option<String>,
    dismissed: bool,
}

fn reason(outcome: &str, end_reason: Option<&str>) -> FailedDispatchReason {
    match (outcome, end_reason) {
        ("refused", _) => FailedDispatchReason::Refused,
        (_, Some("oom_killed")) => FailedDispatchReason::OomKilled,
        (_, Some("spawn_failed")) => FailedDispatchReason::SpawnFailed,
        (_, Some("crashed")) => FailedDispatchReason::Crashed,
        _ => FailedDispatchReason::NeverRegistered,
    }
}

const fn blocker(env: bool, truncated: bool, has_replay: bool) -> Option<RerunBlocker> {
    if env {
        Some(RerunBlocker::Env)
    } else if truncated {
        Some(RerunBlocker::PromptTruncated)
    } else if !has_replay {
        Some(RerunBlocker::PayloadTooLarge)
    } else {
        None
    }
}

impl From<FailedRow> for FailedDispatch {
    fn from(r: FailedRow) -> Self {
        Self {
            reason: reason(&r.outcome, r.end_reason.as_deref()),
            rerun_blocked: blocker(!r.env_keys.is_empty(), r.prompt_truncated, r.has_replay),
            session_id: r.session_id,
            dispatcher: r.dispatcher_name,
            name: r.name,
            profile: r.profile,
            repo: r.repo,
            model: r.model,
            adapter: r.adapter,
            prompt_head: r.prompt_head,
            prompt_file: r.prompt_file,
            env_keys: r.env_keys,
            detail: r.detail,
            dispatched_at: r.created_at,
            superseded_by: r.superseded_by,
            dismissed: r.dismissed,
            worker_exit: r.worker_exit.map(|j| j.0),
        }
    }
}

/// Which failed dispatches to list.
#[derive(Debug, Clone, Copy, Default)]
pub struct Listing {
    /// `None` lists every owner's.
    pub owner: Option<Uuid>,
    /// Whose dismissals apply; `None` applies none.
    pub viewer: Option<Uuid>,
    /// Keep the viewer's dismissed records, flagged, for the history.
    pub include_dismissed: bool,
}

/// Failed dispatches of the last week.
///
/// Those the dispatcher refused, whose worker did not register in time once
/// out of the dispatcher's queue, or whose session died of a start or kill
/// failure before its workflow guard exited; records already re-run are left
/// out. Each names the later run of the same work (same caller, name and
/// prompt file) that registered and did not fail.
pub async fn list(
    exec: impl PgExecutor<'_>,
    listing: Listing,
) -> Result<Vec<FailedDispatch>, sqlx::Error> {
    let rows: Vec<FailedRow> = sqlx::query_as(
        "SELECT r.session_id, r.dispatcher_name, r.name, r.profile, r.repo, r.model, r.adapter, \
                r.prompt_head, r.prompt_file, r.prompt_truncated, r.env_keys, \
                (r.replay IS NOT NULL) AS has_replay, r.outcome, \
                COALESCE(r.end_reason, s.end_reason) AS end_reason, \
                COALESCE(r.detail, s.end_detail) AS detail, r.created_at, r.worker_exit, \
                sup.session_id AS superseded_by, (d.session_id IS NOT NULL) AS dismissed \
         FROM dispatch_records r \
         LEFT JOIN sessions s ON s.id = r.session_id \
         LEFT JOIN dispatch_record_dismissals d \
                ON d.session_id = r.session_id AND d.user_id = $5 \
         LEFT JOIN LATERAL ( \
             SELECT n.session_id FROM dispatch_records n \
             LEFT JOIN sessions ns ON ns.id = n.session_id \
             WHERE n.user_id IS NOT DISTINCT FROM r.user_id \
               AND n.name = r.name \
               AND n.prompt_file IS NOT DISTINCT FROM r.prompt_file \
               AND n.created_at > r.created_at \
               AND (n.outcome = 'registered' OR ns.id IS NOT NULL) \
               AND NOT (COALESCE(ns.end_reason, '') = ANY($3)) \
             ORDER BY n.created_at \
             LIMIT 1 \
         ) sup ON true \
         WHERE ($1::uuid IS NULL OR r.user_id = $1) \
           AND r.redispatched_as IS NULL \
           AND r.created_at > now() - interval '7 days' \
           AND ($6 OR d.session_id IS NULL) \
           AND (r.outcome IN ('refused', 'failed') \
                OR (r.outcome IN ('pending', 'forwarded') AND s.id IS NULL AND NOT r.queued \
                    AND COALESCE(r.started_at, r.created_at) < now() - make_interval(mins => $2)) \
                OR (s.end_reason = ANY($3) AND NOT EXISTS ( \
                    SELECT 1 FROM guard_steps g \
                    WHERE g.session_id = r.session_id AND (g.state->>'step')::bigint < 0))) \
         ORDER BY r.created_at DESC \
         LIMIT $4",
    )
    .bind(listing.owner)
    .bind(REGISTER_TIMEOUT_MINUTES)
    .bind(SESSION_FAILURES)
    .bind(LIST_LIMIT)
    .bind(listing.viewer)
    .bind(listing.include_dismissed)
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// [`list`] with no dismissals applied.
#[cfg(any(test, feature = "test-support"))]
pub async fn list_failed(
    exec: impl PgExecutor<'_>,
    owner: Option<Uuid>,
) -> Result<Vec<FailedDispatch>, sqlx::Error> {
    list(exec, Listing { owner, ..Listing::default() }).await
}

/// Hide a record from `viewer`'s list.
///
/// `false` when no such record is visible to `owner` (`None`: any owner's).
pub async fn dismiss(
    exec: impl PgExecutor<'_>,
    viewer: Uuid,
    owner: Option<Uuid>,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "WITH rec AS ( \
             SELECT session_id FROM dispatch_records \
             WHERE session_id = $2 AND ($3::uuid IS NULL OR user_id = $3)), \
         ins AS ( \
             INSERT INTO dispatch_record_dismissals (user_id, session_id) \
             SELECT $1, session_id FROM rec ON CONFLICT DO NOTHING) \
         SELECT EXISTS (SELECT 1 FROM rec)",
    )
    .bind(viewer)
    .bind(session_id)
    .bind(owner)
    .fetch_one(exec)
    .await
}

/// Dismiss everything `viewer`'s list shows now; how many were dismissed.
pub async fn dismiss_all(
    pool: &sqlx::PgPool,
    viewer: Uuid,
    owner: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    let shown =
        list(pool, Listing { owner, viewer: Some(viewer), include_dismissed: false }).await?;
    let ids: Vec<String> = shown.into_iter().map(|f| f.session_id).collect();
    let done = sqlx::query(
        "INSERT INTO dispatch_record_dismissals (user_id, session_id) \
         SELECT $1, id FROM unnest($2::text[]) AS id \
         ON CONFLICT DO NOTHING",
    )
    .bind(viewer)
    .bind(&ids)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

#[derive(Debug, sqlx::FromRow)]
pub struct StoredRecord {
    pub user_id: Option<Uuid>,
    pub prompt_head: Option<String>,
    pub prompt_truncated: bool,
    pub env_keys: Vec<String>,
    pub replay: Option<serde_json::Value>,
    pub redispatched_as: Option<String>,
}

impl StoredRecord {
    #[must_use]
    pub const fn rerun_blocker(&self) -> Option<RerunBlocker> {
        blocker(!self.env_keys.is_empty(), self.prompt_truncated, self.replay.is_some())
    }

    /// The stored request, prompt restored, as a new dispatch. `None` when a
    /// re-run is blocked.
    #[must_use]
    pub fn replay_request(&self) -> Option<DispatchRequest> {
        if self.rerun_blocker().is_some() {
            return None;
        }
        let mut req: DispatchRequest = serde_json::from_value(self.replay.clone()?).ok()?;
        if let Some(prompt) = &self.prompt_head {
            if !req.payload.is_object() {
                req.payload = serde_json::Value::Object(serde_json::Map::new());
            }
            if let Some(obj) = req.payload.as_object_mut() {
                obj.insert("prompt".into(), serde_json::Value::String(prompt.clone()));
            }
        }
        Some(req)
    }
}

pub async fn get(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<Option<StoredRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, prompt_head, prompt_truncated, env_keys, replay, redispatched_as \
         FROM dispatch_records WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_optional(exec)
    .await
}

/// Claim the record for one re-run; `false` when another already took it.
pub async fn claim_redispatch(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE dispatch_records SET redispatched_as = '' \
         WHERE session_id = $1 AND redispatched_as IS NULL",
    )
    .bind(session_id)
    .execute(exec)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Settle a claim: the new session id, or `None` to release it.
pub async fn finish_redispatch(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    new_session_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dispatch_records SET redispatched_as = $2 \
         WHERE session_id = $1 AND redispatched_as = ''",
    )
    .bind(session_id)
    .bind(new_session_id)
    .execute(exec)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NewRecord, PROMPT_HEAD_BYTES, RerunBlocker, StoredRecord};
    use cctui_proto::api::{DispatchRequest, FailedDispatchReason};
    use uuid::Uuid;

    fn req(payload: &serde_json::Value) -> DispatchRequest {
        serde_json::from_value(serde_json::json!({
            "dispatcher": "kube",
            "session_id": "triage-PROJ-1",
            "timeout": 30,
            "reply_url": "https://n8n/reply/CAPABILITY",
            "notify_url": "https://n8n/notify",
            "notify_secret": "SIGNING-SECRET",
            "account": "work",
            "payload": payload,
        }))
        .unwrap()
    }

    fn stored(r: &NewRecord) -> StoredRecord {
        StoredRecord {
            user_id: r.user_id,
            prompt_head: r.prompt_head.clone(),
            prompt_truncated: r.prompt_truncated,
            env_keys: r.env_keys.clone(),
            replay: r.replay.clone(),
            redispatched_as: None,
        }
    }

    #[test]
    fn a_record_keeps_the_identifying_fields_and_no_secret() {
        let r = req(&serde_json::json!({
            "profile": "tonbo", "repo": "cctui", "model": "opus", "adapter": "codex",
            "prompt": "fix the build", "context": {"owner": "o"},
            "env": {"GITHUB_TOKEN": "ghp_SECRETVALUE"},
            "cctui_machine_key": "cmk_SECRET",
            "server_env_keys": ["X"],
        }));
        let rec = NewRecord::from_request("s1", None, &r, Some("triage-PROJ-1"), Some("codex"));
        assert_eq!(rec.name.as_deref(), Some("triage-PROJ-1"));
        assert_eq!(rec.profile.as_deref(), Some("tonbo"));
        assert_eq!(rec.repo.as_deref(), Some("cctui"));
        assert_eq!(rec.model.as_deref(), Some("opus"));
        assert_eq!(rec.adapter.as_deref(), Some("codex"));
        assert_eq!(rec.prompt_head.as_deref(), Some("fix the build"));
        assert_eq!(rec.env_keys, ["GITHUB_TOKEN"]);

        let dump = format!("{rec:?}");
        for secret in
            ["ghp_SECRETVALUE", "cmk_SECRET", "CAPABILITY", "SIGNING-SECRET", "n8n/notify"]
        {
            assert!(!dump.contains(secret), "{secret} leaked into the record: {dump}");
        }
        let replay = rec.replay.as_ref().unwrap();
        assert!(replay.get("session_id").is_none(), "a re-run is a new dispatch");
        assert!(replay["payload"].get("server_env_keys").is_none());
        assert_eq!(replay["payload"]["context"]["owner"], "o");
        assert_eq!(stored(&rec).rerun_blocker(), Some(RerunBlocker::Env));
        assert!(stored(&rec).replay_request().is_none());
    }

    #[test]
    fn a_long_prompt_keeps_its_head_and_blocks_the_rerun() {
        let prompt = "é".repeat(PROMPT_HEAD_BYTES);
        let rec = NewRecord::from_request(
            "s1",
            None,
            &req(&serde_json::json!({ "prompt": prompt })),
            None,
            None,
        );
        let head = rec.prompt_head.as_deref().unwrap();
        assert!(head.len() <= PROMPT_HEAD_BYTES && prompt.starts_with(head));
        assert!(rec.prompt_truncated);
        assert_eq!(stored(&rec).rerun_blocker(), Some(RerunBlocker::PromptTruncated));
    }

    #[test]
    fn the_replay_resends_the_stored_fields_as_a_new_dispatch() {
        let r = req(&serde_json::json!({ "profile": "tonbo", "prompt": "go", "repo": "cctui" }));
        let rec = NewRecord::from_request("s1", None, &r, Some("triage-PROJ-1"), None);
        let again = stored(&rec).replay_request().expect("re-runnable");
        assert_eq!(again.dispatcher, "kube");
        assert_eq!(again.session_id, None);
        assert_eq!(again.timeout, Some(30));
        assert_eq!(again.account.as_deref(), Some("work"));
        assert!(again.reply_url.is_none() && again.notify_url.is_none());
        assert!(again.notify_secret.is_none());
        assert_eq!(
            again.payload,
            serde_json::json!({
                "profile": "tonbo", "prompt": "go", "repo": "cctui", "name": "triage-PROJ-1"
            })
        );
    }

    #[test]
    fn reasons_follow_the_outcome_then_the_end_reason() {
        use super::reason;
        assert_eq!(reason("refused", Some("crashed")), FailedDispatchReason::Refused);
        assert_eq!(reason("failed", Some("oom_killed")), FailedDispatchReason::OomKilled);
        assert_eq!(reason("registered", Some("spawn_failed")), FailedDispatchReason::SpawnFailed);
        assert_eq!(reason("forwarded", None), FailedDispatchReason::NeverRegistered);
    }

    async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("dispatch-records-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    async fn age(pool: &sqlx::PgPool, sid: &str, minutes: i32) {
        sqlx::query(
            "UPDATE dispatch_records SET created_at = now() - make_interval(mins => $2) \
             WHERE session_id = $1",
        )
        .bind(sid)
        .bind(minutes)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A forwarded worker is listed once the registration timeout passes with
    /// no session, a refused one at once, a registered one never; each owner
    /// sees only its own.
    #[tokio::test]
    async fn a_never_registered_dispatch_is_listed_for_its_owner() {
        let Some(url) = crate::test_support::db_url("dispatch_records_never_registered") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let (uid, other) = (seed_user(&pool).await, seed_user(&pool).await);
        let sid = |tag: &str| format!("{tag}-{}", Uuid::new_v4());
        let (lost, fresh, refused, registered) =
            (sid("lost"), sid("fresh"), sid("refused"), sid("registered"));
        let r = req(&serde_json::json!({ "prompt": "go", "env": {"K": "V"} }));
        for s in [&lost, &fresh, &refused, &registered] {
            super::insert(&pool, &NewRecord::from_request(s, Some(uid), &r, None, None))
                .await
                .unwrap();
            super::set_outcome(&pool, s, "forwarded", None).await.unwrap();
        }
        super::set_outcome(&pool, &refused, "refused", Some("quota exceeded")).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
             VALUES ($1, 'm1', '/w', $2, 'claude-code')",
        )
        .bind(&registered)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        for s in [&lost, &registered] {
            age(&pool, s, super::REGISTER_TIMEOUT_MINUTES + 1).await;
        }
        super::mark_registered(&pool).await.unwrap();

        let listed = super::list_failed(&pool, Some(uid)).await.unwrap();
        let ids: Vec<&str> = listed.iter().map(|f| f.session_id.as_str()).collect();
        assert_eq!(ids, [refused.as_str(), lost.as_str()]);
        assert_eq!(listed[0].reason, FailedDispatchReason::Refused);
        assert_eq!(listed[0].detail.as_deref(), Some("quota exceeded"));
        assert_eq!(listed[1].reason, FailedDispatchReason::NeverRegistered);
        assert_eq!(listed[1].env_keys, ["K"]);
        assert_eq!(listed[1].rerun_blocked, Some(RerunBlocker::Env));
        assert!(super::list_failed(&pool, Some(other)).await.unwrap().is_empty());

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&registered)
            .execute(&pool)
            .await
            .ok();
        for u in [uid, other] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
        }
    }

    #[test]
    fn a_worker_exit_is_bounded_and_scrubbed_keeping_the_last_lines() {
        let noise = (0..2000).map(|i| format!("line {i}\n")).collect::<Vec<_>>().concat();
        let exit = super::scrub_worker_exit(&cctui_proto::api::WorkerExit {
            container: Some("worker".into()),
            exit_code: Some(1),
            signal: Some(9),
            log_tail: Some(format!("{noise}push failed: ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123")),
            ..Default::default()
        });
        let log = exit.log_tail.unwrap();
        assert!(log.len() <= super::WORKER_LOG_MAX_BYTES, "{}", log.len());
        assert!(log.contains("push failed: [REDACTED"), "{log}");
        assert!(!log.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"));
        assert!(!log.contains("line 0\n"), "the head is dropped, the end kept");
        assert_eq!((exit.exit_code, exit.signal), (Some(1), Some(9)));
        assert_eq!(exit.container.as_deref(), Some("worker"));
    }

    async fn refused(pool: &sqlx::PgPool, uid: Uuid, sid: &str, payload: &serde_json::Value) {
        let rec = NewRecord::from_request(sid, Some(uid), &req(payload), None, None);
        super::insert(pool, &rec).await.unwrap();
        super::set_outcome(pool, sid, "refused", Some("no slot")).await.unwrap();
    }

    async fn cleanup(pool: &sqlx::PgPool, users: &[Uuid]) {
        for u in users {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(u).execute(pool).await.ok();
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(pool).await.ok();
        }
    }

    /// A dismissal hides the card from the one who dismissed it only, and
    /// stays in the history.
    #[tokio::test]
    async fn dismissals_are_per_user_and_dismiss_all_clears_the_list() {
        use super::Listing;
        let Some(url) = crate::test_support::db_url("dispatch_records_dismiss") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let (uid, admin) = (seed_user(&pool).await, seed_user(&pool).await);
        let (one, two) = (format!("one-{}", Uuid::new_v4()), format!("two-{}", Uuid::new_v4()));
        for s in [&one, &two] {
            refused(&pool, uid, s, &serde_json::json!({ "prompt": "go" })).await;
        }
        let ids = |l: Vec<cctui_proto::api::FailedDispatch>| {
            let mut ids: Vec<(String, bool)> =
                l.into_iter().map(|f| (f.session_id, f.dismissed)).collect();
            ids.sort();
            ids
        };
        let mine = Listing { owner: Some(uid), viewer: Some(uid), include_dismissed: false };
        let all_for_admin = Listing { owner: None, viewer: Some(admin), include_dismissed: false };

        assert!(super::dismiss(&pool, uid, Some(uid), &one).await.unwrap());
        assert!(super::dismiss(&pool, uid, Some(uid), &one).await.unwrap(), "idempotent");
        assert!(!super::dismiss(&pool, admin, Some(admin), &two).await.unwrap(), "not theirs");
        assert!(!super::dismiss(&pool, uid, Some(uid), "no-such-record").await.unwrap());

        assert_eq!(ids(super::list(&pool, mine).await.unwrap()), [(two.clone(), false)]);
        let history = Listing { include_dismissed: true, ..mine };
        assert_eq!(
            ids(super::list(&pool, history).await.unwrap()),
            [(one.clone(), true), (two.clone(), false)]
        );
        let admin_sees: Vec<_> = ids(super::list(&pool, all_for_admin).await.unwrap())
            .into_iter()
            .filter(|(s, _)| s == &one || s == &two)
            .collect();
        assert_eq!(admin_sees, [(one, false), (two, false)]);

        assert_eq!(super::dismiss_all(&pool, uid, Some(uid)).await.unwrap(), 1);
        assert!(super::list(&pool, mine).await.unwrap().is_empty());
        assert_eq!(super::list_failed(&pool, Some(uid)).await.unwrap().len(), 2);
        cleanup(&pool, &[uid, admin]).await;
    }

    async fn seed_session(pool: &sqlx::PgPool, sid: &str, uid: Uuid, end_reason: Option<&str>) {
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id, status, \
                                   end_reason) \
             VALUES ($1, 'm1', '/w', $2, 'claude-code', $3, $4)",
        )
        .bind(sid)
        .bind(uid)
        .bind(if end_reason.is_some() { "ended" } else { "active" })
        .bind(end_reason)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A later run of the same work by the same caller that registered and
    /// did not fail resolves the failure; a run that failed, of other work, or
    /// by someone else does not.
    #[tokio::test]
    async fn a_later_successful_run_of_the_same_work_supersedes_the_failure() {
        let Some(url) = crate::test_support::db_url("dispatch_records_supersede") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let (uid, other) = (seed_user(&pool).await, seed_user(&pool).await);
        let sid = |tag: &str| format!("{tag}-{}", Uuid::new_v4());
        let work = |name: &str, file: &str| serde_json::json!({ "name": name, "prompt_file": file, "prompt": "go" });
        let (failed, ok, elsewhere) = (sid("failed"), sid("ok"), sid("elsewhere"));
        let (again, crashed, theirs) = (sid("again"), sid("crashed"), sid("theirs"));
        refused(&pool, uid, &failed, &work("review-PR-7", "prompts/review.md")).await;
        refused(&pool, uid, &elsewhere, &work("review-PR-7", "prompts/triage.md")).await;
        refused(&pool, uid, &again, &work("review-PR-8", "prompts/review.md")).await;
        for s in [&failed, &elsewhere, &again] {
            age(&pool, s, 30).await;
        }
        for (s, owner, name, end) in [
            (&ok, uid, "review-PR-7", None),
            (&crashed, uid, "review-PR-8", Some("crashed")),
            (&theirs, other, "review-PR-8", None),
        ] {
            let payload = work(name, "prompts/review.md");
            let rec = NewRecord::from_request(s, Some(owner), &req(&payload), None, None);
            super::insert(&pool, &rec).await.unwrap();
            super::set_outcome(&pool, s, "forwarded", None).await.unwrap();
            seed_session(&pool, s, owner, end).await;
        }

        let listed = super::list_failed(&pool, Some(uid)).await.unwrap();
        let sup =
            |id: &str| listed.iter().find(|f| f.session_id == id).map(|f| f.superseded_by.clone());
        assert_eq!(sup(&failed), Some(Some(ok.clone())));
        assert_eq!(sup(&elsewhere), Some(None), "another prompt file is other work");
        assert_eq!(sup(&again), Some(None), "a crashed re-run or someone else's does not count");
        assert_eq!(sup(&crashed), Some(None));
        assert_eq!(sup(&ok), None, "a running session is no failure");
        cleanup(&pool, &[uid, other]).await;
    }

    async fn age_start(pool: &sqlx::PgPool, sid: &str, minutes: i32) {
        sqlx::query(
            "UPDATE dispatch_records SET started_at = now() - make_interval(mins => $2) \
             WHERE session_id = $1",
        )
        .bind(sid)
        .bind(minutes)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Time in the dispatcher's queue is not time to register: the clock
    /// starts when the worker leaves the queue.
    #[tokio::test]
    async fn a_queued_dispatch_is_late_only_once_it_left_the_queue() {
        let Some(url) = crate::test_support::db_url("dispatch_records_queued") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let sid = |tag: &str| format!("{tag}-{}", Uuid::new_v4());
        let (waiting, resumed, late) = (sid("waiting"), sid("resumed"), sid("late"));
        let r = req(&serde_json::json!({ "prompt": "go" }));
        for s in [&waiting, &resumed, &late] {
            super::insert(&pool, &NewRecord::from_request(s, Some(uid), &r, None, None))
                .await
                .unwrap();
            super::set_outcome(&pool, s, "forwarded", None).await.unwrap();
            assert!(super::set_queued(&pool, s, true).await.unwrap());
            age(&pool, s, 3 * super::REGISTER_TIMEOUT_MINUTES).await;
        }
        assert!(!super::set_queued(&pool, &waiting, true).await.unwrap(), "already queued");
        for s in [&resumed, &late] {
            assert!(super::set_queued(&pool, s, false).await.unwrap());
        }
        age_start(&pool, &late, super::REGISTER_TIMEOUT_MINUTES + 1).await;

        let listed = super::list_failed(&pool, Some(uid)).await.unwrap();
        let ids: Vec<&str> = listed.iter().map(|f| f.session_id.as_str()).collect();
        assert_eq!(ids, [late.as_str()]);
        assert_eq!(listed[0].reason, FailedDispatchReason::NeverRegistered);
        cleanup(&pool, &[uid]).await;
    }

    /// A session ended as a crash after its workflow guard exited did its
    /// work: no failure card, even for a verdict recorded before that was known.
    #[tokio::test]
    async fn a_crash_after_the_guard_exited_is_not_listed() {
        let Some(url) = crate::test_support::db_url("dispatch_records_guard_exit") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let (done, lost) = (format!("done-{}", Uuid::new_v4()), format!("lost-{}", Uuid::new_v4()));
        let r = req(&serde_json::json!({ "prompt": "go" }));
        for (s, step) in [(&done, -1_i64), (&lost, 3)] {
            super::insert(&pool, &NewRecord::from_request(s, Some(uid), &r, None, None))
                .await
                .unwrap();
            super::set_outcome(&pool, s, "registered", None).await.unwrap();
            seed_session(&pool, s, uid, Some("crashed")).await;
            let guard = cctui_proto::guard_step::GuardStepState { step, ..Default::default() };
            sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
                .bind(s)
                .bind(sqlx::types::Json(&guard))
                .execute(&pool)
                .await
                .unwrap();
        }
        let listed = super::list_failed(&pool, Some(uid)).await.unwrap();
        let ids: Vec<&str> = listed.iter().map(|f| f.session_id.as_str()).collect();
        assert_eq!(ids, [lost.as_str()]);
        assert_eq!(listed[0].reason, FailedDispatchReason::Crashed);
        cleanup(&pool, &[uid]).await;
    }
}
