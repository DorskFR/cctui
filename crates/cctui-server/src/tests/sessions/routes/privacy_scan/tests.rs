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
        ))
    };
    assert!(row("mytoken", samples(10, 1)).categories[0].identifier_warning);
    assert!(!row("mytoken", samples(10, 9)).categories[0].identifier_warning);
    assert!(!row("mytoken", samples(2, 0)).categories[0].identifier_warning);
    assert!(!row("github_token", samples(10, 0)).categories[0].identifier_warning);
}
