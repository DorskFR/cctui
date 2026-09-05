//! The deterministic half of self-update: `self_update_runs` and the two
//! daemon-facing endpoints that feed it.
//!
//! A hook run is the one operation in cctui whose *caller does not survive it*
//! — the update restarts this process. So nothing about a run lives in memory:
//! the row is written before the frame goes out, and the daemon reports
//! progress over HTTP to whichever server process is alive by then.
//!
//! Endpoints here carry machine-key Bearer auth (like `daemon/blobs`), so they
//! sit outside the user-token `api_router`:
//!
//! - `GET /api/v1/daemon/version` — what the deployment currently serves. The
//!   daemon's health check needs this and cannot call `/api/v1/version`, which
//!   requires a *user* token.
//! - `POST /api/v1/daemon/update-hook/{run_id}` — one progress report.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use cctui_proto::updatehook::{UpdateHookPhase, UpdateHookReport};
use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

use crate::state::AppState;

/// A hook run as the webui sees it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SelfUpdateRun {
    pub id: Uuid,
    pub machine_id: Uuid,
    /// Version this run is deploying.
    pub version: String,
    /// Version that was running when the run started.
    pub from_version: String,
    pub phase: UpdateHookPhase,
    /// Whether the run has finished, either way.
    pub done: bool,
    pub exit_code: Option<i32>,
    pub detail: String,
    /// Tail of the hook's output; `null` until a command has produced any.
    pub output_tail: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Row shape shared by the queries below.
