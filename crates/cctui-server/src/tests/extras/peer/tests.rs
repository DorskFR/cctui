use super::*;
use uuid::Uuid;

async fn test_pool(tag: &str) -> Option<sqlx::PgPool> {
    let url = crate::routes::gateway::test_db_url(tag)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn seed_owner(pool: &sqlx::PgPool, uid: Uuid, machines: &[Uuid]) {
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'peer-test', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    for &m in machines {
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(m)
            .bind(uid)
            .bind(format!("box-{m}"))
            .bind(format!("kh-{m}"))
            .execute(pool)
            .await
            .expect("seed machine");
    }
}

async fn seed_session(
    pool: &sqlx::PgPool,
    id: &str,
    parent: Option<&str>,
    uid: Uuid,
    machine: Uuid,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
         machine_uuid, adapter_id, session_name, status) \
         VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code', $6, $7)",
    )
    .bind(id)
    .bind(parent)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .bind(format!("name-{id}"))
    .bind(status)
    .execute(pool)
    .await
    .expect("seed session");
}

async fn cleanup(pool: &sqlx::PgPool, uid: Uuid) {
    for sql in [
        "DELETE FROM rooms WHERE user_id = $1",
        "DELETE FROM stream_events WHERE session_id IN \
         (SELECT id FROM sessions WHERE user_id = $1)",
        "DELETE FROM sessions WHERE user_id = $1",
        "DELETE FROM machines WHERE user_id = $1",
        "DELETE FROM users WHERE id = $1",
    ] {
        sqlx::query(sql).bind(uid).execute(pool).await.ok();
    }
}

/// DB-gated: a child reads the transcript of an ARCHIVED parent — the events
/// outlive the session's status — pages backwards with `before`, and leaves
/// an audit marker in its own transcript.
#[tokio::test]
async fn history_reads_an_archived_parent_and_pages_backwards() {
    let Some(pool) = test_pool("peer_history").await else { return };
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    seed_owner(&pool, uid, &[machine]).await;
    let parent = Uuid::new_v4().to_string();
    let child = Uuid::new_v4().to_string();
    seed_session(&pool, &parent, None, uid, machine, "archived").await;
    seed_session(&pool, &child, Some(&parent), uid, machine, "active").await;
    for (role, text) in [
        ("user", "plan the migration"),
        ("assistant", "here is the plan"),
        ("assistant", "and the follow-up"),
    ] {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ($1, 'message', $2)",
        )
        .bind(&parent)
        .bind(json!({ "role": role, "text": text }))
        .execute(&pool)
        .await
        .expect("seed event");
    }

    let q = cctui_server_sessions::routes::sessions::ConversationQuery {
        limit: Some(DEFAULT_HISTORY_EVENTS),
        ..Default::default()
    };
    let mut rows =
        cctui_server_sessions::routes::sessions::renderable_rows(&pool, &parent, "claude-code", &q)
            .await
            .expect("rows");
    rows.reverse();
    let events: Vec<(i64, Value)> = rows.into_iter().map(|(id, v, _, _)| (id, v)).collect();
    let node = peer_policy::load_node(&pool, &parent).await.unwrap().unwrap();
    assert_eq!(node.state(), "archived");
    assert!(!node.is_live(), "an archived target must refuse a send");
    let header = transcript_md::Header {
        session_id: node.id.clone(),
        name: node.name.clone(),
        adapter: node.adapter_id.clone(),
        machine: node.machine_name.clone(),
        state: node.state(),
    };
    let all = transcript_md::render(&header, &events, &[], HISTORY_BUDGET_BYTES);
    assert_eq!(all.events, 3, "{}", all.text);
    assert!(all.text.contains("plan the migration"), "{}", all.text);
    assert!(all.text.contains("state: archived"), "{}", all.text);

    let only_user = transcript_md::render(&header, &events, &["user".into()], HISTORY_BUDGET_BYTES);
    assert_eq!(only_user.events, 1);

    // Pagination: one event at a time, walking back with `before`.
    let page = cctui_server_sessions::routes::sessions::ConversationQuery {
        limit: Some(1),
        ..Default::default()
    };
    let newest = cctui_server_sessions::routes::sessions::renderable_rows(
        &pool,
        &parent,
        "claude-code",
        &page,
    )
    .await
    .expect("newest page");
    assert_eq!(newest.len(), 1);
    let newest_seq = newest[0].0;
    let older = cctui_server_sessions::routes::sessions::renderable_rows(
        &pool,
        &parent,
        "claude-code",
        &cctui_server_sessions::routes::sessions::ConversationQuery {
            limit: Some(1),
            before: Some(newest_seq),
            ..Default::default()
        },
    )
    .await
    .expect("older page");
    assert_eq!(older.len(), 1);
    assert!(older[0].0 < newest_seq, "before must page backwards");

    audit(&pool, &child, "consulted history of the parent").await;
    let marker: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'text' FROM stream_events \
         WHERE session_id = $1 AND payload->>'role' = 'system_marker' \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&child)
    .fetch_optional(&pool)
    .await
    .expect("audit row");
    assert_eq!(marker.as_deref(), Some("consulted history of the parent"));
    cleanup(&pool, uid).await;
}

