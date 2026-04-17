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

#[tokio::test]
#[ignore = "requires running server"]
async fn user_enroll_revoke_flow() {
    let client = Client::new();
    let base = server_url();

    // 1. Admin creates a user — receives key once.
    let resp = client
        .post(format!("{base}/api/v1/admin/users"))
        .bearer_auth(admin_token())
        .json(&json!({"name": format!("itest-{}", uuid_like())}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let user_id = body["id"].as_str().unwrap().to_string();
    let user_key = body["key"].as_str().unwrap().to_string();
    assert!(user_key.starts_with("cctui_u_"));

    // 2. User enrols a machine with their key.
    let resp = client
        .post(format!("{base}/api/v1/enroll"))
        .bearer_auth(&user_key)
        .json(&json!({"hostname": "itest-host"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let machine_key = body["machine_key"].as_str().unwrap().to_string();
    assert!(machine_key.starts_with("cctui_m_"));

    // 3. Machine key can register a session.
    let resp = client
        .post(format!("{base}/api/v1/sessions/register"))
        .bearer_auth(&machine_key)
        .json(&json!({
            "machine_id": "itest-host",
            "working_dir": "/tmp/itest",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 4. Admin revokes the user; both keys stop working (after TTL or cache purge).
    let resp = client
        .delete(format!("{base}/api/v1/admin/users/{user_id}"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let resp = client
        .post(format!("{base}/api/v1/enroll"))
        .bearer_auth(&user_key)
        .json(&json!({"hostname": "itest-host-2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    let resp = client
        .post(format!("{base}/api/v1/sessions/register"))
        .bearer_auth(&machine_key)
        .json(&json!({"machine_id": "itest-host", "working_dir": "/tmp/itest"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
#[ignore = "requires running server"]
async fn machine_rotate_invalidates_old_key() {
    let client = Client::new();
    let base = server_url();

    let u: serde_json::Value = client
        .post(format!("{base}/api/v1/admin/users"))
        .bearer_auth(admin_token())
        .json(&json!({"name": format!("rot-{}", uuid_like())}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_key = u["key"].as_str().unwrap().to_string();

    let m: serde_json::Value = client
        .post(format!("{base}/api/v1/enroll"))
        .bearer_auth(&user_key)
        .json(&json!({"hostname": "rot-host"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine_id = m["machine_id"].as_str().unwrap().to_string();
    let old_key = m["machine_key"].as_str().unwrap().to_string();

    let r: serde_json::Value = client
        .post(format!("{base}/api/v1/admin/machines/{machine_id}/rotate"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_key = r["key"].as_str().unwrap().to_string();
    assert_ne!(old_key, new_key);

    // Old machine key rejected.
    let resp = client
        .post(format!("{base}/api/v1/sessions/register"))
        .bearer_auth(&old_key)
        .json(&json!({"machine_id": "rot-host", "working_dir": "/tmp/rot"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // New machine key works.
    let resp = client
        .post(format!("{base}/api/v1/sessions/register"))
        .bearer_auth(&new_key)
        .json(&json!({"machine_id": "rot-host", "working_dir": "/tmp/rot"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}
