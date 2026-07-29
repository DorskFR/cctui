//! Localhost HTTP server exposing the workflow guard over axum.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::engine::WorkflowEngine;

type Engine = Arc<WorkflowEngine>;

/// Build the axum router for the guard daemon.
pub fn router(engine: Engine) -> Router {
    Router::new()
        .route("/state", get(get_state).post(post_state))
        .route("/health", get(health))
        .route("/check", post(check))
        .route("/transition", post(transition))
        .with_state(engine)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn get_state(State(engine): State<Engine>) -> impl IntoResponse {
    let state = tokio::task::spawn_blocking(move || engine.get_state()).await.unwrap_or_else(|e| {
        tracing::error!("guard state task failed: {e}");
        json!({ "error": "internal guard error" })
    });
    Json(state)
}

/// `SessionStart`/compact hook — returns context text for re-injection. Always
/// carries the authoritative step prompt body so a long or compacted session
/// re-anchors on trusted instructions rather than its own drifting summary
/// (CCT-440); the compact-context directive is included only for steps that
/// opt in via `[compact]` (CCT-450).
async fn post_state(State(engine): State<Engine>) -> impl IntoResponse {
    let body = tokio::task::spawn_blocking(move || {
        let state = engine.get_state();
        let step_num = state.get("step").and_then(Value::as_i64).unwrap_or(0);
        let allowed = state.get("allowed").and_then(Value::as_str).unwrap_or("").to_owned();
        let disallowed = state.get("disallowed").and_then(Value::as_str).unwrap_or("").to_owned();
        let reinject = u32::try_from(step_num).map(|n| engine.reinjection(n)).unwrap_or_default();
        format!(
            "{reinject}\n\n\
             Allowed: {allowed}\n\
             Disallowed: {disallowed}\n\
             To transition steps: curl -s -X POST http://127.0.0.1:9999/transition \
             -H \"Content-Type: application/json\" -d '{{\"step\": N}}'\n\
             To check current state: curl -s http://127.0.0.1:9999/state"
        )
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("guard state task failed: {e}");
        String::new()
    });
    ([("Content-Type", "text/plain")], body)
}

async fn check(State(engine): State<Engine>, Json(data): Json<Value>) -> impl IntoResponse {
    let tool = data.get("tool_name").and_then(Value::as_str).unwrap_or("").to_owned();
    let tool_input = data.get("tool_input").cloned().unwrap_or_else(|| json!({}));
    let decision = tokio::task::spawn_blocking(move || engine.check(&tool, &tool_input))
        .await
        .unwrap_or_else(|e| {
            tracing::error!("guard check task failed: {e}");
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason":
                        "internal guard error (check task failed) — failing closed",
                }
            })
        });
    Json(decision)
}

async fn transition(State(engine): State<Engine>, Json(data): Json<Value>) -> impl IntoResponse {
    let target = data.get("step").cloned().unwrap_or_else(|| json!(""));
    let result =
        tokio::task::spawn_blocking(move || engine.transition(&target)).await.unwrap_or_else(|e| {
            tracing::error!("guard transition task failed: {e}");
            json!({
                "ok": false,
                "error": "internal guard error (transition task failed) — failing closed",
            })
        });
    Json(result)
}
