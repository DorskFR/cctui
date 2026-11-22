use cctui_proto::api::presets::{OnStale, SpawnTemplate};
use cctui_proto::injected::PREVIOUS_CONVERSATION;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::claim::{ClaimedLane, LaneFilter, claim_lanes, release_lane};
use super::preset::{
    Current, Plan, Preset, Prior, Resolved, dispatch_request, instruction, lane, opened, plan,
    prior_from, resolve, reuse_window_secs, run_env, spawn_prompt, spawn_request,
};
use super::store::{NewEvent, insert_event};
use crate::followup::FollowupPrompt;
use crate::presets::store_tests::{Owner, seed_owner};

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
    let prompt = instruction(&p, "[cctui inbox: 1 event]");
    let none = crate::sealed_env::EnvMap::new();
    let req = dispatch_request(&p, "kube".into(), batch, &prompt, &none);
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
    let plain = spawn_request(&p, &Plan::Spawn, "go").unwrap();
    assert_eq!(plain.machine_id, Uuid::nil().to_string());
    assert_eq!(plain.working_dir, "/w");
    assert_eq!(plain.adapter_id.as_deref(), Some("claude-code"));
    assert_eq!(plain.prompt.as_deref(), Some("go"));
    assert_eq!(plain.name.as_deref(), Some("triage"));
    assert!(plain.relation.is_none() && plain.parent_session_id.is_none());
    assert!(plain.env.is_empty());
    let followup = spawn_request(&p, &Plan::Followup { parent: "s-1".into() }, "go").unwrap();
    assert_eq!(followup.relation.as_deref(), Some(crate::followup::RELATION));
    assert_eq!(followup.parent_session_id.as_deref(), Some("s-1"));
    assert!(spawn_request(&preset(false), &Plan::Spawn, "go").is_none());
}

#[test]
fn a_machine_spawn_carries_the_account_pick_and_elects_one_when_none_is_set() {
    let auto = spawn_request(&preset(true), &Plan::Spawn, "go").unwrap();
    assert!(auto.auto_account && !auto.no_account);
    assert!(auto.account.is_none() && auto.pool.is_none());
    let named = Preset { account_name: Some("personal".into()), ..preset(true) };
    let named = spawn_request(&named, &Plan::Spawn, "go").unwrap();
    assert_eq!(named.account.as_deref(), Some("personal"));
    assert!(!named.auto_account);
    let pooled = Preset { pool_name: Some("bots".into()), ..preset(true) };
    let pooled = spawn_request(&pooled, &Plan::Spawn, "go").unwrap();
    assert_eq!(pooled.pool.as_deref(), Some("bots"));
    assert!(!pooled.auto_account);
    let unbound = Preset { no_account: true, ..preset(true) };
    let unbound = spawn_request(&unbound, &Plan::Spawn, "go").unwrap();
    assert!(unbound.no_account);
}

#[test]
fn a_machine_spawn_carries_the_presets_template() {
    let profile = Uuid::from_u128(5);
    let template = SpawnTemplate {
        effort: Some("high".into()),
        service_tier: Some("fast".into()),
        label_ids: vec!["l1".into()],
        context: Some(cctui_proto::api::SpawnContext { items: vec!["notes".into()], auto: true }),
        profile_id: Some(profile),
        auto_archive: true,
        attachment_names: vec!["brief.md".into()],
    };
    let p = Preset { template: sqlx::types::Json(template), ..preset(true) };
    let req = spawn_request(&p, &Plan::Spawn, "go").unwrap();
    assert_eq!(req.effort.as_deref(), Some("high"));
    assert_eq!(req.service_tier.as_deref(), Some("fast"));
    assert_eq!(req.label_ids, ["l1"]);
    let context = req.context.as_ref().map(|c| (c.items.as_slice(), c.auto));
    assert_eq!(context, Some((&["notes".to_owned()][..], true)));
    assert_eq!(req.profile_id, Some(profile));
    assert!(req.auto_archive);
    assert!(req.attachment_names.is_empty(), "files ride as uploads, not names");
    let blank = spawn_request(&preset(true), &Plan::Spawn, "go").unwrap();
    assert!(blank.effort.is_none() && blank.label_ids.is_empty() && !blank.auto_archive);
}

