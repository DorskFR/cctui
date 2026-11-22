use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::auto_followup::{
    AutoFollowupOverride, AutoFollowupSetting, AutoFollowupThreshold, SendFollowup, ThresholdUnit,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{Hold, claim_intent, fires, measure, prefs_for, put_override, remember_intent, route};
use crate::auth::AuthContext;
use crate::state::AppState;

const fn tokens(value: u64) -> AutoFollowupThreshold {
    AutoFollowupThreshold { unit: ThresholdUnit::Tokens, value }
}

const fn percent(value: u64) -> AutoFollowupThreshold {
    AutoFollowupThreshold { unit: ThresholdUnit::Percent, value }
}

const ON_150K: AutoFollowupSetting =
    AutoFollowupSetting { enabled: true, threshold: tokens(150_000) };

struct Fixture {
    state: AppState,
    pool: PgPool,
    user: Uuid,
    machine: Uuid,
    ctx: AuthContext,
}

async fn fixture(name: &str, setting: serde_json::Value) -> Option<Fixture> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    let owner = crate::presets::test_support::seed_owner(&pool, name).await;
    sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
        .bind(owner.user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2)")
        .bind(owner.user)
        .bind(json!({ "autoFollowup": setting }))
        .execute(&pool)
        .await
        .unwrap();
    let ctx = crate::inbox::queued::acting_as(&pool, owner.user).await.unwrap();
    Some(Fixture {
        state: AppState::for_test(pool.clone()),
        pool,
        user: owner.user,
        machine: owner.machine,
        ctx,
    })
}

fn on_150k() -> serde_json::Value {
    json!({"enabled": true, "threshold": {"unit": "tokens", "value": 150_000}})
}

async fn session_at(f: &Fixture, context: i64) -> String {
    claude_session(f, None, context).await
}

async fn claude_session(f: &Fixture, model: Option<&str>, context: i64) -> String {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id, agent_state, activity, model) \
         VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code', 'idle', 'success', $5)",
    )
    .bind(&id)
    .bind(f.machine.to_string())
    .bind(f.machine)
    .bind(f.user)
    .bind(model)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO session_token_usage (session_id, message_id, input_tokens, output_tokens, \
         cache_read_tokens, cache_creation_tokens) VALUES ($1, $2, $3, 10, $4, 0)",
    )
    .bind(&id)
    .bind(format!("{id}-m1"))
    .bind(context / 10)
    .bind(context - context / 10)
    .execute(&f.pool)
    .await
    .unwrap();
    id
}

