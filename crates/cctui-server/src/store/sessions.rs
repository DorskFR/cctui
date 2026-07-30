use sqlx::PgExecutor;

use crate::routes::sessions::DbSession;

pub async fn set_inactive(
    exec: impl PgExecutor<'_>,
    id: &str,
    require_archived: bool,
) -> Result<(), sqlx::Error> {
    let sql = if require_archived {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1 AND status = 'archived'"
    } else {
        "UPDATE sessions SET status = 'inactive' WHERE id = $1"
    };
    sqlx::query(sql).bind(id).execute(exec).await?;
    Ok(())
}

pub async fn fetch_by_id(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<DbSession>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
                s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
                COALESCE(m.display_name, m.name) AS resolved_machine_name, \
                m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await
}

pub async fn adapter_id(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT adapter_id FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

pub async fn working_dir(
    exec: impl PgExecutor<'_>,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

pub async fn child_ids(
    exec: impl PgExecutor<'_>,
    parent_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM sessions WHERE parent_id = $1")
        .bind(parent_id)
        .fetch_all(exec)
        .await
}
