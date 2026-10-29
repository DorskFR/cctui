//! Idempotent daemon usage ingestion shared by both server distributions.
use cctui_proto::adapter::AdapterEvent;
/// `LIKE` patterns matching every adapter id (and its variants) whose usage
/// the gateway meters itself.
#[must_use]
pub fn gateway_metered_adapter_patterns() -> Vec<String> {
    cctui_proto::adapter::gateway_metered_harness_ids()
        .into_iter()
        .map(|id| format!("{id}%"))
        .collect()
}

pub async fn insert_token_usage(
    pool: &sqlx::PgPool,
    local_id: &str,
    message_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
) -> sqlx::Result<()> {
    // Cast u64 → i64 for sqlx; transcripts in the wild never approach
    // i64::MAX so saturating is safe.
    let i = i64::try_from(input_tokens).unwrap_or(i64::MAX);
    let o = i64::try_from(output_tokens).unwrap_or(i64::MAX);
    let cr = i64::try_from(cache_read_tokens).unwrap_or(i64::MAX);
    let cc = i64::try_from(cache_creation_tokens).unwrap_or(i64::MAX);
    // A gateway-metered harness (the row flag; opencode today) has its usage
    // captured from the proxied response, model stamped; the daemon's
    // model-NULL row for the same message would double-count, so exactly one
    // path meters it. The OpenAI-wire ACP agents are not teed, so their own
    // `usage_update` rows stay. A session with no row
    // (its registration was lost with a dropped WS, and the daemon re-announces
    // it on the next connect) is skipped like `Message` is, rather than failing
    // the FK once per usage frame.
    sqlx::query(
        "INSERT INTO session_token_usage \
            (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) \
         SELECT $1, $2, $3, $4, $5, $6 \
         WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
         AND NOT EXISTS ( \
             SELECT 1 FROM sessions WHERE id = $1 AND adapter_id LIKE ANY($7)) \
         ON CONFLICT (session_id, message_id) DO NOTHING",
    )
    .bind(local_id)
    .bind(message_id)
    .bind(i)
    .bind(o)
    .bind(cr)
    .bind(cc)
    .bind(gateway_metered_adapter_patterns())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn on_token_usage(pool: &sqlx::PgPool, event: AdapterEvent) -> sqlx::Result<()> {
    let AdapterEvent::TokenUsage {
        local_id,
        message_id,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    } = event
    else {
        return Ok(());
    };
    insert_token_usage(
        pool,
        &local_id,
        &message_id,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    )
    .await?;
    Ok(())
}
