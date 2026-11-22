use cctui_proto::api::outcome::{DeclareOutcomeRequest, OutcomeSource, OutcomeStatus};
use uuid::Uuid;

use super::turn_end::evaluate;
use crate::state::AppState;

async fn test_state(name: &str) -> Option<AppState> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn seed(pool: &sqlx::PgPool, tag: &str) -> String {
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    let sid = format!("{tag}-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{tag}-{uid}"))
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
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $2, $3, '/w', 'active')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    sid
}

async fn message(pool: &sqlx::PgPool, sid: &str, role: &str, text: &str) {
    sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         VALUES ($1, 'message', jsonb_build_object('role', $2::text, 'text', $3::text))",
    )
    .bind(sid)
    .bind(role)
    .bind(text)
    .execute(pool)
    .await
    .unwrap();
}

fn tool(status: OutcomeStatus, headline: &str) -> DeclareOutcomeRequest {
    DeclareOutcomeRequest {
        status,
        headline: headline.to_owned(),
        body: Some("details".to_owned()),
        evidence: vec!["https://example.test/pr/1".to_owned()],
        source: OutcomeSource::Tool,
    }
}

async fn count(pool: &sqlx::PgPool, sid: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM session_outcomes WHERE session_id = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_interactive_session_declaring_a_result_shows_it_and_infers_nothing_more() {
    let Some(state) = test_state("outcome_interactive_result").await else { return };
    let sid = seed(&state.pool, "outcome-interactive").await;
    message(&state.pool, &sid, "user", "fix the bug").await;
    message(&state.pool, &sid, "assistant", "on it").await;
    let (stored, queued) =
        super::declare(&state, &sid, &tool(OutcomeStatus::Result, "Fixed it")).await.unwrap();
    assert!(!queued, "an interactive session has no dispatch callback");
    assert_eq!(stored.source, OutcomeSource::Tool);
    message(&state.pool, &sid, "assistant", "Fixed it, see the PR.").await;
    evaluate(&state, &sid).await.unwrap();
    let latest = super::latest(&state.pool, &sid).await.unwrap().unwrap();
    assert_eq!(latest.status, OutcomeStatus::Result);
    assert_eq!(latest.headline, "Fixed it");
    assert_eq!(latest.evidence, ["https://example.test/pr/1"]);
    assert_eq!(count(&state.pool, &sid).await, 1);
}

#[tokio::test]
async fn a_session_ending_its_turn_without_an_outcome_is_inferred_once() {
    let Some(state) = test_state("outcome_inferred_once").await else { return };
    let sid = seed(&state.pool, "outcome-inferred").await;
    message(&state.pool, &sid, "user", "look around").await;
    message(&state.pool, &sid, "assistant", "I looked around.").await;
    evaluate(&state, &sid).await.unwrap();
    evaluate(&state, &sid).await.unwrap();
    let latest = super::latest(&state.pool, &sid).await.unwrap().unwrap();
    assert_eq!(latest.status, OutcomeStatus::EndedWithoutOutcome);
    assert_eq!(latest.source, OutcomeSource::Inferred);
    assert_eq!(count(&state.pool, &sid).await, 1, "a repeated turn end stores one outcome");
}

#[tokio::test]
async fn a_new_prompt_after_a_result_lets_the_next_quiet_turn_supersede_it() {
    let Some(state) = test_state("outcome_superseded").await else { return };
    let sid = seed(&state.pool, "outcome-superseded").await;
    message(&state.pool, &sid, "user", "ship it").await;
    super::declare(&state, &sid, &tool(OutcomeStatus::Result, "Shipped")).await.unwrap();
    message(&state.pool, &sid, "user", "and now?").await;
    message(&state.pool, &sid, "assistant", "Thinking.").await;
    evaluate(&state, &sid).await.unwrap();
    let latest = super::latest(&state.pool, &sid).await.unwrap().unwrap();
    assert_eq!(latest.status, OutcomeStatus::EndedWithoutOutcome);
}

#[tokio::test]
async fn a_text_line_in_the_final_message_is_stored_as_a_text_outcome() {
    let Some(state) = test_state("outcome_text_line").await else { return };
    let sid = seed(&state.pool, "outcome-text").await;
    message(&state.pool, &sid, "user", "do it").await;
    message(&state.pool, &sid, "assistant", "Done.\nresult: rotated the key").await;
    evaluate(&state, &sid).await.unwrap();
    let latest = super::latest(&state.pool, &sid).await.unwrap().unwrap();
    assert_eq!((latest.status, latest.source), (OutcomeStatus::Result, OutcomeSource::Text));
    assert_eq!(latest.headline, "rotated the key");
}

#[tokio::test]
async fn an_open_user_action_keeps_the_turn_from_being_inferred() {
    let Some(state) = test_state("outcome_open_action").await else { return };
    let sid = seed(&state.pool, "outcome-open-action").await;
    message(&state.pool, &sid, "user", "deploy").await;
    message(&state.pool, &sid, "assistant", "Waiting on your approval.").await;
    sqlx::query("INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'approve')")
        .bind(&sid)
        .execute(&state.pool)
        .await
        .unwrap();
    evaluate(&state, &sid).await.unwrap();
    assert_eq!(count(&state.pool, &sid).await, 0);
}

#[tokio::test]
async fn a_dispatched_session_queues_its_first_declared_outcome_as_the_callback() {
    let Some(state) = test_state("outcome_dispatch_callback").await else { return };
    let sid = seed(&state.pool, "outcome-dispatch").await;
    sqlx::query(
        "INSERT INTO session_webhooks (session_id, notify_url, task_id, next_attempt_at) \
         VALUES ($1, 'https://example.com/hook', 'T-9', now() + interval '1 hour')",
    )
    .bind(&sid)
    .execute(&state.pool)
    .await
    .unwrap();
    let (_, queued) =
        super::declare(&state, &sid, &tool(OutcomeStatus::Result, "Opened PR")).await.unwrap();
    assert!(queued);
    let (_, again) =
        super::declare(&state, &sid, &tool(OutcomeStatus::Failed, "late")).await.unwrap();
    assert!(!again, "the first declaration owns the callback");
    let (payload, due): (serde_json::Value, bool) = sqlx::query_as(
        "SELECT payload, next_attempt_at <= now() FROM session_webhooks WHERE session_id = $1",
    )
    .bind(&sid)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert!(due, "a queued outcome is due on the next sweep");
    assert_eq!(payload["task_id"], "T-9");
    assert_eq!(payload["status"], "success");
    assert_eq!(payload["outcome"]["headline"], "Opened PR");
    sqlx::query("DELETE FROM session_webhooks WHERE session_id = $1")
        .bind(&sid)
        .execute(&state.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_result_file_relayed_twice_is_stored_once() {
    let Some(state) = test_state("outcome_result_file_dedupe").await else { return };
    let sid = seed(&state.pool, "outcome-result-file").await;
    let req = DeclareOutcomeRequest::from_result_file(&serde_json::json!({
        "status": "success", "message": "Done\n\nwith details",
    }))
    .normalized()
    .unwrap();
    let (first, _) = super::declare(&state, &sid, &req).await.unwrap();
    let (second, _) = super::declare(&state, &sid, &req).await.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(count(&state.pool, &sid).await, 1);
}
