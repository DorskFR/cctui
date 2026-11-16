use cctui_proto::api::presets::OnStale;
use cctui_proto::injected::PREVIOUS_CONVERSATION;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::claim::{ClaimedLane, LaneFilter, claim_lanes, release_lane};
use super::deliver::deliver_due;
use super::preset::{
    Current, DISPATCHER_GONE, MACHINE_GONE, Plan, Preset, Prior, Resolved, UNDELIVERED,
    dispatch_request, instruction, lane, plan, prior_from, resolve, reuse_window_secs,
    spawn_prompt, spawn_request,
};
use super::store::{NewEvent, insert_event};
use crate::presets::store_tests::{Owner, seed_owner};
use crate::state::AppState;

fn session(status: &str, adapter: &str, idle_secs: i64) -> Current {
    Current {
        id: "s-1".into(),
        status: status.into(),
        adapter_id: Some(adapter.into()),
        model: None,
        idle_secs,
    }
}

fn preset(machine: bool) -> Preset {
    Preset {
        user_id: Uuid::nil(),
        name: "triage".into(),
        prompt: "triage the queue\n".into(),
        machine_id: machine.then(Uuid::nil),
        working_dir: machine.then(|| "/w".to_owned()),
        adapter_id: machine.then(|| "claude-code".to_owned()),
        model: None,
        permission_mode: machine.then(|| "ask".to_owned()),
        profile: (!machine).then(|| "tonbo".to_owned()),
        fresh_secs: 900,
        on_stale: "followup".into(),
        notify_url: (!machine).then(|| "https://n8n.example/hook".to_owned()),
        machine_live: machine,
        dispatcher_name: (!machine).then(|| "kube".to_owned()),
    }
}

#[test]
fn a_fresh_live_session_gets_the_reply() {
    let fresh = session("active", "claude-code", 60);
    assert_eq!(plan(900, OnStale::Followup, Some(&fresh), None), Plan::Reply("s-1".into()));
    assert_eq!(plan(900, OnStale::New, Some(&fresh), Some("p")), Plan::Reply("s-1".into()));
    let idle = session("inactive", "claude-code", 60);
    assert_eq!(plan(900, OnStale::Followup, Some(&idle), None), Plan::Reply("s-1".into()));
}

#[test]
fn a_stale_session_is_followed_up_from_the_lane_session_or_the_events_parent() {
    let stale = session("active", "claude-code", 901);
    assert_eq!(
        plan(900, OnStale::Followup, Some(&stale), None),
        Plan::Followup { parent: "s-1".into() }
    );
    assert_eq!(
        plan(900, OnStale::Followup, Some(&stale), Some("asker")),
        Plan::Followup { parent: "asker".into() }
    );
    for status in ["ended", "archived", "failed"] {
        let gone = session(status, "claude-code", 0);
        assert_eq!(
            plan(900, OnStale::Followup, Some(&gone), None),
            Plan::Followup { parent: "s-1".into() },
            "{status}"
        );
    }
}

#[test]
fn stale_with_on_stale_new_or_no_parent_is_a_plain_spawn() {
    let stale = session("active", "claude-code", 5000);
    assert_eq!(plan(900, OnStale::New, Some(&stale), Some("asker")), Plan::Spawn);
    assert_eq!(plan(900, OnStale::Followup, None, None), Plan::Spawn);
    assert_eq!(plan(900, OnStale::New, None, None), Plan::Spawn);
}

#[test]
fn the_reuse_window_is_the_shorter_of_the_preset_and_the_cache() {
    assert_eq!(reuse_window_secs(900, &session("active", "claude-code", 0)), 900);
    assert_eq!(reuse_window_secs(2700, &session("active", "claude-code", 0)), 2700);
    let opencode = session("active", "opencode", 200);
    assert_eq!(reuse_window_secs(900, &opencode), 180, "5 minute TTL less the margin");
    assert_eq!(plan(900, OnStale::New, Some(&opencode), None), Plan::Spawn);
}

#[test]
fn only_a_launch_that_never_reached_the_daemon_is_sent_again() {
    for sent in ["prepared", "delivered", "accepted", "registered"] {
        assert_eq!(
            prior_from(sent, None, "pre".into(), false),
            Prior::Launched("pre".into()),
            "{sent}: a timeout after delivery is not retried"
        );
    }
    assert_eq!(prior_from("delivery_failed", None, "pre".into(), false), Prior::Undelivered);
    assert_eq!(
        prior_from("delivery_failed", None, "pre".into(), true),
        Prior::Launched("pre".into()),
        "a worker that registered anyway counts as launched"
    );
    assert_eq!(
        prior_from("refused", Some("bad cwd".into()), "pre".into(), false),
        Prior::Refused("bad cwd".into())
    );
}

