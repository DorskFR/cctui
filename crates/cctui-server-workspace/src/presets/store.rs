use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::presets::{LaunchPreset, OnStale};

use super::{Target, ValidPreset};

macro_rules! columns {
    () => {
        "id, user_id, name, prompt, machine_id, working_dir, adapter_id, model, \
         permission_mode, dispatcher_id, profile, external_sources, fresh_secs, on_stale, \
         max_batch, cron, next_fire_at, then_preset_id, created_at, updated_at, notify_url"
    };
}

#[derive(Debug)]
pub enum WriteError {
    Invalid(String),
    NameTaken,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for WriteError {
    fn from(e: sqlx::Error) -> Self {
        if matches!(&e, sqlx::Error::Database(db) if db.is_unique_violation()) {
            Self::NameTaken
        } else {
            Self::Db(e)
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    user_id: Uuid,
    name: String,
    prompt: String,
    machine_id: Option<Uuid>,
    working_dir: Option<String>,
    adapter_id: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    dispatcher_id: Option<Uuid>,
    profile: Option<String>,
    external_sources: bool,
    fresh_secs: i32,
    on_stale: String,
    max_batch: i32,
    cron: Option<String>,
    next_fire_at: Option<DateTime<Utc>>,
    then_preset_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    notify_url: Option<String>,
}

impl From<Row> for LaunchPreset {
    fn from(r: Row) -> Self {
        let permission_mode =
            r.permission_mode.as_deref().and_then(PermissionMode::from_session_label);
        Self {
            id: r.id,
            user_id: r.user_id,
            name: r.name,
            prompt: r.prompt,
            machine_id: r.machine_id,
            working_dir: r.working_dir,
            adapter_id: r.adapter_id,
            model: r.model,
            permission_mode,
            dispatcher_id: r.dispatcher_id,
            profile: r.profile,
            external_sources: r.external_sources,
            fresh_secs: r.fresh_secs,
            on_stale: OnStale::parse(&r.on_stale).unwrap_or_default(),
            max_batch: r.max_batch,
            cron: r.cron,
            next_fire_at: r.next_fire_at,
            then_preset_id: r.then_preset_id,
            notify_url: r.notify_url,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

const fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
        PermissionMode::Whip => "whip",
    }
}

#[derive(Default)]
struct TargetColumns<'a> {
    machine_id: Option<Uuid>,
    working_dir: Option<&'a str>,
    adapter_id: Option<&'a str>,
    model: Option<&'a str>,
    permission_mode: Option<&'static str>,
    dispatcher_id: Option<Uuid>,
    profile: Option<&'a str>,
}

fn columns(target: &Target) -> TargetColumns<'_> {
    match target {
        Target::Machine { machine_id, working_dir, adapter_id, model, permission_mode } => {
            TargetColumns {
                machine_id: Some(*machine_id),
                working_dir: Some(working_dir.as_str()),
                adapter_id: Some(adapter_id.as_str()),
                model: model.as_deref(),
                permission_mode: permission_mode.map(mode_name),
                ..TargetColumns::default()
            }
        }
        Target::Container { dispatcher_id, profile } => TargetColumns {
            dispatcher_id: Some(*dispatcher_id),
            profile: Some(profile.as_str()),
            ..TargetColumns::default()
        },
    }
}

fn next_fire(preset: &ValidPreset) -> Option<DateTime<Utc>> {
    preset.cron.as_deref().and_then(|expr| super::cron::next_fire(expr, Utc::now()))
}

async fn exists(pool: &PgPool, sql: &'static str, id: Uuid, owner: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(sql).bind(id).bind(owner).fetch_one(pool).await
}

/// The target and the chained preset must belong to the preset's owner.
async fn check_refs(
    pool: &PgPool,
    owner: Uuid,
    preset_id: Option<Uuid>,
    preset: &ValidPreset,
) -> Result<(), WriteError> {
    let target_owned = match &preset.target {
        Target::Machine { machine_id, .. } => {
            exists(
                pool,
                "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND user_id = $2 \
                 AND revoked_at IS NULL AND deleted_at IS NULL)",
                *machine_id,
                owner,
            )
            .await?
        }
        Target::Container { dispatcher_id, .. } => {
            exists(
                pool,
                "SELECT EXISTS (SELECT 1 FROM dispatchers WHERE id = $1 AND user_id = $2 \
                 AND revoked_at IS NULL AND deleted_at IS NULL)",
                *dispatcher_id,
                owner,
            )
            .await?
        }
    };
    if !target_owned {
        return Err(WriteError::Invalid("no machine or dispatcher of yours with that id".into()));
    }
    let Some(then) = preset.then_preset_id else { return Ok(()) };
    if Some(then) == preset_id {
        return Err(WriteError::Invalid("a preset cannot chain to itself".into()));
    }
    let chained = exists(
        pool,
        "SELECT EXISTS (SELECT 1 FROM presets WHERE id = $1 AND user_id = $2)",
        then,
        owner,
    )
    .await?;
    if chained {
        Ok(())
    } else {
        Err(WriteError::Invalid("then_preset_id names no preset of yours".into()))
    }
}

/// `owner` is [`crate::auth::AuthContext::owner_filter`].
pub async fn list(pool: &PgPool, owner: Option<Uuid>) -> sqlx::Result<Vec<LaunchPreset>> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM presets WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name, id"
    ))
    .bind(owner)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get(
    pool: &PgPool,
    owner: Option<Uuid>,
    id: Uuid,
) -> sqlx::Result<Option<LaunchPreset>> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM presets WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)"
    ))
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    preset: &ValidPreset,
) -> Result<LaunchPreset, WriteError> {
    check_refs(pool, user_id, None, preset).await?;
    let t = columns(&preset.target);
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, model, \
           permission_mode, dispatcher_id, profile, external_sources, fresh_secs, on_stale, \
           max_batch, cron, then_preset_id, notify_url, next_fire_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
           $18) \
         RETURNING ",
        columns!()
    ))
    .bind(user_id)
    .bind(&preset.name)
    .bind(&preset.prompt)
    .bind(t.machine_id)
    .bind(t.working_dir)
    .bind(t.adapter_id)
    .bind(t.model)
    .bind(t.permission_mode)
    .bind(t.dispatcher_id)
    .bind(t.profile)
    .bind(preset.external_sources)
    .bind(preset.fresh_secs)
    .bind(preset.on_stale.as_str())
    .bind(preset.max_batch)
    .bind(preset.cron.as_deref())
    .bind(preset.then_preset_id)
    .bind(preset.notify_url.as_deref())
    .bind(next_fire(preset))
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

