//! `/api/v1/machines/{machine_id}/adapters` — which harnesses a machine runs.
//!
//! Reads are for the machine's owner (the spawn picker narrows to them).
//! Writes are admin-only and push a fresh Reconcile so the daemon converges
//! without a reconnect. Running sessions are never touched.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;

pub use cctui_proto::api::machine_adapters::{MachineAdapterInfo, SetMachineAdapterRequest};
use cctui_proto::harness::{AdapterBinary, HarnessReport};

type ApiResult<T> = Result<Json<T>, AppError>;

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

type Binaries = std::collections::BTreeMap<String, AdapterBinary>;

/// The machine's view of the harness table: every table row, plus any row
/// the machine pins for a harness the table does not know.
fn merge(
    rows: Vec<(String, serde_json::Value, bool)>,
    binaries: &Binaries,
) -> Vec<MachineAdapterInfo> {
    let mut out: Vec<MachineAdapterInfo> = cctui_proto::adapter::harnesses()
        .into_iter()
        .map(|h| {
            let row = rows.iter().find(|(id, _, _)| *id == h.id);
            MachineAdapterInfo {
                enabled: row.map_or(h.default_enabled, |(_, _, enabled)| *enabled),
                config: row.map_or_else(|| serde_json::json!({}), |(_, config, _)| config.clone()),
                pinned: row.is_some(),
                default_enabled: h.default_enabled,
                binary: binaries.get(&h.id).cloned(),
                adapter_id: h.id,
            }
        })
        .collect();
    for (id, config, enabled) in rows {
        if !out.iter().any(|a| a.adapter_id == id) {
            out.push(MachineAdapterInfo {
                binary: binaries.get(&id).cloned(),
                adapter_id: id,
                enabled,
                config,
                pinned: true,
                default_enabled: false,
            });
        }
    }
    out
}

async fn rows(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
) -> Result<Vec<(String, serde_json::Value, bool)>, AppError> {
    sqlx::query_as("SELECT adapter_id, config, enabled FROM adapters_enabled WHERE machine_id = $1")
        .bind(machine_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::from)
}

/// The machine's display name and what its daemon last reported.
pub async fn machine_report(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
) -> Result<(String, Option<HarnessReport>), AppError> {
    let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT COALESCE(display_name, name), harness_report FROM machines WHERE id = $1",
    )
    .bind(machine_id)
    .fetch_optional(pool)
    .await?;
    let (name, report) = row.unwrap_or_default();
    Ok((name, report.and_then(|v| serde_json::from_value::<HarnessReport>(v).ok())))
}

/// What the machine's daemon last reported for its enabled harnesses' binaries.
pub async fn reported_binaries(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
) -> Result<Binaries, AppError> {
    Ok(machine_report(pool, machine_id).await?.1.map(|r| r.binaries).unwrap_or_default())
}

/// The refusal for a spawn whose harness binary the daemon reports missing.
/// A harness the daemon has not probed is let through.
#[must_use]
pub fn missing_binary(binaries: &Binaries, adapter_id: &str) -> Option<String> {
    binaries.get(adapter_id).and_then(|b| b.refusal(adapter_id))
}

/// The refusal for a spawn on a machine whose daemon reports a dropped
/// privacy grant. A daemon that does not probe refuses nothing.
#[must_use]
pub fn denied_health(report: Option<&HarnessReport>, machine: &str) -> Option<String> {
    report?.health.as_ref()?.refusal(machine)
}

/// Every spawn refusal the stored report implies, missing binary first.
#[must_use]
pub fn spawn_refusal(
    report: Option<&HarnessReport>,
    machine: &str,
    adapter_id: &str,
) -> Option<String> {
    let binaries = report.map(|r| &r.binaries);
    binaries.and_then(|b| missing_binary(b, adapter_id)).or_else(|| denied_health(report, machine))
}

async fn machine_exists(pool: &sqlx::PgPool, machine_id: Uuid) -> Result<(), AppError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND revoked_at IS NULL)",
    )
    .bind(machine_id)
    .fetch_one(pool)
    .await?;
    if found { Ok(()) } else { Err(AppError::new(StatusCode::NOT_FOUND, "machine not found")) }
}

