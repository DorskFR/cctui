//! Window closes with their account and the last sample before each reset.

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LedgerClose {
    pub provider_id: Uuid,
    pub account_name: String,
    pub account_emoji: Option<String>,
    pub provider: String,
    pub window_key: String,
    pub resets_at: DateTime<Utc>,
    pub final_utilization: f64,
    pub wasted_pct: f64,
    pub last_sampled_at: Option<DateTime<Utc>>,
    pub last_amount_usd: Option<f64>,
}

/// `account` matches a credential id or an account identity id.
pub async fn closes(
    pool: &sqlx::PgPool,
    owner: Option<Uuid>,
    account: Option<Uuid>,
    window_key: Option<&str>,
    from: DateTime<Utc>,
) -> Result<Vec<LedgerClose>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.provider_id, a.name AS account_name, a.emoji AS account_emoji, p.provider, \
                c.window_key, c.resets_at, c.final_utilization, c.wasted_pct, \
                s.sampled_at AS last_sampled_at, s.amount_usd AS last_amount_usd \
           FROM account_window_closes c \
           JOIN account_providers p ON p.id = c.provider_id \
           JOIN accounts a ON a.id = p.account_id \
           LEFT JOIN LATERAL ( \
                SELECT sampled_at, amount_usd FROM account_usage_samples \
                 WHERE provider_id = c.provider_id AND window_key = c.window_key \
                   AND sampled_at <= c.resets_at \
                 ORDER BY sampled_at DESC LIMIT 1) s ON true \
          WHERE ($1::uuid IS NULL OR p.user_id = $1) \
            AND ($2::uuid IS NULL OR p.id = $2 OR p.account_id = $2) \
            AND ($3::text IS NULL OR c.window_key = $3) \
            AND c.resets_at >= $4 \
          ORDER BY c.resets_at DESC, a.name, c.window_key",
    )
    .bind(owner)
    .bind(account)
    .bind(window_key)
    .bind(from)
    .fetch_all(pool)
    .await
}
