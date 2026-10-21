//! What each dispatch asked for, kept so a worker that never registers can be
//! identified and re-run. Env values, callback URLs, the callback secret and
//! the worker key are never stored; the prompt is kept to its first 2 KiB.

use cctui_proto::api::{DispatchRequest, FailedDispatch, FailedDispatchReason, RerunBlocker};
use sqlx::PgExecutor;
use uuid::Uuid;

pub const PROMPT_HEAD_BYTES: usize = 2048;
pub const REPLAY_MAX_BYTES: usize = 16 * 1024;
pub const REGISTER_TIMEOUT_MINUTES: i32 = 15;
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
        }
    }
}

/// Dispatches of the last week that the dispatcher refused, whose worker
/// never registered, or whose session died of a start or kill failure. `None`
/// lists every owner's. Records already re-run are left out.
pub async fn list_failed(
    exec: impl PgExecutor<'_>,
    owner: Option<Uuid>,
) -> Result<Vec<FailedDispatch>, sqlx::Error> {
    let rows: Vec<FailedRow> = sqlx::query_as(
        "SELECT r.session_id, r.dispatcher_name, r.name, r.profile, r.repo, r.model, r.adapter, \
                r.prompt_head, r.prompt_file, r.prompt_truncated, r.env_keys, \
                (r.replay IS NOT NULL) AS has_replay, r.outcome, \
                COALESCE(r.end_reason, s.end_reason) AS end_reason, \
                COALESCE(r.detail, s.end_detail) AS detail, r.created_at \
         FROM dispatch_records r \
         LEFT JOIN sessions s ON s.id = r.session_id \
         WHERE ($1::uuid IS NULL OR r.user_id = $1) \
           AND r.redispatched_as IS NULL \
           AND r.created_at > now() - interval '7 days' \
           AND (r.outcome IN ('refused', 'failed') \
                OR (r.outcome IN ('pending', 'forwarded') AND s.id IS NULL \
                    AND r.created_at < now() - make_interval(mins => $2)) \
                OR s.end_reason = ANY($3)) \
         ORDER BY r.created_at DESC \
         LIMIT $4",
    )
    .bind(owner)
    .bind(REGISTER_TIMEOUT_MINUTES)
    .bind(SESSION_FAILURES)
    .bind(LIST_LIMIT)
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
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
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_records_never_registered")
        else {
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
}
