/// A dispatched worker SIGTERMs its daemon once the guard exits: the
/// disconnect must end that session `completed`, and the later
/// `daemon_lost` mark only the sessions with no completion evidence.
#[tokio::test]
async fn a_guard_exited_session_ends_completed_not_daemon_lost() {
    let Some(url) = cctui_server_kernel::test_support::db_url("daemon_lost_guard_exited") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let (uid, mid) = crate::events::tests::seed_user_machine(&pool, "dl-guard-exit").await;
    let seed = async |step: Option<i64>| {
        let sid = crate::events::tests::seed_session(&pool, uid, mid, None).await;
        if let Some(step) = step {
            let guard = cctui_proto::guard_step::GuardStepState { step, ..Default::default() };
            sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
                .bind(&sid)
                .bind(sqlx::types::Json(&guard))
                .execute(&pool)
                .await
                .expect("seed guard step");
        }
        sid
    };
    let exited = seed(Some(-1)).await;
    let exited_here = seed(None).await;
    let mid_workflow = seed(Some(3)).await;
    let unguarded = seed(None).await;
    let sessions =
        vec![exited.clone(), exited_here.clone(), mid_workflow.clone(), unguarded.clone()];

    let mut completed =
        super::mark_guard_exited(&pool, mid, &sessions, std::slice::from_ref(&exited_here)).await;
    completed.sort();
    let mut want = vec![exited.clone(), exited_here.clone()];
    want.sort();
    assert_eq!(completed, want);

    let mut lost = super::mark_daemon_lost(&pool, mid, &sessions).await;
    lost.sort();
    let mut want = vec![mid_workflow.clone(), unguarded.clone()];
    want.sort();
    assert_eq!(lost, want, "completed sessions are never re-marked daemon_lost");

    let end_reason = async |id: &str| -> Option<String> {
        sqlx::query_scalar("SELECT end_reason FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("session row")
    };
    assert_eq!(end_reason(&exited).await.as_deref(), Some("completed"));
    assert_eq!(end_reason(&exited_here).await.as_deref(), Some("completed"));
    assert_eq!(end_reason(&mid_workflow).await.as_deref(), Some("daemon_lost"));
    assert_eq!(end_reason(&unguarded).await.as_deref(), Some("daemon_lost"));
}

/// A dispatched run that settled done before its worker's socket closed
/// ends `completed` though no `SessionEnded` arrived; a busy run, an
/// interactive session and a guard short of its exit still end lost.
#[tokio::test]
async fn a_finished_dispatch_without_session_ended_ends_completed() {
    let Some(url) = cctui_server_kernel::test_support::db_url("daemon_lost_finished_dispatch")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let (uid, mid) = crate::events::tests::seed_user_machine(&pool, "dl-finished").await;
    let seed = async |signals: &str, dispatched: bool, guard: Option<i64>| {
        let sid = crate::events::tests::seed_session(&pool, uid, mid, None).await;
        sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE sessions SET {signals} WHERE id = $1")))
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("set signals");
        if dispatched {
            sqlx::query(
                "INSERT INTO dispatch_records (session_id, user_id, dispatcher_name, outcome) \
                 VALUES ($1, $2, 'fixture', 'registered')",
            )
            .bind(&sid)
            .bind(uid)
            .execute(&pool)
            .await
            .expect("seed dispatch record");
        }
        if let Some(step) = guard {
            let guard = cctui_proto::guard_step::GuardStepState { step, ..Default::default() };
            sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
                .bind(&sid)
                .bind(sqlx::types::Json(&guard))
                .execute(&pool)
                .await
                .expect("seed guard step");
        }
        sid
    };
    let settled = "tempo = 'idle', agent_state = 'done'";
    let finished = seed(settled, true, None).await;
    let busy = seed("tempo = 'active', agent_state = 'working'", true, None).await;
    let interactive = seed(settled, false, None).await;
    let mid_workflow = seed(settled, true, Some(2)).await;
    let sessions = vec![finished.clone(), busy.clone(), interactive.clone(), mid_workflow.clone()];

    let completed = super::mark_finished_dispatches(&pool, mid, &sessions).await;
    assert_eq!(completed, vec![finished.clone()]);

    let mut lost = super::mark_daemon_lost(&pool, mid, &sessions).await;
    lost.sort();
    let mut want = vec![busy, interactive, mid_workflow];
    want.sort();
    assert_eq!(lost, want, "a completed dispatch is never re-marked daemon_lost");

    let (end_reason, end_detail): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT end_reason, end_detail FROM sessions WHERE id = $1")
            .bind(&finished)
            .fetch_one(&pool)
            .await
            .expect("session row");
    assert_eq!(end_reason.as_deref(), Some("completed"));
    assert_eq!(end_detail.as_deref(), Some(super::FINISHED_BEFORE_END));
}

/// A dispatched pod that reconnected to a peer replica re-announced its
/// session there; this replica's pending mark must not end it.
#[tokio::test]
async fn a_session_live_on_a_peer_replica_is_not_marked() {
    let Some(url) = cctui_server_kernel::test_support::db_url("daemon_lost_presence") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let (uid, mid) = crate::events::tests::seed_user_machine(&pool, "dl-presence").await;
    let moved = crate::events::tests::seed_session(&pool, uid, mid, None).await;
    let stale = crate::events::tests::seed_session(&pool, uid, mid, None).await;
    let lost = crate::events::tests::seed_session(&pool, uid, mid, None).await;
    for (sid, age_secs) in [(&moved, 1), (&stale, 600)] {
        sqlx::query(
            "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip, heartbeat_at) \
             VALUES ('session', $1::uuid, 'peer', '10.0.0.2', \
                     now() - make_interval(secs => $2))",
        )
        .bind(sid)
        .bind(f64::from(age_secs))
        .execute(&pool)
        .await
        .expect("seed presence");
    }

    let mut marked =
        super::mark_daemon_lost(&pool, mid, &[moved.clone(), stale.clone(), lost.clone()]).await;
    marked.sort();
    let mut want = vec![stale.clone(), lost.clone()];
    want.sort();
    assert_eq!(marked, want, "a stale presence row is a crashed pod, not a live session");

    let cleanup = [moved, stale];
    sqlx::query("DELETE FROM ws_presence WHERE entity_id::text = ANY($1)")
        .bind(&cleanup[..])
        .execute(&pool)
        .await
        .expect("drop presence");
}
