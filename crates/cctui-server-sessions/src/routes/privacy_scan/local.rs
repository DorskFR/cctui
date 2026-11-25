//! Privacy scan of the harness transcripts kept on the user's machines.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use cctui_server_kernel::bus::{BusError, DaemonRequest, DaemonResponse};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

pub use cctui_proto::api::privacy_scan::{
    LocalTranscriptsScan, MachineScanState, MachineTranscriptScan, NotScannedReason, ScanCategory,
    ScanSample, TranscriptAuditReport,
};

use crate::context::SessionsContext;
use crate::routes::settings::secret_scrub_of;

const ONLINE_WINDOW_SECS: i64 = 5 * 60;

const CANCEL_POLL: std::time::Duration = std::time::Duration::from_secs(1);

pub type Shared = Arc<Mutex<Vec<MachineTranscriptScan>>>;

pub struct Running {
    machines: Shared,
    task: tokio::task::JoinHandle<()>,
}

type MachineRow = (Uuid, String, bool, bool);

#[must_use]
pub fn initial(id: Uuid, name: String, online: bool, capable: bool) -> MachineTranscriptScan {
    let (state, reason) = match (online, capable) {
        (false, _) => (MachineScanState::NotScanned, Some(NotScannedReason::Offline)),
        (true, false) => (MachineScanState::NotScanned, Some(NotScannedReason::Outdated)),
        (true, true) => (MachineScanState::Pending, None),
    };
    MachineTranscriptScan {
        machine_id: id.to_string(),
        machine_name: name,
        state,
        reason,
        detail: None,
        report: None,
    }
}

pub fn settle(entry: &mut MachineTranscriptScan, outcome: Result<DaemonResponse, BusError>) {
    let not_scanned = |reason, detail| (MachineScanState::NotScanned, Some(reason), detail, None);
    let (state, reason, detail, report) = match outcome {
        Ok(DaemonResponse::TranscriptAudit(report)) => {
            (MachineScanState::Scanned, None, None, Some(*report))
        }
        Ok(_) => not_scanned(NotScannedReason::Failed, Some("unexpected daemon reply".to_owned())),
        Err(BusError::NoDaemon(_) | BusError::Closed | BusError::Disconnected) => {
            not_scanned(NotScannedReason::Offline, None)
        }
        Err(BusError::Timeout) => not_scanned(NotScannedReason::Timeout, None),
        Err(BusError::TranscriptAudit(msg)) => not_scanned(NotScannedReason::Failed, Some(msg)),
        Err(err) => not_scanned(NotScannedReason::Failed, Some(err.to_string())),
    };
    entry.state = state;
    entry.reason = reason;
    entry.detail = detail;
    entry.report = report;
}

pub fn cancel_pending(machines: &mut [MachineTranscriptScan]) {
    for entry in machines.iter_mut().filter(|m| m.state == MachineScanState::Pending) {
        entry.state = MachineScanState::NotScanned;
        entry.reason = Some(NotScannedReason::Cancelled);
    }
}

/// `None` when the user has not opted in.
pub async fn start<S: SessionsContext>(
    state: &S,
    job: Uuid,
    user_id: Uuid,
    dry_run: bool,
) -> Result<Option<Running>, sqlx::Error> {
    let data: Value = sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(state.pool())
        .await?
        .unwrap_or(Value::Null);
    if !secret_scrub_of(&data).local_transcripts {
        return Ok(None);
    }
    let rows: Vec<MachineRow> = sqlx::query_as(
        "SELECT id, COALESCE(display_name, name), \
                last_seen_at > now() - $2::bigint * interval '1 second', \
                COALESCE(transcript_audit, false) \
         FROM machines \
         WHERE user_id = $1 AND revoked_at IS NULL AND deleted_at IS NULL AND kind <> 'dispatch' \
         ORDER BY 2, 1",
    )
    .bind(user_id)
    .bind(ONLINE_WINDOW_SECS)
    .fetch_all(state.pool())
    .await?;
    let entries: Vec<MachineTranscriptScan> = rows
        .into_iter()
        .map(|(id, name, online, capable)| initial(id, name, online, capable))
        .collect();
    save(state.pool(), job, &entries).await?;
    let machines: Shared = Arc::new(Mutex::new(entries));
    let task = tokio::spawn(scan(state.clone(), job, dry_run, Arc::clone(&machines)));
    Ok(Some(Running { machines, task }))
}

