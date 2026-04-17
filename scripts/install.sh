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

# ── Check for bun (runtime for the channel) ───────────────────────────────────
if ! command -v bun >/dev/null 2>&1 && [ ! -x "$HOME/.bun/bin/bun" ]; then
  warn "bun not found. The channel requires bun at runtime."
  if [ -t 0 ] && [ -t 1 ]; then
    printf 'Install bun now via https://bun.sh/install? [Y/n] '
    read -r ans
    case "${ans:-Y}" in
      [nN]*) warn "skipping bun install; install manually before starting Claude" ;;
      *)     curl -fsSL https://bun.sh/install | bash ;;
    esac
  else
    warn "run: curl -fsSL https://bun.sh/install | bash"
  fi
fi

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

# ── Download the channel bundle ───────────────────────────────────────────────
CHANNEL_DEST="$CCTUI_HOME/channel.js"
CHANNEL_URL="$CCTUI_URL/channel/latest.js"
log "downloading channel bundle from $CHANNEL_URL"
tmpch="$(mktemp)"
if ! curl -fsSL -o "$tmpch" "$CHANNEL_URL"; then
  rm -f "$tmpch"
  die "failed to download channel bundle from $CHANNEL_URL"
fi
mv "$tmpch" "$CHANNEL_DEST"
log "installed channel -> $CHANNEL_DEST"

# ── Write machine config (token) ──────────────────────────────────────────────
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/cctui"
mkdir -p "$CONFIG_DIR"
MACHINE_JSON="$CONFIG_DIR/machine.json"
CCTUI_TOKEN="$CCTUI_TOKEN" CCTUI_URL="$CCTUI_URL" MACHINE_JSON="$MACHINE_JSON" "$PY" - <<'PY'
import json, os
path = os.environ["MACHINE_JSON"]
try:
    with open(path) as f:
        data = json.load(f)
except (FileNotFoundError, ValueError):
    data = {}
data["machine_key"] = os.environ["CCTUI_TOKEN"]
data["server_url"] = os.environ["CCTUI_URL"]
with open(path, "w") as f:
    json.dump(data, f, indent=2)
os.chmod(path, 0o600)
PY
log "wrote machine config -> $MACHINE_JSON"

# ── Configure Claude Code (~/.claude.json + ~/.claude/settings.json) ──────────
CCTUI_URL="$CCTUI_URL" CCTUI_TOKEN="$CCTUI_TOKEN" CHANNEL_DEST="$CHANNEL_DEST" "$PY" - <<'PY'
import json, os, shutil

server_url = os.environ["CCTUI_URL"]
token      = os.environ["CCTUI_TOKEN"]
channel    = os.environ["CHANNEL_DEST"]
home       = os.path.expanduser("~")

bun = shutil.which("bun") or os.path.join(home, ".bun/bin/bun")

# MCP server entry in ~/.claude.json
claude_json = os.path.join(home, ".claude.json")
try:
    with open(claude_json) as f:
        cfg = json.load(f)
except (FileNotFoundError, ValueError):
    cfg = {}
cfg.setdefault("mcpServers", {})
cfg["mcpServers"]["cctui"] = {
    "command": bun,
    "args": ["run", channel],
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

session_start_cmd = (
    'KEY="${CCTUI_AGENT_TOKEN:-$(jq -r .machine_key '
    '"${XDG_CONFIG_HOME:-$HOME/.config}/cctui/machine.json" 2>/dev/null)}"; '
    f'[ -z "$KEY" ] && KEY="{token}"; '
    'jq -c --arg ppid "$PPID" --arg mid "$(hostname)" '
    "'. + {ppid: ($ppid | tonumber), machine_id: $mid}' "
    f'| curl -sf -X POST {server_url}/api/v1/hooks/session-start '
    "-H 'Content-Type: application/json' "
    '-H "Authorization: Bearer $KEY" -d @-'
)

settings.setdefault("hooks", {})
settings["hooks"].update({
    "SessionStart": [{"hooks": [{"type": "command", "command": session_start_cmd}]}],
    "PreToolUse":   [{"hooks": [{"type": "http", "url": f"{server_url}/api/v1/check"}]}],
    "PostToolUse":  [{"hooks": [{"type": "http", "url": f"{server_url}/api/v1/hooks/post-tool-use"}]}],
    "Stop":         [{"hooks": [{"type": "http", "url": f"{server_url}/api/v1/hooks/stop"}]}],
})

with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)
print(f"[cctui] updated {settings_path}")
PY

log "done. binary: $BIN_DEST  channel: $CHANNEL_DEST  server: $CCTUI_URL"
case ":$PATH:" in
  *":$PREFIX:"*) : ;;
  *) warn "$PREFIX is not in \$PATH — add it to your shell profile" ;;
esac
