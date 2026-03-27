use axum::Extension;
use axum::extract::State;

use crate::auth::AuthContext;
use crate::state::AppState;

pub async fn bootstrap(
    State(_state): State<AppState>,
    Extension(_ctx): Extension<AuthContext>,
) -> String {
    "echo '[cctui] use make setup/claude to install hooks'".to_string()
}

pub async fn setup(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> String {
    let server_url = &state.config.external_url;
    let token = &ctx.token;

    // Generate a python3 script that writes:
    //   1. ~/.cctui/bin/bootstrap.sh — reads hook stdin, registers session
    //   2. merges hooks into ~/.claude/settings.json
    //
    // The bootstrap.sh uses a python one-liner for JSON construction
    // to avoid shell escaping nightmares.
    format!(
        r#"#!/usr/bin/env python3
import json, os, stat, textwrap

SERVER_URL = "{server_url}"
TOKEN = "{token}"

cctui_dir = os.path.expanduser("~/.cctui/bin")
os.makedirs(cctui_dir, exist_ok=True)

# --- Write local bootstrap script ---
bootstrap_path = os.path.join(cctui_dir, "bootstrap.sh")
# The bootstrap script uses python3 to build JSON safely (no shell escaping)
with open(bootstrap_path, "w") as f:
    f.write(textwrap.dedent(f"""\
        #!/bin/sh
        set -e
        HOOK_INPUT=$(cat)
        # Use python3 to parse hook input and register — avoids shell JSON escaping
        echo "$HOOK_INPUT" | python3 -c "
import json, subprocess, sys, os, socket

hook = json.load(sys.stdin)
sid = hook.get('session_id', '')
cwd = hook.get('cwd', os.getcwd())
model = hook.get('model', '')

try:
    branch = subprocess.check_output(
        ['git', '-C', cwd, 'rev-parse', '--abbrev-ref', 'HEAD'],
        stderr=subprocess.DEVNULL
    ).decode().strip()
except Exception:
    branch = 'none'

body = json.dumps(dict(
    claude_session_id=sid,
    machine_id=socket.gethostname(),
    working_dir=cwd,
    metadata=dict(
        git_branch=branch,
        project_name=os.path.basename(cwd),
        model=model,
    ),
))

subprocess.run([
    'curl', '-sf', '-X', 'POST',
    '{{SERVER_URL}}/api/v1/sessions/register',
    '-H', 'Authorization: Bearer {{TOKEN}}',
    '-H', 'Content-Type: application/json',
    '-d', body,
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

os.makedirs(os.path.expanduser('~/.cctui'), exist_ok=True)
with open(os.path.expanduser('~/.cctui/session_id'), 'w') as f:
    f.write(sid)
"
    """))
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
            "url": f"{{SERVER_URL}}/api/v1/check"
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

print(f"[cctui] done — Claude sessions will register with {{SERVER_URL}}")
"#
    )
}
