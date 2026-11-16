use sqlx::PgPool;
use uuid::Uuid;

const PURGE: [&str; 3] = [
    "DELETE FROM notification_action_tokens t USING notifications n \
     WHERE t.notification_id = n.id AND n.state IN ('resolved', 'expired') \
       AND ($1::uuid IS NULL OR n.user_id = $1)",
    "DELETE FROM notification_deliveries d USING notifications n \
     WHERE d.notification_id = n.id AND n.state IN ('resolved', 'expired') AND d.op IS NULL \
       AND ($1::uuid IS NULL OR n.user_id = $1)",
    "DELETE FROM notifications WHERE state IN ('resolved', 'expired') \
       AND resolved_at < now() - interval '7 days' \
       AND ($1::uuid IS NULL OR user_id = $1)",
];

pub async fn sweep(pool: &PgPool) {
    if let Err(e) = purge(pool, None).await {
        tracing::warn!("notification retention sweep failed: {e}");
    }
}

pub async fn purge(pool: &PgPool, user: Option<Uuid>) -> sqlx::Result<u64> {
    let mut purged = 0;
    for sql in PURGE {
        purged += sqlx::query(sql).bind(user).execute(pool).await?.rows_affected();
    }
    Ok(purged)
}