#[test]
fn a_run_opens_the_presets_sealed_prompt() {
    let key = [3u8; 32];
    let real = "log in with <redacted>hunter2-xyz</redacted>";
    let stored = Preset {
        prompt: "log in with [REDACTED:user]".into(),
        sealed_prompt: Some(crate::crypto::encrypt(real, &key)),
        ..preset(true)
    };
    let open = opened(stored.clone(), || key.to_vec()).unwrap();
    assert_eq!(open.prompt, real);
    assert!(opened(stored, || vec![4u8; 32]).unwrap_err().contains("vault key"));
    let plain = opened(preset(true), || panic!("nothing sealed")).unwrap();
    assert_eq!(plain.prompt, "triage the queue\n");
}

fn sealed(pairs: &[(&str, &str)], key: &[u8]) -> Option<String> {
    let env: crate::sealed_env::EnvMap =
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    crate::sealed_env::seal(&env, || key.to_vec()).unwrap()
}

#[test]
fn a_run_opens_the_presets_sealed_env_for_the_spawn_and_the_dispatch() {
    let key = [3u8; 32];
    let machine = Preset { sealed_env: sealed(&[("FOO", "bar-value")], &key), ..preset(true) };
    let env = run_env(&machine, || key.to_vec()).unwrap();
    assert_eq!(env.get("FOO").map(String::as_str), Some("bar-value"));
    assert!(run_env(&preset(true), || panic!("nothing sealed")).unwrap().is_empty());
    assert!(run_env(&machine, || vec![4u8; 32]).unwrap_err().contains("re-enter"));

    let container = Preset { sealed_env: sealed(&[("FEED", "f-value")], &key), ..preset(false) };
    let env = run_env(&container, || key.to_vec()).unwrap();
    let req = dispatch_request(&container, "kube".into(), Uuid::new_v4(), "go", &env);
    assert_eq!(req.payload["env"], json!({"FEED": "f-value"}));
    assert_eq!(req.payload["profile"], "tonbo");
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
    let out = spawn_prompt(&pool, &followup, ask.clone()).await.unwrap();
    let prompt = &out.prompt;
    assert!(prompt.starts_with(&format!("<{PREVIOUS_CONVERSATION} ")), "{prompt}");
    assert!(prompt.contains("fix the flaky test"), "{prompt}");
    assert!(prompt.ends_with(&ask), "{prompt}");
    assert_eq!(out.file, None);
    let plain = FollowupPrompt { prompt: ask.clone(), file: None };
    let orphan = Plan::Followup { parent: "no-such-session".into() };
    assert_eq!(spawn_prompt(&pool, &orphan, ask.clone()).await.unwrap(), plain);
    assert_eq!(spawn_prompt(&pool, &Plan::Spawn, ask.clone()).await.unwrap(), plain);
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

#[tokio::test]
async fn a_session_follow_up_keeps_the_account_its_parent_ran_on() {
    let Some(pool) = pool("a_session_follow_up_keeps_the_account").await else { return };
    let (owner, sid, _) = archived_session_lane(&pool, "follow-account").await;
    let unbound = super::preset::from_session(&pool, owner.user, &sid).await.unwrap().unwrap();
    assert_eq!((unbound.account_name, unbound.pool_name, unbound.no_account), (None, None, false));

    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (user_id, name) VALUES ($1, 'personal') RETURNING id",
    )
    .bind(owner.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let credential: Uuid = sqlx::query_scalar(
        "INSERT INTO account_providers (user_id, provider, account_id) \
         VALUES ($1, 'anthropic', $2) RETURNING id",
    )
    .bind(owner.user)
    .bind(account)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(format!("t-{sid}"))
    .bind(&sid)
    .bind(credential)
    .execute(&pool)
    .await
    .unwrap();
    let bound = super::preset::from_session(&pool, owner.user, &sid).await.unwrap().unwrap();
    assert_eq!(bound.account_name.as_deref(), Some("personal"));
    let req = spawn_request(&bound, &Plan::Followup { parent: sid.clone() }, "go").unwrap();
    assert_eq!((req.account.as_deref(), req.auto_account), (Some("personal"), false));
}
