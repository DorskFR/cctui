#!/bin/sh
# cctui installer. Fetches the cctui binary and MCP channel bundle, then wires
# Claude Code to route through them. Idempotent: re-running upgrades in place.
#
# Usage:
#   curl -fsSL https://cctui.dorsk.dev/install.sh | sh
#
# Environment:
#   CCTUI_URL     server URL (default: https://cctui.dorsk.dev)
#   CCTUI_TOKEN   agent token (prompted if unset and stdin is a TTY)
#   CCTUI_PREFIX  install dir for the binary (default: ~/.local/bin, falls back to /usr/local/bin)
#   CCTUI_REPO    GitHub repo for binary releases (default: DorskFR/cctui)
#   CCTUI_TAG     release tag to install (default: latest)

set -eu

CCTUI_URL="${CCTUI_URL:-https://cctui.dorsk.dev}"
CCTUI_REPO="${CCTUI_REPO:-DorskFR/cctui}"
CCTUI_TAG="${CCTUI_TAG:-latest}"
CCTUI_HOME="${CCTUI_HOME:-$HOME/.cctui}"

log()  { printf '[cctui] %s\n' "$*"; }
warn() { printf '[cctui] WARN: %s\n' "$*" >&2; }
die()  { printf '[cctui] ERROR: %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "required command missing: $1"; }

need curl
need uname

# Python is used to merge JSON config files without clobbering existing keys.
PY="$(command -v python3 || command -v python || true)"
[ -n "$PY" ] || die "python3 required for config merge"

# ── Detect OS / arch ──────────────────────────────────────────────────────────
os_raw="$(uname -s)"
arch_raw="$(uname -m)"
case "$os_raw" in
  Linux)  OS=linux ;;
  Darwin) OS=darwin ;;
  *) die "unsupported OS: $os_raw" ;;
esac
case "$arch_raw" in
  x86_64|amd64)  ARCH=amd64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) die "unsupported arch: $arch_raw" ;;
esac
log "detected $OS/$ARCH"

# ── Resolve install prefix ────────────────────────────────────────────────────
if [ -n "${CCTUI_PREFIX:-}" ]; then
  PREFIX="$CCTUI_PREFIX"
elif [ -w "/usr/local/bin" ] 2>/dev/null; then
  PREFIX="/usr/local/bin"
else
  PREFIX="$HOME/.local/bin"
fi
mkdir -p "$PREFIX" "$CCTUI_HOME"

# ── Prompt for token if needed ────────────────────────────────────────────────
if [ -z "${CCTUI_TOKEN:-}" ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    printf 'cctui agent token: '
    stty -echo 2>/dev/null || true
    read -r CCTUI_TOKEN
    stty echo 2>/dev/null || true
    printf '\n'
  else
    die "CCTUI_TOKEN not set and no TTY for prompt"
  fi
fi
[ -n "$CCTUI_TOKEN" ] || die "CCTUI_TOKEN is empty"

# ── Download the cctui binary ─────────────────────────────────────────────────
BIN_NAME="cctui-${OS}-${ARCH}"
if [ "$CCTUI_TAG" = "latest" ]; then
  BIN_URL="https://github.com/${CCTUI_REPO}/releases/latest/download/${BIN_NAME}"
else
  BIN_URL="https://github.com/${CCTUI_REPO}/releases/download/${CCTUI_TAG}/${BIN_NAME}"
fi
BIN_DEST="$PREFIX/cctui"
log "downloading binary from $BIN_URL"
tmpbin="$(mktemp)"
if ! curl -fsSL -o "$tmpbin" "$BIN_URL"; then
  rm -f "$tmpbin"
  die "failed to download $BIN_URL (has a release been published yet?)"
fi
chmod +x "$tmpbin"
mv "$tmpbin" "$BIN_DEST"
log "installed binary -> $BIN_DEST"

# ── Write identity config ─────────────────────────────────────────────────────
# A user key (cctui_u_…) is exchanged at /api/v1/enroll for a machine key and
# written to both user.json (consumed by the TUI) and machine.json (consumed
# by the MCP channel). Anything else is treated as a raw machine key
# (back-compat for older enrollment flows / dev tokens).
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/cctui"
mkdir -p "$CONFIG_DIR"
MACHINE_JSON="$CONFIG_DIR/machine.json"
USER_JSON="$CONFIG_DIR/user.json"
HOSTNAME_VAL="$(hostname 2>/dev/null || echo unknown)"

# Enroll via curl (not urllib) so we use the system cert store — avoids
# macOS python.org installers that ship without a CA bundle.
ENROLL_BODY=""
if [ "${CCTUI_TOKEN#cctui_u_}" != "$CCTUI_TOKEN" ]; then
  enroll_tmp="$(mktemp)"
  http_code="$(curl -sS -o "$enroll_tmp" -w '%{http_code}' \
    -X POST "$CCTUI_URL/api/v1/enroll" \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $CCTUI_TOKEN" \
    -d "{\"hostname\":\"$HOSTNAME_VAL\"}" || echo "000")"
  if [ "$http_code" != "200" ]; then
    body="$(cat "$enroll_tmp" 2>/dev/null || true)"
    rm -f "$enroll_tmp"
    die "enroll failed: HTTP $http_code: $body"
  fi
  ENROLL_BODY="$(cat "$enroll_tmp")"
  rm -f "$enroll_tmp"
