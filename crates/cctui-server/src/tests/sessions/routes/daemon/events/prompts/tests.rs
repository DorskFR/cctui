use super::*;

fn state_with(features: cctui_proto::features::FeatureSet) -> AppState {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    let mut state = AppState::for_test(pool);
    state.config.features = features;
    state
}

fn request(local_id: &str) -> AdapterEvent {
    AdapterEvent::PermissionRequest {
        local_id: local_id.to_owned(),
        request_id: "r1".to_owned(),
        tool: "Bash".to_owned(),
        input: serde_json::Value::Null,
        options: Vec::new(),
    }
}

#[tokio::test]
async fn auto_approve_feature_on_reads_the_session_toggle() {
    let state = state_with(cctui_proto::features::FeatureSet::all());
    state.permission_store.write().await.set_auto_approve("s1", true);
    assert!(auto_approve_on(&state, "s1").await);
    assert!(!auto_approve_on(&state, "s2").await);
}

#[tokio::test]
async fn auto_approve_feature_off_parks_the_request_for_a_human() {
    let state = state_with(cctui_proto::features::FeatureSet::all().without(Feature::AutoApprove));
    state.permission_store.write().await.set_auto_approve("s1", true);
    assert!(!auto_approve_on(&state, "s1").await);

    on_permission_request(&state, &Bumps::default(), request("s1")).await;
    let pending = state.permission_store.read().await.list_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request_id, "r1");
}
