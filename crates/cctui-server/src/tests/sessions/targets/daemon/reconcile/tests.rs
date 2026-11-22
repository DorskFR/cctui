use super::*;

#[tokio::test]
async fn archived_jobs_matches_reported_shorts_on_this_machine_only() {
    let name = "archived_jobs_matches_reported_shorts_on_this_machine_only";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{name}-{uid}"))
        .bind(format!("h-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    let (mine, other) = (Uuid::new_v4(), Uuid::new_v4());
    for machine in [mine, other] {
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    let sid = |short: &str| format!("{short}-{}", &uid.to_string()[9..]);
    let rows = [
        (sid("aaaaaaaa"), mine, "archived", "claude-code"),
        (sid("bbbbbbbb"), mine, "active", "claude-code"),
        (sid("cccccccc"), mine, "archived", "codex"),
        (sid("dddddddd"), other, "archived", "claude-code"),
        (sid("eeeeeeee"), mine, "archived", "claude-code"),
    ];
    for (id, machine, status, adapter) in &rows {
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, \
             status, adapter_id) VALUES ($1, $2, $2, $3, '/w', $4, $5)",
        )
        .bind(id)
        .bind(machine)
        .bind(uid)
        .bind(status)
        .bind(adapter)
        .execute(&pool)
        .await
        .unwrap();
    }

    let reported: Vec<String> =
        ["aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd", "ffffffff"].map(String::from).into();
    let got = super::archived_jobs(&pool, mine, &reported).await.unwrap();
    assert_eq!(got, vec![sid("aaaaaaaa")]);
    assert!(super::archived_jobs(&pool, mine, &[]).await.unwrap().is_empty());

    for sql in [
        "DELETE FROM sessions WHERE user_id = $1",
        "DELETE FROM machines WHERE user_id = $1",
        "DELETE FROM users WHERE id = $1",
    ] {
        sqlx::query(sql).bind(uid).execute(&pool).await.unwrap();
    }
}

#[tokio::test]
async fn archived_jobs_probes_the_prefix_index_on_a_large_dispatch_machine() {
    cctui_server_kernel::test_support::with_isolated_database(
        "archived_jobs_index",
        |pool| async move {
            cctui_server_kernel::schema::core::bootstrap(&pool).await.unwrap();
            let (uid, machine) = (Uuid::new_v4(), Uuid::new_v4());
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'dispatch', 'h')")
                .bind(uid)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', 'mk')",
            )
            .bind(machine)
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, \
             status, adapter_id) \
             SELECT lpad(g::text, 8, '0') || '-job', 'm', $1, $2, '/w', \
                    CASE WHEN g % 10 = 0 THEN 'active' ELSE 'archived' END, 'claude-code' \
             FROM generate_series(1, 5000) g",
            )
            .bind(machine)
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("ANALYZE sessions").execute(&pool).await.unwrap();

            let reported: Vec<String> =
                ["00000042", "00000043", "00000050", "zzzzzzzz"].map(String::from).into();
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("SET LOCAL enable_seqscan = off").execute(&mut *tx).await.unwrap();
            let explain = format!("EXPLAIN (FORMAT JSON) {ARCHIVED_JOBS}");
            let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain))
                .bind(machine)
                .bind(&reported)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            let plan = plan.to_string();
            assert!(plan.contains("idx_sessions_archived_claude_short"), "{plan}");

            let mut got = super::archived_jobs(&pool, machine, &reported).await.unwrap();
            got.sort_unstable();
            assert_eq!(got, ["00000042-job", "00000043-job"]);
        },
    )
    .await;
}
