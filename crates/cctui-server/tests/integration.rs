//! Integration tests — require a running cctui-server and `DATABASE_URL`.
//!
//! Run: `TEST_CCTUI_URL=http://localhost:8700 cargo test -p cctui-server --test integration -- --ignored`

use reqwest::Client;
use serde_json::json;

fn server_url() -> String {
    std::env::var("TEST_CCTUI_URL").unwrap_or_else(|_| "http://localhost:8700".into())
}

fn agent_token() -> String {
    std::env::var("TEST_AGENT_TOKEN").unwrap_or_else(|_| "test-agent".into())
}

fn admin_token() -> String {
    std::env::var("TEST_ADMIN_TOKEN").unwrap_or_else(|_| "test-admin".into())
}

#[tokio::test]
#[ignore = "requires running server"]
async fn health_check() {
    let client = Client::new();
    let resp = client.get(format!("{}/health", server_url())).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
#[ignore = "requires running server"]
async fn register_and_list_session() {
    let client = Client::new();
    let base = server_url();

    // Register
    let resp = client
        .post(format!("{base}/api/v1/sessions/register"))
        .bearer_auth(agent_token())
        .json(&json!({
            "machine_id": "test-machine",
            "working_dir": "/tmp/test",
            "metadata": {"project_name": "test-project"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let session_id = body["session_id"].as_str().unwrap();

    // List
    let resp = client
        .get(format!("{base}/api/v1/sessions"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|s| s["id"].as_str() == Some(session_id)));

    // Deregister
    let resp = client
        .post(format!("{base}/api/v1/sessions/{session_id}/deregister"))
        .bearer_auth(agent_token())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
#[ignore = "requires running server"]
async fn auth_rejects_bad_token() {
    let client = Client::new();
    let resp = client
        .get(format!("{}/api/v1/sessions", server_url()))
        .bearer_auth("wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
