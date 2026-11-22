use cctui_proto::api::presets::SpawnTemplate;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::claim::{BatchEvent, ClaimedLane, LaneFilter, claim_lanes};
use super::deliver::deliver_due;
use super::preset::{
    DISPATCHER_GONE, MACHINE_GONE, Plan, Preset, Resolved, UNDELIVERED, instruction, lane, resolve,
    run_labels, spawn_prompt,
};
use super::store::{NewEvent, insert_event};
use crate::presets::test_support::{Owner, seed_owner};
use crate::state::AppState;

fn preset(machine: bool) -> Preset {
    Preset {
        id: None,
        user_id: Uuid::nil(),
        name: "triage".into(),
        prompt: "triage the queue\n".into(),
        machine_id: machine.then(Uuid::nil),
        working_dir: machine.then(|| "/w".to_owned()),
        adapter_id: machine.then(|| "claude-code".to_owned()),
        model: None,
        permission_mode: machine.then(|| "ask".to_owned()),
        account_name: None,
        pool_name: None,
        no_account: false,
        profile: (!machine).then(|| "tonbo".to_owned()),
        fresh_secs: 900,
        on_stale: "followup".into(),
        notify_url: (!machine).then(|| "https://n8n.example/hook".to_owned()),
        machine_live: machine,
        dispatcher_name: (!machine).then(|| "kube".to_owned()),
        sealed_env: None,
        sealed_prompt: None,
        template: sqlx::types::Json(SpawnTemplate::default()),
    }
}

async fn pool(name: &str) -> Option<PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
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
async fn a_chain_followup_stages_the_same_file_as_the_webui_path() {
    use axum::extract::{Path, Query, State};
    use axum::{Extension, Json};

    let Some(pool) = pool("inbox_preset_chain_followup").await else { return };
    let (user, root) = cctui_server_kernel::test_support::seed_session(&pool, "preset-chain").await;
    let parent = format!("{root}-next");
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id, parent_id, metadata) \
         SELECT $2, machine_id, machine_uuid, user_id, working_dir, 'active', adapter_id, $1, \
         jsonb_build_object('relation', 'followup') FROM sessions WHERE id = $1",
    )
    .bind(&root)
    .bind(&parent)
    .execute(&pool)
    .await
    .unwrap();
    let ask_a = format!("the root ask {}", "a".repeat(10 * 1024));
    let mut rows = vec![(root.as_str(), json!({"role": "user", "text": &ask_a}))];
    for i in 0..150 {
        let text = format!("<root-{i:03}>");
        rows.push(if i % 2 == 0 {
            (root.as_str(), json!({"role": "assistant", "text": text, "message_id": "m"}))
        } else {
            (root.as_str(), json!({"role": "user", "text": text}))
        });
    }
    rows.push((parent.as_str(), json!({"role": "user", "text": "ask B"})));
    for (sid, payload) in rows {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ($1, 'message', $2)",
        )
        .bind(sid)
        .bind(payload)
        .execute(&pool)
        .await
        .unwrap();
    }
    let ask = instruction(&preset(true), "[cctui inbox: 1 event]\n1. (schedule, x) nightly");
    let out =
        spawn_prompt(&pool, &Plan::Followup { parent: parent.clone() }, ask.clone()).await.unwrap();

    let ctx = crate::auth::AuthContext {
        user_id: user,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::collections::BTreeSet::new(),
    };
    let q = crate::brief::BriefQuery { chain: Some("1".into()), ..Default::default() };
    let state = AppState::for_test(pool.clone());
    let Json(served) =
        crate::brief::session_brief(State(state), Extension(ctx), Path(parent.clone()), Query(q))
            .await
            .map_err(|(status, _)| status)
            .expect("chain brief");
    assert_eq!(out, crate::followup::followup_from_brief(&served, &ask));

    let file = out.file.clone().expect("a chain always ships the file");
    assert!(file.contains(&ask_a), "the root ask arrives whole");
    for i in [0, 100, 149] {
        assert_eq!(file.matches(&format!("<root-{i:03}>")).count(), 1, "root turn {i}");
    }
    assert!(out.prompt.contains(crate::brief::BRIEF_FILE_NAME));
    assert!(out.prompt.ends_with(&ask));
    let (encoded, raw) = super::preset::brief_upload(file.clone());
    assert_eq!(encoded.name, crate::brief::BRIEF_FILE_NAME);
    assert_eq!(raw.name, crate::brief::BRIEF_FILE_NAME);
    assert_eq!(&raw.bytes[..], file.as_bytes());

    let all = [parent, root];
    sqlx::query("DELETE FROM stream_events WHERE session_id = ANY($1)")
        .bind(&all[..])
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM sessions WHERE id = ANY($1)").bind(&all[..]).execute(&pool).await.ok();
}