#[test]
fn a_container_batch_dispatches_under_its_batch_id_with_no_secret() {
    let batch = Uuid::new_v4();
    let p = preset(false);
    let req = dispatch_request(&p, "kube".into(), batch, instruction(&p, "[cctui inbox: 1 event]"));
    assert_eq!(req.dispatcher, "kube");
    assert_eq!(req.session_id, Some(batch.to_string()));
    assert_eq!(req.notify_url.as_deref(), Some("https://n8n.example/hook"));
    assert!(req.notify_secret.is_none() && req.reply_url.is_none());
    assert!(req.account.is_none() && req.accounts.is_empty());
    assert_eq!(
        req.payload,
        json!({
            "profile": "tonbo",
            "prompt": "triage the queue\n\n[cctui inbox: 1 event]",
            "name": "triage",
        })
    );
}

#[test]
fn a_machine_spawn_carries_the_preset_target_and_a_followup_names_its_parent() {
    let p = preset(true);
    let plain = spawn_request(&p, &Plan::Spawn, "go".into()).unwrap();
    assert_eq!(plain.machine_id, Uuid::nil().to_string());
    assert_eq!(plain.working_dir, "/w");
    assert_eq!(plain.adapter_id.as_deref(), Some("claude-code"));
    assert_eq!(plain.prompt.as_deref(), Some("go"));
    assert_eq!(plain.name.as_deref(), Some("triage"));
    assert!(plain.relation.is_none() && plain.parent_session_id.is_none());
    assert!(plain.env.is_empty());
    let followup =
        spawn_request(&p, &Plan::Followup { parent: "s-1".into() }, "go".into()).unwrap();
    assert_eq!(followup.relation.as_deref(), Some(crate::followup::RELATION));
    assert_eq!(followup.parent_session_id.as_deref(), Some("s-1"));
    assert!(spawn_request(&preset(false), &Plan::Spawn, "go".into()).is_none());
}

async fn pool(name: &str) -> Option<PgPool> {
    let url = crate::routes::gateway::test_db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn grant_dispatch(pool: &PgPool, user: Uuid) {
    sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
}

async fn machine_preset(pool: &PgPool, owner: &Owner) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id) \
         VALUES ($1, $2, 'triage', $3, '/w', 'claude-code') RETURNING id",
    )
    .bind(owner.user)
    .bind(format!("m-{}", Uuid::new_v4()))
    .bind(owner.machine)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn container_preset(pool: &PgPool, owner: &Owner) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO presets (user_id, name, prompt, dispatcher_id, profile) \
         VALUES ($1, $2, 'triage', $3, 'tonbo') RETURNING id",
    )
    .bind(owner.user)
    .bind(format!("c-{}", Uuid::new_v4()))
    .bind(owner.dispatcher)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn add_event(pool: &PgPool, lane_id: Uuid, user: Uuid) -> Uuid {
    insert_event(
        pool,
        &NewEvent {
            lane_id,
            user_id: user,
            source: Source::Schedule,
            source_id: Some(&Uuid::new_v4().to_string()),
            body: "nightly run",
            payload: None,
            not_before: None,
            expires_at: None,
        },
    )
    .await
    .unwrap()
    .id
}

type EventRow = (String, i32, Option<Uuid>, Option<String>, Option<String>, Option<String>);

