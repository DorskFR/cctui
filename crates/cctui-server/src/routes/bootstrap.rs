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
    format!(
        r#"#!/bin/sh
set -e

SERVER_URL="{server_url}"
TOKEN="{token}"
CCTUI_DIR="$HOME/.cctui"
mkdir -p "$CCTUI_DIR/bin"

# Collect metadata
MACHINE_ID=$(hostname)
WORKING_DIR=$(pwd)
GIT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "none")
PROJECT_NAME=$(basename "$WORKING_DIR")

# Register session
RESPONSE=$(curl -sf -X POST "$SERVER_URL/api/v1/sessions/register" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{{\"machine_id\":\"$MACHINE_ID\",\"working_dir\":\"$WORKING_DIR\",\"metadata\":{{\"git_branch\":\"$GIT_BRANCH\",\"project_name\":\"$PROJECT_NAME\"}}}}")

SESSION_ID=$(echo "$RESPONSE" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)
echo "$SESSION_ID" > "$CCTUI_DIR/session_id"

# Download shim if not present
if [ ! -f "$CCTUI_DIR/bin/cctui-shim" ]; then
    ARCH=$(uname -m)
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    curl -sf -o "$CCTUI_DIR/bin/cctui-shim" \
        "$SERVER_URL/api/v1/shim/$OS/$ARCH" && \
    chmod +x "$CCTUI_DIR/bin/cctui-shim" || true
fi

# Start shim in background if available
if [ -x "$CCTUI_DIR/bin/cctui-shim" ]; then
    WS_URL=$(echo "$RESPONSE" | grep -o '"ws_url":"[^"]*"' | cut -d'"' -f4)
    "$CCTUI_DIR/bin/cctui-shim" relay \
        --session-id "$SESSION_ID" \
        --ws-url "$WS_URL" &
fi

echo "[cctui] registered as $SESSION_ID on $MACHINE_ID"
"#
    )
}

pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;
    // The heredoc uses a quoted delimiter so shell variables are NOT expanded
    // But we need the Rust format args to be expanded, so we write the values directly
    format!(
        r#"#!/bin/sh
set -e

CLAUDE_DIR="$HOME/.claude"
mkdir -p "$CLAUDE_DIR"

cat > "$CLAUDE_DIR/managed-settings.json" << 'CCTUI_EOF'
{{
  "hooks": {{
    "SessionStart": [{{
      "type": "command",
      "command": "curl -sf -H 'Authorization: Bearer {token}' {server_url}/api/v1/bootstrap | sh"
    }}],
    "PreToolUse": [{{
      "type": "http",
      "url": "{server_url}/api/v1/check",
      "headers": {{"Authorization": "Bearer {token}"}}
    }}],
    "Stop": [{{
      "type": "command",
      "command": "curl -sf -X POST -H 'Authorization: Bearer {token}' {server_url}/api/v1/sessions/$(cat ~/.cctui/session_id)/deregister"
    }}]
  }}
}}
CCTUI_EOF

echo "[cctui] managed-settings.json written to $CLAUDE_DIR/managed-settings.json"
echo "[cctui] Claude Code will now auto-register sessions with {server_url}"
"#
    )
}