async fn scan<S: SessionsContext>(state: S, job: Uuid, dry_run: bool, machines: Shared) {
    let mut asks: FuturesUnordered<_> = lock(&machines)
        .iter()
        .enumerate()
        .filter(|(_, m)| m.state == MachineScanState::Pending)
        .filter_map(|(i, m)| Uuid::parse_str(&m.machine_id).ok().map(|id| (i, id)))
        .map(|(i, machine)| {
            let bus = state.bus().clone();
            async move {
                let request = DaemonRequest::TranscriptAudit { dry_run };
                (i, bus.request_daemon(machine, request).await)
            }
        })
        .collect();
    while let Some((i, outcome)) = asks.next().await {
        let snapshot = {
            let mut entries = lock(&machines);
            if let Some(entry) = entries.get_mut(i) {
                settle(entry, outcome);
            }
            entries.clone()
        };
        if let Err(e) = save(state.pool(), job, &snapshot).await {
            tracing::warn!("privacy scan could not record a machine's transcript audit: {e}");
        }
    }
}

/// Wait for every machine to answer; `false` when the user cancelled first.
pub async fn wait(pool: &PgPool, job: Uuid, running: &mut Running) -> bool {
    loop {
        tokio::select! {
            _ = &mut running.task => return true,
            () = tokio::time::sleep(CANCEL_POLL) => {
                if super::cancel_requested(pool, job).await {
                    return false;
                }
            }
        }
    }
}

/// Stop asking and record the machines still pending as cancelled. A daemon
/// already rewriting finishes its pass; its result is no longer recorded.
pub async fn abort(pool: &PgPool, job: Uuid, running: Running) {
    running.task.abort();
    let _ = running.task.await;
    let snapshot = {
        let mut entries = lock(&running.machines);
        cancel_pending(&mut entries);
        entries.clone()
    };
    if let Err(e) = save(pool, job, &snapshot).await {
        tracing::warn!("privacy scan could not record cancelled machines: {e}");
    }
}

fn lock(machines: &Shared) -> std::sync::MutexGuard<'_, Vec<MachineTranscriptScan>> {
    machines.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn save(
    pool: &PgPool,
    job: Uuid,
    machines: &[MachineTranscriptScan],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE privacy_scan_jobs SET local_transcripts = $2, updated_at = now() WHERE id = $1",
    )
    .bind(job)
    .bind(serde_json::to_value(machines).unwrap_or_else(|_| serde_json::json!([])))
    .execute(pool)
    .await?;
    Ok(())
}

/// The job view of the stored machines: their findings summed per category.
#[must_use]
pub fn summarize(stored: Option<Value>) -> Option<LocalTranscriptsScan> {
    let machines: Vec<MachineTranscriptScan> = serde_json::from_value(stored?).ok()?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut samples: BTreeMap<String, Vec<ScanSample>> = BTreeMap::new();
    let (mut substitutions, mut files_changed) = (0, 0);
    for report in machines.iter().filter_map(|m| m.report.as_ref()) {
        substitutions += report.substitutions;
        files_changed += report.files_changed;
        for (cat, n) in &report.by_category {
            *counts.entry(cat.clone()).or_insert(0) += n;
        }
        for (cat, list) in &report.samples {
            let kept = samples.entry(cat.clone()).or_default();
            let room = cctui_crypto::redact::SAMPLES_PER_CATEGORY.saturating_sub(kept.len());
            kept.extend(list.iter().take(room).cloned());
        }
    }
    let builtin: std::collections::BTreeSet<&str> =
        cctui_crypto::redact::builtin_categories().into_iter().map(|(c, _)| c).collect();
    let mut categories: Vec<ScanCategory> = counts
        .into_iter()
        .map(|(category, count)| {
            let samples = samples.remove(&category).unwrap_or_default();
            let with_value = samples.iter().filter(|s| s.value_follows).count();
            let identifier_warning = !builtin.contains(category.as_str())
                && samples.len() >= 3
                && with_value * 2 < samples.len();
            ScanCategory { category, count, samples, identifier_warning }
        })
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.category.cmp(&b.category)));
    Some(LocalTranscriptsScan { machines, categories, substitutions, files_changed })
}