async fn event_row(pool: &PgPool, id: Uuid) -> EventRow {
    sqlx::query_as(
        "SELECT status, attempts, batch_id, last_error, delivered_as, delivered_session_id \
         FROM inbox_events WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn launches(pool: &PgPool, machine: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT command_id, state FROM machine_launches WHERE machine_id = $1")
        .bind(machine)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn record_launch(pool: &PgPool, owner: &Owner, command: Uuid, state: &str) -> String {
    let session_id = format!("pre-{command}");
    sqlx::query(
        "INSERT INTO machine_launches (command_id, session_id, machine_id, user_id, \
             acting_user_id, adapter_id, working_dir, state) \
         VALUES ($1, $2, $3, $4, $4, 'claude-code', '/w', $5)",
    )
    .bind(command)
    .bind(&session_id)
    .bind(owner.machine)
    .bind(owner.user)
    .bind(state)
    .execute(pool)
    .await
    .unwrap();
    session_id
}

async fn strand(pool: &PgPool, event: Uuid, batch: Uuid) {
    sqlx::query("UPDATE inbox_events SET batch_id = $2 WHERE id = $1")
        .bind(event)
        .bind(batch)
        .execute(pool)
        .await
        .unwrap();
}

const fn only(lane_id: Uuid) -> LaneFilter {
    LaneFilter { lane: Some(lane_id), machine: None }
}

#[tokio::test]
async fn a_preset_lane_without_a_session_is_claimable() {
    let Some(pool) = pool("inbox_preset_claimable").await else { return };
    let owner = seed_owner(&pool, "preset-claim").await;
    let preset_id = machine_preset(&pool, &owner).await;
    let lane_id = lane(&pool, owner.user, preset_id, "ci").await.unwrap();
    assert_eq!(lane(&pool, owner.user, preset_id, "ci").await.unwrap(), lane_id, "one per topic");
    add_event(&pool, lane_id, owner.user).await;
    let claimed = claim_lanes(&pool, 10, only(lane_id)).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].preset_id, Some(preset_id));
    assert_eq!((claimed[0].user_id, claimed[0].session_id.clone()), (owner.user, None));
    assert_eq!(claimed[0].key, format!("preset:{preset_id}:ci"));
    release_lane(&pool, lane_id).await.unwrap();
}

#[tokio::test]
async fn a_delivery_error_retries_under_a_fresh_batch_and_the_launch_is_keyed_by_the_batch() {
    let Some(pool) = pool("inbox_preset_delivery_error").await else { return };
    let owner = seed_owner(&pool, "preset-undelivered").await;
    grant_dispatch(&pool, owner.user).await;
    let preset_id = machine_preset(&pool, &owner).await;
    let lane_id = lane(&pool, owner.user, preset_id, "ci").await.unwrap();
    let event = add_event(&pool, lane_id, owner.user).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    assert_eq!(launches(&pool, owner.machine).await, [(batch, "delivery_failed".to_owned())]);
    let (status, attempts, batch_after, error, ..) = event_row(&pool, event).await;
    assert_eq!((status.as_str(), attempts, batch_after), ("pending", 1, None));
    assert!(error.is_some(), "the delivery error is kept");
}

