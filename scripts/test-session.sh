#!/bin/sh
# Simulate a Claude session registering, streaming events, and deregistering.
# Usage: ./scripts/test-session.sh [server_url] [token]
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"

echo "==> Registering session..."
RESP=$(curl -sf \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "machine_id": "'"$(hostname)"'",
    "working_dir": "'"$(pwd)"'",
    "metadata": {"project_name": "cctui-test", "git_branch": "main"}
  }' \
  "$URL/api/v1/sessions/register")

SESSION_ID=$(echo "$RESP" | grep -o '"session_id":"[^"]*"' | cut -d'"' -f4)
WS_URL=$(echo "$RESP" | grep -o '"ws_url":"[^"]*"' | cut -d'"' -f4)

echo "    Session ID: $SESSION_ID"
echo "    WS URL: $WS_URL"

echo ""
echo "==> Listing sessions (admin)..."
curl -sf -H "Authorization: Bearer dev-admin" "$URL/api/v1/sessions" | python3 -m json.tool 2>/dev/null || \
  curl -sf -H "Authorization: Bearer dev-admin" "$URL/api/v1/sessions"

echo ""
echo "==> Streaming fake events via WebSocket (5 events)..."

# Check if websocat is available for WS testing
if command -v websocat > /dev/null 2>&1; then
  for i in 1 2 3 4 5; do
    echo '{"type":"text","content":"Agent is working on step '"$i"'...","ts":'"$(date +%s)"'}' | \
      websocat -n1 "$WS_URL" 2>/dev/null || echo "    (websocat send $i — WS may not be reachable without async client)"
    sleep 0.5
  done
  echo '{"type":"tool_call","tool":"Read","input":{"file_path":"src/main.rs"},"ts":'"$(date +%s)"'}' | \
    websocat -n1 "$WS_URL" 2>/dev/null || true
  echo '{"type":"heartbeat","tokens_in":1200,"tokens_out":400,"cost_usd":0.03,"ts":'"$(date +%s)"'}' | \
    websocat -n1 "$WS_URL" 2>/dev/null || true
else
  echo "    websocat not installed — skipping WS streaming test"
  echo "    Install: cargo install websocat"
fi

echo ""
echo "==> Fetching conversation..."
curl -sf -H "Authorization: Bearer dev-admin" "$URL/api/v1/sessions/$SESSION_ID/conversation" | python3 -m json.tool 2>/dev/null || \
  curl -sf -H "Authorization: Bearer dev-admin" "$URL/api/v1/sessions/$SESSION_ID/conversation"

echo ""
echo "==> Session is live. Check the TUI!"
echo "    Press Enter to deregister and clean up..."
read -r

echo "==> Deregistering..."
curl -sf -X POST -H "Authorization: Bearer $TOKEN" "$URL/api/v1/sessions/$SESSION_ID/deregister"
echo "    Done."
