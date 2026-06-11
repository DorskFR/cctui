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
    Json(engine.get_state())
}

/// `SessionStart`/compact hook — returns context text for re-injection.
async fn post_state(State(engine): State<Engine>) -> impl IntoResponse {
    let state = engine.get_state();
    let step = state.get("step").cloned().unwrap_or_else(|| json!(0));
    let title = state.get("title").and_then(Value::as_str).unwrap_or("");
    let allowed = state.get("allowed").and_then(Value::as_str).unwrap_or("");
    let disallowed = state.get("disallowed").and_then(Value::as_str).unwrap_or("");
    let body = format!(
        "[Workflow Guard] You are currently on Step {step}: {title}.\n\
         Allowed: {allowed}\n\
         Disallowed: {disallowed}\n\
         To transition steps: curl -s -X POST http://127.0.0.1:9999/transition \
         -H \"Content-Type: application/json\" -d '{{\"step\": N}}'\n\
         To check current state: curl -s http://127.0.0.1:9999/state"
    );
    ([("Content-Type", "text/plain")], body)
}

async fn check(State(engine): State<Engine>, Json(data): Json<Value>) -> impl IntoResponse {
    let tool = data.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let tool_input = data.get("tool_input").cloned().unwrap_or_else(|| json!({}));
    Json(engine.check(tool, &tool_input))
}

async fn transition(State(engine): State<Engine>, Json(data): Json<Value>) -> impl IntoResponse {
    let target = data.get("step").cloned().unwrap_or_else(|| json!(""));
    Json(engine.transition(&target))
}
