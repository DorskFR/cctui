#!/bin/sh
# Configure Claude Code to use the cctui MCP channel (dev / local setup).
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

CCTUI_BIN="${CCTUI_BIN:-$REPO_DIR/target/debug/cctui}"
if [ ! -x "$CCTUI_BIN" ]; then
  CCTUI_BIN_RELEASE="$REPO_DIR/target/release/cctui"
  if [ -x "$CCTUI_BIN_RELEASE" ]; then
    CCTUI_BIN="$CCTUI_BIN_RELEASE"
  else
    echo "[cctui] ERROR: cctui binary not found at $CCTUI_BIN"
    echo "        Build it first with: cargo build -p cctui-tui"
    exit 1
  fi
fi

echo "==> Configuring Claude Code hooks and MCP server..."
CCTUI_BIN="$CCTUI_BIN" \
  __SERVER_URL__="$URL" \
  __TOKEN__="$TOKEN" \
  python3 "$SCRIPT_DIR/setup.py.tpl"

echo ""
echo "==> Done! Next time you start Claude Code, the cctui channel will activate."
echo "    Server: $URL"
