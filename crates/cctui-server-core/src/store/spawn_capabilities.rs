use cctui_proto::api::SpawnCapability;
use sqlx::PgExecutor;

pub async fn upsert(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    capability: &SpawnCapability,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(capability).unwrap_or(serde_json::Value::Null);
    sqlx::query(
        "INSERT INTO session_spawn_capabilities (session_id, capability) VALUES ($1, $2) \
         ON CONFLICT (session_id) DO UPDATE SET capability = EXCLUDED.capability",
    )
    .bind(session_id)
    .bind(json)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<Option<SpawnCapability>, sqlx::Error> {
    let row: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT capability FROM session_spawn_capabilities WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(|v| serde_json::from_value(v).ok()))
}

/// Re-key a capability from the spawn key onto the id the harness registered
/// under, mirroring the token rebind so a rebound session keeps its capability.
pub async fn rebind(
    exec: impl PgExecutor<'_>,
    spawn_key: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE session_spawn_capabilities SET session_id = $2 WHERE session_id = $1 \
         AND NOT EXISTS (SELECT 1 FROM session_spawn_capabilities WHERE session_id = $2)",
    )
    .bind(spawn_key)
    .bind(session_id)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete(exec: impl PgExecutor<'_>, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM session_spawn_capabilities WHERE session_id = $1")
        .bind(session_id)
        .execute(exec)
        .await?;
    Ok(())
}
