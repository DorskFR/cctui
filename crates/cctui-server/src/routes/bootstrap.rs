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
        r#"#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="{server_url}"
TOKEN="{token}"

# Register session and get session_id + ws_url
RESPONSE=$(curl -sf -X POST "$SERVER_URL/api/v1/sessions/register" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{{\"machine_id\":\"$(hostname)\",\"working_dir\":\"$(pwd)\"}}")

SESSION_ID=$(echo "$RESPONSE" | jq -r '.session_id')
WS_URL=$(echo "$RESPONSE" | jq -r '.ws_url')

# Download shim
SHIM_PATH="$HOME/.local/bin/cctui-shim"
mkdir -p "$(dirname "$SHIM_PATH")"
curl -sf "$SERVER_URL/api/v1/shim" -o "$SHIM_PATH"
chmod +x "$SHIM_PATH"

# Start relay
exec "$SHIM_PATH" --session-id "$SESSION_ID" --ws-url "$WS_URL" --token "$TOKEN"
"#
    )
}

pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="{server_url}"
TOKEN="{token}"

SETTINGS_DIR="$HOME/.claude"
MANAGED_SETTINGS="$SETTINGS_DIR/managed-settings.json"

mkdir -p "$SETTINGS_DIR"

cat > "$MANAGED_SETTINGS" <<'SETTINGS'
{{
  "cctui": {{
    "server_url": "{server_url}",
    "token": "{token}"
  }}
}}
SETTINGS

echo "Wrote managed-settings.json to $MANAGED_SETTINGS"
"#
    )
}
