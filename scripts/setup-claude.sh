#!/bin/sh
# Configure a Claude Code instance to auto-register with the cctui server.
# Run this on any machine where you want Claude to phone home.
#
# Usage: ./scripts/setup-claude.sh [server_url] [token]
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"

echo "==> Configuring Claude Code hooks from $URL..."
curl -sf -H "Authorization: Bearer $TOKEN" "$URL/api/v1/setup" | python3

echo ""
echo "==> Done! Next time you start Claude Code, it will auto-register."
echo "    Server: $URL"
echo "    Token: $TOKEN"
