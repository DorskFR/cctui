use super::{RemoveInitiator, normalize_last_message};
use axum::http::StatusCode;
use cctui_server_kernel::auth::AuthContext;
use chrono::{DateTime, Utc};

async fn seeded_session(test_name: &str) -> Option<(sqlx::PgPool, String)> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = uuid::Uuid::new_v4();
    let machine = uuid::Uuid::new_v4();
    let sid = format!("cct1086-{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("u-{uid}"))
        .bind(format!("k-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $2, $3, '/w', 'active')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(&pool)
    .await
    .unwrap();
    // Renderable assistant lines interleaved with turn bookkeeping the
    // client never sees; the newest row is bookkeeping, as after any
    // completed turn.
    let payloads = [
        serde_json::json!({"role": "assistant", "text": "one"}),
        serde_json::json!({"role": "no_such_role", "text": "hidden a"}),
        serde_json::json!({"role": "assistant", "text": "two"}),
        serde_json::json!({"role": "assistant", "text": "three"}),
        serde_json::json!({"role": "no_such_role", "text": "hidden b"}),
        serde_json::json!({"role": "assistant", "text": "four"}),
        serde_json::json!({"role": "no_such_role", "text": "hidden c"}),
    ];
    for p in payloads {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
        )
        .bind(&sid)
        .bind(p)
        .execute(&pool)
        .await
        .unwrap();
    }
    Some((pool, sid))
}

/// A user with one machine and one live session, plus the state the
/// result route reads it through.
async fn result_session(test_name: &str) -> Option<(crate::state::AppState, uuid::Uuid, String)> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = uuid::Uuid::new_v4();
    let machine = uuid::Uuid::new_v4();
    let sid = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("u-{uid}"))
        .bind(format!("k-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $2, $3, '/w', 'active')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(&pool)
    .await
    .unwrap();
    Some((crate::state::AppState::for_test(pool), uid, sid))
}

async fn store_rows(pool: &sqlx::PgPool, sid: &str, rows: &[(&str, serde_json::Value)]) {
    for (event_type, payload) in rows {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(sid)
        .bind(*event_type)
        .bind(payload)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn result_of(
    state: &crate::state::AppState,
    sid: &str,
) -> Result<cctui_proto::api::SessionResult, cctui_server_kernel::error::AppError> {
    super::get_result(axum::extract::State(state.clone()), axum::extract::Path(sid.to_owned()))
        .await
        .map(|j| j.0)
}

async fn queued_record(
    pool: &sqlx::PgPool,
    uid: uuid::Uuid,
    sid: &str,
    profile: &str,
    created: &str,
    queued: bool,
) {
    sqlx::query(
        "INSERT INTO dispatch_records \
           (session_id, user_id, dispatcher_name, profile, outcome, queued, created_at) \
         VALUES ($1, $2, 'k8s-' || $2::text, $3, 'forwarded', $4, $5::timestamptz)",
    )
    .bind(sid)
    .bind(uid)
    .bind(profile)
    .bind(queued)
    .bind(created)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_queued_dispatch_without_a_session_reports_its_queue_position() {
    let Some((state, uid, _)) =
        result_session("a_queued_dispatch_without_a_session_reports_its_queue_position").await
    else {
        return;
    };
    let ids: Vec<String> = (0..4).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let pool = &state.pool;
    queued_record(pool, uid, &ids[0], "lean", "2026-10-21T10:00:00Z", true).await;
    queued_record(pool, uid, &ids[1], "full", "2026-10-21T10:00:01Z", true).await;
    queued_record(pool, uid, &ids[2], "lean", "2026-10-21T10:00:02Z", true).await;
    queued_record(pool, uid, &ids[3], "lean", "2026-10-21T10:00:03Z", false).await;

    let second = result_of(&state, &ids[2]).await.unwrap();
    assert!(!second.done);
    assert!(second.queued);
    assert_eq!(second.queue_position, Some(2), "the other profile's queue is not counted");
    assert_eq!(result_of(&state, &ids[1]).await.unwrap().queue_position, Some(1));
    assert!(result_of(&state, &ids[3]).await.is_err(), "left the queue: still not found");
    assert!(result_of(&state, &uuid::Uuid::new_v4().to_string()).await.is_err());
}

#[tokio::test]
async fn a_live_session_reports_its_latest_message_as_not_done() {
    let Some((state, _, sid)) =
        result_session("a_live_session_reports_its_latest_message_as_not_done").await
    else {
        return;
    };
    store_rows(
        &state.pool,
        &sid,
        &[
            ("message", serde_json::json!({"role": "user", "text": "go"})),
            ("message", serde_json::json!({"role": "assistant", "text": "draft", "message_id": "m1"})),
            ("tool_use", serde_json::json!({"kind": "tool_use", "name": "Bash", "input": {}})),
            ("message", serde_json::json!({"role": "assistant_thinking", "text": "hmm", "message_id": "m2"})),
            ("message", serde_json::json!({"role": "assistant", "text": "head", "message_id": "m2"})),
            ("message", serde_json::json!({"role": "assistant", "text": "tail", "message_id": "m2"})),
        ],
    )
    .await;
    let result = result_of(&state, &sid).await.unwrap();
    assert!(!result.done);
    assert_eq!(result.end_reason, None);
    assert_eq!(result.message_id.as_deref(), Some("m2"));
    assert_eq!(result.final_assistant_text.as_deref(), Some("head\n\ntail"));
    assert!(result.token_usage.is_none(), "no usage row was stored for m2");
}

#[tokio::test]
async fn an_ended_session_reports_done_with_its_final_text_and_usage() {
    let Some((state, _, sid)) =
        result_session("an_ended_session_reports_done_with_its_final_text_and_usage").await
    else {
        return;
    };
    store_rows(
        &state.pool,
        &sid,
        &[
            (
                "message",
                serde_json::json!({"role": "assistant", "text": "old", "message_id": "m1"}),
            ),
            ("message", serde_json::json!({"role": "user", "text": "and then?"})),
            (
                "message",
                serde_json::json!({"role": "assistant_thinking", "text": "x", "message_id": "m2"}),
            ),
            (
                "message",
                serde_json::json!({"role": "assistant", "text": "answer", "message_id": "m2"}),
            ),
            ("session_ended", serde_json::json!({"reason": {"kind": "completed"}})),
        ],
    )
    .await;
    sqlx::query(
        "INSERT INTO session_token_usage (session_id, message_id, input_tokens, output_tokens) \
         VALUES ($1, 'm2', 120, 7)",
    )
    .bind(&sid)
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET ended_at = now(), end_reason = 'completed' WHERE id = $1")
        .bind(&sid)
        .execute(&state.pool)
        .await
        .unwrap();
    let result = result_of(&state, &sid).await.unwrap();
    assert!(result.done);
    assert_eq!(result.end_reason.as_deref(), Some("completed"));
    assert_eq!(result.message_id.as_deref(), Some("m2"));
    assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
    let usage = result.token_usage.expect("usage of m2");
    assert_eq!((usage.tokens_in, usage.tokens_out), (120, 7));
}

async fn set_signals(pool: &sqlx::PgPool, sid: &str, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE sessions SET {sql} WHERE id = $1")))
        .bind(sid)
        .execute(pool)
        .await
        .unwrap();
}

async fn store_answer(pool: &sqlx::PgPool, sid: &str) {
    store_rows(
        pool,
        sid,
        &[
            ("message", serde_json::json!({"role": "user", "text": "go"})),
            (
                "message",
                serde_json::json!({"role": "assistant", "text": "answer", "message_id": "m1"}),
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn a_running_turn_is_not_done_even_with_a_stale_success_activity() {
    let Some((state, _, sid)) =
        result_session("a_running_turn_is_not_done_even_with_a_stale_success_activity").await
    else {
        return;
    };
    store_answer(&state.pool, &sid).await;
    set_signals(
        &state.pool,
        &sid,
        "tempo = 'active', agent_state = 'working', activity = 'success'",
    )
    .await;
    let result = result_of(&state, &sid).await.unwrap();
    assert!(!result.done);
    assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
}

#[tokio::test]
async fn a_finished_turn_on_a_live_session_is_done() {
    let Some((state, _, sid)) = result_session("a_finished_turn_on_a_live_session_is_done").await
    else {
        return;
    };
    store_answer(&state.pool, &sid).await;
    set_signals(&state.pool, &sid, "tempo = 'idle', agent_state = 'done'").await;
    let result = result_of(&state, &sid).await.unwrap();
    assert!(result.done);
    assert_eq!(result.end_reason, None);
    assert_eq!(result.message_id.as_deref(), Some("m1"));
    assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
}

#[tokio::test]
async fn a_turn_waiting_on_input_is_not_done() {
    let Some((state, _, sid)) = result_session("a_turn_waiting_on_input_is_not_done").await else {
        return;
    };
    set_signals(&state.pool, &sid, "tempo = 'blocked', agent_state = 'done'").await;
    assert!(!result_of(&state, &sid).await.unwrap().done);
}

#[tokio::test]
async fn codex_and_opencode_turns_read_done_only_after_going_idle() {
    let turns = [
        ("codex", "tempo = 'active', agent_state = 'working'", "agent_state = 'idle'"),
        (
            "opencode",
            "tempo = 'working', agent_state = 'working'",
            "tempo = 'idle', agent_state = 'idle'",
        ),
    ];
    for (adapter, busy, idle) in turns {
        let Some((state, _, sid)) =
            result_session(&format!("{adapter}_turns_read_done_only_after_going_idle")).await
        else {
            return;
        };
        set_signals(&state.pool, &sid, &format!("adapter_id = '{adapter}', {busy}")).await;
        assert!(!result_of(&state, &sid).await.unwrap().done, "{adapter} busy");
        set_signals(&state.pool, &sid, idle).await;
        assert!(result_of(&state, &sid).await.unwrap().done, "{adapter} idle");
        set_signals(&state.pool, &sid, busy).await;
        assert!(!result_of(&state, &sid).await.unwrap().done, "{adapter} next turn");
    }
}

/// Archiving clears the classifier signals and leaves `ended_at` unset,
/// so a session archived mid-turn is done on its status alone and
/// reports whatever text it had produced.
#[tokio::test]
async fn an_archived_session_is_done_with_the_text_it_had() {
    let Some((state, _, sid)) =
        result_session("an_archived_session_is_done_with_the_text_it_had").await
    else {
        return;
    };
    store_answer(&state.pool, &sid).await;
    set_signals(&state.pool, &sid, "tempo = 'active', agent_state = 'working'").await;
    set_signals(
        &state.pool,
        &sid,
        "status = 'archived', tempo = NULL, agent_state = NULL, activity = NULL, \
         archived_by = 'automatic'",
    )
    .await;
    let result = result_of(&state, &sid).await.unwrap();
    assert!(result.done);
    assert_eq!(result.end_reason, None);
    assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
}

/// The route sits behind the session read guard: the owner reads it,
/// another user is refused, and an id nobody owns is unknown — the same
/// 404 the handler itself returns for a session that never registered.
#[tokio::test]
async fn the_result_route_is_owner_only_and_unknown_ids_are_404() {
    let Some((state, owner, sid)) =
        result_session("the_result_route_is_owner_only_and_unknown_ids_are_404").await
    else {
        return;
    };
    let other = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(other)
        .bind(format!("u-{other}"))
        .bind(format!("k-{other}"))
        .execute(&state.pool)
        .await
        .unwrap();
    let ctx = |user_id| AuthContext {
        user_id,
        key_id: uuid::Uuid::new_v4(),
        machine_id: None,
        scopes: std::collections::BTreeSet::new(),
    };
    assert_eq!(
        cctui_server_kernel::authz::authorize_session_read(&ctx(owner), &sid, &state.pool).await,
        Ok(())
    );
    assert_eq!(
        cctui_server_kernel::authz::authorize_session_read(&ctx(other), &sid, &state.pool).await,
        Err(StatusCode::FORBIDDEN)
    );
    let unknown = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        cctui_server_kernel::authz::authorize_session_read(&ctx(owner), &unknown, &state.pool)
            .await,
        Err(StatusCode::NOT_FOUND)
    );
    let err = result_of(&state, &unknown).await.expect_err("unknown session");
    assert_eq!(err.status(), StatusCode::NOT_FOUND);

    let empty = result_of(&state, &sid).await.unwrap();
    assert!(!empty.done);
    assert_eq!(empty.final_assistant_text, None);
    assert_eq!(empty.message_id, None);
}

async fn account_on(pool: &sqlx::PgPool, uid: uuid::Uuid, name: &str) -> uuid::Uuid {
    let account = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(account)
        .bind(uid)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query_scalar(
        "INSERT INTO account_providers \
             (user_id, provider, encrypted_refresh_token, account_id) \
         VALUES ($1, 'anthropic', 'x', $2) RETURNING id",
    )
    .bind(uid)
    .bind(account)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn token(pool: &sqlx::PgPool, sid: &str, provider: uuid::Uuid, revoked: bool, age: i32) {
    sqlx::query(
        "INSERT INTO session_tokens \
             (token_hash, session_id, account_id, created_at, revoked_at) \
         VALUES ($1, $2, $3, now() - make_interval(mins => $4), \
                 CASE WHEN $5 THEN now() END)",
    )
    .bind(format!("th-{}", uuid::Uuid::new_v4()))
    .bind(sid)
    .bind(provider)
    .bind(age)
    .bind(revoked)
    .execute(pool)
    .await
    .unwrap();
}

/// Traffic observed is whether a token was ever used: ending the session
/// revokes it and the session must not fall back to the warning. A bound
/// token never presented at the gateway stays unobserved.
#[tokio::test]
async fn a_used_token_stays_observed_once_revoked() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("a_used_token_stays_observed_once_revoked")
    else {
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
        .bind(format!("u-{uid}"))
        .bind(format!("k-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    let used = format!("used-{}", uuid::Uuid::new_v4());
    let unused = format!("unused-{}", uuid::Uuid::new_v4());
    for sid in [&used, &unused] {
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, user_id, working_dir, status) \
             VALUES ($1, 'm', $2, '/w', 'active')",
        )
        .bind(sid)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    }
    let provider = account_on(&pool, uid, &format!("acct-{uid}")).await;
    token(&pool, &used, provider, false, 5).await;
    token(&pool, &unused, provider, false, 5).await;
    sqlx::query("UPDATE session_tokens SET last_used_at = now() WHERE session_id = $1")
        .bind(&used)
        .execute(&pool)
        .await
        .unwrap();

    let state = crate::state::AppState::for_test(pool.clone());
    let ids = vec![used.clone(), unused.clone()];
    let live = super::fetch_traffic_observed(&state, &ids).await.unwrap();
    assert!(live.contains(&used), "used live token is observed");
    assert!(!live.contains(&unused), "unused token is not observed");

    for sid in &ids {
        cctui_server_accounts::session_credentials::revoke_session_tokens(&state, sid).await;
    }
    let revoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM session_tokens \
         WHERE session_id = ANY($1) AND revoked_at IS NOT NULL",
    )
    .bind(&ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked, 2, "ending revokes every token");

    let ended = super::fetch_traffic_observed(&state, &ids).await.unwrap();
    assert!(ended.contains(&used), "a revoked token that was used stays observed");
    assert!(!ended.contains(&unused), "a revoked unused token stays unobserved");

    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
}

#[test]
fn a_spoken_prompt_previews_as_its_utterance() {
    let spoken = cctui_server_voice::text::spoken_prompt(
        "why is the sky blue",
        Some(cctui_proto::voice::SpokenStyle::Brief),
    );
    assert_eq!(super::preview_last_message(&spoken, false), "why is the sky blue");
    let only = "<cctui-session-context>\nctx\n</cctui-session-context>";
    assert_eq!(super::preview_last_message(only, false), normalize_last_message(only));
}

async fn index_session(pool: &sqlx::PgPool, sid: &str) {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM stream_events WHERE session_id = $1")
        .bind(sid)
        .fetch_all(pool)
        .await
        .unwrap();
    cctui_server_search::reindex(pool, &ids).await.unwrap();
}

async fn insert_payload(pool: &sqlx::PgPool, sid: &str, kind: &str, payload: serde_json::Value) {
    sqlx::query("INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)")
        .bind(sid)
        .bind(kind)
        .bind(payload)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn conversation_search_matches_text_copied_from_the_rendered_view() {
    let Some((pool, sid)) = seeded_session("conversation_search_rendered").await else {
        return;
    };
    let say = |text: String| serde_json::json!({"role": "assistant", "text": text});
    insert_payload(
        &pool,
        &sid,
        "message",
        say("Change the gate to `master...HEAD` (three dots).".into()),
    )
    .await;
    insert_payload(
        &pool,
        &sid,
        "message",
        say(format!("{} the hidden tail phrase", "filler ".repeat(2000))),
    )
    .await;
    insert_payload(&pool, &sid, "message", say("It\u{2019}s \u{201C}quoted\u{201D} text".into()))
        .await;
    insert_payload(&pool, &sid, "message", say("first line\nsecond **line**\n\n- third".into()))
        .await;
    let blob = "iVBORw0KGgoAAAANSUhEUgAA".repeat(40);
    insert_payload(
        &pool,
        &sid,
        "tool_use",
        serde_json::json!({"kind": "tool_result", "content": [
            {"type": "text", "text": "screenshot saved"},
            {"type": "image", "source": {"type": "base64", "data": blob}},
        ]}),
    )
    .await;
    index_session(&pool, &sid).await;

    let hits = |q: &'static str| {
        let (pool, sid) = (pool.clone(), sid.clone());
        async move {
            let root = cctui_query::parse(&super::straighten_phrase_quotes(q));
            let terms = super::Terms::of(&root.free_text_terms());
            let filters = super::conv_filters(&[]);
            let limits = super::ConvScanLimits::DEFAULT;
            let scan =
                super::conv_search_rows(&pool, &sid, &terms, &filters, limits).await.unwrap();
            assert!(!scan.partial, "no timeout");
            scan.rows.len()
        }
    };
    assert_eq!(hits("\"Change the gate to master...HEAD (three dots).\"").await, 1);
    assert_eq!(hits("\"the hidden tail phrase\"").await, 1, "text past 8 KB");
    assert_eq!(hits("\"It's \"").await, 1, "a straight quote finds a smart one");
    assert_eq!(
        hits("\"It\u{2019}s \u{201C}quoted\u{201D} text\"").await,
        1,
        "smart quotes copied inside a phrase"
    );
    assert_eq!(
        hits("\u{201C}Change the gate to master...HEAD (three dots).\u{201D}").await,
        1,
        "a phrase pasted between smart quotes"
    );
    assert_eq!(hits("\"first line second line third\"").await, 1, "multi-line copy");
    assert_eq!(hits("screenshot saved").await, 0, "tool output is not indexed");
    assert_eq!(hits("iVBORw0KGgo").await, 0, "image payloads are not indexed");
    assert_eq!(hits("gate dots").await, 1, "unquoted words must all appear");
    assert_eq!(hits("gate absentword").await, 0);

    let page = search_page(10, None);
    let root = cctui_query::parse("\"master...HEAD (three dots)\"");
    let found = super::scan_matches(&pool, &root, None, &page).await.unwrap();
    assert!(found.rows.iter().any(|r| r.id == sid), "the session list finds it too");

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
}

const fn search_page(limit: i64, uid: Option<uuid::Uuid>) -> super::SearchPage {
    super::SearchPage { limit, offset: 0, cursor: None, uid, exclude_dispatched: false }
}

#[derive(Clone, Copy)]
struct Seed<'a> {
    id: &'a str,
    machine: uuid::Uuid,
    parent: Option<&'a str>,
    pinned: bool,
    relation: Option<&'a str>,
    minutes_ago: i64,
}

impl<'a> Seed<'a> {
    const fn on(id: &'a str, machine: uuid::Uuid, minutes_ago: i64) -> Self {
        Self { id, machine, parent: None, pinned: false, relation: None, minutes_ago }
    }
}

async fn search_owner(test_name: &str) -> Option<(sqlx::PgPool, uuid::Uuid)> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("u-{uid}"))
        .bind(format!("k-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    Some((pool, uid))
}

async fn add_machine(pool: &sqlx::PgPool, uid: uuid::Uuid, kind: &str) -> uuid::Uuid {
    let machine = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO machines (id, user_id, name, key_hash, kind) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(machine)
    .bind(uid)
    .bind(format!("m-{machine}"))
    .bind(format!("mk-{machine}"))
    .bind(kind)
    .execute(pool)
    .await
    .unwrap();
    machine
}

async fn add_session(pool: &sqlx::PgPool, uid: uuid::Uuid, dir: &str, seed: Seed<'_>) {
    let metadata =
        seed.relation.map_or_else(|| serde_json::json!({}), |r| serde_json::json!({"relation": r}));
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         parent_id, pinned, metadata, registered_at) \
         VALUES ($1, $2, $2, $3, $4, 'active', $5, $6, $7, \
                 now() - make_interval(mins => $8::int))",
    )
    .bind(seed.id)
    .bind(seed.machine)
    .bind(uid)
    .bind(dir)
    .bind(seed.parent)
    .bind(seed.pinned)
    .bind(metadata)
    .bind(i32::try_from(seed.minutes_ago).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

async fn drop_owner(pool: &sqlx::PgPool, uid: uuid::Uuid) {
    for sql in [
        "DELETE FROM sessions WHERE user_id = $1",
        "DELETE FROM machines WHERE user_id = $1",
        "DELETE FROM users WHERE id = $1",
    ] {
        sqlx::query(sql).bind(uid).execute(pool).await.unwrap();
    }
}

async fn search_pages(
    pool: &sqlx::PgPool,
    q: &str,
    mut page: super::SearchPage,
) -> (Vec<Vec<String>>, Option<cctui_proto::api::SearchTotal>) {
    let root = cctui_query::parse(q);
    let mut pages = Vec::new();
    let mut total = None;
    loop {
        let found = super::scan_matches(pool, &root, None, &page).await.unwrap();
        assert!(!found.partial, "{q}: no timeout");
        if pages.is_empty() {
            total = found.total;
        } else {
            assert_eq!(found.total, None, "{q}: only the first page counts");
        }
        pages.push(found.rows.into_iter().map(|r| r.id).collect::<Vec<_>>());
        let Some(next) = found.next_cursor else { break };
        page.cursor = super::parse_search_cursor(&next);
    }
    (pages, total)
}

fn sorted(pages: &[Vec<String>]) -> Vec<String> {
    let mut ids: Vec<String> = pages.concat();
    ids.sort();
    ids
}

#[derive(Clone, Copy)]
struct SearchPageOpts {
    limit: i64,
    uid: uuid::Uuid,
    exclude: bool,
}

impl SearchPageOpts {
    fn page(self) -> super::SearchPage {
        super::SearchPage {
            exclude_dispatched: self.exclude,
            ..search_page(self.limit, Some(self.uid))
        }
    }
}

#[tokio::test]
async fn a_bare_term_matches_from_a_word_start_and_each_term_narrows() {
    let Some((pool, uid)) = search_owner("search_word_start").await else {
        return;
    };
    let own = add_machine(&pool, uid, "persistent").await;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let texts = [
        ("both", "kanade hook render"),
        ("kanade", "kanade only"),
        ("lefthook", "lefthook config kanade"),
        ("usehook", "useHook here"),
        ("hooks", "the hooks are loaded"),
    ];
    for (i, (name, text)) in (0_i64..).zip(texts) {
        let sid = format!("{token}-{name}");
        add_session(&pool, uid, "/w", Seed::on(&sid, own, i)).await;
        let say = serde_json::json!({"role": "assistant", "text": text});
        insert_payload(&pool, &sid, "message", say).await;
        index_session(&pool, &sid).await;
    }
    let opts = SearchPageOpts { limit: 50, uid, exclude: false };
    let hits = |q: &'static str| {
        let pool = pool.clone();
        async move {
            let (pages, total) = search_pages(&pool, q, opts.page()).await;
            let ids = sorted(&pages);
            let count = u32::try_from(ids.len()).unwrap();
            assert_eq!(total, Some(cctui_proto::api::SearchTotal { count, at_least: false }));
            ids.iter().map(|id| id.rsplit('-').next().unwrap().to_owned()).collect::<Vec<_>>()
        }
    };
    assert_eq!(hits("hook").await, ["both", "hooks"], "not lefthook nor useHook");
    assert_eq!(hits("kanade").await, ["both", "kanade", "lefthook"]);
    assert_eq!(hits("kanade hook").await, ["both"], "a b is a subset of a");
    assert_eq!(hits("*hook").await, ["both", "hooks", "lefthook", "usehook"]);
    assert_eq!(hits("\"hook\"").await, ["both", "hooks", "lefthook", "usehook"]);

    let one = SearchPageOpts { limit: 1, ..opts };
    let root = cctui_query::parse("*hook");
    let found = super::scan_matches(&pool, &root, None, &one.page()).await.unwrap();
    assert_eq!(found.rows.len(), 1);
    assert_eq!(
        found.total,
        Some(cctui_proto::api::SearchTotal { count: 4, at_least: false }),
        "the count goes on past a full page"
    );

    drop_owner(&pool, uid).await;
}

async fn conv_scan(
    pool: &sqlx::PgPool,
    sid: &str,
    q: &str,
    limits: super::ConvScanLimits,
) -> super::ConvScan {
    let terms = super::Terms::of(&cctui_query::parse(q).free_text_terms());
    super::conv_search_rows(pool, sid, &terms, &super::conv_filters(&[]), limits).await.unwrap()
}

#[tokio::test]
async fn conversation_search_out_of_time_returns_the_windows_it_reached() {
    let Some((pool, sid)) = seeded_session("conversation_search_partial").await else {
        return;
    };
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM stream_events WHERE session_id = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    for i in 0..5 {
        let say = serde_json::json!({"role": "assistant", "text": format!("needle {i}")});
        insert_payload(&pool, &sid, "message", say).await;
    }
    index_session(&pool, &sid).await;
    let texts = |scan: &super::ConvScan| {
        scan.rows
            .iter()
            .map(|r| r.2["text"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };
    let narrow = before + 2;

    let cut = super::ConvScanLimits { window: narrow, budget: std::time::Duration::ZERO };
    let scan = conv_scan(&pool, &sid, "needle", cut).await;
    assert!(scan.partial, "windows were left unscanned");
    assert_eq!(texts(&scan), ["needle 0", "needle 1"], "the hits of the first window");

    let walked = super::ConvScanLimits { window: narrow, budget: super::SEARCH_BUDGET };
    let scan = conv_scan(&pool, &sid, "needle", walked).await;
    assert!(!scan.partial);
    assert_eq!(scan.rows.len(), 5, "every window walked: {:?}", texts(&scan));

    let one = super::ConvScanLimits { window: 10_000, budget: std::time::Duration::ZERO };
    let scan = conv_scan(&pool, &sid, "needle", one).await;
    assert!(!scan.partial, "a single window covers the session");
    assert_eq!(scan.rows.len(), 5);

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn search_finds_an_event_through_its_chunks_only() {
    let Some((pool, sid)) = seeded_session("search_chunks_only").await else {
        return;
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         VALUES ($1, 'message', jsonb_build_object('role', 'assistant', 'text', $2::text)) \
         RETURNING id",
    )
    .bind(&sid)
    .bind("an ancient phrase")
    .fetch_one(&pool)
    .await
    .unwrap();
    let hits = |q: &'static str| {
        let (pool, sid) = (pool.clone(), sid.clone());
        async move {
            let terms = super::Terms::of(&cctui_query::parse(q).free_text_terms());
            let filters = super::conv_filters(&[]);
            let limits = super::ConvScanLimits::DEFAULT;
            let scan =
                super::conv_search_rows(&pool, &sid, &terms, &filters, limits).await.unwrap();
            scan.rows.into_iter().map(|r| r.0).collect::<Vec<_>>()
        }
    };
    let listed = |q: &'static str| {
        let pool = pool.clone();
        async move {
            let page = search_page(10, None);
            super::scan_matches(&pool, &cctui_query::parse(q), None, &page).await.unwrap().rows
        }
    };
    assert!(hits("\"ancient phrase\"").await.is_empty(), "no chunks, no hit");
    assert!(!listed("\"ancient phrase\"").await.iter().any(|r| r.id == sid));

    cctui_server_search::reindex(&pool, &[id]).await.unwrap();
    assert_eq!(hits("\"ancient phrase\"").await, [id]);
    assert!(listed("\"ancient phrase\"").await.iter().any(|r| r.id == sid));
    let terms = super::Terms::of_query(&cctui_query::parse("ancient"));
    let snippets = super::fetch_snippets(&pool, std::slice::from_ref(&sid), &terms).await.unwrap();
    assert_eq!(snippets.len(), 1, "{snippets:?}");
    assert_eq!(snippets[0].2, id);

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn conversation_search_sql_ands_terms_and_honours_the_date_window() {
    let Some((pool, sid)) = seeded_session("conversation_search_sql").await else {
        return;
    };
    for (text, at) in [
        ("alpha and beta together", "2026-09-05T00:00:00Z"),
        ("alpha only", "2026-09-05T00:00:00Z"),
        ("alpha and beta, but later", "2026-10-05T00:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload, created_at) \
             VALUES ($1, 'message', jsonb_build_object('role', 'assistant', 'text', $2::text), $3)",
        )
        .bind(&sid)
        .bind(text)
        .bind(at.parse::<DateTime<Utc>>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }

    index_session(&pool, &sid).await;
    let run =
        |patterns: Vec<String>, after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>| {
            let (pool, sid) = (pool.clone(), sid.clone());
            async move {
                let mut q = sqlx::query_as::<_, super::ConvSearchRow>(sqlx::AssertSqlSafe(
                    super::conv_search_sql(patterns.len()),
                ))
                .bind(&sid);
                for p in &patterns {
                    q = q.bind(p);
                }
                q.bind(after)
                    .bind(before)
                    .bind(100_i64)
                    .bind(0_i64)
                    .bind(i64::MAX)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
            }
        };
    let texts = |rows: Vec<super::ConvSearchRow>| {
        rows.into_iter()
            .map(|r| r.2["text"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };

    let both = texts(run(vec!["%alpha%".into(), "%beta%".into()], None, None).await);
    assert_eq!(both.len(), 2, "only events carrying BOTH terms: {both:?}");
    assert!(both.iter().all(|t| t.contains("alpha") && t.contains("beta")));

    let one = run(vec!["%alpha%".into()], None, None).await;
    assert_eq!(one.len(), 3);
    let ids: Vec<i64> = one.iter().map(|r| r.0).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "hits come back oldest first");

    let windowed = run(
        vec!["%alpha%".into()],
        None,
        Some("2026-10-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
    )
    .await;
    assert_eq!(windowed.len(), 2, "`before` is exclusive of the bound");

    let after = run(
        vec!["%alpha%".into()],
        Some("2026-10-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
        None,
    )
    .await;
    assert_eq!(after.len(), 1);

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn archiving_forgets_the_guard_step_mirror() {
    let Some(url) = cctui_server_kernel::test_support::db_url("archiving_forgets_the_guard_step")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let state = crate::state::AppState::for_test(pool.clone());
    let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "arch-guard").await;
    let sid = crate::events::tests::seed_session(&pool, uid, machine, Some("g")).await;
    cctui_server_sessions::guard_step::record(
        sid.clone(),
        cctui_proto::guard_step::GuardStepState { step: 2, ..Default::default() },
    );
    let outcome = super::archive_one(
        &state,
        &sid,
        true,
        RemoveInitiator::User,
        cctui_server_kernel::state_events::Actor::User(uid),
    )
    .await
    .unwrap();
    assert_eq!(outcome, super::ArchiveOutcome::Archived);
    assert_eq!(cctui_server_sessions::guard_step::get(&sid), None);
}

async fn search_without_transcript_search(
    q: &str,
    include_archived: bool,
) -> Result<axum::Json<cctui_proto::api::SessionListResponse>, cctui_server_kernel::error::AppError>
{
    use cctui_proto::features::{Feature, FeatureSet};
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    let mut state = crate::state::AppState::for_test(pool);
    state.config.features = FeatureSet::all().without(Feature::TranscriptSearch);
    let ctx = AuthContext {
        user_id: uuid::Uuid::new_v4(),
        key_id: uuid::Uuid::new_v4(),
        machine_id: None,
        scopes: std::collections::BTreeSet::new(),
    };
    super::search_sessions(
        axum::extract::State(state),
        axum::Extension(ctx),
        axum::extract::Query(super::SearchParams {
            q: q.to_owned(),
            include_archived,
            limit: None,
            offset: None,
            cursor: None,
            exclude_dispatched: false,
        }),
    )
    .await
}

#[tokio::test]
async fn a_text_query_is_refused_when_transcript_search_is_off() {
    for q in ["foo", "status:archived", "  \"fix bug\"  "] {
        let Err(err) = search_without_transcript_search(q, true).await else {
            panic!("{q:?} must be refused with transcript search off");
        };
        assert_eq!(err.status(), StatusCode::BAD_REQUEST, "{q:?}");
        assert_eq!(err.message(), super::TRANSCRIPT_SEARCH_OFF, "{q:?}");
    }
}

#[tokio::test]
async fn a_blank_query_is_not_refused_when_transcript_search_is_off() {
    let axum::Json(found) =
        search_without_transcript_search("   ", false).await.expect("blank query is a browse");
    assert!(found.sessions.is_empty());
}