async fn ops_of(pool: &PgPool, source: &str) -> Vec<(String, String, bool, String)> {
    sqlx::query_as(
        "SELECT reason, instruction, archive_source, idempotency_key \
         FROM session_followup_ops WHERE source_session_id = $1",
    )
    .bind(source)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn cleanup(f: &Fixture) {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(f.user).execute(&f.pool).await.ok();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(f.user).execute(&f.pool).await.ok();
}

#[tokio::test]
async fn send_here_opts_out() {
    assert!(!fires(SendFollowup::Never, "go on", Some(200_000), Some(150_000)));
    let Some(f) = fixture("auto_followup_send_here", on_150k()).await else { return };
    let sid = session_at(&f, 160_000).await;

    let routed = route(&f.state, Some(&f.ctx), &sid, SendFollowup::Never, "here", None).await;

    assert_eq!(routed.unwrap(), None);
    assert!(ops_of(&f.pool, &sid).await.is_empty());
    cleanup(&f).await;
}

async fn nest_under(f: &Fixture, child: &str, parent: &str, metadata: serde_json::Value) {
    sqlx::query("UPDATE sessions SET parent_id = $2, metadata = $3 WHERE id = $1")
        .bind(child)
        .bind(parent)
        .bind(metadata)
        .execute(&f.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_cctuiagent_child_never_follows_up_but_a_followup_child_does() {
    let Some(f) = fixture("auto_followup_child_exclusion", on_150k()).await else { return };
    let parent = session_at(&f, 10).await;
    let agent_child = session_at(&f, 160_000).await;
    let subagent = session_at(&f, 160_000).await;
    let continued = session_at(&f, 160_000).await;
    nest_under(&f, &agent_child, &parent, json!({})).await;
    nest_under(&f, &subagent, &parent, json!({"relation": "subagent"})).await;
    nest_under(&f, &continued, &parent, json!({"relation": "followup"})).await;

    for child in [&agent_child, &subagent] {
        let measured = measure(&f.pool, child).await.unwrap().unwrap();
        assert_eq!((measured.held, measured.at), (Some(Hold::Child), None), "{child}");
        let routed = route(&f.state, Some(&f.ctx), child, SendFollowup::Auto, "go", None).await;
        assert_eq!(routed.unwrap(), None);
        assert!(ops_of(&f.pool, child).await.is_empty());
    }
    let measured = measure(&f.pool, &continued).await.unwrap().unwrap();
    assert_eq!((measured.held, measured.at), (None, Some(150_000)), "a follow-up is no child");

    let ids = [agent_child.clone(), continued.clone()];
    let prefs = prefs_for(&f.pool, &ids).await.unwrap();
    assert!(prefs[&agent_child].child && !prefs[&continued].child, "the list sees the same");
    cleanup(&f).await;
}

#[tokio::test]
async fn threshold_150k_at_160k_routes_the_send_through_an_auto_followup() {
    let Some(f) = fixture("auto_followup_routes_the_send", on_150k()).await else { return };
    let sid = session_at(&f, 160_000).await;
    let _daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));
    let turn = Uuid::new_v4();

    let (code, Json(res)) = crate::routes::sessions::send_message(
        State(f.state.clone()),
        Extension(f.ctx.clone()),
        Path(sid.clone()),
        Json(serde_json::from_value(json!({"content": "carry on", "turn_id": turn})).unwrap()),
    )
    .await
    .unwrap();

    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(res.status, "followup");
    assert!(res.session_id.is_some(), "claude pre-mints the follow-up's id");
    assert_eq!(
        ops_of(&f.pool, &sid).await,
        [("auto".to_owned(), "carry on".to_owned(), true, format!("auto:{turn}"))]
    );
    cleanup(&f).await;
}

#[tokio::test]
async fn under_the_threshold_or_with_the_setting_off_nothing_starts() {
    let Some(f) = fixture("auto_followup_plain_send", on_150k()).await else { return };
    let under = session_at(&f, 140_000).await;
    let routed = route(&f.state, Some(&f.ctx), &under, SendFollowup::Auto, "hi", None).await;
    assert_eq!(routed.unwrap(), None);
    sqlx::query("UPDATE user_settings SET data = '{}'::jsonb WHERE user_id = $1")
        .bind(f.user)
        .execute(&f.pool)
        .await
        .unwrap();
    let above = session_at(&f, 200_000).await;
    let routed = route(&f.state, Some(&f.ctx), &above, SendFollowup::Auto, "hi", None).await;
    assert_eq!(routed.unwrap(), None);
    assert!(ops_of(&f.pool, &under).await.is_empty() && ops_of(&f.pool, &above).await.is_empty());
    cleanup(&f).await;
}

fn scheduled_row(f: &Fixture, sid: &str, turn: Uuid) -> crate::scheduled_messages::ClaimedRow {
    crate::scheduled_messages::ClaimedRow {
        id: Uuid::new_v4(),
        session_id: sid.to_owned(),
        body: "nightly check".into(),
        sealed_body: None,
        ask_picks: None,
        attempts: 0,
        turn_id: turn,
        user_id: f.user,
        deliver_at: chrono::Utc::now(),
        session_status: Some("active".into()),
        session_tempo: None,
    }
}

fn inbox_event(f: &Fixture, batch: Uuid) -> crate::inbox::claim::BatchEvent {
    crate::inbox::claim::BatchEvent {
        seq: 1,
        user_id: f.user,
        source: "github".into(),
        body: "CI failed".into(),
        sealed_body: None,
        parent_session_id: None,
        attempts: 0,
        batch_id: batch,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn a_scheduled_message_and_an_inbox_event_above_the_threshold_follow_up() {
    use crate::inbox::deliver::Outcome;
    let Some(f) = fixture("auto_followup_scheduled_and_inbox", on_150k()).await else { return };
    let scheduled = session_at(&f, 160_000).await;
    let inboxed = session_at(&f, 160_000).await;
    let (turn, batch) = (Uuid::new_v4(), Uuid::new_v4());

    let _daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));
    let sent =
        crate::scheduled_messages::deliver(&f.state, scheduled_row(&f, &scheduled, turn)).await;
    assert!(
        matches!(sent, Outcome::Delivered { delivered_as: "followup", .. }),
        "scheduled: {sent:?}"
    );
    let ops = ops_of(&f.pool, &scheduled).await;
    assert_eq!(ops.len(), 1);
    assert_eq!((ops[0].0.as_str(), ops[0].1.as_str()), ("auto", "nightly check"));
    assert_eq!(ops[0].3, format!("auto:{turn}"));

    let _daemon = crate::routes::fs::answer_list_dirs(&f.state, f.machine, Ok(Vec::new()));
    let replied = crate::inbox::deliver::reply(&f.state, &inboxed, &[inbox_event(&f, batch)]).await;
    assert!(
        matches!(replied, Outcome::Delivered { delivered_as: "followup", .. }),
        "inbox: {replied:?}"
    );
    let ops = ops_of(&f.pool, &inboxed).await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].0, "auto");
    assert!(ops[0].1.contains("CI failed"), "the batch is the instruction: {}", ops[0].1);
    assert_eq!(ops[0].3, format!("auto:{batch}"));
    cleanup(&f).await;
}

