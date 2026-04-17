#!/usr/bin/env python3
"""Setup script. Configures Claude Code to use the cctui channel subcommand."""
import json
import os
import shutil

SERVER_URL = os.environ.get("__SERVER_URL__", "__SERVER_URL__")
# TOKEN is used only for hook curl lines (SessionStart). The channel itself
# reads its key from ~/.config/cctui/machine.json.
TOKEN = os.environ.get("__TOKEN__", "__TOKEN__")

# --- Locate the cctui binary ---
CCTUI_BIN = os.environ.get("CCTUI_BIN") or shutil.which("cctui")
if not CCTUI_BIN or not os.path.isfile(CCTUI_BIN):
    print(f"[cctui] ERROR: cctui binary not found (set CCTUI_BIN or install to PATH)")
    exit(1)

# --- Write MCP server config to ~/.claude.json ---
claude_json_path = os.path.expanduser("~/.claude.json")

mcp_entry = {
    "command": CCTUI_BIN,
    "args": ["channel"],
    "env": {
        # Server URL is safe to embed; token is loaded by the channel from
        # ~/.config/cctui/machine.json (created by `cctui-admin enroll`).
        # CCTUI_AGENT_TOKEN is still honored as an env override for local dev.
        "CCTUI_URL": SERVER_URL,
    },
}

if os.path.exists(claude_json_path):
    with open(claude_json_path) as f:
        claude_config = json.load(f)
else:
    claude_config = {}

claude_config.setdefault("mcpServers", {})
claude_config["mcpServers"]["cctui"] = mcp_entry

with open(claude_json_path, "w") as f:
    json.dump(claude_config, f, indent=2)

print(f"[cctui] wrote MCP server config to {claude_json_path}")

# --- Merge hooks into settings.json ---
settings_path = os.path.expanduser("~/.claude/settings.json")

# SessionStart: pipe stdin through jq to inject $PPID and machine hostname, then POST to server.
# The bearer is read from ~/.config/cctui/machine.json (written by `cctui-admin enroll`);
# CCTUI_AGENT_TOKEN env var still overrides, and the hard-coded TOKEN is the last-resort
# fallback for dev / pre-enrolment environments.
session_start_cmd = (
    'KEY="${CCTUI_AGENT_TOKEN:-$(jq -r .machine_key '
    f'"${{XDG_CONFIG_HOME:-$HOME/.config}}/cctui/machine.json" 2>/dev/null)}"; '
    f'[ -z "$KEY" ] && KEY="{TOKEN}"; '
    f'jq -c --arg ppid "$PPID" --arg mid "$(hostname)" '
    f"'. + {{ppid: ($ppid | tonumber), machine_id: $mid}}' "
    f'| curl -sf -X POST {SERVER_URL}/api/v1/hooks/session-start '
    f"-H 'Content-Type: application/json' "
    f'-H "Authorization: Bearer $KEY" -d @-'
)

hooks = {
    "SessionStart": [{
        "hooks": [{
            "type": "command",
            "command": session_start_cmd,
        }],
    }],
    "PreToolUse": [{
        "hooks": [{
            "type": "http",
            "url": f"{SERVER_URL}/api/v1/check",
        }],
    }],
    "PostToolUse": [{
        "hooks": [{
            "type": "http",
            "url": f"{SERVER_URL}/api/v1/hooks/post-tool-use",
        }],
    }],
    "Stop": [{
        "hooks": [{
            "type": "http",
            "url": f"{SERVER_URL}/api/v1/hooks/stop",
        }],
    }],
}

if os.path.exists(settings_path):
    with open(settings_path) as f:
        settings = json.load(f)
    print(f"[cctui] merging hooks into {settings_path}")
else:
    settings = {}
    print(f"[cctui] creating {settings_path}")

settings["hooks"] = hooks

os.makedirs(os.path.dirname(settings_path), exist_ok=True)
with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)

print(f"[cctui] done - hooks route to {SERVER_URL}")
print()
print("[cctui] IMPORTANT: To enable TUI→Claude messaging, start Claude with:")
print("        claude --dangerously-load-development-channels server:cctui")
print("        Without this flag, Claude sees the tools but won't receive channel messages.")