/// `Ok(None)` when no preset the caller can see has that id. An unchanged cron
/// keeps its pending fire time.
pub async fn update(
    pool: &PgPool,
    owner: Option<Uuid>,
    id: Uuid,
    preset: &ValidPreset,
) -> Result<Option<LaunchPreset>, WriteError> {
    let Some(user_id): Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM presets WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    check_refs(pool, user_id, Some(id), preset).await?;
    let t = columns(&preset.target);
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE presets SET name = $3, prompt = $4, machine_id = $5, working_dir = $6, \
           adapter_id = $7, model = $8, permission_mode = $9, dispatcher_id = $10, \
           profile = $11, external_sources = $12, fresh_secs = $13, on_stale = $14, \
           max_batch = $15, \
           next_fire_at = CASE WHEN cron IS NOT DISTINCT FROM $16 AND next_fire_at IS NOT NULL \
                               THEN next_fire_at ELSE $19 END, \
           cron = $16, then_preset_id = $17, notify_url = $18, updated_at = now() \
         WHERE id = $1 AND user_id = $2 \
         RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(user_id)
    .bind(&preset.name)
    .bind(&preset.prompt)
    .bind(t.machine_id)
    .bind(t.working_dir)
    .bind(t.adapter_id)
    .bind(t.model)
    .bind(t.permission_mode)
    .bind(t.dispatcher_id)
    .bind(t.profile)
    .bind(preset.external_sources)
    .bind(preset.fresh_secs)
    .bind(preset.on_stale.as_str())
    .bind(preset.max_batch)
    .bind(preset.cron.as_deref())
    .bind(preset.then_preset_id)
    .bind(preset.notify_url.as_deref())
    .bind(next_fire(preset))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

/// Deleting a preset deletes its lanes and their events.
pub async fn delete(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<bool> {
    let done =
        sqlx::query("DELETE FROM presets WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
            .bind(id)
            .bind(owner)
            .execute(pool)
            .await?;
    Ok(done.rows_affected() > 0)
}
