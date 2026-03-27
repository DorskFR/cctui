use axum::Extension;
use axum::extract::State;

use crate::auth::AuthContext;
use crate::state::AppState;

pub async fn bootstrap(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;
    // The SessionStart hook receives JSON on stdin with Claude's own session_id.
    // We read it and pass it to the register endpoint so PreToolUse calls
    // (which also include Claude's session_id) can be correlated.
    format!(
        r#"#!/bin/sh
set -e

SERVER_URL="{server_url}"
TOKEN="{token}"
CCTUI_DIR="$HOME/.cctui"
mkdir -p "$CCTUI_DIR"

# Read Claude's hook input from stdin to get the session_id
HOOK_INPUT=$(cat)
CLAUDE_SESSION_ID=$(echo "$HOOK_INPUT" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)

# Collect metadata
MACHINE_ID=$(hostname)
WORKING_DIR=$(echo "$HOOK_INPUT" | grep -o '"cwd":"[^"]*"' | cut -d'"' -f4)
WORKING_DIR="${{WORKING_DIR:-$(pwd)}}"
GIT_BRANCH=$(git -C "$WORKING_DIR" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "none")
PROJECT_NAME=$(basename "$WORKING_DIR")
MODEL=$(echo "$HOOK_INPUT" | grep -o '"model":"[^"]*"' | cut -d'"' -f4)

# Register with Claude's session_id so PreToolUse calls can be matched
curl -sf -X POST "$SERVER_URL/api/v1/sessions/register" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{{\"claude_session_id\":\"$CLAUDE_SESSION_ID\",\"machine_id\":\"$MACHINE_ID\",\"working_dir\":\"$WORKING_DIR\",\"metadata\":{{\"git_branch\":\"$GIT_BRANCH\",\"project_name\":\"$PROJECT_NAME\",\"model\":\"$MODEL\"}}}}" > /dev/null

echo "$CLAUDE_SESSION_ID" > "$CCTUI_DIR/session_id"
"#
    )
}

pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;

    // Generate a self-contained python3 script that merges hooks into settings.local.json.
    // Python avoids all the shell quoting nightmares with nested JSON + shell variables.
    format!(
        r#"#!/usr/bin/env python3
import json, os, sys

settings_path = os.path.expanduser("~/.claude/settings.json")

hooks = {{
    "SessionStart": [{{
        "hooks": [{{
            "type": "command",
            "command": "curl -sf -H 'Authorization: Bearer {token}' {server_url}/api/v1/bootstrap | sh"
        }}]
    }}],
    "PreToolUse": [{{
        "hooks": [{{
            "type": "http",
            "url": "{server_url}/api/v1/check"
        }}]
    }}]
}}

if os.path.exists(settings_path):
    with open(settings_path) as f:
        settings = json.load(f)
    print(f"[cctui] merging hooks into {{settings_path}}")
else:
    settings = {{}}
    print(f"[cctui] creating {{settings_path}}")

settings["hooks"] = hooks

os.makedirs(os.path.dirname(settings_path), exist_ok=True)
with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)

print(f"[cctui] Claude Code will now auto-register sessions with {server_url}")
"#
    )
}
