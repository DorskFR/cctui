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
    let url = cctui_server_kernel::test_support::db_url(name)?;
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
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM machines WHERE id = $1").bind(machine).execute(pool).await.unwrap();
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

    let texts: Vec<String> =
        sqlx::query_scalar("SELECT payload ->> 'text' FROM stream_events WHERE session_id = $1")
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
            None,
        ))
    };
    assert!(row("mytoken", samples(10, 1)).categories[0].identifier_warning);
    assert!(!row("mytoken", samples(10, 9)).categories[0].identifier_warning);
    assert!(!row("mytoken", samples(2, 0)).categories[0].identifier_warning);
    assert!(!row("github_token", samples(10, 0)).categories[0].identifier_warning);
}

mod local_transcripts {
    use std::collections::BTreeMap;

    use cctui_proto::api::privacy_scan::{
        MachineScanState, NotScannedReason, ScanSample, TranscriptAuditReport,
    };
    use cctui_proto::ws::DaemonFrameDown;
    use tokio::sync::mpsc;

    use super::super::local;
    use super::*;

    async fn opt_in(pool: &PgPool, uid: Uuid, on: bool) {
        sqlx::query("INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2)")
            .bind(uid)
            .bind(json!({ "secretScrubLocalTranscripts": on }))
            .execute(pool)
            .await
            .unwrap();
    }

