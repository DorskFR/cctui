//! Replays `fixtures/parity/domainTables.json` against the derivations here.
//!
//! The webui ships the same tables as `TypeScript` constants and replays the same
//! file against them, so both sides stay pure — no round-trip needed to know a
//! tone or a static model list — and a changed rule fails on both sides until
//! both agree.

use cctui_proto::adapter::PermissionMode;
use cctui_proto::domain_meta::DomainMeta;
use cctui_proto::harness_models::harness_models;
use serde_json::Value;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/parity/domainTables.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

#[test]
fn end_reason_table_matches_the_fixture() {
    let fx = fixture();
    let meta = DomainMeta::new(vec![]);
    assert_eq!(serde_json::to_value(&meta.end_reasons).unwrap(), fx["end_reasons"]);
}

#[test]
fn provider_table_matches_the_fixture() {
    let fx = fixture();
    let meta = DomainMeta::new(vec![]);
    assert_eq!(serde_json::to_value(&meta.providers).unwrap(), fx["providers"]);
}

#[test]
fn permission_mode_order_matches_the_fixture() {
    let fx = fixture();
    assert_eq!(serde_json::to_value(PermissionMode::ALL).unwrap(), fx["permission_modes"]);
}

#[test]
fn harness_table_matches_the_fixture() {
    let fx = fixture();
    assert_eq!(serde_json::to_value(cctui_proto::adapter::harnesses()).unwrap(), fx["harnesses"]);
}

#[test]
fn static_harness_model_lists_match_the_fixture() {
    let fx = fixture();
    let listed = fx["harness_models"].as_array().expect("harness_models is a list");
    assert!(!listed.is_empty());
    for case in listed {
        let harness = case["harness"].as_str().expect("harness is a string");
        let derived = harness_models(harness, None, "");
        assert_eq!(serde_json::to_value(&derived).unwrap(), *case, "harness {harness}");
    }
}