#[tokio::test]
async fn a_session_override_beats_the_user_setting_field_by_field() {
    let over = AutoFollowupOverride { enabled: None, threshold: Some(tokens(170_000)) };
    assert_eq!(ON_150K.with(Some(&over)).threshold, tokens(170_000));
    let Some(f) = fixture("auto_followup_override_wins", on_150k()).await else { return };
    let sid = session_at(&f, 160_000).await;
    assert_eq!(measure(&f.pool, &sid).await.unwrap().unwrap().at, Some(150_000));

    super::store_override(&f.pool, &sid, &over).await.unwrap();
    let measured = measure(&f.pool, &sid).await.unwrap().unwrap();
    assert_eq!((measured.at, measured.over), (Some(170_000), Some(over)), "enabled inherited");
    let routed = route(&f.state, Some(&f.ctx), &sid, SendFollowup::Auto, "hi", None).await;
    assert_eq!(routed.unwrap(), None, "160k is under the session's 170k");

    let off = AutoFollowupOverride { enabled: Some(false), threshold: None };
    super::store_override(&f.pool, &sid, &off).await.unwrap();
    assert_eq!(measure(&f.pool, &sid).await.unwrap().unwrap().at, None);
    cleanup(&f).await;
}

async fn override_after_registering(
    f: &Fixture,
    req: &cctui_proto::api::SpawnRequest,
) -> (String, Option<serde_json::Value>) {
    let key = Uuid::new_v4().to_string();
    remember_intent(&f.pool, &key, req, f.user).await;
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $3, $4, '/w', 'active')",
    )
    .bind(&key)
    .bind(f.machine.to_string())
    .bind(f.machine)
    .bind(f.user)
    .execute(&f.pool)
    .await
    .unwrap();
    claim_intent(&f.pool, &key, Some(&key)).await;
    let stored: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT metadata->'auto_followup' FROM sessions WHERE id = $1")
            .bind(&key)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    (key, stored)
}

