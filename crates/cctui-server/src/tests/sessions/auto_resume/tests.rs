use chrono::Utc;

use super::{Action, STUCK_SELECT, claim_attempt, plan};

async fn seed_session(pool: &sqlx::PgPool, name: &str) -> String {
    let uid = uuid::Uuid::new_v4();
    let machine = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{name}-{uid}"))
        .bind(format!("h-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    let sid = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    sid
}

async fn message(pool: &sqlx::PgPool, sid: &str, role: &str, text: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2) RETURNING id")
        .bind(sid).bind(serde_json::json!({"role":role,"text":text}))
        .fetch_one(pool).await.unwrap()
}

async fn selected(pool: &sqlx::PgPool, sid: &str) -> Option<super::StuckRow> {
    let machine: uuid::Uuid = sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id=$1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query_as(STUCK_SELECT)
        .bind(super::LOOKBACK_SECS.to_string())
        .bind(super::BATCH)
        .bind(cctui_server_sessions::store::sessions::SessionRowStatus::names(
            cctui_server_sessions::store::sessions::SessionRowStatus::NOT_RESUMABLE,
        ))
        .bind(vec![machine])
        .bind(sid)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn opt_in(pool: &sqlx::PgPool, sid: &str) {
    sqlx::query("INSERT INTO user_settings(user_id, data) SELECT user_id, '{\"autoResumeOnConnectionLoss\":true}'::jsonb FROM sessions WHERE id=$1")
        .bind(sid).execute(pool).await.unwrap();
}

async fn bind_gateway(pool: &sqlx::PgPool, sid: &str, uid: uuid::Uuid) {
    let account = uuid::Uuid::new_v4();
    let provider = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO accounts(id,user_id,name) VALUES($1,$2,'auth-test')")
        .bind(account)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO account_providers(id,user_id,provider,encrypted_refresh_token,account_id) VALUES($1,$2,'anthropic','test',$3)").bind(provider).bind(uid).bind(account).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO session_tokens(token_hash,session_id,account_id,revoked_at) VALUES($1,$2,$3,now())").bind(uuid::Uuid::new_v4().to_string()).bind(sid).bind(provider).execute(pool).await.unwrap();
}

#[tokio::test]
async fn login_error_requires_opt_in_and_stays_bounded_until_real_progress() {
    let Some(url) = cctui_server_kernel::test_support::db_url("auto_resume_login") else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let sid = seed_session(&pool, "login").await;
    let first = message(
        &pool,
        &sid,
        "assistant",
        "Please run /login · API Error: 401 Invalid bearer token",
    )
    .await;
    assert!(selected(&pool, &sid).await.is_none(), "default is opt-out");
    opt_in(&pool, &sid).await;
    assert!(selected(&pool, &sid).await.is_none(), "an unrelated local account is not recoverable");
    // A revoked gateway credential still proves the family binding needed
    // to mint a replacement; it is never reused as a credential.
    let uid: uuid::Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    bind_gateway(&pool, &sid, uid).await;
    assert!(selected(&pool, &sid).await.unwrap().gateway_bound);
    let state = crate::state::AppState::for_test(pool.clone());
    assert!(
        !super::should_recover_gateway_auth(&state, &sid).await,
        "manual retry requires a capable daemon"
    );
    let machine: uuid::Uuid = sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id=$1")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    state.auth_recovery_daemons.insert(machine, ());
    sqlx::query("UPDATE user_settings SET data='{}'::jsonb WHERE user_id=$1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        super::should_recover_gateway_auth(&state, &sid).await,
        "an explicit retry does not need auto-resume opt-in"
    );
    sqlx::query("UPDATE user_settings SET data='{\"autoResumeOnConnectionLoss\":true}'::jsonb WHERE user_id=$1").bind(uid).execute(&pool).await.unwrap();
    let no_capability: Vec<super::StuckRow> = sqlx::query_as(STUCK_SELECT)
        .bind(super::LOOKBACK_SECS.to_string())
        .bind(super::BATCH)
        .bind(cctui_server_sessions::store::sessions::SessionRowStatus::names(
            cctui_server_sessions::store::sessions::SessionRowStatus::NOT_RESUMABLE,
        ))
        .bind(Vec::<uuid::Uuid>::new())
        .bind(&sid)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(no_capability.is_empty(), "old daemons must not receive a blind auth retry");
    assert!(claim_attempt(&pool, &sid, first, 1, None).await.unwrap());
    message(&pool, &sid, "user", "[cctui auto-resume attempt 1] continue").await;
    assert!(
        !super::should_recover_gateway_auth(&state, &sid).await,
        "a newer human message blocks manual stale error recovery"
    );
    assert!(selected(&pool, &sid).await.is_none(), "never replay past a later user message");
    let second = message(
        &pool,
        &sid,
        "assistant",
        "Please run /login · API Error: 401 Invalid bearer token again",
    )
    .await;
    let row = selected(&pool, &sid).await.unwrap();
    assert!(!row.made_progress, "an auto reply then another 401 is not success");
    let tracked =
        Some((row.tracked_event_id.unwrap(), row.tracked_attempts.unwrap(), Utc::now(), false));
    assert_eq!(plan(tracked, second, row.error_at, Utc::now()), Action::Fire { attempt: 2 });
    assert!(claim_attempt(&pool, &sid, second, 2, Some((first, 1))).await.unwrap());
    assert!(
        !claim_attempt(&pool, &sid, first, 1, Some((first, 1))).await.unwrap(),
        "a stale sweep cannot rewind the budget"
    );
    message(
        &pool,
        &sid,
        "assistant",
        "This session was automatically restarted after its process exited unexpectedly.",
    )
    .await;
    message(&pool, &sid, "assistant", "API Error: 401 Invalid bearer token after restart").await;
    assert!(!selected(&pool, &sid).await.unwrap().made_progress);
    message(&pool, &sid, "assistant", "The build completed successfully.").await;
    message(&pool, &sid, "assistant", "API Error: 401 Invalid bearer token").await;
    assert!(selected(&pool, &sid).await.unwrap().made_progress);
}
