//! The automation host can select an account without linking workspace services.
use cctui_server_core::account_pick::Candidate;
use cctui_server_core::account_resolve::elect;
use cctui_server_core::gateway::Family;
use cctui_server_core::store::account_pools::STRATEGY_HEADROOM;

#[test]
fn headless_selection_preserves_provider_and_reauthentication_boundaries() {
    assert!(Family::from_adapter("unknown-harness").is_err());
    assert_eq!(Family::from_adapter("codex-app-server").unwrap(), Family::Openai);
    let candidates = [Candidate {
        name: "automation".into(),
        windows: vec![],
        limits: cctui_proto::api::soft_limit::SoftLimits::default(),
        usage_known: false,
        in_flight: 0,
        needs_reauth: true,
    }];
    assert!(elect("workers", STRATEGY_HEADROOM, &candidates, None, chrono::Utc::now()).is_err());
    let candidates = [Candidate { needs_reauth: false, ..candidates[0].clone() }];
    assert_eq!(
        elect("workers", STRATEGY_HEADROOM, &candidates, None, chrono::Utc::now()).unwrap().name,
        "automation"
    );
}