#[tokio::test]
async fn a_profile_spawn_copies_the_override_and_a_followup_inherits_it() {
    let Some(f) = fixture("auto_followup_profile_spawn", on_150k()).await else { return };
    let early = json!({"enabled": true, "threshold": {"unit": "percent", "value": 40}});
    let profile: Uuid = sqlx::query_scalar(
        "INSERT INTO session_profiles (user_id, name, harness, auto_followup) \
         VALUES ($1, 'early', 'claude-code', $2) RETURNING id",
    )
    .bind(f.user)
    .bind(&early)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let spawn = |extra: serde_json::Value| -> cctui_proto::api::SpawnRequest {
        let mut req = json!({"machine_id": f.machine.to_string(), "working_dir": "/w"});
        req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(req).unwrap()
    };

    let (from_profile, stored) = override_after_registering(
        &f,
        &spawn(json!({
            "profile_id": profile,
        })),
    )
    .await;
    assert_eq!(stored, Some(early.clone()), "the profile's override lands on the session");

    let (_, inherited) = override_after_registering(
        &f,
        &spawn(json!({
            "relation": "followup", "parent_session_id": from_profile,
        })),
    )
    .await;
    assert_eq!(inherited, Some(early), "a follow-up carries its source's override");

    let (_, plain) = override_after_registering(&f, &spawn(json!({}))).await;
    assert_eq!(plain, None, "no profile, no override: the user setting applies");
    cleanup(&f).await;
}

#[tokio::test]
async fn put_auto_followup_sets_and_clears_the_override() {
    let Some(f) = fixture("auto_followup_put_override", on_150k()).await else { return };
    let sid = session_at(&f, 160_000).await;
    let over = AutoFollowupOverride { enabled: Some(true), threshold: Some(tokens(120_000)) };

    let Json(set) =
        put_override(State(f.state.clone()), Path(sid.clone()), Json(over)).await.unwrap();
    assert_eq!(set.auto_followup_override, Some(over));
    assert_eq!(set.auto_followup_at, Some(120_000));
    assert_eq!(set.context_tokens, Some(160_000));
    assert_eq!(set.context_window, Some(200_000));

    let clear = Json(AutoFollowupOverride::default());
    let Json(cleared) =
        put_override(State(f.state.clone()), Path(sid.clone()), clear).await.unwrap();
    assert_eq!(cleared.auto_followup_override, None);
    assert_eq!(cleared.auto_followup_at, Some(150_000), "back on the user setting");

    let bad = AutoFollowupOverride { enabled: None, threshold: Some(percent(0)) };
    let refused = put_override(State(f.state.clone()), Path(sid.clone()), Json(bad)).await;
    assert_eq!(refused.unwrap_err().status(), StatusCode::BAD_REQUEST);
    let missing = put_override(State(f.state.clone()), Path("nope".into()), Json(over)).await;
    assert_eq!(missing.unwrap_err().status(), StatusCode::NOT_FOUND);
    cleanup(&f).await;
}

#[tokio::test]
async fn a_1m_spawn_hint_survives_registration() {
    let Some(f) = fixture("auto_followup_1m_hint", on_150k()).await else { return };
    let sid = claude_session(&f, Some("claude-opus-5-5"), 50_000).await;
    let launch = cctui_server_kernel::commands::FailedSpawnRow {
        session_id: sid.clone(),
        machine_id: f.machine,
        user_id: f.user,
        adapter_id: "claude-code".into(),
        working_dir: "/w".into(),
        name: None,
        model: Some("claude-opus-5-5[1m]".into()),
        effort: None,
    };
    let command = Uuid::new_v4();
    cctui_server_core::machine_launches::prepare(&f.pool, command, f.user, &launch).await.unwrap();
    cctui_server_core::machine_launches::registered(&f.pool, &sid, &sid, f.machine, f.user)
        .await
        .unwrap();

    let measured = measure(&f.pool, &sid).await.unwrap().unwrap();
    assert_eq!(measured.context_window, Some(1_000_000), "the launch model, not the reported one");
    let prefs = prefs_for(&f.pool, std::slice::from_ref(&sid)).await.unwrap();
    assert_eq!(prefs[&sid].launched_model.as_deref(), Some("claude-opus-5-5[1m]"));
    let Json(item) =
        crate::routes::sessions::get_session(State(f.state.clone()), Path(sid.clone()))
            .await
            .unwrap();
    assert_eq!(item.context_window, Some(1_000_000));

    let plain = claude_session(&f, Some("claude-opus-5-5"), 50_000).await;
    let measured = measure(&f.pool, &plain).await.unwrap().unwrap();
    assert_eq!(measured.context_window, Some(200_000), "no launch row, no evidence");
    cleanup(&f).await;
}