async fn archived_session_lane(pool: &PgPool, name: &str) -> (Owner, String, Uuid) {
    let (user, sid) = cctui_server_kernel::test_support::seed_session(pool, name).await;
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

fn batch_event(user: Uuid, source: &str) -> BatchEvent {
    BatchEvent {
        seq: 1,
        user_id: user,
        source: source.into(),
        body: "run".into(),
        sealed_body: None,
        parent_session_id: None,
        attempts: 0,
        batch_id: Uuid::new_v4(),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn a_scheduled_or_webhook_run_is_labelled_with_its_preset_name() {
    let Some(pool) = pool("inbox_preset_run_label").await else { return };
    let owner = seed_owner(&pool, "preset-label").await;
    let state = AppState::for_test(pool.clone());
    let named = Preset { user_id: owner.user, name: "Nightly Triage".into(), ..preset(true) };

    let first = run_labels(&state, &named, &[batch_event(owner.user, "schedule")]).await;
    assert_eq!(first.len(), 1);
    let renamed = Preset { name: "nightly triage".into(), ..named.clone() };
    let again = run_labels(&state, &renamed, &[batch_event(owner.user, "webhook")]).await;
    assert_eq!(again, first, "one label per preset name, whatever its case");
    let labels: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT name, user_id FROM labels WHERE id::text = $1")
            .bind(&first[0])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(labels, [("Nightly Triage".to_owned(), owner.user)]);

    for source in ["n8n", "human", "timer"] {
        let none = run_labels(&state, &named, &[batch_event(owner.user, source)]).await;
        assert!(none.is_empty(), "{source}");
    }
}

#[tokio::test]
async fn a_machine_preset_with_env_fires_a_spawn_carrying_it() {
    use cctui_proto::api::presets::{EnvValues, LaunchPresetRequest};
    let Some(pool) = pool("inbox_preset_env_fire").await else { return };
    crate::crypto::install_vault_key(vec![7u8; 32]);
    let owner = seed_owner(&pool, "preset-env").await;
    grant_dispatch(&pool, owner.user).await;
    let req = LaunchPresetRequest {
        name: format!("env-{}", Uuid::new_v4()),
        prompt: "read the feed".into(),
        machine_id: Some(owner.machine),
        working_dir: Some("/w".into()),
        no_account: true,
        env: Some(EnvValues(std::collections::BTreeMap::from([(
            "MINIFLUX_TOKEN".to_owned(),
            "feed-t0ken".to_owned(),
        )]))),
        ..LaunchPresetRequest::default()
    };
    let valid = crate::presets::validate(&req).unwrap();
    let preset = crate::presets::store::create(&pool, owner.user, &valid).await.unwrap();
    assert_eq!(preset.env_keys, ["MINIFLUX_TOKEN"]);
    let lane_id = lane(&pool, owner.user, preset.id, "ci").await.unwrap();
    let event = add_event(&pool, lane_id, owner.user).await;
    let state = AppState::for_test(pool.clone());
    let daemon = crate::routes::fs::answer_list_dirs(&state, owner.machine, Ok(Vec::new()));

    deliver_due(&state, 1, only(lane_id)).await;

    let (status, _, _, error, delivered_as, _) = event_row(&pool, event).await;
    let delivered = (status.as_str(), delivered_as.as_deref());
    assert_eq!(delivered, ("delivered", Some("spawn")), "{error:?}");
    let mut rx = daemon.await.unwrap();
    let Some(cctui_proto::ws::DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let cctui_proto::adapter::AdapterCommand::Spawn { spec, .. } = *command else {
        panic!("expected a spawn")
    };
    assert_eq!(spec.env.get("MINIFLUX_TOKEN").map(String::as_str), Some("feed-t0ken"));
}

#[tokio::test]
async fn a_schedule_fires_a_spawn_with_its_effort_labels_and_files() {
    use cctui_proto::api::presets::LaunchPresetRequest;
    let Some(pool) = pool("inbox_preset_template_fire").await else { return };
    crate::crypto::install_vault_key(vec![7u8; 32]);
    let owner = seed_owner(&pool, "preset-template").await;
    grant_dispatch(&pool, owner.user).await;
    let label: Uuid = sqlx::query_scalar(
        "INSERT INTO labels (name, color, user_id) VALUES ('digest', '120', $1) RETURNING id",
    )
    .bind(owner.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let req = LaunchPresetRequest {
        name: format!("tpl-{}", Uuid::new_v4()),
        prompt: "summarise the attached brief".into(),
        machine_id: Some(owner.machine),
        working_dir: Some("/w".into()),
        no_account: true,
        template: Some(SpawnTemplate {
            effort: Some("high".into()),
            label_ids: vec![label.to_string()],
            ..SpawnTemplate::default()
        }),
        ..LaunchPresetRequest::default()
    };
    let valid = crate::presets::validate(&req).unwrap();
    let preset = crate::presets::store::create(&pool, owner.user, &valid).await.unwrap();
    let brief = crate::uploads::RawUpload {
        name: "brief.md".into(),
        bytes: b"# the brief".to_vec().into(),
        content_type: Some("text/markdown".into()),
    };
    let stored = crate::presets::store::set_files::<crate::state::AppState>(
        &pool,
        Some(owner.user),
        preset.id,
        &[brief],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stored.template.attachment_names, ["brief.md"]);
    assert_eq!(stored.template.effort.as_deref(), Some("high"));
    let lane_id = lane(&pool, owner.user, preset.id, "ci").await.unwrap();
    let event = add_event(&pool, lane_id, owner.user).await;
    let state = AppState::for_test(pool.clone());
    let daemon = crate::routes::fs::answer_list_dirs(&state, owner.machine, Ok(Vec::new()));

    deliver_due(&state, 1, only(lane_id)).await;

    let (status, _, _, error, _, _) = event_row(&pool, event).await;
    assert_eq!(status, "delivered", "{error:?}");
    let mut rx = daemon.await.unwrap();
    let Some(cctui_proto::ws::DaemonFrameDown::Command { command, .. }) = rx.recv().await else {
        panic!("expected a command frame")
    };
    let cctui_proto::adapter::AdapterCommand::Spawn { spec, session_id, .. } = *command else {
        panic!("expected a spawn")
    };
    assert_eq!(spec.effort.as_deref(), Some("high"));
    assert!(spec.bootstrap.to_string().contains("brief.md"), "{}", spec.bootstrap);
    let key = session_id.expect("a spawn key").to_string();
    let intents: Vec<Uuid> =
        sqlx::query_scalar("SELECT label_ids FROM session_label_intents WHERE spawn_key = $1")
            .bind(&key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(intents.contains(&label), "{intents:?}");
}

type OpRow = (String, String, bool, Uuid, Option<String>, String);

#[tokio::test]
async fn inbox_preset_followup_goes_through_the_op() {
    let Some(pool) = pool("inbox_preset_followup_op").await else { return };
    let (owner, sid, lane_id) = archived_session_lane(&pool, "timer-followup-op").await;
    let event = timer_event(&pool, lane_id, owner.user, Some(&sid)).await;
    let batch = Uuid::new_v4();
    strand(&pool, event, batch).await;
    let state = AppState::for_test(pool.clone());
    let daemon = crate::routes::fs::answer_list_dirs(&state, owner.machine, Ok(Vec::new()));

    deliver_due(&state, 1, only(lane_id)).await;

    let (status, _, _, error, delivered_as, delivered_to) = event_row(&pool, event).await;
    let delivered = (status.as_str(), delivered_as.as_deref());
    assert_eq!(delivered, ("delivered", Some("followup")), "{error:?}");
    let op: OpRow = sqlx::query_as(
        "SELECT reason, state, archive_source, command_id, spawn_key, instruction \
         FROM session_followup_ops WHERE source_session_id = $1 AND idempotency_key = $2",
    )
    .bind(&sid)
    .bind(batch.to_string())
    .fetch_one(&pool)
    .await
    .expect("the follow-up is recorded as an op keyed by the batch");
    let (reason, state_name, archive_source, command_id, spawn_key, instruction) = op;
    assert_eq!((reason.as_str(), state_name.as_str()), ("inbox", "pending"));
    assert!(!archive_source, "an inbox follow-up leaves its source as it was");
    assert_eq!(command_id, batch);
    assert_eq!(spawn_key, delivered_to, "the op waits for the session the spawn pre-minted");
    assert!(instruction.contains("check the CI run"), "{instruction}");
    let mut rx = daemon.await.unwrap();
    assert!(matches!(rx.recv().await, Some(cctui_proto::ws::DaemonFrameDown::Command { .. })));
    let busy = crate::followup_op::in_progress(&pool, &sid).await.unwrap();
    assert!(busy, "the pending op holds the source against a plain send");
}
