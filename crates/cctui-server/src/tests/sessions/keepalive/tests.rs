use chrono::Utc;

use super::tick_prompt;

fn state_with(features: cctui_proto::features::FeatureSet) -> crate::state::AppState {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    let mut state = crate::state::AppState::for_test(pool);
    state.config.features = features;
    state
}

fn tick_payload() -> serde_json::Value {
    serde_json::json!({ "role": "user", "text": tick_prompt(Utc::now()) })
}

#[tokio::test]
async fn automation_on_observes_ingested_ticks() {
    let state = state_with(cctui_proto::features::FeatureSet::all());
    let mut payload = tick_payload();
    super::observe_message(&state, "s1", &mut payload).await;
    assert_eq!(payload["metadata"]["keepalive"], true);
}

#[tokio::test]
async fn automation_off_leaves_ingested_messages_untouched() {
    use cctui_proto::features::{Feature, FeatureSet};
    let state = state_with(FeatureSet::all().without(Feature::Automation));
    let mut tick = tick_payload();
    let before = tick.clone();
    super::observe_message(&state, "s1", &mut tick).await;
    assert_eq!(tick, before);
    let mut human = serde_json::json!({ "role": "user", "text": "hello" });
    super::observe_message(&state, "s1", &mut human).await;
    assert_eq!(human, serde_json::json!({ "role": "user", "text": "hello" }));
}
