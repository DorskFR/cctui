use super::*;

/// A streamed delta fans out to the bus as it is and leaves no row behind.
#[tokio::test]
async fn a_text_delta_is_relayed_and_never_persisted() {
    let Some(url) = cctui_server_kernel::test_support::db_url("relay_text_delta") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let mut frames = state.bus.subscribe_server();
    let session_id = uuid::Uuid::new_v4().to_string();

    on_relay_event(
        &state,
        uuid::Uuid::nil(),
        uuid::Uuid::nil(),
        AdapterEvent::TextDelta {
            local_id: session_id.clone(),
            item_id: "msg_01:0".into(),
            delta: "Hel".into(),
        },
    )
    .await;

    let frame = frames.try_recv().expect("the delta is on the bus");
    assert!(frame.json.contains(r#""type":"text_delta""#), "{}", frame.json);
    assert!(frame.json.contains(r#""delta":"Hel""#), "{}", frame.json);
    let (rows,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM stream_events WHERE session_id = $1")
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("count rows");
    assert_eq!(rows, 0, "a delta must never reach stream_events");
}

/// A refused spawn's waiting request hears of it only once the failed row
/// is on the list, so the caller and the session list agree.
#[tokio::test]
async fn a_refused_spawn_is_written_before_the_waiting_request_hears_of_it() {
    refused_spawn(false).await;
}

#[tokio::test]
async fn a_durable_refusal_recovers_metadata_and_rejects_a_foreign_machine() {
    refused_spawn(true).await;
}

async fn refused_spawn(durable: bool) {
    let Some(url) = cctui_server_kernel::test_support::db_url("relay_refused_spawn") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("relay-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(&pool)
        .await
        .expect("seed user");
    let mid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(mid)
        .bind(uid)
        .bind(format!("m-{mid}"))
        .bind(format!("mk-{mid}"))
        .execute(&pool)
        .await
        .expect("seed machine");
    let state = AppState::for_test(pool.clone());
    let command_id = uuid::Uuid::new_v4();
    let session_id = uuid::Uuid::new_v4().to_string();
    let rx = cctui_server_kernel::commands::wait_for_verdict(&state.command_waiters, command_id);
    cctui_server_kernel::commands::track_command(
        &state.pending_commands,
        command_id,
        None,
        Some(cctui_server_kernel::commands::FailedSpawnRow {
            session_id: session_id.clone(),
            machine_id: mid,
            user_id: uid,
            adapter_id: "claude-code".into(),
            working_dir: "/w".into(),
            name: None,
            model: None,
            effort: None,
        }),
    );

    if durable {
        let row = state.pending_commands.get(&command_id).unwrap().spawn.clone().unwrap();
        cctui_server_core::machine_launches::prepare(&pool, command_id, uid, &row).await.unwrap();
        on_relay_event(
            &state,
            uuid::Uuid::new_v4(),
            uid,
            AdapterEvent::CommandResult {
                command_id,
                ok: false,
                error: Some("foreign refusal".into()),
            },
        )
        .await;
        assert!(state.pending_commands.contains_key(&command_id));
        assert!(state.command_waiters.contains_key(&command_id));
        assert_eq!(
            cctui_server_core::machine_launches::find(&pool, &command_id.to_string(), None)
                .await
                .unwrap()
                .unwrap()
                .state,
            "prepared"
        );
        state.pending_commands.clear();
    }
    on_relay_event(
        &state,
        mid,
        uid,
        AdapterEvent::CommandResult {
            command_id,
            ok: false,
            error: Some("adapter claude-code command queue is full".into()),
        },
    )
    .await;

    let verdict = rx.await.expect("the waiting request is told");
    assert!(!verdict.ok);
    assert_eq!(verdict.error.as_deref(), Some("adapter claude-code command queue is full"));
    let (status, end_reason): (String, Option<String>) =
        sqlx::query_as("SELECT status, end_reason FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("the failed row exists");
    assert_eq!(status, "ended");
    assert_eq!(end_reason.as_deref(), Some("spawn_failed"));
    assert!(state.command_waiters.is_empty());
}
