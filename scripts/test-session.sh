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

if ! command -v websocat > /dev/null 2>&1; then
  echo "websocat not installed — install with: cargo install websocat"
  exit 1
fi

echo "==> Session registered! Select it in the TUI (j/k), then press Enter here to stream events..."
read -r _

# Use a single long-lived WS connection with a FIFO for sending
FIFO=$(mktemp -u)
mkfifo "$FIFO"

# Keep the connection open by tailing the FIFO (blocks until we write to it and close)
tail -f "$FIFO" | websocat "$WS_URL" &
WS_PID=$!
sleep 0.5

echo "==> Streaming events (watch the TUI detail pane)..."

send() {
  echo "$1" > "$FIFO"
  sleep 0.3
}

send '{"type":"text","content":"I'\''ll start by reading the project structure...","ts":'"$(date +%s)"'}'
send '{"type":"tool_call","tool":"Read","input":{"file_path":"src/main.rs"},"ts":'"$(date +%s)"'}'
send '{"type":"tool_result","tool":"Read","output_summary":"58 lines","ts":'"$(date +%s)"'}'
send '{"type":"text","content":"The main entry point sets up an Axum server with session registry...","ts":'"$(date +%s)"'}'
send '{"type":"tool_call","tool":"Bash","input":{"command":"cargo test --workspace"},"ts":'"$(date +%s)"'}'
send '{"type":"tool_result","tool":"Bash","output_summary":"10 passed, 0 failed","ts":'"$(date +%s)"'}'
send '{"type":"text","content":"All tests pass. The implementation looks correct.","ts":'"$(date +%s)"'}'
send '{"type":"heartbeat","tokens_in":2400,"tokens_out":800,"cost_usd":0.06,"ts":'"$(date +%s)"'}'

echo ""
echo "==> Events sent! You should see them in the TUI now."
echo "    Press Enter to deregister and clean up..."
read -r _

# Clean up WS connection
kill "$WS_PID" 2>/dev/null || true
rm -f "$FIFO"

echo "==> Deregistering..."
curl -sf -X POST -H "Authorization: Bearer $TOKEN" "$URL/api/v1/sessions/$SESSION_ID/deregister"
echo "    Done."
