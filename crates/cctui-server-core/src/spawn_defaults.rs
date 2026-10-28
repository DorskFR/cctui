//! Resolution of configured execution ceilings shared by both distributions.
use cctui_proto::api::settings::{
    SettingSource, SpawnDefaults, SpawnDefaultsInfo, SpawnDefaultsSources,
};
/// Built-in layer: only the depth is capped. Child count and tree budget are
/// unlimited unless a saved setting or env sets a ceiling explicitly.
const fn builtin_spawn_defaults() -> SpawnDefaults {
    SpawnDefaults {
        max_children: None,
        max_depth: Some(cctui_proto::api::DEFAULT_MAX_DEPTH),
        max_tree_budget_usd: None,
    }
}

fn pick<T: Copy>(
    settings: Option<T>,
    env: Option<T>,
    default: Option<T>,
) -> (Option<T>, SettingSource) {
    settings
        .map(|v| (Some(v), SettingSource::Settings))
        .or_else(|| env.map(|v| (Some(v), SettingSource::Env)))
        .unwrap_or((default, SettingSource::Default))
}

#[must_use]
pub fn resolve_spawn_defaults(settings: SpawnDefaults, env: SpawnDefaults) -> SpawnDefaultsInfo {
    let defaults = builtin_spawn_defaults();
    let (c, cs) = pick(settings.max_children, env.max_children, defaults.max_children);
    let (d, ds) = pick(settings.max_depth, env.max_depth, defaults.max_depth);
    let (b, bs) =
        pick(settings.max_tree_budget_usd, env.max_tree_budget_usd, defaults.max_tree_budget_usd);
    SpawnDefaultsInfo {
        effective: SpawnDefaults { max_children: c, max_depth: d, max_tree_budget_usd: b },
        sources: SpawnDefaultsSources { max_children: cs, max_depth: ds, max_tree_budget_usd: bs },
        settings,
        env,
        defaults,
    }
}

pub async fn read(pool: &sqlx::PgPool, env: SpawnDefaults) -> SpawnDefaultsInfo {
    let settings = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM instance_settings WHERE key = 'spawn_defaults'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|v| serde_json::from_value(v).ok())
    .unwrap_or_default();
    resolve_spawn_defaults(settings, env)
}
pub async fn capability(
    pool: &sqlx::PgPool,
    env: SpawnDefaults,
) -> cctui_proto::api::SpawnCapability {
    let e = read(pool, env).await.effective;
    cctui_proto::api::SpawnCapability {
        max_children: e.max_children,
        max_depth: e.max_depth,
        max_tree_budget_usd: e.max_tree_budget_usd,
        ..cctui_proto::api::SpawnCapability::machine_default()
    }
}