#[tokio::test]
async fn a_launch_already_recorded_for_the_batch_is_not_sent_again() {
    let Some(pool) = pool("inbox_preset_already_launched").await else { return };
    let owner = seed_owner(&pool, "preset-launched").await;
    grant_dispatch(&pool, owner.user).await;
    let preset_id = machine_preset(&pool, &owner).await;
    let lane_id = lane(&pool, owner.user, preset_id, "ci").await.unwrap();
    let event = add_event(&pool, lane_id, owner.user).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;
    let session_id = record_launch(&pool, &owner, batch, "delivered").await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    assert_eq!(launches(&pool, owner.machine).await.len(), 1, "no second launch");
    let (status, _, _, _, delivered_as, delivered_to) = event_row(&pool, event).await;
    assert_eq!(status, "delivered");
    assert_eq!(delivered_as.as_deref(), Some("spawn"));
    assert_eq!(delivered_to, Some(session_id));
    let followed: Option<Uuid> =
        sqlx::query_scalar("SELECT spawn_command_id FROM inbox_lanes WHERE id = $1")
            .bind(lane_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(followed, Some(batch), "the lane follows the spawn it launched");
}

#[tokio::test]
async fn an_earlier_undelivered_attempt_dissolves_the_batch_without_sending() {
    let Some(pool) = pool("inbox_preset_undelivered_prior").await else { return };
    let owner = seed_owner(&pool, "preset-prior").await;
    grant_dispatch(&pool, owner.user).await;
    let preset_id = machine_preset(&pool, &owner).await;
    let lane_id = lane(&pool, owner.user, preset_id, "ci").await.unwrap();
    let event = add_event(&pool, lane_id, owner.user).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;
    record_launch(&pool, &owner, batch, "delivery_failed").await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    assert_eq!(launches(&pool, owner.machine).await.len(), 1, "nothing sent this time");
    let (status, attempts, batch_after, error, ..) = event_row(&pool, event).await;
    assert_eq!((status.as_str(), attempts, batch_after), ("pending", 1, None));
    assert_eq!(error.as_deref(), Some(UNDELIVERED));
}

#[tokio::test]
async fn a_preset_whose_machine_or_dispatcher_is_gone_dead_letters() {
    let Some(pool) = pool("inbox_preset_target_gone").await else { return };
    let owner = seed_owner(&pool, "preset-gone").await;
    grant_dispatch(&pool, owner.user).await;
    let machine_lane =
        lane(&pool, owner.user, machine_preset(&pool, &owner).await, "a").await.unwrap();
    let container_lane =
        lane(&pool, owner.user, container_preset(&pool, &owner).await, "b").await.unwrap();
    let on_machine = add_event(&pool, machine_lane, owner.user).await;
    let on_container = add_event(&pool, container_lane, owner.user).await;
    sqlx::query("UPDATE machines SET revoked_at = now() WHERE id = $1")
        .bind(owner.machine)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE dispatchers SET deleted_at = now() WHERE id = $1")
        .bind(owner.dispatcher)
        .execute(&pool)
        .await
        .unwrap();

    let state = AppState::for_test(pool.clone());
    deliver_due(&state, 1, only(machine_lane)).await;
    deliver_due(&state, 1, only(container_lane)).await;

    let (status, _, _, error, ..) = event_row(&pool, on_machine).await;
    assert_eq!((status.as_str(), error.as_deref()), ("dead", Some(MACHINE_GONE)));
    let (status, _, _, error, ..) = event_row(&pool, on_container).await;
    assert_eq!((status.as_str(), error.as_deref()), ("dead", Some(DISPATCHER_GONE)));
    assert!(launches(&pool, owner.machine).await.is_empty());
}

fn spawning_lane(lane_id: Uuid, user: Uuid, command: Uuid) -> ClaimedLane {
    ClaimedLane {
        id: lane_id,
        user_id: user,
        key: "preset".into(),
        preset_id: Some(Uuid::nil()),
        session_id: None,
        session_status: None,
        spawn_command_id: Some(command),
    }
}

#[tokio::test]
async fn an_in_flight_spawn_holds_the_lane_until_it_registers_then_is_adopted() {
    let Some(pool) = pool("inbox_preset_adopt").await else { return };
    let owner = seed_owner(&pool, "preset-adopt").await;
    let preset_id = machine_preset(&pool, &owner).await;
    let lane_id = lane(&pool, owner.user, preset_id, "ci").await.unwrap();
    let command = Uuid::new_v4();
    let session_id = record_launch(&pool, &owner, command, "delivered").await;
    sqlx::query("UPDATE inbox_lanes SET spawn_command_id = $2 WHERE id = $1")
        .bind(lane_id)
        .bind(command)
        .execute(&pool)
        .await
        .unwrap();
    let spawning = spawning_lane(lane_id, owner.user, command);
    assert_eq!(resolve(&pool, &spawning).await.unwrap(), Resolved::Wait, "not registered yet");

    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
    )
    .bind(&session_id)
    .bind(owner.machine)
    .bind(owner.user)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(resolve(&pool, &spawning).await.unwrap(), Resolved::Wait, "adopted, not yet read");
    let adopted: (Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT session_id, spawn_command_id FROM inbox_lanes WHERE id = $1")
            .bind(lane_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(adopted, (Some(session_id.clone()), None));
    let reread =
        ClaimedLane { session_id: Some(session_id.clone()), spawn_command_id: None, ..spawning };
    let Resolved::Session(Some(current)) = resolve(&pool, &reread).await.unwrap() else {
        panic!("the adopted session is the lane's current one");
    };
    assert_eq!((current.id, current.status.as_str()), (session_id, "active"));
    assert!(current.idle_secs < 60);
}

#[tokio::test]
async fn a_followup_prompt_embeds_the_parents_brief_then_the_batch() {
    let Some(pool) = pool("inbox_preset_followup_prompt").await else { return };
    let (_, parent) = super::store_tests::seed_session(&pool, "preset-brief").await;
    sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
    )
    .bind(&parent)
    .bind(json!({"role": "user", "text": "fix the flaky test"}))
    .execute(&pool)
    .await
    .unwrap();
    let ask = instruction(&preset(true), "[cctui inbox: 1 event]\n1. (schedule, x) nightly");
    let followup = Plan::Followup { parent: parent.clone() };
    let prompt = spawn_prompt(&pool, &followup, ask.clone()).await.unwrap();
    assert!(prompt.starts_with(&format!("<{PREVIOUS_CONVERSATION} ")), "{prompt}");
    assert!(prompt.contains("fix the flaky test"), "{prompt}");
    assert!(prompt.ends_with(&ask), "{prompt}");
    let orphan = Plan::Followup { parent: "no-such-session".into() };
    assert_eq!(spawn_prompt(&pool, &orphan, ask.clone()).await.unwrap(), ask);
    assert_eq!(spawn_prompt(&pool, &Plan::Spawn, ask.clone()).await.unwrap(), ask);
}

async fn archived_session_lane(pool: &PgPool, name: &str) -> (Owner, String, Uuid) {
    let (user, sid) = super::store_tests::seed_session(pool, name).await;
    let machine: Uuid = sqlx::query_scalar(
        "UPDATE sessions SET status = 'archived', session_name = 'worker' WHERE id = $1 \
         RETURNING machine_uuid",
    )
    .bind(&sid)
    .fetch_one(pool)
    .await
    .unwrap();
    grant_dispatch(pool, user).await;
    let lane_id = super::store::session_lane(pool, user, &sid).await.unwrap();
    (Owner { user, machine, dispatcher: Uuid::nil() }, sid, lane_id)
}

async fn timer_event(pool: &PgPool, lane_id: Uuid, user: Uuid, parent: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO inbox_events (lane_id, user_id, source, body, parent_session_id) \
         VALUES ($1, $2, 'timer', 'check the CI run', $3) RETURNING id",
    )
    .bind(lane_id)
    .bind(user)
    .bind(parent)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn lane_follow(pool: &PgPool, lane_id: Uuid) -> (Option<String>, Option<Uuid>) {
    sqlx::query_as("SELECT session_id, spawn_command_id FROM inbox_lanes WHERE id = $1")
        .bind(lane_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_timer_for_an_archived_session_follows_it_up_under_the_batch_key() {
    let Some(pool) = pool("inbox_session_followup_spawn").await else { return };
    let (owner, sid, lane_id) = archived_session_lane(&pool, "timer-followup").await;
    let event = timer_event(&pool, lane_id, owner.user, Some(&sid)).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    assert_eq!(
        launches(&pool, owner.machine).await,
        [(batch, "delivery_failed".to_owned())],
        "a follow-up spawn was attempted under the batch id"
    );
    let (status, attempts, batch_after, error, ..) = event_row(&pool, event).await;
    assert_eq!((status.as_str(), attempts, batch_after), ("pending", 1, None), "retried, not dead");
    assert!(error.is_some());
    assert_eq!(lane_follow(&pool, lane_id).await, (Some(sid), None), "nothing launched yet");
}

#[tokio::test]
async fn a_launched_session_follow_up_is_delivered_and_the_lane_follows_it() {
    let Some(pool) = pool("inbox_session_followup_follow").await else { return };
    let (owner, sid, lane_id) = archived_session_lane(&pool, "timer-follow").await;
    let event = timer_event(&pool, lane_id, owner.user, Some(&sid)).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;
    let new_session = record_launch(&pool, &owner, batch, "delivered").await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    assert_eq!(launches(&pool, owner.machine).await.len(), 1, "not sent again");
    let (status, _, _, _, delivered_as, delivered_to) = event_row(&pool, event).await;
    assert_eq!(status, "delivered");
    assert_eq!(delivered_as.as_deref(), Some("followup"));
    assert_eq!(delivered_to, Some(new_session.clone()));
    assert_eq!(lane_follow(&pool, lane_id).await, (None, Some(batch)));

    let claimed = claim_lanes(&pool, 10, only(lane_id)).await.unwrap();
    assert!(claimed.is_empty(), "no due events left");
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
    )
    .bind(&new_session)
    .bind(owner.machine)
    .bind(owner.user)
    .execute(&pool)
    .await
    .unwrap();
    let following = spawning_lane(lane_id, owner.user, batch);
    let following = ClaimedLane { preset_id: None, ..following };
    assert_eq!(super::deliver::route(&following), super::deliver::Route::Following);
    assert_eq!(resolve(&pool, &following).await.unwrap(), Resolved::Wait);
    assert_eq!(lane_follow(&pool, lane_id).await, (Some(new_session), None), "adopted");
}

#[tokio::test]
async fn an_archived_session_lane_without_a_parent_still_dead_letters() {
    let Some(pool) = pool("inbox_session_no_parent_dead").await else { return };
    let (owner, _, lane_id) = archived_session_lane(&pool, "timer-orphan").await;
    let event = timer_event(&pool, lane_id, owner.user, None).await;

    deliver_due(&AppState::for_test(pool.clone()), 1, only(lane_id)).await;

    let (status, _, _, error, ..) = event_row(&pool, event).await;
    assert_eq!(
        (status.as_str(), error.as_deref()),
        ("dead", Some("session archived before delivery"))
    );
    assert!(launches(&pool, owner.machine).await.is_empty());
}