async fn read_all(state: &AppState, machine_id: Uuid) -> ApiResult<Vec<MachineAdapterInfo>> {
    machine_exists(&state.pool, machine_id).await?;
    let binaries = reported_binaries(&state.pool, machine_id).await?;
    Ok(Json(merge(rows(&state.pool, machine_id).await?, &binaries)))
}

fn clean_adapter(adapter: &str) -> Result<String, AppError> {
    let id = adapter.trim();
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(id.to_owned())
    } else {
        Err(AppError::new(StatusCode::BAD_REQUEST, "adapter id must be lowercase [a-z0-9-]"))
    }
}

fn clean_config(config: serde_json::Value) -> Result<serde_json::Value, AppError> {
    if config.is_object() {
        Ok(config)
    } else {
        Err(AppError::new(StatusCode::BAD_REQUEST, "config must be a JSON object"))
    }
}

async fn push(state: &AppState, machine_id: Uuid) {
    if let Err(e) = crate::bus::push_reconcile(state, machine_id).await {
        tracing::debug!(%machine_id, error = %e, "adapter change: no live daemon to reconcile");
    }
}

pub async fn list(
    State(state): State<AppState>,
    Path(machine_id): Path<Uuid>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    read_all(&state, machine_id).await
}

pub async fn set(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((machine_id, adapter)): Path<(Uuid, String)>,
    Json(req): Json<SetMachineAdapterRequest>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    admin(&ctx)?;
    machine_exists(&state.pool, machine_id).await?;
    let adapter = clean_adapter(&adapter)?;
    let config = req.config.map(clean_config).transpose()?;
    sqlx::query(
        "INSERT INTO adapters_enabled (machine_id, adapter_id, config, enabled, updated_at) \
         VALUES ($1, $2, COALESCE($3, '{}'::jsonb), COALESCE($4, TRUE), now()) \
         ON CONFLICT (machine_id, adapter_id) DO UPDATE SET \
            config = COALESCE(EXCLUDED.config, adapters_enabled.config), \
            enabled = COALESCE($4, adapters_enabled.enabled), \
            updated_at = now()",
    )
    .bind(machine_id)
    .bind(&adapter)
    .bind(config)
    .bind(req.enabled)
    .execute(&state.pool)
    .await?;
    tracing::info!(
        %machine_id,
        adapter,
        enabled = ?req.enabled,
        by = ?ctx.user_id,
        "machine adapter set"
    );
    push(&state, machine_id).await;
    read_all(&state, machine_id).await
}

