//! Privacy scan: re-apply the caller's effective scrub list to their stored
//! `stream_events` as a background job.
//!
//! The sweep takes minutes over a real history, so the HTTP layer only starts,
//! polls and cancels it. Job state lives in `privacy_scan_jobs` rather than in
//! the starting pod's memory: the server runs behind a load balancer, so a
//! progress poll or a cancel routinely lands on a different replica than the
//! worker. The database is the rendez-vous point — the worker heartbeats
//! progress into the row and reads `cancel_requested` back out once per batch.
//!
//! Samples are stored and returned, so they never carry a credential: the
//! redactor masks the value side of every match before it leaves memory.
//!
//! Regex work runs on `spawn_blocking`; apply-mode writes go out one
//! transaction per batch, and the progress counters only advance after that
//! transaction commits, so a cancel can never leave a row half-written or
//! reported as changed when it was not.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::routes::settings::secret_scrub_of;
use crate::state::AppState;

pub use cctui_proto::api::privacy_scan::{
    PrivacyScanJob, RescrubRequest, ScanCategory, ScanSample,
};

/// id-keyset batch size for the sweep.
const BATCH: i64 = 500;

#[derive(Serialize, Deserialize, Default, Clone)]
struct StoredSamples {
    samples: Vec<ScanSample>,
    examined: usize,
    with_value: usize,
}

impl StoredSamples {
    fn merge(&mut self, other: &cctui_crypto::redact::CategorySamples) {
        self.examined += other.examined;
        self.with_value += other.with_value;
        for s in &other.samples {
            if self.samples.len() >= cctui_crypto::redact::SAMPLES_PER_CATEGORY {
                break;
            }
            self.samples.push(ScanSample {
                text: s.text.clone(),
                context: s.context.clone(),
                value_follows: s.value_follows,
            });
        }
    }

    const fn looks_like_identifiers(&self) -> bool {
        self.examined >= 3 && self.with_value * 2 < self.examined
    }
}

type JobRow = (
    Uuid,
    String,
    bool,
    bool,
    Option<i64>,
    i64,
    i64,
    i64,
    Value,
    Value,
    Option<String>,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// sqlx 0.9 takes only `&'static str`, so the shared column list is spliced at
/// compile time instead of being formatted in.
macro_rules! job_query {
    ($tail:literal) => {
        concat!(
            "SELECT id, status, dry_run, cancel_requested, rows_total, rows_scanned, ",
            "rows_changed, substitutions, by_category, samples, error, created_at, ",
            "finished_at FROM privacy_scan_jobs ",
            $tail
        )
    };
}

/// A `running` row whose worker died with its pod is reaped after this, so a
/// lost pod cannot wedge the user's one-scan-at-a-time slot.
const REAP_STALE: &str = "UPDATE privacy_scan_jobs \
     SET status = 'failed', error = 'interrupted', finished_at = now() \
     WHERE user_id = $1 AND status = 'running' \
       AND updated_at < now() - interval '10 minutes'";

fn to_job(row: JobRow) -> PrivacyScanJob {
    let (
        id,
        status,
        dry_run,
        cancel_requested,
        rows_total,
        rows_scanned,
        rows_changed,
        substitutions,
        by_category,
        samples,
        error,
        created_at,
        finished_at,
    ) = row;
    let by_category: BTreeMap<String, i64> =
        serde_json::from_value(by_category).unwrap_or_default();
    let samples: BTreeMap<String, StoredSamples> =
        serde_json::from_value(samples).unwrap_or_default();
    let builtin: std::collections::BTreeSet<&str> =
        cctui_crypto::redact::builtin_categories().into_iter().map(|(c, _)| c).collect();
    let mut categories: Vec<ScanCategory> = by_category
        .iter()
        .map(|(category, &count)| {
            let s = samples.get(category);
            ScanCategory {
                category: category.clone(),
                count,
                samples: s.map(|s| s.samples.clone()).unwrap_or_default(),
                identifier_warning: !builtin.contains(category.as_str())
                    && s.is_some_and(StoredSamples::looks_like_identifiers),
            }
        })
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.category.cmp(&b.category)));
    PrivacyScanJob {
        id: id.to_string(),
        status,
        dry_run,
        cancel_requested,
        rows_total,
        rows_scanned,
        rows_changed,
        substitutions,
        by_category,
        categories,
        error,
        created_at,
        finished_at,
    }
}

