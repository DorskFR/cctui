//! Admin-editable server settings seeded from env. Each value resolves as:
//! saved in `instance_settings` > env > built-in default, and is read at use
//! time so an edit applies without a restart.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;

const SPAWN_KEY: &str = "spawn_defaults";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SettingSource {
    Settings,
    Env,
    Default,
}

/// Default `CctuiAgent` limits; `null` fields are unset at that layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[allow(clippy::struct_field_names)]
pub struct SpawnDefaults {
    #[serde(default)]
    pub max_children: Option<u32>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub max_tree_budget_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[allow(clippy::struct_field_names)]
pub struct SpawnDefaultsSources {
    pub max_children: SettingSource,
    pub max_depth: SettingSource,
    pub max_tree_budget_usd: SettingSource,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SpawnDefaultsInfo {
    /// Every field set: the values new sessions get.
    pub effective: SpawnDefaults,
    pub sources: SpawnDefaultsSources,
    pub settings: SpawnDefaults,
    pub env: SpawnDefaults,
    pub defaults: SpawnDefaults,
}

const fn builtin_spawn_defaults() -> SpawnDefaults {
    SpawnDefaults {
        max_children: Some(cctui_proto::api::DEFAULT_MAX_CHILDREN),
        max_depth: Some(cctui_proto::api::DEFAULT_MAX_DEPTH),
        max_tree_budget_usd: Some(cctui_proto::api::DEFAULT_TREE_BUDGET_USD),
    }
}

const fn env_spawn_defaults(config: &crate::config::Config) -> SpawnDefaults {
    SpawnDefaults {
        max_children: config.spawn_max_children,
        max_depth: config.spawn_max_depth,
        max_tree_budget_usd: config.spawn_max_tree_budget_usd,
    }
}

fn pick<T: Copy>(settings: Option<T>, env: Option<T>, default: T) -> (T, SettingSource) {
    settings
        .map(|v| (v, SettingSource::Settings))
        .or_else(|| env.map(|v| (v, SettingSource::Env)))
        .unwrap_or((default, SettingSource::Default))
}

pub fn resolve_spawn_defaults(settings: SpawnDefaults, env: SpawnDefaults) -> SpawnDefaultsInfo {
    let defaults = builtin_spawn_defaults();
    let (c, cs) =
        pick(settings.max_children, env.max_children, cctui_proto::api::DEFAULT_MAX_CHILDREN);
    let (d, ds) = pick(settings.max_depth, env.max_depth, cctui_proto::api::DEFAULT_MAX_DEPTH);
    let (b, bs) = pick(
        settings.max_tree_budget_usd,
        env.max_tree_budget_usd,
        cctui_proto::api::DEFAULT_TREE_BUDGET_USD,
    );
    SpawnDefaultsInfo {
        effective: SpawnDefaults {
            max_children: Some(c),
            max_depth: Some(d),
            max_tree_budget_usd: Some(b),
        },
        sources: SpawnDefaultsSources { max_children: cs, max_depth: ds, max_tree_budget_usd: bs },
        settings,
        env,
        defaults,
    }
}

async fn stored<T: serde::de::DeserializeOwned>(pool: &sqlx::PgPool, key: &str) -> Option<T> {
    sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM instance_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
}

async fn store(
    pool: &sqlx::PgPool,
    key: &str,
    value: Option<serde_json::Value>,
) -> Result<(), AppError> {
    match value {
        Some(v) => {
            sqlx::query(
                "INSERT INTO instance_settings (key, value, updated_at) VALUES ($1, $2, now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(key)
            .bind(v)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM instance_settings WHERE key = $1")
                .bind(key)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn read_spawn_defaults(state: &AppState) -> SpawnDefaultsInfo {
    let settings = stored(&state.pool, SPAWN_KEY).await.unwrap_or_default();
    resolve_spawn_defaults(settings, env_spawn_defaults(&state.config))
}

/// Capability for a session that declares none.
pub async fn spawn_default_capability(state: &AppState) -> cctui_proto::api::SpawnCapability {
    let e = read_spawn_defaults(state).await.effective;
    cctui_proto::api::SpawnCapability {
        max_children: e.max_children,
        max_depth: e.max_depth,
        max_tree_budget_usd: e.max_tree_budget_usd,
        ..cctui_proto::api::SpawnCapability::machine_default()
    }
}

fn validate_spawn_defaults(v: SpawnDefaults) -> Result<SpawnDefaults, AppError> {
    let bad = |msg: &str| AppError::new(StatusCode::BAD_REQUEST, msg);
    if v.max_children == Some(0) {
        return Err(bad("max_children must be a positive integer"));
    }
    if v.max_depth == Some(0) {
        return Err(bad("max_depth must be a positive integer"));
    }
    if v.max_tree_budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
        return Err(bad("max_tree_budget_usd must be a non-negative number"));
    }
    Ok(v)
}

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

pub async fn get_spawn_defaults(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SpawnDefaultsInfo>, AppError> {
    admin(&ctx)?;
    Ok(Json(read_spawn_defaults(&state).await))
}

/// Body is the saved layer; a `null` field falls back to env/default.
pub async fn update_spawn_defaults(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SpawnDefaults>,
) -> Result<Json<SpawnDefaultsInfo>, AppError> {
    admin(&ctx)?;
    let v = validate_spawn_defaults(req)?;
    let value =
        (v != SpawnDefaults::default()).then(|| serde_json::to_value(v).expect("serializable"));
    store(&state.pool, SPAWN_KEY, value).await?;
    Ok(Json(read_spawn_defaults(&state).await))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn ctx(scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: scopes.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn spawn_defaults_resolve_settings_then_env_then_default() {
        let settings = SpawnDefaults { max_children: Some(2), ..Default::default() };
        let env =
            SpawnDefaults { max_children: Some(5), max_depth: Some(1), max_tree_budget_usd: None };
        let info = resolve_spawn_defaults(settings, env);
        assert_eq!(info.effective.max_children, Some(2));
        assert_eq!(info.sources.max_children, SettingSource::Settings);
        assert_eq!(info.effective.max_depth, Some(1));
        assert_eq!(info.sources.max_depth, SettingSource::Env);
        assert_eq!(
            info.effective.max_tree_budget_usd,
            Some(cctui_proto::api::DEFAULT_TREE_BUDGET_USD)
        );
        assert_eq!(info.sources.max_tree_budget_usd, SettingSource::Default);
    }

    #[test]
    fn spawn_defaults_validation() {
        let ok = SpawnDefaults {
            max_children: Some(1),
            max_depth: Some(1),
            max_tree_budget_usd: Some(0.0),
        };
        assert!(validate_spawn_defaults(ok).is_ok());
        for bad in [
            SpawnDefaults { max_children: Some(0), ..Default::default() },
            SpawnDefaults { max_depth: Some(0), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(-1.0), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(f64::INFINITY), ..Default::default() },
            SpawnDefaults { max_tree_budget_usd: Some(f64::NAN), ..Default::default() },
        ] {
            assert_eq!(validate_spawn_defaults(bad).unwrap_err().status(), StatusCode::BAD_REQUEST);
        }
        let parsed: Result<SpawnDefaults, _> = serde_json::from_str(r#"{"max_children": -1}"#);
        assert!(parsed.is_err());
    }

    async fn state(tag: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(AppState::for_test(pool))
    }

    #[tokio::test]
    async fn spawn_defaults_round_trip_and_apply_without_restart() {
        let Some(mut state) = state("spawn_defaults_round_trip_and_apply_without_restart").await
        else {
            return;
        };
        state.config.spawn_max_depth = Some(2);
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(SPAWN_KEY)
            .execute(&state.pool)
            .await
            .unwrap();

        let denied = get_spawn_defaults(State(state.clone()), Extension(ctx(&[]))).await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        let denied = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[])),
            Json(SpawnDefaults { max_children: Some(3), ..Default::default() }),
        )
        .await;
        assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);

        let cap = spawn_default_capability(&state).await;
        assert_eq!(cap.max_children, Some(cctui_proto::api::DEFAULT_MAX_CHILDREN));
        assert_eq!(cap.max_depth, Some(2));

        let Json(info) = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults {
                max_children: Some(3),
                max_depth: Some(4),
                max_tree_budget_usd: Some(9.5),
            }),
        )
        .await
        .unwrap();
        assert_eq!(info.sources.max_depth, SettingSource::Settings);
        let cap = spawn_default_capability(&state).await;
        assert_eq!(
            (cap.max_children, cap.max_depth, cap.max_tree_budget_usd),
            (Some(3), Some(4), Some(9.5))
        );

        let bad = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults { max_depth: Some(0), ..Default::default() }),
        )
        .await;
        assert_eq!(bad.unwrap_err().status(), StatusCode::BAD_REQUEST);

        let Json(info) = update_spawn_defaults(
            State(state.clone()),
            Extension(ctx(&[Scope::Admin])),
            Json(SpawnDefaults::default()),
        )
        .await
        .unwrap();
        assert_eq!(info.sources.max_depth, SettingSource::Env);
        assert_eq!(info.effective.max_depth, Some(2));
        assert_eq!(info.sources.max_children, SettingSource::Default);
    }
}
