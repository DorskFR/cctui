//! Durable machine launch identities without prompts, environment or credentials.
use cctui_server_kernel::commands::FailedSpawnRow;
use sqlx::PgPool;
use uuid::Uuid;

pub use cctui_server_kernel::dispatch_types::Launch;

pub async fn prepare(
    pool: &PgPool,
    command: Uuid,
    actor: Uuid,
    row: &FailedSpawnRow,
) -> Result<(), sqlx::Error> {
    cctui_telemetry::db::traced("INSERT INTO machine_launches (command_id,session_id,machine_id,user_id,acting_user_id,adapter_id,working_dir,name,model,effort) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)", |sql| sqlx::query(sql)
        .bind(command).bind(&row.session_id).bind(row.machine_id).bind(row.user_id).bind(actor)
        .bind(&row.adapter_id).bind(&row.working_dir).bind(&row.name).bind(&row.model).bind(&row.effort).execute(pool)).await?;
    Ok(())
}

pub async fn delivered(pool: &PgPool, command: Uuid) -> Result<(), sqlx::Error> {
    cctui_telemetry::db::traced("UPDATE machine_launches SET state='delivered',updated_at=now() WHERE command_id=$1 AND state='prepared'", |sql| sqlx::query(sql)
        .bind(command).execute(pool)).await?;
    Ok(())
}

pub async fn delivery_failed(
    pool: &PgPool,
    command: Uuid,
    detail: &str,
) -> Result<(), sqlx::Error> {
    cctui_telemetry::db::traced("UPDATE machine_launches SET state='delivery_failed',detail=$2,updated_at=now() WHERE command_id=$1 AND state='prepared'", |sql| sqlx::query(sql)
        .bind(command).bind(detail).execute(pool)).await?;
    Ok(())
}

/// Only the target machine and its owner may settle this command. A late
/// refusal cannot replace a session that already registered successfully.
pub async fn verdict(
    pool: &PgPool,
    command: Uuid,
    machine: Uuid,
    owner: Uuid,
    ok: bool,
    detail: Option<&str>,
) -> Result<Option<Launch>, sqlx::Error> {
    sqlx::query_as("UPDATE machine_launches SET state=CASE WHEN $4 THEN 'accepted' ELSE 'refused' END, detail=$5,updated_at=now() WHERE command_id=$1 AND machine_id=$2 AND user_id=$3 AND state IN ('prepared','delivered','accepted') RETURNING command_id,session_id,registered_session_id,machine_id,user_id,acting_user_id,adapter_id,working_dir,name,model,effort,state,detail")
        .bind(command).bind(machine).bind(owner).bind(ok).bind(detail).fetch_optional(pool).await
}

/// Attach the canonical worker identity only after the host has authenticated
/// the announcing machine and checked the launch-key ownership.
pub async fn registered(
    pool: &PgPool,
    key: &str,
    session: &str,
    machine: Uuid,
    owner: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE machine_launches SET state='registered',registered_session_id=$2,updated_at=now() WHERE session_id=$1 AND machine_id=$3 AND user_id=$4 AND (registered_session_id IS NULL OR registered_session_id=$2) AND state NOT IN ('refused','delivery_failed')")
        .bind(key).bind(session).bind(machine).bind(owner).execute(pool).await?;
    Ok(())
}

pub async fn find(
    pool: &PgPool,
    identity: &str,
    actor: Option<Uuid>,
) -> Result<Option<Launch>, sqlx::Error> {
    sqlx::query_as("SELECT command_id,session_id,registered_session_id,machine_id,user_id,acting_user_id,adapter_id,working_dir,name,model,effort,state,detail FROM machine_launches WHERE (command_id::text=$1 OR session_id=$1 OR registered_session_id=$1) AND ($2::uuid IS NULL OR acting_user_id=$2)")
        .bind(identity).bind(actor).fetch_optional(pool).await
}
