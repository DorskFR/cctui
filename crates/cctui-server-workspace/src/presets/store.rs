use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::presets::{LaunchPreset, OnStale, Overlap};

use super::{AccountPick, EnvEdit, Target, ValidPreset};

macro_rules! columns {
    () => {
        "id, user_id, name, prompt, machine_id, working_dir, adapter_id, model, \
         permission_mode, dispatcher_id, profile, external_sources, fresh_secs, on_stale, \
         max_batch, cron, next_fire_at, then_preset_id, created_at, updated_at, notify_url, \
         timezone, enabled, overlap, last_fire_at, account_id, pool_id, no_account, env_keys"
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
    timezone: String,
    enabled: bool,
    overlap: String,
    last_fire_at: Option<DateTime<Utc>>,
    account_id: Option<Uuid>,
    pool_id: Option<Uuid>,
    no_account: bool,
    env_keys: Vec<String>,
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
            account_id: r.account_id,
            pool_id: r.pool_id,
            no_account: r.no_account,
            dispatcher_id: r.dispatcher_id,
            profile: r.profile,
            external_sources: r.external_sources,
            fresh_secs: r.fresh_secs,
            on_stale: OnStale::parse(&r.on_stale).unwrap_or_default(),
            max_batch: r.max_batch,
            cron: r.cron,
            timezone: r.timezone,
            enabled: r.enabled,
            overlap: Overlap::parse(&r.overlap).unwrap_or_default(),
            next_fire_at: r.next_fire_at,
            last_fire_at: r.last_fire_at,
            then_preset_id: r.then_preset_id,
            notify_url: r.notify_url,
            env_keys: r.env_keys,
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
    account_id: Option<Uuid>,
    pool_id: Option<Uuid>,
    no_account: bool,
    dispatcher_id: Option<Uuid>,
    profile: Option<&'a str>,
}

fn columns(target: &Target) -> TargetColumns<'_> {
    match target {
        Target::Machine {
            machine_id,
            working_dir,
            adapter_id,
            model,
            permission_mode,
            account,
        } => TargetColumns {
            machine_id: Some(*machine_id),
            working_dir: Some(working_dir.as_str()),
            adapter_id: Some(adapter_id.as_str()),
            model: model.as_deref(),
            permission_mode: permission_mode.map(mode_name),
            account_id: match account {
                AccountPick::Account(id) => Some(*id),
                _ => None,
            },
            pool_id: match account {
                AccountPick::Pool(id) => Some(*id),
                _ => None,
            },
            no_account: matches!(account, AccountPick::Unbound),
            ..TargetColumns::default()
        },
        Target::Container { dispatcher_id, profile } => TargetColumns {
            dispatcher_id: Some(*dispatcher_id),
            profile: Some(profile.as_str()),
            ..TargetColumns::default()
        },
    }
}

#[derive(Default)]
struct SealedEnv {
    keys: Vec<String>,
    sealed: Option<String>,
}

/// `None` leaves the stored env as it is.
fn sealed_env(edit: &EnvEdit, stored: Option<&str>) -> Result<Option<SealedEnv>, WriteError> {
    let EnvEdit::Edit { keep, set } = edit else { return Ok(None) };
    let invalid = |e: crate::sealed_env::SealError| WriteError::Invalid(e.message().to_owned());
    let env = crate::sealed_env::apply_edit(stored, keep, set, crate::crypto::vault_key)
        .map_err(invalid)?;
    let sealed = crate::sealed_env::seal(&env, crate::crypto::vault_key).map_err(invalid)?;
    Ok(Some(SealedEnv { keys: crate::sealed_env::names(&env), sealed }))
}

async fn next_fire(pool: &PgPool, preset: &ValidPreset) -> sqlx::Result<Option<DateTime<Utc>>> {
    match preset.cron.as_deref() {
        Some(expr) if preset.enabled => {
            super::tz::next_fire_in(pool, expr, &preset.timezone, Utc::now()).await
        }
        _ => Ok(None),
    }
}

async fn exists(pool: &PgPool, sql: &'static str, id: Uuid, owner: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(sql).bind(id).bind(owner).fetch_one(pool).await
}

