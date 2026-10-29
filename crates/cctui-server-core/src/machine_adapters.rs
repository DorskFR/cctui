//! Machine harness reports and launch refusal reasons.
use crate::error::AppError;
use cctui_proto::harness::{AdapterBinary, HarnessReport};
use uuid::Uuid;
type Binaries = std::collections::BTreeMap<String, AdapterBinary>;
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

/// Harness enablement/configuration and live reconciliation shared by both hosts.
pub mod configuration;