async fn load(pool: &PgPool, id: Uuid) -> Result<Option<PrivacyScanJob>, sqlx::Error> {
    let row: Option<JobRow> =
        sqlx::query_as(job_query!("WHERE id = $1")).bind(id).fetch_optional(pool).await?;
    Ok(row.map(to_job))
}

fn db_error(e: &sqlx::Error) -> StatusCode {
    tracing::error!("privacy scan db error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// `POST /api/v1/settings/rescrub`: start a scan and return its job id
/// immediately. One scan per user at a time.
pub async fn start(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<RescrubRequest>,
) -> Result<Json<PrivacyScanJob>, StatusCode> {
    reap_stale(&state.pool, ctx.user_id).await.map_err(|e| db_error(&e))?;

    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO privacy_scan_jobs (id, user_id, dry_run, session_ids, since, status) \
         VALUES ($1, $2, $3, $4, $5, 'running') ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(ctx.user_id)
    .bind(req.dry_run)
    .bind(req.session_ids.as_deref())
    .bind(req.since)
    .execute(&state.pool)
    .await
    .map_err(|e| db_error(&e))?;
    if inserted.rows_affected() == 0 {
        return Err(StatusCode::CONFLICT);
    }

    let spec = JobSpec {
        id,
        user_id: ctx.user_id,
        dry_run: req.dry_run,
        session_ids: req.session_ids,
        since: req.since,
    };
    let worker_state = state.clone();
    tokio::spawn(async move { run(worker_state, spec).await });

    load(&state.pool, id).await.map_err(|e| db_error(&e))?.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/v1/settings/rescrub`: the caller's most recent scan, so a browser
/// that navigated away re-attaches to a job still running on some replica.
pub async fn latest(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Option<PrivacyScanJob>>, StatusCode> {
    reap_stale(&state.pool, ctx.user_id).await.map_err(|e| db_error(&e))?;
    let row: Option<JobRow> =
        sqlx::query_as(job_query!("WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1"))
            .bind(ctx.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_error(&e))?;
    Ok(Json(row.map(to_job)))
}

/// `POST /api/v1/settings/rescrub/cancel`: ask the worker — wherever it runs —
/// to stop. It notices within one batch.
pub async fn cancel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Option<PrivacyScanJob>>, StatusCode> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE privacy_scan_jobs SET cancel_requested = true \
         WHERE user_id = $1 AND status = 'running' RETURNING id",
    )
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_error(&e))?;
    let Some(id) = id else { return Ok(Json(None)) };
    load(&state.pool, id).await.map(Json).map_err(|e| db_error(&e))
}

async fn reap_stale(pool: &PgPool, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(REAP_STALE).bind(user_id).execute(pool).await?;
    Ok(())
}

pub struct JobSpec {
    pub id: Uuid,
    pub user_id: Uuid,
    pub dry_run: bool,
    pub session_ids: Option<Vec<String>>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Default)]
struct Progress {
    rows_scanned: i64,
    rows_changed: i64,
    substitutions: i64,
    by_category: BTreeMap<String, i64>,
    samples: BTreeMap<String, StoredSamples>,
}

struct BatchOutcome {
    scanned: i64,
    updates: Vec<(i64, Value)>,
    stats: BTreeMap<String, usize>,
    samples: BTreeMap<String, cctui_crypto::redact::CategorySamples>,
    changed: i64,
    cursor: i64,
}

/// Scan one batch off the async executor: ~70 regexes over 500 payloads is CPU
/// work that would otherwise stall the runtime it shares with every request.
fn scan_batch(
    rows: Vec<(i64, Value)>,
    patterns: &cctui_crypto::redact::CompiledPatterns,
    want_samples: bool,
) -> BatchOutcome {
    let mut out = BatchOutcome {
        scanned: 0,
        updates: Vec::new(),
        stats: BTreeMap::new(),
        samples: BTreeMap::new(),
        changed: 0,
        cursor: 0,
    };
    for (id, mut payload) in rows {
        out.cursor = id;
        out.scanned += 1;
        if want_samples {
            cctui_crypto::redact::collect_samples(&payload, patterns, &mut out.samples);
        }
        let hits = cctui_crypto::redact::redact_json_stats(&mut payload, patterns);
        if hits.is_empty() {
            continue;
        }
        out.changed += 1;
        for (cat, c) in hits {
            *out.stats.entry(cat).or_insert(0) += c;
        }
        out.updates.push((id, payload));
    }
    out
}

const SET_SQL: &str = "UPDATE stream_events SET payload = u.payload \
                       FROM UNNEST($1::bigint[], $2::jsonb[]) AS u (id, payload) \
                       WHERE stream_events.id = u.id";

/// Write one batch of redacted payloads. A single statement is its own
/// transaction, so a cancel between batches leaves whole rows behind, never a
/// partially rewritten one. Returns how many rows were written.
async fn apply_batch(pool: &PgPool, updates: &[(i64, Value)]) -> usize {
    if updates.is_empty() {
        return 0;
    }
    let ids: Vec<i64> = updates.iter().map(|(id, _)| *id).collect();
    let payloads: Vec<Value> = updates.iter().map(|(_, p)| p.clone()).collect();

    match sqlx::query(SET_SQL).bind(&ids).bind(&payloads).execute(pool).await {
        Ok(_) => ids.len(),
        Err(e) => {
            // A redacted payload can collide with an existing redacted row on
            // the (session, type, content_hash, turn) dedup index, which aborts
            // the whole statement; retry row by row and skip the collisions.
            tracing::warn!("privacy scan batch update fell back to per-row: {e}");
            let mut written = 0;
            for (id, payload) in updates {
                match sqlx::query("UPDATE stream_events SET payload = $1 WHERE id = $2")
                    .bind(payload)
                    .bind(id)
                    .execute(pool)
                    .await
                {
                    Ok(_) => written += 1,
                    Err(e) => tracing::warn!(id, "privacy scan update skipped: {e}"),
                }
            }
            written
        }
    }
}

async fn save_progress(pool: &PgPool, id: Uuid, p: &Progress) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE privacy_scan_jobs SET rows_scanned = $2, rows_changed = $3, substitutions = $4, \
         by_category = $5, samples = $6, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(p.rows_scanned)
    .bind(p.rows_changed)
    .bind(p.substitutions)
    .bind(serde_json::to_value(&p.by_category).unwrap_or_else(|_| serde_json::json!({})))
    .bind(serde_json::to_value(&p.samples).unwrap_or_else(|_| serde_json::json!({})))
    .execute(pool)
    .await?;
    Ok(())
}

async fn cancel_requested(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT cancel_requested FROM privacy_scan_jobs WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

async fn finish(pool: &PgPool, id: Uuid, status: &str, error: Option<String>) {
    if let Err(e) = sqlx::query(
        "UPDATE privacy_scan_jobs SET status = $2, error = $3, finished_at = now(), \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .execute(pool)
    .await
    {
        tracing::error!("privacy scan could not record terminal state: {e}");
    }
}

/// Drive one job to a terminal state. Public so a test can await the sweep
/// instead of racing the spawned task.
pub async fn run(state: AppState, spec: JobSpec) {
    match sweep(&state, &spec).await {
        Ok(status) => finish(&state.pool, spec.id, status, None).await,
        Err(e) => {
            tracing::error!("privacy scan failed: {e}");
            finish(&state.pool, spec.id, "failed", Some(e.to_string())).await;
        }
    }
}

type SweepError = Box<dyn std::error::Error + Send + Sync>;

async fn sweep(state: &AppState, spec: &JobSpec) -> Result<&'static str, SweepError> {
    let data: Value = sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
        .bind(spec.user_id)
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(Value::Null);
    let user: Vec<(String, String)> =
        secret_scrub_of(&data).patterns.into_iter().map(|p| (p.name, p.regex)).collect();
    // The defaults always apply on an explicit re-scrub, regardless of the live
    // `secretScrubEnabled` toggle.
    let patterns = Arc::new(
        tokio::task::spawn_blocking(move || {
            cctui_crypto::redact::compile(true, &user, &cctui_crypto::vault_key())
        })
        .await
        .unwrap_or_else(|_| cctui_crypto::redact::CompiledPatterns::disabled()),
    );

    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM stream_events se JOIN sessions s ON s.id = se.session_id \
         WHERE s.user_id = $1 \
           AND ($2::text[] IS NULL OR se.session_id = ANY($2)) \
           AND ($3::timestamptz IS NULL OR se.created_at >= $3)",
    )
    .bind(spec.user_id)
    .bind(spec.session_ids.as_deref())
    .bind(spec.since)
    .fetch_one(&state.pool)
    .await?;
    sqlx::query("UPDATE privacy_scan_jobs SET rows_total = $2, updated_at = now() WHERE id = $1")
        .bind(spec.id)
        .bind(total)
        .execute(&state.pool)
        .await?;

    let mut progress = Progress::default();
    let mut cursor: i64 = 0;
    loop {
        if cancel_requested(&state.pool, spec.id).await {
            save_progress(&state.pool, spec.id, &progress).await?;
            return Ok("cancelled");
        }
        let rows: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT se.id, se.payload FROM stream_events se \
             JOIN sessions s ON s.id = se.session_id \
             WHERE s.user_id = $1 AND se.id > $2 \
               AND ($3::text[] IS NULL OR se.session_id = ANY($3)) \
               AND ($4::timestamptz IS NULL OR se.created_at >= $4) \
             ORDER BY se.id LIMIT $5",
        )
        .bind(spec.user_id)
        .bind(cursor)
        .bind(spec.session_ids.as_deref())
        .bind(spec.since)
        .bind(BATCH)
        .fetch_all(&state.pool)
        .await?;
        let fetched = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        if fetched == 0 {
            break;
        }

        let p = Arc::clone(&patterns);
        let dry_run = spec.dry_run;
        let outcome = tokio::task::spawn_blocking(move || scan_batch(rows, &p, dry_run))
            .await
            .map_err(|e| format!("scan worker panicked: {e}"))?;
        cursor = outcome.cursor;

        let changed = if spec.dry_run {
            outcome.changed
        } else {
            i64::try_from(apply_batch(&state.pool, &outcome.updates).await).unwrap_or(i64::MAX)
        };

        progress.rows_scanned += outcome.scanned;
        progress.rows_changed += changed;
        if changed > 0 || spec.dry_run {
            for (cat, c) in &outcome.stats {
                let c = i64::try_from(*c).unwrap_or(i64::MAX);
                *progress.by_category.entry(cat.clone()).or_insert(0) += c;
                progress.substitutions += c;
            }
        }
        for (cat, s) in &outcome.samples {
            progress.samples.entry(cat.clone()).or_default().merge(s);
        }
        save_progress(&state.pool, spec.id, &progress).await?;

        if fetched < BATCH {
            break;
        }
    }
    save_progress(&state.pool, spec.id, &progress).await?;
    if cancel_requested(&state.pool, spec.id).await { Ok("cancelled") } else { Ok("completed") }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{AppState, JobSpec, PgPool, PrivacyScanJob, StoredSamples, load, run, to_job};

    const TOKEN: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123";

    async fn seed_session(pool: &PgPool, uid: Uuid, machine: Uuid) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id) \
             VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(uid)
        .bind(machine)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    async fn seed_event(pool: &PgPool, sid: &str, text: &str) {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
        )
        .bind(sid)
        .bind(json!({ "role": "user", "text": text }))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn setup(name: &str) -> Option<(PgPool, Uuid, Uuid)> {
        let url = crate::routes::gateway::test_db_url(name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("scan-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        Some((pool, uid, machine))
    }

    async fn cleanup(pool: &PgPool, uid: Uuid, machine: Uuid) {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(uid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(machine)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(pool).await.unwrap();
    }

    fn spec(id: Uuid, uid: Uuid, dry_run: bool) -> JobSpec {
        JobSpec { id, user_id: uid, dry_run, session_ids: None, since: None }
    }

    async fn insert_job(pool: &PgPool, uid: Uuid, dry_run: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO privacy_scan_jobs (id, user_id, dry_run, status) \
             VALUES ($1, $2, $3, 'running')",
        )
        .bind(id)
        .bind(uid)
        .bind(dry_run)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn job(pool: &PgPool, id: Uuid) -> PrivacyScanJob {
        load(pool, id).await.unwrap().expect("job row")
    }

    #[tokio::test]
    async fn job_reports_progress_samples_and_completes() {
        let Some((pool, uid, machine)) = setup("privacy_scan_lifecycle").await else { return };
        let sid = seed_session(&pool, uid, machine).await;
        seed_event(&pool, &sid, &format!("export TOKEN={TOKEN}")).await;
        seed_event(&pool, &sid, "nothing secret here").await;

        let state = AppState::for_test(pool.clone());
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;

        let j = job(&pool, id).await;
        assert_eq!(j.status, "completed");
        assert_eq!((j.rows_total, j.rows_scanned, j.rows_changed), (Some(2), 2, 1));
        assert_eq!(j.by_category.get("github_token"), Some(&1));
        let cat = j.categories.iter().find(|c| c.category == "github_token").expect("category");
        assert_eq!(cat.samples.len(), 1);
        assert!(cat.samples[0].context.contains("export TOKEN"), "{:?}", cat.samples[0]);
        assert!(!cat.samples[0].text.contains("MNOPQRSTUVWX"), "{:?}", cat.samples[0]);
        assert!(!cat.identifier_warning);

        let stored: Option<String> = sqlx::query_scalar(
            "SELECT payload ->> 'text' FROM stream_events WHERE session_id = $1 \
             ORDER BY id LIMIT 1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(stored.unwrap().contains(TOKEN), "a dry run must not write");
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn samples_never_persist_the_matched_secret() {
        const SECRET: &str = "s3cretvalue123XY";
        let Some((pool, uid, machine)) = setup("privacy_scan_samples").await else { return };
        let sid = seed_session(&pool, uid, machine).await;
        seed_event(&pool, &sid, &format!("MY_token={SECRET}")).await;
        sqlx::query("INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2)")
            .bind(uid)
            .bind(json!({
                "secretScrubPatterns": [
                    { "name": "mytoken", "regex": r"\w+_token", "enabled": true }
                ]
            }))
            .execute(&pool)
            .await
            .unwrap();

        let state = AppState::for_test(pool.clone());
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;

        let stored: String =
            sqlx::query_scalar("SELECT samples::text FROM privacy_scan_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!stored.contains(SECRET), "the scan stored the secret it matched: {stored}");
        assert!(stored.contains("MY_token"), "{stored}");

        let j = job(&pool, id).await;
        let body = serde_json::to_string(&j.categories).unwrap();
        assert!(!body.contains(SECRET), "the API returned the secret it matched: {body}");
        let cat = j.categories.iter().find(|c| c.category == "mytoken").expect("category");
        assert_eq!(cat.samples[0].text, "MY_token");
        assert!(!cat.identifier_warning, "one match is too few to warn");

        sqlx::query("DELETE FROM user_settings WHERE user_id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM privacy_scan_jobs WHERE user_id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn cancel_during_apply_leaves_no_half_written_row() {
        let Some((pool, uid, machine)) = setup("privacy_scan_cancel").await else { return };
        let sid = seed_session(&pool, uid, machine).await;
        for i in 0..1200 {
            seed_event(&pool, &sid, &format!("run {i} with TOKEN={TOKEN}")).await;
        }

        let state = AppState::for_test(pool.clone());
        let id = insert_job(&pool, uid, false).await;
        let worker = tokio::spawn(run(state, spec(id, uid, false)));
        sqlx::query("UPDATE privacy_scan_jobs SET cancel_requested = true WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), worker)
            .await
            .expect("cancel stops the sweep promptly")
            .unwrap();

        let j = job(&pool, id).await;
        assert_eq!(j.status, "cancelled");

        let texts: Vec<String> = sqlx::query_scalar(
            "SELECT payload ->> 'text' FROM stream_events WHERE session_id = $1",
        )
        .bind(&sid)
        .fetch_all(&pool)
        .await
        .unwrap();
        let redacted = texts
            .iter()
            .filter(|t| {
                assert!(
                    t.contains(TOKEN) ^ t.contains("[REDACTED:github_token"),
                    "row is neither untouched nor fully redacted: {t}"
                );
                t.contains("[REDACTED:github_token")
            })
            .count();
        let redacted = i64::try_from(redacted).unwrap_or(i64::MAX);
        assert_eq!(redacted, j.rows_changed, "progress counted rows that were not committed");
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn apply_pass_is_idempotent_and_scoped() {
        let Some((pool, uid, machine)) = setup("privacy_scan_apply").await else { return };
        let old = seed_session(&pool, uid, machine).await;
        let fresh = seed_session(&pool, uid, machine).await;
        seed_event(&pool, &old, &format!("a {TOKEN}")).await;
        seed_event(&pool, &fresh, &format!("b {TOKEN}")).await;
        let state = AppState::for_test(pool.clone());

        let scoped = insert_job(&pool, uid, true).await;
        run(
            state.clone(),
            JobSpec {
                id: scoped,
                user_id: uid,
                dry_run: true,
                session_ids: Some(vec![fresh.clone()]),
                since: None,
            },
        )
        .await;
        let j = job(&pool, scoped).await;
        assert_eq!((j.rows_scanned, j.rows_changed), (1, 1));

        let unknown = insert_job(&pool, uid, true).await;
        run(
            state.clone(),
            JobSpec {
                id: unknown,
                user_id: uid,
                dry_run: true,
                session_ids: Some(vec!["not-a-uuid".to_owned()]),
                since: None,
            },
        )
        .await;
        assert_eq!(job(&pool, unknown).await.rows_scanned, 0);

        let apply = insert_job(&pool, uid, false).await;
        run(state.clone(), spec(apply, uid, false)).await;
        assert_eq!(job(&pool, apply).await.rows_changed, 2);
        let texts: Vec<String> = sqlx::query_scalar(
            "SELECT se.payload ->> 'text' FROM stream_events se \
             JOIN sessions s ON s.id = se.session_id WHERE s.user_id = $1",
        )
        .bind(uid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(texts.len(), 2);
        for t in &texts {
            assert!(!t.contains(TOKEN), "{t}");
            assert!(t.contains("[REDACTED:github_token"), "{t}");
        }

        let again = insert_job(&pool, uid, false).await;
        run(state, spec(again, uid, false)).await;
        assert_eq!(job(&pool, again).await.rows_changed, 0);
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn one_running_job_per_user() {
        let Some((pool, uid, machine)) = setup("privacy_scan_single").await else { return };
        insert_job(&pool, uid, true).await;
        let second = sqlx::query(
            "INSERT INTO privacy_scan_jobs (id, user_id, dry_run, status) \
             VALUES ($1, $2, true, 'running') ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(second.rows_affected(), 0);
        sqlx::query("DELETE FROM privacy_scan_jobs WHERE user_id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        cleanup(&pool, uid, machine).await;
    }

    #[test]
    fn identifier_warning_fires_only_for_user_patterns() {
        let samples = |examined: usize, with_value: usize| {
            serde_json::to_value(StoredSamples { samples: vec![], examined, with_value }).unwrap()
        };
        let row = |cat: &str, s: serde_json::Value| {
            to_job((
                Uuid::new_v4(),
                "completed".to_owned(),
                true,
                false,
                Some(10),
                10,
                4,
                4,
                json!({ cat: 4 }),
                json!({ cat: s }),
                None,
                chrono::Utc::now(),
                None,
            ))
        };
        assert!(row("mytoken", samples(10, 1)).categories[0].identifier_warning);
        assert!(!row("mytoken", samples(10, 9)).categories[0].identifier_warning);
        assert!(!row("mytoken", samples(2, 0)).categories[0].identifier_warning);
        assert!(!row("github_token", samples(10, 0)).categories[0].identifier_warning);
    }
}
