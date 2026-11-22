use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use super::reply_gate_tests::queue_state;
use super::{claim_one, settle};
use crate::auth::{AuthContext, Scope};
use crate::inbox::deliver::Outcome;
use crate::outbox::MAX_ATTEMPTS;
use crate::schema::BOOTSTRAP;
use crate::state::AppState;

const FOLD: &str = include_str!("../../../schema/0014_fold_outboxes.sql");
const DROP: &str = include_str!("../../../schema/0016_drop_outboxes.sql");

async fn pool(name: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn seed_owner(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("fold-{uid}"))
        .bind(format!("hf-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    crate::store::acls::set_user_ceiling(&mut conn, uid, &[Scope::Read, Scope::Dispatch])
        .await
        .unwrap();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mkf-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    (uid, machine)
}

async fn seed_session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid, status: &str) -> String {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id, tempo) VALUES ($1, $2, $3, $4, '/w', $5, 'claude-code', 'idle')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sid
}

fn baseline_table(name: &str) -> &'static str {
    let header = format!("-- TABLE: {name}\n");
    let (_, block) = BOOTSTRAP.split_at(BOOTSTRAP.find(&header).expect("a baseline table"));
    block.split_at(block.find("\n\n").expect("the block ends")).0
}

/// The outboxes as they stood before the drop, which every test database has run.
async fn legacy_outboxes(conn: &mut sqlx::PgConnection) {
    for sql in [
        "DROP TABLE IF EXISTS session_message_queue, draft_launch_queue",
        baseline_table("session_message_queue"),
        baseline_table("draft_launch_queue"),
        include_str!("../../../schema/0003_scheduled_message_sealed_body.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&mut *conn).await.unwrap();
    }
}

async fn outboxes_exist(conn: &mut sqlx::PgConnection) -> (bool, bool) {
    sqlx::query_as(
        "SELECT to_regclass('public.session_message_queue') IS NOT NULL, \
                to_regclass('public.draft_launch_queue') IS NOT NULL",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn old_message(
    conn: &mut sqlx::PgConnection,
    sid: &str,
    user: Uuid,
    state: &str,
    sent_ago: &str,
) -> (Uuid, Uuid) {
    sqlx::query_as(
        "INSERT INTO session_message_queue \
           (session_id, user_id, body, sealed_body, ask_picks, state, deliver_at, attempts, \
            next_attempt_at, last_error, sent_at) \
         VALUES ($1, $2, 'later', \
                 CASE WHEN $3::text = 'scheduled' THEN 'v1:00' END, \
                 CASE WHEN $3::text = 'scheduled' THEN '{\"0\": [\"a\"]}'::jsonb END, \
                 $3, now() + interval '1 hour', 2, now() + interval '1 hour', 'boom', \
                 CASE WHEN $3::text = 'sent' THEN now() - $4::interval END) \
         RETURNING id, turn_id",
    )
    .bind(sid)
    .bind(user)
    .bind(state)
    .bind(sent_ago)
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn old_draft(conn: &mut sqlx::PgConnection, draft: &str, user: Uuid, state: &str) {
    sqlx::query(
        "INSERT INTO draft_launch_queue \
           (draft_id, user_id, state, launch_at, attempts, next_attempt_at, last_error) \
         VALUES ($1, $2, $3, now() + interval '2 hours', 1, now() + interval '2 hours', 'off')",
    )
    .bind(draft)
    .bind(user)
    .bind(state)
    .execute(conn)
    .await
    .unwrap();
}

#[derive(Debug, sqlx::FromRow)]
struct Folded {
    status: String,
    source: String,
    source_id: Option<String>,
    key: String,
    lane_user: Uuid,
    lane_session: Option<String>,
    batch_id: Option<Uuid>,
    kind: Option<String>,
    has_ask_picks: bool,
    sealed_body: Option<String>,
    attempts: i32,
    last_error: Option<String>,
    delivered_as: Option<String>,
}

async fn folded_message(conn: &mut sqlx::PgConnection, id: Uuid) -> Option<Folded> {
    sqlx::query_as(concat!(
        "SELECT e.status, e.source, e.source_id, l.key, l.user_id AS lane_user, \
                l.session_id AS lane_session, e.batch_id, e.payload->>'kind' AS kind, \
                e.payload ? 'ask_picks' AS has_ask_picks, e.sealed_body, e.attempts, \
                e.last_error, e.delivered_as \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id ",
        "WHERE e.id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await
    .unwrap()
}

async fn folded_draft(conn: &mut sqlx::PgConnection, draft: &str) -> Option<Folded> {
    sqlx::query_as(concat!(
        "SELECT e.status, e.source, e.source_id, l.key, l.user_id AS lane_user, \
                l.session_id AS lane_session, e.batch_id, e.payload->>'kind' AS kind, \
                e.payload ? 'ask_picks' AS has_ask_picks, e.sealed_body, e.attempts, \
                e.last_error, e.delivered_as \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id ",
        "WHERE l.key = 'draft:' || $1::text AND e.source_id = $1"
    ))
    .bind(draft)
    .fetch_optional(conn)
    .await
    .unwrap()
}

async fn footprint(conn: &mut sqlx::PgConnection, keys: &[String], sid: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM inbox_lanes WHERE key = ANY($1)), \
                (SELECT count(*) FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
                 WHERE l.key = ANY($1)), \
                (SELECT count(*) FROM scheduled_turns WHERE session_id = $2)",
    )
    .bind(keys)
    .bind(sid)
    .fetch_one(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_fold_backfills_every_live_state_once_and_a_second_run_adds_nothing() {
    let name = "the_fold_backfills_every_live_state_once_and_a_second_run_adds_nothing";
    let Some(pool) = pool(name).await else { return };
    let (owner, machine) = seed_owner(&pool).await;
    let sid = seed_session(&pool, owner, machine, "active").await;
    let mut drafts = Vec::new();
    for _ in 0..4 {
        drafts.push(seed_session(&pool, owner, machine, "draft").await);
    }
    let mut tx = pool.begin().await.unwrap();
    legacy_outboxes(&mut tx).await;

    let mut messages = Vec::new();
    for (state, ago) in [
        ("scheduled", "0"),
        ("sending", "0"),
        ("sent", "1 hour"),
        ("sent", "2 days"),
        ("dead", "0"),
        ("cancelled", "0"),
    ] {
        messages.push((state, ago, old_message(&mut tx, &sid, owner, state, ago).await));
    }
    for (draft, state) in drafts.iter().zip(["scheduled", "launching", "launched", "dead"]) {
        old_draft(&mut tx, draft, owner, state).await;
    }

    sqlx::raw_sql(FOLD).execute(&mut *tx).await.unwrap();

    let key = format!("session:{sid}");
    for (state, ago, (id, turn_id)) in &messages {
        let folded = folded_message(&mut tx, *id).await;
        let expected = match (*state, *ago) {
            ("scheduled", _) => Some("pending"),
            ("sending", _) => Some("claimed"),
            ("sent", "1 hour") => Some("delivered"),
            ("dead", _) => Some("dead"),
            _ => None,
        };
        let Some(expected) = expected else {
            assert!(folded.is_none(), "{state} ({ago}) is terminal history the routes never show");
            continue;
        };
        let folded = folded.unwrap_or_else(|| panic!("{state} was not folded"));
        assert_eq!(folded.status, expected, "{state}");
        assert_eq!((folded.source.as_str(), folded.source_id), ("human", None));
        assert_eq!((&folded.key, folded.lane_user), (&key, owner));
        assert_eq!(folded.lane_session.as_deref(), Some(sid.as_str()));
        assert_eq!(folded.batch_id, Some(*turn_id), "{state}: turn_id becomes batch_id");
        assert_eq!(folded.kind.as_deref(), Some(super::KIND));
        assert_eq!((folded.attempts, folded.last_error.as_deref()), (2, Some("boom")));
        let scheduled = *state == "scheduled";
        assert_eq!(folded.has_ask_picks, scheduled, "{state}");
        assert_eq!(folded.sealed_body.is_some(), scheduled, "{state}");
        assert_eq!(folded.delivered_as.as_deref(), (*state == "sent").then_some("reply"));
    }
    let turns: Vec<Uuid> =
        sqlx::query_scalar("SELECT turn_id FROM scheduled_turns WHERE session_id = $1")
            .bind(&sid)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    let sent: Vec<Uuid> =
        messages.iter().filter(|(s, ..)| *s == "sent").map(|(.., (_, turn))| *turn).collect();
    assert_eq!(turns.len(), 2, "every sent turn keeps its marker, however old");
    assert!(sent.iter().all(|t| turns.contains(t)));

    for (draft, state) in drafts.iter().zip(["scheduled", "launching", "launched", "dead"]) {
        let folded = folded_draft(&mut tx, draft).await;
        let expected = match state {
            "scheduled" => Some("pending"),
            "launching" => Some("claimed"),
            "dead" => Some("dead"),
            _ => None,
        };
        let Some(expected) = expected else {
            assert!(folded.is_none(), "a launched draft has nothing left to do");
            continue;
        };
        let folded = folded.unwrap_or_else(|| panic!("draft {state} was not folded"));
        assert_eq!(folded.status, expected, "draft {state}");
        assert_eq!(folded.source, "timer");
        assert_eq!(folded.source_id.as_deref(), Some(draft.as_str()));
        assert_eq!((folded.key, folded.lane_user), (format!("draft:{draft}"), owner));
        assert_eq!(folded.lane_session, None, "a launch deletes the draft row");
        assert_eq!(folded.kind.as_deref(), Some("draft_launch"));
        assert_eq!((folded.attempts, folded.last_error.as_deref()), (1, Some("off")));
        assert_eq!(folded.batch_id.is_some(), state == "launching");
    }

    let mut keys: Vec<String> = drafts.iter().map(|d| format!("draft:{d}")).collect();
    keys.push(key);
    let before = footprint(&mut tx, &keys, &sid).await;
    assert_eq!(before, (4, 7, 2));
    let (_, _, (pending, _)) = messages[0];
    sqlx::query("UPDATE inbox_events SET status = 'delivered', delivered_at = now() WHERE id = $1")
        .bind(pending)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(FOLD).execute(&mut *tx).await.unwrap();
    assert_eq!(footprint(&mut tx, &keys, &sid).await, before, "a second run adds nothing");
    let status = folded_message(&mut tx, pending).await.map(|f| f.status);
    assert_eq!(status.as_deref(), Some("delivered"), "a folded row moves on untouched");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_drop_removes_both_outboxes_once_every_live_row_is_in_the_inbox() {
    let name = "the_drop_removes_both_outboxes_once_every_live_row_is_in_the_inbox";
    let Some(pool) = pool(name).await else { return };
    let (owner, machine) = seed_owner(&pool).await;
    let sid = seed_session(&pool, owner, machine, "active").await;
    let draft = seed_session(&pool, owner, machine, "draft").await;
    let mut tx = pool.begin().await.unwrap();
    legacy_outboxes(&mut tx).await;
    let (delivered, turn) = old_message(&mut tx, &sid, owner, "scheduled", "0").await;
    for (state, ago) in [("dead", "0"), ("sent", "2 days"), ("cancelled", "0")] {
        old_message(&mut tx, &sid, owner, state, ago).await;
    }
    old_draft(&mut tx, &draft, owner, "scheduled").await;
    sqlx::raw_sql(FOLD).execute(&mut *tx).await.unwrap();

    sqlx::query(
        "INSERT INTO scheduled_turns (turn_id, session_id, event_id, scheduled_at) \
         VALUES ($1, $2, $3, now())",
    )
    .bind(turn)
    .bind(&sid)
    .bind(delivered)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("DELETE FROM inbox_events WHERE id = $1")
        .bind(delivered)
        .execute(&mut *tx)
        .await
        .unwrap();

    sqlx::raw_sql(DROP).execute(&mut *tx).await.unwrap();
    assert_eq!(outboxes_exist(&mut tx).await, (false, false));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_drop_refuses_and_keeps_both_outboxes_while_a_live_row_is_unfolded() {
    let name = "the_drop_refuses_and_keeps_both_outboxes_while_a_live_row_is_unfolded";
    let Some(pool) = pool(name).await else { return };
    let (owner, machine) = seed_owner(&pool).await;
    let sid = seed_session(&pool, owner, machine, "active").await;
    let folded = seed_session(&pool, owner, machine, "draft").await;
    let late = seed_session(&pool, owner, machine, "draft").await;
    let mut tx = pool.begin().await.unwrap();
    legacy_outboxes(&mut tx).await;
    old_message(&mut tx, &sid, owner, "scheduled", "0").await;
    old_draft(&mut tx, &folded, owner, "scheduled").await;
    sqlx::raw_sql(FOLD).execute(&mut *tx).await.unwrap();
    old_message(&mut tx, &sid, owner, "sending", "0").await;
    old_draft(&mut tx, &late, owner, "dead").await;

    sqlx::raw_sql("SAVEPOINT guard").execute(&mut *tx).await.unwrap();
    let err = sqlx::raw_sql(DROP).execute(&mut *tx).await.unwrap_err().to_string();
    assert!(err.contains("1 scheduled messages and 1 draft launches"), "{err}");
    sqlx::raw_sql("ROLLBACK TO SAVEPOINT guard").execute(&mut *tx).await.unwrap();
    assert_eq!(outboxes_exist(&mut tx).await, (true, true));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn send_now_claims_a_message_once_and_a_failed_send_keeps_its_turn_id() {
    let name = "send_now_claims_a_message_once_and_a_failed_send_keeps_its_turn_id";
    let Some(pool) = pool(name).await else { return };
    let state = AppState::for_test(pool.clone());
    let (owner, machine) = seed_owner(&pool).await;
    let sid = seed_session(&pool, owner, machine, "active").await;
    let ctx = AuthContext {
        user_id: owner,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
    };
    let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (code, Json(resp)) =
        crate::routes::scheduled_messages::schedule(&state, &ctx, &sid, "ping", &at).await.unwrap();
    assert_eq!((code, resp.status.as_str()), (StatusCode::ACCEPTED, "scheduled"));
    let id = resp.command_id;

    let (a, b) = tokio::join!(claim_one(&pool, &sid, id), claim_one(&pool, &sid, id));
    let claims: Vec<super::ClaimedRow> = a.unwrap().into_iter().chain(b.unwrap()).collect();
    assert_eq!(claims.len(), 1, "one send-now wins");
    let turn_id = claims[0].turn_id;
    assert_eq!(queue_state(&pool, id).await.0, "sending");
    assert!(claim_one(&pool, &sid, id).await.unwrap().is_none(), "leased");

    let failed = settle(&pool, turn_id, 0, Outcome::RetryKept("no daemon".into())).await.unwrap();
    assert_eq!(failed, Err("no daemon".to_owned()));
    let (st, attempts, err, batch): (String, i32, Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT status, attempts, last_error, batch_id FROM inbox_events WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((st.as_str(), attempts, err.as_deref()), ("pending", 1, Some("no daemon")));
    assert_eq!(batch, Some(turn_id), "a retried message keeps its turn");

    let again = claim_one(&pool, &sid, id).await.unwrap().expect("claimable again");
    assert_eq!(again.turn_id, turn_id);
    let dead = settle(&pool, turn_id, MAX_ATTEMPTS - 1, Outcome::RetryKept("offline".into()))
        .await
        .unwrap();
    assert!(dead.is_err());
    assert_eq!(queue_state(&pool, id).await.0, "dead");

    let Json(listed) =
        crate::routes::scheduled_messages::list(State(state.clone()), Path(sid.clone()))
            .await
            .unwrap();
    let shown = listed.iter().find(|m| m.id == id).expect("dead messages stay listed");
    assert_eq!(
        (shown.state.as_str(), shown.turn_id, shown.origin.as_str()),
        ("dead", turn_id, "human")
    );
    assert_eq!(shown.body, "ping");

    let cancelled =
        crate::routes::scheduled_messages::cancel(State(state.clone()), Path((sid.clone(), id)))
            .await
            .unwrap();
    assert_eq!(cancelled, StatusCode::NO_CONTENT);
    let Json(listed) =
        crate::routes::scheduled_messages::list(State(state), Path(sid)).await.unwrap();
    assert!(listed.iter().all(|m| m.id != id), "a cancelled message leaves the list");
}