/// The target and the chained preset must belong to the preset's owner, and
/// the timezone must be one Postgres knows.
async fn check_refs(
    pool: &PgPool,
    owner: Uuid,
    preset_id: Option<Uuid>,
    preset: &ValidPreset,
) -> Result<(), WriteError> {
    if preset.timezone != super::tz::UTC && !super::tz::exists(pool, &preset.timezone).await? {
        return Err(WriteError::Invalid(format!(
            "unknown timezone '{}': use an IANA name such as Asia/Tokyo",
            preset.timezone
        )));
    }
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
    check_account(pool, owner, &preset.target).await?;
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

async fn check_account(pool: &PgPool, owner: Uuid, target: &Target) -> Result<(), WriteError> {
    let Target::Machine { account, .. } = target else { return Ok(()) };
    let (sql, id, what) = match *account {
        AccountPick::Account(id) => (
            "SELECT EXISTS (SELECT 1 FROM accounts a WHERE a.id = $1 \
               AND (a.user_id = $2 OR EXISTS ( \
                   SELECT 1 FROM resource_shares s \
                    WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                      AND s.grantee_id = $2 AND s.revoked_at IS NULL)))",
            id,
            "account_id names no account you can use",
        ),
        AccountPick::Pool(id) => (
            "SELECT EXISTS (SELECT 1 FROM account_pools WHERE id = $1 AND user_id = $2)",
            id,
            "pool_id names no account pool of yours",
        ),
        AccountPick::Auto | AccountPick::Unbound => return Ok(()),
    };
    if exists(pool, sql, id, owner).await? { Ok(()) } else { Err(WriteError::Invalid(what.into())) }
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
    let env = sealed_env(&preset.env, None)?.unwrap_or_default();
    let t = columns(&preset.target);
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, model, \
           permission_mode, dispatcher_id, profile, external_sources, fresh_secs, on_stale, \
           max_batch, cron, then_preset_id, notify_url, next_fire_at, timezone, enabled, \
           overlap, account_id, pool_id, no_account, env_keys, sealed_env) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
           $18, $19, $20, $21, $22, $23, $24, $25, $26) \
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
    .bind(next_fire(pool, preset).await?)
    .bind(&preset.timezone)
    .bind(preset.enabled)
    .bind(preset.overlap.as_str())
    .bind(t.account_id)
    .bind(t.pool_id)
    .bind(t.no_account)
    .bind(&env.keys)
    .bind(env.sealed.as_deref())
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

/// `Ok(None)` when no preset the caller can see has that id. An unchanged,
/// enabled schedule keeps its pending fire time; a paused one has none.
pub async fn update(
    pool: &PgPool,
    owner: Option<Uuid>,
    id: Uuid,
    preset: &ValidPreset,
) -> Result<Option<LaunchPreset>, WriteError> {
    let Some((user_id, stored_env)): Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT user_id, sealed_env FROM presets \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    check_refs(pool, user_id, Some(id), preset).await?;
    let (env_set, env_keys, env_sealed) = sealed_env(&preset.env, stored_env.as_deref())?
        .map_or((false, Vec::new(), None), |e| (true, e.keys, e.sealed));
    let t = columns(&preset.target);
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE presets SET name = $3, prompt = $4, machine_id = $5, working_dir = $6, \
           adapter_id = $7, model = $8, permission_mode = $9, dispatcher_id = $10, \
           profile = $11, external_sources = $12, fresh_secs = $13, on_stale = $14, \
           max_batch = $15, \
           next_fire_at = CASE WHEN cron IS NOT DISTINCT FROM $16 AND timezone = $20 \
                                    AND $21 AND next_fire_at IS NOT NULL \
                               THEN next_fire_at ELSE $19 END, \
           cron = $16, then_preset_id = $17, notify_url = $18, timezone = $20, enabled = $21, \
           overlap = $22, account_id = $23, pool_id = $24, no_account = $25, \
           env_keys = CASE WHEN $26 THEN $27 ELSE env_keys END, \
           sealed_env = CASE WHEN $26 THEN $28 ELSE sealed_env END, \
           updated_at = now() \
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
    .bind(next_fire(pool, preset).await?)
    .bind(&preset.timezone)
    .bind(preset.enabled)
    .bind(preset.overlap.as_str())
    .bind(t.account_id)
    .bind(t.pool_id)
    .bind(t.no_account)
    .bind(env_set)
    .bind(&env_keys)
    .bind(env_sealed.as_deref())
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
