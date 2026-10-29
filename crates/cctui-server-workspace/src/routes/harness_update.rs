//! Harness auto-update policy: instance default in
//! `instance_settings.harness_autoupdate`, per-machine override in
//! `machines.harness_autoupdate`, and the per-machine report the heartbeat
//! carries. The policy reaches a daemon in reply to its heartbeat, only when
//! the one it holds differs, so a daemon that cannot parse the frame (it sends
//! no report) never gets one and an absent setting sends nothing.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::harness::{HarnessReport, HarnessUpdatePolicy};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;

pub use cctui_proto::api::harness_update::{
    HarnessAutoupdateInfo, HarnessPolicyRequest, HarnessRestartRequest, MachineHarnessInfo,
};

const KEY: &str = "harness_autoupdate";

type ApiResult<T> = Result<Json<T>, AppError>;

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

fn parse(v: Option<serde_json::Value>) -> Option<HarnessUpdatePolicy> {
    v.and_then(|v| serde_json::from_value::<HarnessUpdatePolicy>(v).ok())
        .map(HarnessUpdatePolicy::normalized)
}

async fn instance_policy(pool: &sqlx::PgPool) -> Option<HarnessUpdatePolicy> {
    parse(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT value FROM instance_settings WHERE key = $1",
        )
        .bind(KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten(),
    )
}

async fn machine_policy(pool: &sqlx::PgPool, machine_id: Uuid) -> Option<HarnessUpdatePolicy> {
    parse(
        sqlx::query_scalar::<_, Option<serde_json::Value>>(
            "SELECT harness_autoupdate FROM machines WHERE id = $1",
        )
        .bind(machine_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten(),
    )
}

pub async fn effective_policy(pool: &sqlx::PgPool, machine_id: Uuid) -> HarnessUpdatePolicy {
    let instance = instance_policy(pool).await;
    let machine = machine_policy(pool, machine_id).await;
    HarnessUpdatePolicy::resolve(instance.as_ref(), machine.as_ref())
}

/// Persist the heartbeat's report and resend the policy if the daemon holds
/// a different one.
pub async fn on_heartbeat(state: &AppState, machine_id: Uuid, report: &HarnessReport) {
    let stored = HarnessReport { policy: None, ..report.clone() };
    if let Err(err) = sqlx::query(
        "UPDATE machines SET harness_report = $2, harness_report_at = now() \
         WHERE id = $1 AND harness_report IS DISTINCT FROM $2",
    )
    .bind(machine_id)
    .bind(serde_json::to_value(&stored).expect("serializable"))
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%err, %machine_id, "harness report write failed");
    }
    let policy = effective_policy(&state.pool, machine_id).await;
    if !report.needs_policy(&policy) {
        return;
    }
    if let Err(err) =
        state.bus.command_daemon(machine_id, DaemonFrameDown::HarnessUpdatePolicy { policy }).await
    {
        tracing::warn!(%err, %machine_id, "could not send HarnessUpdatePolicy");
    }
}

async fn read_info(pool: &sqlx::PgPool) -> Result<HarnessAutoupdateInfo, sqlx::Error> {
    type Row = (
        Uuid,
        String,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let instance = instance_policy(pool).await;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, COALESCE(display_name, name), harness_autoupdate, harness_report, \
                harness_report_at \
         FROM machines WHERE revoked_at IS NULL AND deleted_at IS NULL AND kind = 'persistent' \
         ORDER BY COALESCE(display_name, name)",
    )
    .fetch_all(pool)
    .await?;
    let machines = rows
        .into_iter()
        .map(|(id, name, policy, report, report_at)| {
            let policy = parse(policy);
            MachineHarnessInfo {
                machine_id: id.to_string(),
                name,
                effective: HarnessUpdatePolicy::resolve(instance.as_ref(), policy.as_ref()),
                policy,
                report: report.and_then(|v| serde_json::from_value(v).ok()),
                report_at,
            }
        })
        .collect();
    Ok(HarnessAutoupdateInfo { instance, machines })
}

