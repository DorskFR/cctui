use cctui_proto::api::admin::{DispatchPolicy, DispatchPolicyRules};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct Row {
    user_id: Uuid,
    updated_at: chrono::DateTime<chrono::Utc>,
    dispatchers: Vec<String>,
    profiles: Vec<String>,
    adapters: Vec<String>,
    models: Vec<String>,
    context_packs: serde_json::Value,
    max_timeout_minutes: Option<i32>,
    max_concurrent: Option<i32>,
    daily_cap: Option<i32>,
}

fn limit(v: Option<i32>) -> Option<u32> {
    v.and_then(|n| u32::try_from(n).ok())
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

impl From<Row> for DispatchPolicy {
    fn from(r: Row) -> Self {
        Self {
            user_id: r.user_id,
            updated_at: r.updated_at,
            rules: DispatchPolicyRules {
                dispatchers: r.dispatchers,
                profiles: r.profiles,
                adapters: r.adapters,
                models: r.models,
                context_packs: serde_json::from_value(r.context_packs).unwrap_or_default(),
                max_timeout_minutes: limit(r.max_timeout_minutes),
                max_concurrent: limit(r.max_concurrent),
                daily_cap: limit(r.daily_cap),
            },
        }
    }
}

pub async fn get(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<Option<DispatchPolicy>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT user_id, updated_at, dispatchers, profiles, adapters, models, context_packs, \
                max_timeout_minutes, max_concurrent, daily_cap \
         FROM dispatch_policies WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(Into::into))
}

pub async fn upsert(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
    rules: &DispatchPolicyRules,
) -> Result<(), sqlx::Error> {
    let packs = serde_json::to_value(&rules.context_packs).unwrap_or_default();
    sqlx::query(
        "INSERT INTO dispatch_policies \
           (user_id, dispatchers, profiles, adapters, models, context_packs, \
            max_timeout_minutes, max_concurrent, daily_cap) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         ON CONFLICT (user_id) DO UPDATE SET \
           dispatchers = EXCLUDED.dispatchers, profiles = EXCLUDED.profiles, \
           adapters = EXCLUDED.adapters, models = EXCLUDED.models, \
           context_packs = EXCLUDED.context_packs, \
           max_timeout_minutes = EXCLUDED.max_timeout_minutes, \
           max_concurrent = EXCLUDED.max_concurrent, daily_cap = EXCLUDED.daily_cap, \
           updated_at = now()",
    )
    .bind(user_id)
    .bind(&rules.dispatchers)
    .bind(&rules.profiles)
    .bind(&rules.adapters)
    .bind(&rules.models)
    .bind(packs)
    .bind(rules.max_timeout_minutes.map(to_i32))
    .bind(rules.max_concurrent.map(to_i32))
    .bind(rules.daily_cap.map(to_i32))
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete(exec: impl PgExecutor<'_>, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM dispatch_policies WHERE user_id = $1")
        .bind(user_id)
        .execute(exec)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Whether `user_id`'s policy names `dispatcher`, which lets the name resolve
/// across owners.
pub async fn grants_dispatcher(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
    dispatcher: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM dispatch_policies WHERE user_id = $1 AND $2 = ANY(dispatchers))",
    )
    .bind(user_id)
    .bind(dispatcher)
    .fetch_one(exec)
    .await
}

/// Whether the user owning `session_id` has a dispatch policy: the registered
/// session's user, or the one its dispatch key was minted for.
pub async fn session_is_policy_bound(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM dispatch_policies WHERE user_id IN ( \
             SELECT user_id FROM sessions WHERE id = $1 \
             UNION ALL \
             SELECT user_id FROM auth_keys WHERE session_id = $1 AND kind = 'ephemeral'))",
    )
    .bind(session_id)
    .fetch_one(exec)
    .await
}

/// Forwarded dispatches of `user_id` still alive: an unexpired, unrevoked
/// ephemeral worker key whose session has not reached a terminal state. Only
/// recorded uses count, as a refused forward still minted a key.
pub async fn live_dispatches(exec: impl PgExecutor<'_>, user_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(DISTINCT k.session_id) FROM auth_keys k \
         JOIN dispatch_policy_uses u ON u.session_id = k.session_id AND u.user_id = k.user_id \
         WHERE k.user_id = $1 AND k.kind = 'ephemeral' AND k.revoked_at IS NULL \
           AND (k.expires_at IS NULL OR k.expires_at > now()) \
           AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = k.session_id \
                             AND s.status IN ('ended', 'failed', 'archived'))",
    )
    .bind(user_id)
    .fetch_one(exec)
    .await
}

pub async fn dispatches_today(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM dispatch_policy_uses \
         WHERE user_id = $1 AND dispatched_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
    )
    .bind(user_id)
    .fetch_one(exec)
    .await
}

pub async fn record_use(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO dispatch_policy_uses (session_id, user_id) VALUES ($1, $2)")
        .bind(session_id)
        .bind(user_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Drops uses no cap reads any more: only the current UTC day counts.
pub async fn prune_uses(exec: impl PgExecutor<'_>) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM dispatch_policy_uses WHERE dispatched_at < now() - interval '2 days'",
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("policy-uses-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    #[tokio::test]
    async fn two_runs_of_one_session_both_count_but_hold_one_slot() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_uses_two_runs") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let sid = Uuid::new_v4().to_string();
        super::record_use(&pool, uid, &sid).await.unwrap();
        super::record_use(&pool, uid, &sid).await.unwrap();
        assert_eq!(super::dispatches_today(&pool, uid).await.unwrap(), 2);

        sqlx::query(
            "INSERT INTO auth_keys (user_id, key_hash, kind, session_id, expires_at) \
             VALUES ($1, $2, 'ephemeral', $3, now() + interval '1 hour')",
        )
        .bind(uid)
        .bind(format!("eph-{sid}"))
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(super::live_dispatches(&pool, uid).await.unwrap(), 1);

        sqlx::query("DELETE FROM auth_keys WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[tokio::test]
    async fn prune_drops_only_uses_older_than_two_days() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_policy_uses_prune") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let uid = seed_user(&pool).await;
        let (old, recent) = (format!("old-{uid}"), format!("recent-{uid}"));
        for (sid, age) in [(&old, "3 days"), (&recent, "1 day")] {
            sqlx::query(
                "INSERT INTO dispatch_policy_uses (session_id, user_id, dispatched_at) \
                 VALUES ($1, $2, now() - $3::interval)",
            )
            .bind(sid)
            .bind(uid)
            .bind(age)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert!(super::prune_uses(&pool).await.unwrap() >= 1);
        let left: Vec<String> =
            sqlx::query_scalar("SELECT session_id FROM dispatch_policy_uses WHERE user_id = $1")
                .bind(uid)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(left, [recent]);
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