pub async fn reset(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((machine_id, adapter)): Path<(Uuid, String)>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    admin(&ctx)?;
    machine_exists(&state.pool, machine_id).await?;
    let adapter = clean_adapter(&adapter)?;
    sqlx::query("DELETE FROM adapters_enabled WHERE machine_id = $1 AND adapter_id = $2")
        .bind(machine_id)
        .bind(&adapter)
        .execute(&state.pool)
        .await?;
    tracing::info!(%machine_id, adapter, by = ?ctx.user_id, "machine adapter reset to default");
    push(&state, machine_id).await;
    read_all(&state, machine_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_rows_every_harness_reports_its_default() {
        let got = merge(Vec::new(), &Binaries::new());
        let ids: Vec<&str> = got.iter().map(|a| a.adapter_id.as_str()).collect();
        assert_eq!(ids, cctui_proto::adapter::KNOWN_ADAPTERS);
        assert!(got.iter().all(|a| !a.pinned && a.config == json!({})));
        for a in &got {
            assert_eq!(a.enabled, a.default_enabled, "{}", a.adapter_id);
            assert_eq!(
                a.enabled,
                cctui_proto::adapter::is_default_enabled(&a.adapter_id),
                "{}",
                a.adapter_id
            );
        }
    }

    #[test]
    fn a_row_pins_the_harness_and_an_unknown_row_is_listed_after_the_table() {
        let got = merge(
            vec![
                ("opencode".to_owned(), json!({"bin": "/opt/opencode"}), false),
                ("kilo".to_owned(), json!({}), true),
            ],
            &Binaries::new(),
        );
        let opencode = got.iter().find(|a| a.adapter_id == "opencode").unwrap();
        assert!(opencode.pinned && !opencode.enabled && opencode.default_enabled);
        assert_eq!(opencode.config, json!({"bin": "/opt/opencode"}));
        let last = got.last().unwrap();
        assert_eq!(last.adapter_id, "kilo");
        assert!(last.pinned && last.enabled && !last.default_enabled);
    }

    fn binary(bin: &str, path: Option<&str>) -> AdapterBinary {
        AdapterBinary {
            bin: bin.to_owned(),
            config_key: "bin".to_owned(),
            path: path.map(str::to_owned),
        }
    }

    #[test]
    fn each_harness_carries_the_binary_its_daemon_reported() {
        let binaries = Binaries::from([
            ("codex".to_owned(), binary("codex", None)),
            ("gemini".to_owned(), binary("gemini", Some("/usr/bin/gemini"))),
        ]);
        let got = merge(vec![("gemini".to_owned(), json!({}), true)], &binaries);
        let find = |id: &str| got.iter().find(|a| a.adapter_id == id).unwrap();
        assert_eq!(find("codex").binary, Some(binary("codex", None)));
        assert_eq!(find("claude-code").binary, None, "an unprobed harness reports nothing");
        assert!(find("gemini").binary.as_ref().is_some_and(AdapterBinary::installed));
    }

    #[test]
    fn a_denied_grant_refuses_every_spawn_and_names_the_machine() {
        use cctui_proto::harness::MachineHealth;
        let healthy = HarnessReport { health: Some(MachineHealth::Ok), ..Default::default() };
        assert_eq!(spawn_refusal(Some(&healthy), "m2pro", "claude-code"), None);
        assert_eq!(spawn_refusal(None, "m2pro", "claude-code"), None, "no report: no refusal");
        let unprobed = HarnessReport::default();
        assert_eq!(spawn_refusal(Some(&unprobed), "m2pro", "codex"), None);

        let denied = HarnessReport {
            health: Some(MachineHealth::NetworkDenied { detail: "connect: EHOSTUNREACH".into() }),
            binaries: Binaries::from([("codex".to_owned(), binary("codex", None))]),
            ..Default::default()
        };
        let refusal = spawn_refusal(Some(&denied), "m2pro", "claude-code").unwrap();
        assert!(refusal.contains("on m2pro"), "{refusal}");
        assert!(refusal.contains("doctor --grant"), "{refusal}");
        assert!(refusal.contains("EHOSTUNREACH"), "{refusal}");
        let missing = spawn_refusal(Some(&denied), "m2pro", "codex").unwrap();
        assert!(missing.contains("codex is not installed"), "the missing binary is named first");
    }

    #[test]
    fn only_a_binary_reported_missing_refuses_a_spawn() {
        let binaries = Binaries::from([
            ("codex".to_owned(), binary("codex", None)),
            ("opencode".to_owned(), binary("opencode", Some("/usr/bin/opencode"))),
        ]);
        let refusal = missing_binary(&binaries, "codex").unwrap();
        assert!(refusal.contains("codex is not installed on this machine"), "{refusal}");
        assert_eq!(missing_binary(&binaries, "opencode"), None);
        assert_eq!(missing_binary(&binaries, "claude-code"), None);
        assert_eq!(missing_binary(&Binaries::new(), "codex"), None);
    }

    #[test]
    fn adapter_ids_are_lowercase_slugs_and_configs_are_objects() {
        assert_eq!(clean_adapter(" codex ").unwrap(), "codex");
        assert!(clean_adapter("Codex").is_err());
        assert!(clean_adapter("").is_err());
        assert!(clean_adapter("a/b").is_err());
        assert!(clean_config(json!({"bin": "x"})).is_ok());
        assert!(clean_config(json!(["x"])).is_err());
        assert!(clean_config(json!(null)).is_err());
    }
}