pub async fn read(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<HarnessAutoupdateInfo> {
    admin(&ctx)?;
    Ok(Json(read_info(&state.pool).await?))
}

pub async fn set_instance(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<HarnessPolicyRequest>,
) -> ApiResult<HarnessAutoupdateInfo> {
    admin(&ctx)?;
    match req.policy.map(HarnessUpdatePolicy::normalized) {
        Some(policy) => {
            sqlx::query(
                "INSERT INTO instance_settings (key, value, updated_at) VALUES ($1, $2, now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(KEY)
            .bind(serde_json::to_value(&policy).expect("serializable"))
            .execute(&state.pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM instance_settings WHERE key = $1")
                .bind(KEY)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(Json(read_info(&state.pool).await?))
}

pub async fn set_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<Uuid>,
    Json(req): Json<HarnessPolicyRequest>,
) -> ApiResult<HarnessAutoupdateInfo> {
    admin(&ctx)?;
    let value = req.policy.map(|p| serde_json::to_value(p.normalized()).expect("serializable"));
    let res = sqlx::query(
        "UPDATE machines SET harness_autoupdate = $2 WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(machine_id)
    .bind(value)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    Ok(Json(read_info(&state.pool).await?))
}

/// The harnesses whose daemon can restart its long-lived process once idle.
const DRAIN_RESTARTABLE: &[&str] = &[cctui_proto::harness::HARNESS_CLAUDE_CODE];

fn drain_restartable(harness: &str) -> bool {
    DRAIN_RESTARTABLE.contains(&harness)
}

/// The daemon frame parser drops an unknown `drain_restart`, so a daemon older
/// than 0.25.7 would ignore the request. Those daemons never report a health
/// verdict, which every later daemon sends from its first heartbeats.
fn drain_supported(report: Option<&HarnessReport>) -> bool {
    report.is_some_and(|r| r.health.is_some())
}

fn daemon_too_old(machine: &str) -> String {
    format!(
        "the cctui-daemon on {machine} is older than 0.25.7 and would ignore a drain restart; \
         update it first"
    )
}

/// Ask a machine's daemon to restart `harness` onto the installed CLI as soon
/// as it has no live worker. Accepted, not done: the daemon decides when.
pub async fn restart_harness(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<Uuid>,
    Json(req): Json<HarnessRestartRequest>,
) -> Result<StatusCode, AppError> {
    admin(&ctx)?;
    if !drain_restartable(&req.harness) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("{} has no drain restart", req.harness),
        ));
    }
    let online: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND revoked_at IS NULL)",
    )
    .bind(machine_id)
    .fetch_one(&state.pool)
    .await?;
    if !online {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    let (name, report) = super::machine_adapters::machine_report(&state.pool, machine_id).await?;
    if !drain_supported(report.as_ref()) {
        return Err(AppError::new(StatusCode::CONFLICT, daemon_too_old(&name)));
    }
    state
        .bus
        .command_daemon(machine_id, DaemonFrameDown::DrainRestart { harness: req.harness.clone() })
        .await
        .map_err(|err| {
            AppError::new(StatusCode::SERVICE_UNAVAILABLE, format!("daemon unreachable: {err}"))
        })?;
    tracing::info!(%machine_id, harness = %req.harness, "drain restart requested");
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use cctui_proto::harness::{HarnessReport, MachineHealth};

    use super::{daemon_too_old, drain_restartable, drain_supported};

    #[test]
    fn only_claude_code_has_a_drain_restart() {
        assert!(drain_restartable("claude-code"));
        assert!(!drain_restartable("codex"), "codex cycles itself from the updater");
        assert!(!drain_restartable("gemini"));
    }

    #[test]
    fn only_a_daemon_that_reports_health_can_drain() {
        assert!(!drain_supported(None), "no report at all");
        assert!(!drain_supported(Some(&HarnessReport::default())), "pre-0.25.7 report");
        let report = HarnessReport { health: Some(MachineHealth::Ok), ..HarnessReport::default() };
        assert!(drain_supported(Some(&report)));
        let denied = MachineHealth::FolderDenied { detail: "EPERM".to_owned() };
        let report = HarnessReport { health: Some(denied), ..HarnessReport::default() };
        assert!(drain_supported(Some(&report)), "a denial is still a 0.25.7+ daemon");
    }

    #[test]
    fn the_refusal_names_the_machine_and_the_fix() {
        let msg = daemon_too_old("m2pro");
        assert!(msg.contains("m2pro") && msg.contains("0.25.7") && msg.contains("update it first"));
    }
}
