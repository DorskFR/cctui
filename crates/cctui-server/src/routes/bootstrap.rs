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
# This endpoint is no longer used for curl|sh. See setup endpoint instead.
echo "[cctui] use 'make setup/claude' or GET /api/v1/setup to install the local bootstrap script"
echo "[cctui] server={server_url} token={token}"
"#
    )
}

pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;

    // Generate a python3 script that:
    // 1. Writes a local bootstrap.sh that reads stdin (hook input) and registers
    // 2. Merges hooks into ~/.claude/settings.json pointing to the local script
    format!(
        r#"#!/usr/bin/env python3
import json, os, stat

server_url = "{server_url}"
token = "{token}"

cctui_dir = os.path.expanduser("~/.cctui/bin")
os.makedirs(cctui_dir, exist_ok=True)

# --- Write the local bootstrap script ---
# This runs on SessionStart. It reads Claude's hook JSON from stdin,
# extracts the session_id, and registers with the server.
bootstrap_path = os.path.join(cctui_dir, "bootstrap.sh")
with open(bootstrap_path, "w") as f:
    f.write(f'''#!/bin/sh
set -e
HOOK_INPUT=$(cat)
CLAUDE_SESSION_ID=$(echo "$HOOK_INPUT" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)
CWD=$(echo "$HOOK_INPUT" | grep -o '"cwd":"[^"]*"' | cut -d'"' -f4)
CWD="${{CWD:-$(pwd)}}"
MODEL=$(echo "$HOOK_INPUT" | grep -o '"model":"[^"]*"' | cut -d'"' -f4)
GIT_BRANCH=$(git -C "$CWD" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "none")
PROJECT_NAME=$(basename "$CWD")

curl -sf -X POST {server_url}/api/v1/sessions/register \\
  -H "Authorization: Bearer {token}" \\
  -H "Content-Type: application/json" \\
  -d "{{\\"claude_session_id\\":\\"$CLAUDE_SESSION_ID\\",\\"machine_id\\":\\"$(hostname)\\",\\"working_dir\\":\\"$CWD\\",\\"metadata\\":{{\\"git_branch\\":\\"$GIT_BRANCH\\",\\"project_name\\":\\"$PROJECT_NAME\\",\\"model\\":\\"$MODEL\\"}}}}" > /dev/null 2>&1

echo "$CLAUDE_SESSION_ID" > ~/.cctui/session_id
''')
os.chmod(bootstrap_path, os.stat(bootstrap_path).st_mode | stat.S_IEXEC)
print(f"[cctui] wrote {{bootstrap_path}}")

# --- Merge hooks into settings.json ---
settings_path = os.path.expanduser("~/.claude/settings.json")

hooks = {{
    "SessionStart": [{{
        "hooks": [{{
            "type": "command",
            "command": bootstrap_path
        }}]
    }}],
    "PreToolUse": [{{
        "hooks": [{{
            "type": "http",
            "url": f"{{server_url}}/api/v1/check"
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

print(f"[cctui] Claude Code will now auto-register sessions with {{server_url}}")
"#
    )
}