fi

CCTUI_TOKEN="$CCTUI_TOKEN" \
CCTUI_URL="$CCTUI_URL" \
MACHINE_JSON="$MACHINE_JSON" \
USER_JSON="$USER_JSON" \
HOSTNAME_VAL="$HOSTNAME_VAL" \
ENROLL_BODY="$ENROLL_BODY" \
"$PY" - <<'PY'
import json, os

token     = os.environ["CCTUI_TOKEN"]
server    = os.environ["CCTUI_URL"].rstrip("/")
hostname  = os.environ["HOSTNAME_VAL"]
machine_p = os.environ["MACHINE_JSON"]
user_p    = os.environ["USER_JSON"]

def write_json(path, data):
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
    os.chmod(path, 0o600)

if token.startswith("cctui_u_"):
    body = json.loads(os.environ["ENROLL_BODY"])
    write_json(user_p, {"server_url": server, "user_key": token})
    write_json(machine_p, {
        "server_url":  server,
        "machine_key": body["machine_key"],
        "machine_id":  body["machine_id"],
        "hostname":    hostname,
    })
    print(f"[cctui] wrote user config    -> {user_p}")
    print(f"[cctui] wrote machine config -> {machine_p}  (machine_id={body['machine_id']})")
else:
    # Legacy / raw machine key: just persist it.
    write_json(machine_p, {"server_url": server, "machine_key": token, "hostname": hostname})
    print(f"[cctui] wrote machine config -> {machine_p}")
PY

# ── Configure Claude Code (~/.claude.json + ~/.claude/settings.json) ──────────
CCTUI_URL="$CCTUI_URL" CCTUI_TOKEN="$CCTUI_TOKEN" BIN_DEST="$BIN_DEST" "$PY" - <<'PY'
import json, os

server_url = os.environ["CCTUI_URL"]
token      = os.environ["CCTUI_TOKEN"]
bin_path   = os.environ["BIN_DEST"]
home       = os.path.expanduser("~")

# MCP server entry in ~/.claude.json
claude_json = os.path.join(home, ".claude.json")
try:
    with open(claude_json) as f:
        cfg = json.load(f)
except (FileNotFoundError, ValueError):
    cfg = {}
cfg.setdefault("mcpServers", {})
cfg["mcpServers"]["cctui"] = {
    "command": bin_path,
    "args": ["channel"],
    "env": {"CCTUI_URL": server_url},
}
with open(claude_json, "w") as f:
    json.dump(cfg, f, indent=2)
print(f"[cctui] updated {claude_json}")

# Hooks in ~/.claude/settings.json
settings_path = os.path.join(home, ".claude/settings.json")
os.makedirs(os.path.dirname(settings_path), exist_ok=True)
try:
    with open(settings_path) as f:
        settings = json.load(f)
except (FileNotFoundError, ValueError):
    settings = {}

# HTTP hooks in Claude Code block private / link-local IPs (SSRF guard).
# To support homelab servers we shell out to curl instead.
auth_prelude = (
    'KEY="${CCTUI_AGENT_TOKEN:-$(jq -r .machine_key '
    '"${XDG_CONFIG_HOME:-$HOME/.config}/cctui/machine.json" 2>/dev/null)}"; '
    f'[ -z "$KEY" ] && KEY="{token}"; '
)

def curl_cmd(path: str, enrich: str = "") -> str:
    pipe = (
        f"jq -c {enrich}" if enrich else "cat"
    )
    return (
        auth_prelude
        + f'{pipe} | curl -sf -X POST {server_url}{path} '
        "-H 'Content-Type: application/json' "
        '-H "Authorization: Bearer $KEY" -d @-'
    )

session_start_cmd = curl_cmd(
    "/api/v1/hooks/session-start",
    enrich="--arg ppid \"$PPID\" --arg mid \"$(hostname)\" "
           "'. + {ppid: ($ppid | tonumber), machine_id: $mid}'",
)

settings.setdefault("hooks", {})
settings["hooks"].update({
    "SessionStart": [{"hooks": [{"type": "command", "command": session_start_cmd}]}],
    "PreToolUse":   [{"hooks": [{"type": "command", "command": curl_cmd("/api/v1/check")}]}],
    "PostToolUse":  [{"hooks": [{"type": "command", "command": curl_cmd("/api/v1/hooks/post-tool-use")}]}],
    "Stop":         [{"hooks": [{"type": "command", "command": curl_cmd("/api/v1/hooks/stop")}]}],
})

with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)
print(f"[cctui] updated {settings_path}")
PY

log "done. binary: $BIN_DEST  server: $CCTUI_URL"
case ":$PATH:" in
  *":$PREFIX:"*) : ;;
  *) warn "$PREFIX is not in \$PATH — add it to your shell profile" ;;
esac