#[tokio::test]
async fn thread_history_reads_the_follow_up_line_without_the_injected_brief() {
    let Some(pool) = test_pool("peer_thread_history").await else { return };
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    seed_owner(&pool, uid, &[machine]).await;
    let a = Uuid::new_v4().to_string();
    let b = Uuid::new_v4().to_string();
    seed_session(&pool, &a, None, uid, machine, "archived").await;
    seed_session(&pool, &b, Some(&a), uid, machine, "active").await;
    sqlx::query("UPDATE sessions SET metadata = '{\"relation\": \"followup\"}' WHERE id = $1")
        .bind(&b)
        .execute(&pool)
        .await
        .expect("link the follow-up");
    let first = "<cctui-session-context>\nsession: lane\n</cctui-session-context>\n\n\
                 <cctui-previous-sessions>\n<cctui-previous-conversation n=\"1\" id=\"a\">\n\
                 BRIEF-TEXT\n</cctui-previous-conversation>\n</cctui-previous-sessions>\n\n\
                 ship it";
    for (sid, event_type, payload) in [
        (&a, "message", json!({ "role": "user", "text": "plan the migration" })),
        (&a, "tool_use", json!({ "tool": "Bash", "input": { "command": "ls" }, "id": "t" })),
        (&a, "message", json!({ "role": "assistant", "text": "here is the plan" })),
        (&b, "message", json!({ "role": "user", "text": first })),
        (&b, "message", json!({ "role": "assistant", "text": "shipped" })),
    ] {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(sid)
        .bind(event_type)
        .bind(payload)
        .execute(&pool)
        .await
        .expect("seed event");
    }
    let state = AppState::for_test(pool.clone());
    let target = peer_policy::load_node(&pool, &b).await.unwrap().unwrap();
    let params = PeerConversationParams {
        session_id: b.clone(),
        thread: Some("1".to_owned()),
        ..Default::default()
    };

    let out = thread_history(&state, Some(uid), &target, &params).await.unwrap();
    let md = out["markdown"].as_str().unwrap();
    for seen in ["plan the migration", "here is the plan", "ship it", "shipped", "continued"] {
        assert!(md.contains(seen), "{seen}: {md}");
    }
    assert!(md.find("here is the plan") < md.find("ship it"), "root first: {md}");
    assert!(!md.contains("BRIEF-TEXT") && !md.contains("session: lane"), "{md}");
    assert!(!md.contains("Bash"), "user and assistant only by default: {md}");
    assert_eq!(out["events"], 4);
    assert_eq!(out["head"], json!(b));
    assert_eq!(out["thread_root_id"], json!(a));
    assert!(out["next_cursor"].is_null());

    let foreign = Some(Uuid::new_v4());
    let stranger = thread_history(&state, foreign, &target, &params).await.unwrap();
    let md = stranger["markdown"].as_str().unwrap();
    assert!(md.contains("is not readable") && !md.contains("plan the migration"), "{md}");
    assert!(md.contains("shipped"), "the target itself stays readable: {md}");

    let tools = PeerConversationParams {
        roles: Some("user,assistant,tool".to_owned()),
        format: HistoryFormat::Json,
        ..params
    };
    let out = thread_history(&state, Some(uid), &target, &tools).await.unwrap();
    let items = out["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["entry"] == "followup_marker"), "{items:?}");
    assert!(items.iter().any(|i| i["event"]["type"] == "tool_call"), "{items:?}");
    assert!(items.iter().all(|i| i.get("injected").is_none()), "{items:?}");

    let paged = PeerConversationParams { limit: Some(1), ..tools };
    let newest = thread_history(&state, Some(uid), &target, &paged).await.unwrap();
    let cursor = newest["next_cursor"].as_str().expect("an older page").to_owned();
    let older = PeerConversationParams { cursor: Some(cursor), ..paged };
    let before = thread_history(&state, Some(uid), &target, &older).await.unwrap();
    assert_ne!(before["items"], newest["items"]);
    cleanup(&pool, uid).await;
}
