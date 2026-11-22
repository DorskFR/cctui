use super::*;

fn snapshot(step: i64) -> GuardStepState {
    GuardStepState { step, title: "Implement".into(), transitions: vec![3], ..Default::default() }
}

async fn db(test: &str) -> Option<(PgPool, String)> {
    let url = cctui_server_kernel::test_support::db_url(test)?;
    let pool = PgPool::connect(&url).await.expect("connect test db");
    let (uid, machine) = crate::events::tests::seed_user_machine(&pool, test).await;
    let sid = crate::events::tests::seed_session(&pool, uid, machine, Some("g")).await;
    Some((pool, sid))
}

#[tokio::test]
async fn a_replica_that_missed_the_report_reads_the_table() {
    let Some((pool, sid)) = db("guard_step_read_through").await else { return };
    let durable = Durable::new(pool.clone(), None);
    let now = Utc::now();
    save(&pool, &sid, &snapshot(2), now).await.unwrap();
    assert_eq!(get(&sid), None);
    assert_eq!(durable.lookup(&sid).await, Some(snapshot(2)));

    save(&pool, &sid, &snapshot(1), now - chrono::Duration::seconds(5)).await.unwrap();
    assert_eq!(load(&pool, &sid).await.unwrap(), Some(snapshot(2)), "an older report loses");
    save(&pool, &sid, &snapshot(3), now + chrono::Duration::seconds(5)).await.unwrap();
    assert_eq!(load(&pool, &sid).await.unwrap(), Some(snapshot(3)));

    record(sid.clone(), snapshot(3));
    durable.forget(&sid);
    assert_eq!(get(&sid), None);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while load(&pool, &sid).await.unwrap().is_some() {
        assert!(tokio::time::Instant::now() < deadline, "the forget never reached the table");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(durable.lookup(&sid).await, None);
}

#[tokio::test]
async fn an_ended_or_archived_session_stores_no_step() {
    let Some((pool, sid)) = db("guard_step_ended").await else { return };
    sqlx::query("UPDATE sessions SET ended_at = now() WHERE id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
    save(&pool, &sid, &snapshot(2), Utc::now()).await.unwrap();
    assert_eq!(load(&pool, &sid).await.unwrap(), None);

    sqlx::query("UPDATE sessions SET ended_at = NULL, status = 'archived' WHERE id = $1")
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
    save(&pool, &sid, &snapshot(2), Utc::now()).await.unwrap();
    assert_eq!(load(&pool, &sid).await.unwrap(), None);
}
