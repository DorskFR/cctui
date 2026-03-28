#!/bin/sh
# Configure Claude Code to use the cctui-channel MCP server.
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "==> Installing channel dependencies..."
(cd "$REPO_DIR/channel" && bun install --silent)

echo "==> Configuring Claude Code hooks and MCP server..."
CCTUI_CHANNEL_DIR="$REPO_DIR/channel" \
  __SERVER_URL__="$URL" \
  __TOKEN__="$TOKEN" \
  python3 "$SCRIPT_DIR/setup.py.tpl"

echo ""
echo "==> Done! Next time you start Claude Code, the cctui-channel will activate."
echo "    Server: $URL"
