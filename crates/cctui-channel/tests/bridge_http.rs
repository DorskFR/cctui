//! Bridge HTTP contract — port of `channel/test/bridge.test.ts` (subset).

use cctui_channel::bridge::{ArchiveState, Bridge, Decision};
use cctui_channel::types::PreToolUsePayload;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn payload() -> PreToolUsePayload {
    PreToolUsePayload {
        session_id: "s1".into(),
        tool_name: "Bash".into(),
        tool_input: serde_json::json!({"command": "ls"}),
    }
}

#[tokio::test]
async fn check_policy_parses_decision() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/check"))
        .and(header("authorization", "Bearer t"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"decision":"deny"})),
        )
        .mount(&server)
        .await;
    let b = Bridge::new(server.uri(), "t");
    let v = b.check_policy(&payload()).await;
    assert_eq!(v.decision, Decision::Deny);
}

#[tokio::test]
async fn check_policy_defaults_allow_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/check"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let b = Bridge::new(server.uri(), "t");
    let v = b.check_policy(&payload()).await;
    assert_eq!(v.decision, Decision::Allow);
}

#[tokio::test]
async fn fetch_pending_returns_empty_on_error() {
    let b = Bridge::new("http://127.0.0.1:1", "t");
    let msgs = b.fetch_pending_messages("s1").await;
    assert!(msgs.is_empty());
}

#[tokio::test]
async fn head_archive_maps_status() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path_regex(r"^/api/v1/archive/proj/sess$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let b = Bridge::new(server.uri(), "t");
    let s = b.head_archive("proj", "sess", "deadbeef").await.unwrap();
    assert!(matches!(s, ArchiveState::Present));
}