#[derive(Clone)]
struct RunRow {
    id: Uuid,
    machine_id: Uuid,
    version: String,
    from_version: String,
    phase: String,
    exit_code: Option<i32>,
    detail: String,
    output_tail: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RunRow> for SelfUpdateRun {
    fn from(r: RunRow) -> Self {
        let phase = UpdateHookPhase::from_str_lossy(&r.phase);
        Self {
            id: r.id,
            machine_id: r.machine_id,
            version: r.version,
            from_version: r.from_version,
            phase,
            done: phase.is_terminal(),
            exit_code: r.exit_code,
            detail: r.detail,
            output_tail: r.output_tail,
            started_at: r.started_at,
            updated_at: r.updated_at,
        }
    }
}

fn row(r: &sqlx::postgres::PgRow) -> RunRow {
    use sqlx::Row;
    RunRow {
        id: r.get("id"),
        machine_id: r.get("machine_id"),
        version: r.get("version"),
        from_version: r.get("from_version"),
        phase: r.get("phase"),
        exit_code: r.get("exit_code"),
        detail: r.get("detail"),
        output_tail: r.get("output_tail"),
        started_at: r.get("started_at"),
        updated_at: r.get("updated_at"),
    }
}

/// Whether `machine` advertised a deterministic update hook on its last
/// heartbeat. A machine we know nothing about has none.
pub async fn machine_has_hook(pool: &sqlx::PgPool, machine: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT update_hook FROM machines WHERE id = $1")
        .bind(machine)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// Record the daemon's advertised hook flag, but only when it changed — this
/// runs on every heartbeat of every machine.
pub async fn record_hook_flag(pool: &sqlx::PgPool, machine: Uuid, has_hook: bool) {
    if let Err(err) = sqlx::query(
        "UPDATE machines SET update_hook = $2 WHERE id = $1 AND update_hook IS DISTINCT FROM $2",
    )
    .bind(machine)
    .bind(has_hook)
    .execute(pool)
    .await
    {
        tracing::warn!(%err, %machine, "could not record the update-hook flag");
    }
}

/// Open a run row *before* dispatching the frame, so a run is never invisible:
/// if the update lands before the daemon's first report, the row is already
/// there to be updated.
pub async fn open_run(
    pool: &sqlx::PgPool,
    machine: Uuid,
    version: &str,
    from_version: &str,
    started_by: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO self_update_runs (id, machine_id, version, from_version, phase, detail, started_by) \
         VALUES ($1, $2, $3, $4, 'running', 'dispatched to the machine', $5)",
    )
    .bind(id)
    .bind(machine)
    .bind(version)
    .bind(from_version)
    .bind(started_by)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Drop a run whose frame never made it out, so a failed dispatch doesn't
/// leave a run that will never be reported on.
pub async fn abandon_run(pool: &sqlx::PgPool, run: Uuid) {
    let _ = sqlx::query("DELETE FROM self_update_runs WHERE id = $1").bind(run).execute(pool).await;
}

/// The most recent run, whatever its state — what the webui polls.
pub async fn latest_run(pool: &sqlx::PgPool) -> Option<SelfUpdateRun> {
    sqlx::query(
        "SELECT id, machine_id, version, from_version, phase, exit_code, detail, output_tail, \
                started_at, updated_at \
           FROM self_update_runs ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|r| row(&r).into())
}

async fn require_machine(
    state: &AppState,
    headers: &header::HeaderMap,
) -> Result<Uuid, StatusCode> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    ctx.machine_id.ok_or(StatusCode::FORBIDDEN)
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct DaemonVersion {
    pub version: &'static str,
    pub git_hash: &'static str,
}

/// The version this deployment is serving, for a daemon's post-update health
/// check. Deliberately tiny and machine-authed: `/api/v1/version` needs a user
/// token, and no endpoint should become anonymous just to be probed.
pub async fn daemon_version(
    State(state): State<AppState>,
    headers: header::HeaderMap,
) -> Result<Json<DaemonVersion>, StatusCode> {
    require_machine(&state, &headers).await?;
    Ok(Json(DaemonVersion {
        version: env!("CARGO_PKG_VERSION"),
        git_hash: crate::routes::web::GIT_HASH,
    }))
}

/// One progress report from a daemon running a hook.
///
/// Only the machine the run was dispatched to may report on it, and a
/// terminal run is immutable — a late duplicate report cannot un-finish a run
/// that already succeeded.
pub async fn report(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Path(run_id): Path<Uuid>,
    Json(body): Json<UpdateHookReport>,
) -> Result<StatusCode, StatusCode> {
    let machine = require_machine(&state, &headers).await?;

    let affected = sqlx::query(
        "UPDATE self_update_runs \
            SET phase = $3, \
                exit_code = COALESCE($4, exit_code), \
                detail = $5, \
                output_tail = COALESCE($6, output_tail), \
                updated_at = now() \
          WHERE id = $1 AND machine_id = $2 \
            AND phase NOT IN ('succeeded', 'rolled_back', 'failed')",
    )
    .bind(run_id)
    .bind(machine)
    .bind(body.phase.as_str())
    .bind(body.exit_code)
    .bind(&body.detail)
    .bind(body.output_tail.as_deref())
    .execute(&state.pool)
    .await
    .map_err(|err| {
        tracing::error!(%err, %run_id, "update hook report could not be stored");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .rows_affected();

    if affected == 0 {
        // Either the run is not this machine's, does not exist, or is already
        // finished. None of those is worth a retry from the daemon.
        return Ok(StatusCode::NO_CONTENT);
    }
    tracing::info!(%run_id, %machine, phase = body.phase.as_str(), detail = %body.detail, "update hook progress");
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_done_exactly_on_the_terminal_phases() {
        let base = RunRow {
            id: Uuid::nil(),
            machine_id: Uuid::nil(),
            version: "1.0.1".into(),
            from_version: "1.0.0".into(),
            phase: "running".into(),
            exit_code: None,
            detail: String::new(),
            output_tail: None,
            started_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let with = |p: &str| SelfUpdateRun::from(RunRow { phase: p.into(), ..base.clone() });
        assert!(!with("running").done);
        assert!(!with("verifying").done);
        assert!(!with("rolling_back").done);
        assert!(with("succeeded").done);
        assert!(with("rolled_back").done);
        assert!(with("failed").done);
        // An unreadable phase must not read as a success.
        let unknown = with("garbage");
        assert!(unknown.done);
        assert_eq!(unknown.phase, UpdateHookPhase::Failed);
    }
}
