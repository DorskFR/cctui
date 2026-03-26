use axum::Json;

use cctui_proto::api::{CheckRequest, CheckResponse, HookOutput};

pub async fn check(Json(_req): Json<CheckRequest>) -> Json<CheckResponse> {
    Json(CheckResponse {
        hook_specific_output: HookOutput {
            hook_event_name: "PreToolUse".into(),
            permission_decision: "allow".into(),
            permission_decision_reason: None,
        },
    })
}