    async fn machine_state(pool: &PgPool, machine: Uuid, capable: bool, seen_mins_ago: i64) {
        sqlx::query(
            "UPDATE machines SET transcript_audit = $2, \
             last_seen_at = now() - $3::bigint * interval '1 minute' WHERE id = $1",
        )
        .bind(machine)
        .bind(capable)
        .bind(seen_mins_ago)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_machine(pool: &PgPool, uid: Uuid, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(uid)
            .bind(name)
            .bind(format!("kh-{id}"))
            .execute(pool)
            .await
            .unwrap();
        id
    }

    fn report(dry_run: bool, cat: &str, count: i64, files_changed: i64) -> TranscriptAuditReport {
        TranscriptAuditReport {
            dry_run,
            files_scanned: 5,
            files_changed,
            substitutions: count,
            by_category: BTreeMap::from([(cat.to_owned(), count)]),
            samples: BTreeMap::from([(
                cat.to_owned(),
                vec![ScanSample {
                    text: "ghp_AB…".into(),
                    context: "export TOKEN=".into(),
                    value_follows: true,
                }],
            )]),
            ..TranscriptAuditReport::default()
        }
    }

    /// A connected daemon that answers one audit with `reply` and hands back
    /// the `dry_run` it was asked for.
    fn fake_daemon(
        state: &AppState,
        machine: Uuid,
        reply: TranscriptAuditReport,
    ) -> tokio::task::JoinHandle<Option<bool>> {
        let (tx, mut rx) = mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        let bus = state.bus.clone();
        tokio::spawn(async move {
            let Some(DaemonFrameDown::TranscriptAudit { request_id, dry_run }) = rx.recv().await
            else {
                return None;
            };
            bus.resolve_transcript_audit(request_id, Ok(reply));
            Some(dry_run)
        })
    }

    #[tokio::test]
    async fn dry_run_merges_every_daemon_finding_by_category() {
        let Some((pool, uid, machine)) = setup("privacy_scan_local_merge").await else { return };
        let other = add_machine(&pool, uid, "second").await;
        opt_in(&pool, uid, true).await;
        machine_state(&pool, machine, true, 0).await;
        machine_state(&pool, other, true, 0).await;
        let sid = seed_session(&pool, uid, machine).await;
        seed_event(&pool, &sid, &format!("export TOKEN={TOKEN}")).await;

        let state = AppState::for_test(pool.clone());
        let a = fake_daemon(&state, machine, report(true, "github_token", 2, 1));
        let b = fake_daemon(&state, other, report(true, "github_token", 3, 2));
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;
        assert_eq!(a.await.unwrap(), Some(true));
        assert_eq!(b.await.unwrap(), Some(true));

        let j = job(&pool, id).await;
        assert_eq!(j.status, "completed");
        assert_eq!(j.by_category.get("github_token"), Some(&1), "the DB part is unchanged");
        let local = j.local_transcripts.expect("local section");
        assert_eq!(local.machines.len(), 2);
        assert!(local.machines.iter().all(|m| m.state == MachineScanState::Scanned));
        let cat = local.categories.iter().find(|c| c.category == "github_token").unwrap();
        assert_eq!(cat.count, 5);
        assert_eq!(cat.samples.len(), 2);
        assert_eq!((local.substitutions, local.files_changed), (5, 3));
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(other).execute(&pool).await.unwrap();
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn an_offline_machine_is_not_scanned_rather_than_clean() {
        let Some((pool, uid, machine)) = setup("privacy_scan_local_offline").await else { return };
        let gone = add_machine(&pool, uid, "gone").await;
        opt_in(&pool, uid, true).await;
        machine_state(&pool, machine, true, 0).await;
        machine_state(&pool, gone, true, 60).await;

        let state = AppState::for_test(pool.clone());
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;

        let local = job(&pool, id).await.local_transcripts.expect("local section");
        assert_eq!(local.machines.len(), 2);
        for m in &local.machines {
            assert_eq!(m.state, MachineScanState::NotScanned, "{m:?}");
            assert_eq!(m.reason, Some(NotScannedReason::Offline), "{m:?}");
            assert!(m.report.is_none());
        }
        assert!(local.categories.is_empty());
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(gone).execute(&pool).await.unwrap();
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn an_apply_run_asks_the_daemon_to_rewrite_and_reports_changed_files() {
        let Some((pool, uid, machine)) = setup("privacy_scan_local_apply").await else { return };
        opt_in(&pool, uid, true).await;
        machine_state(&pool, machine, true, 0).await;

        let state = AppState::for_test(pool.clone());
        let daemon = fake_daemon(&state, machine, report(false, "github_token", 4, 3));
        let id = insert_job(&pool, uid, false).await;
        run(state, spec(id, uid, false)).await;
        assert_eq!(daemon.await.unwrap(), Some(false), "an apply run is not a dry run");

        let j = job(&pool, id).await;
        assert_eq!(j.status, "completed");
        let local = j.local_transcripts.expect("local section");
        assert_eq!(local.files_changed, 3);
        let scanned = &local.machines[0];
        assert_eq!(scanned.state, MachineScanState::Scanned);
        assert_eq!(scanned.report.as_ref().map(|r| r.files_changed), Some(3));
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn an_older_daemon_is_skipped_with_a_reason() {
        let Some((pool, uid, machine)) = setup("privacy_scan_local_outdated").await else { return };
        opt_in(&pool, uid, true).await;
        machine_state(&pool, machine, false, 0).await;

        let state = AppState::for_test(pool.clone());
        let (tx, mut rx) = mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;

        assert!(rx.try_recv().is_err(), "an older daemon is never sent the audit");
        let local = job(&pool, id).await.local_transcripts.expect("local section");
        assert_eq!(local.machines[0].state, MachineScanState::NotScanned);
        assert_eq!(local.machines[0].reason, Some(NotScannedReason::Outdated));
        cleanup(&pool, uid, machine).await;
    }

    #[tokio::test]
    async fn without_the_opt_in_the_job_is_database_only() {
        let Some((pool, uid, machine)) = setup("privacy_scan_local_off").await else { return };
        opt_in(&pool, uid, false).await;
        machine_state(&pool, machine, true, 0).await;

        let state = AppState::for_test(pool.clone());
        let (tx, mut rx) = mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        let id = insert_job(&pool, uid, true).await;
        run(state, spec(id, uid, true)).await;

        assert!(rx.try_recv().is_err(), "no daemon is asked");
        let j = job(&pool, id).await;
        assert_eq!(j.status, "completed");
        assert!(j.local_transcripts.is_none());
        cleanup(&pool, uid, machine).await;
    }

    #[test]
    fn a_daemon_error_or_timeout_is_a_reason_not_an_empty_report() {
        use cctui_server_kernel::bus::BusError;
        let mut entry = local::initial(Uuid::nil(), "m".into(), true, true);
        assert_eq!(entry.state, MachineScanState::Pending);
        local::settle(&mut entry, Err(BusError::Timeout));
        assert_eq!(entry.reason, Some(NotScannedReason::Timeout));
        local::settle(&mut entry, Err(BusError::TranscriptAudit("disk".into())));
        assert_eq!(
            (entry.reason, entry.detail.as_deref()),
            (Some(NotScannedReason::Failed), Some("disk"))
        );
        let mut pending = vec![local::initial(Uuid::nil(), "m".into(), true, true)];
        local::cancel_pending(&mut pending);
        assert_eq!(pending[0].reason, Some(NotScannedReason::Cancelled));
    }
}
