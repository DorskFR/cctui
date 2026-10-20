#!/usr/bin/env bash
# Exercises seed_policy() + gateway_hostports() from deploy/worker-entrypoint.sh:
# the boot guard-proxy policy allows the host of every minted model-gateway base
# URL (pod env first, payload env as fallback), deduped, and nothing else.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

log() { :; }
POLICY_FILE="$TMP/policy.json"
for fn in url_hostport url_host gateway_hostports seed_policy; do
  eval "$(awk "/^${fn}\\(\\)/,/^}/" "$ENTRY")"
done

reset() {
  unset REPLY_URL CONTEXT_PACK_URL WORKER_NET_ALLOW TASK_PAYLOAD_JSON \
        ANTHROPIC_BASE_URL OPENAI_BASE_URL FIREWORKS_BASE_URL GOOGLE_GEMINI_BASE_URL
  CCTUI_BASE_URL="http://cctui.cctui.svc:8700"
  : > "$POLICY_FILE"
}
allowed() { jq -r '.allowed_hosts | sort | join(" ")' "$POLICY_FILE"; }

echo "== 1. no gateway env: server + reply hosts only =="
reset
REPLY_URL="https://cctui.example.com/cb/abc"
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 cctui.example.com:443" "$(allowed)"
check "default deny" "deny" "$(jq -r .default "$POLICY_FILE")"
check "gateway_hostports empty" "" "$(gateway_hostports)"

echo "== 2. minted gateway host from pod env is allowed, deduped across families =="
reset
export ANTHROPIC_BASE_URL="https://cctui.dorsk.dev/gateway/anthropic"
export OPENAI_BASE_URL="https://cctui.dorsk.dev/gateway/openai"
export FIREWORKS_BASE_URL="https://cctui.dorsk.dev/gateway/fireworks"
export GOOGLE_GEMINI_BASE_URL="https://cctui.dorsk.dev/gateway/google"
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 cctui.dorsk.dev:443" "$(allowed)"
check "gateway_hostports deduped" "cctui.dorsk.dev:443" "$(gateway_hostports)"

echo "== 3. distinct gateway hosts each allowed; explicit port kept =="
reset
export ANTHROPIC_BASE_URL="https://cctui.dorsk.dev/gateway/anthropic"
export OPENAI_BASE_URL="http://gw.internal:8080/gateway/openai"
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 cctui.dorsk.dev:443 gw.internal:8080" "$(allowed)"

echo "== 4. payload env is the fallback when pod env lacks the var =="
reset
TASK_PAYLOAD_JSON='{"env":{"ANTHROPIC_BASE_URL":"https://cctui.dorsk.dev/gateway/anthropic","OTHER_URL":"https://evil.example"}}'
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 cctui.dorsk.dev:443" "$(allowed)"

echo "== 5. pod env wins over payload env =="
reset
export ANTHROPIC_BASE_URL="https://pod.example:9443/gateway/anthropic"
TASK_PAYLOAD_JSON='{"env":{"ANTHROPIC_BASE_URL":"https://payload.example/gateway/anthropic"}}'
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 pod.example:9443" "$(allowed)"

echo "== 6. gateway host joins the existing structural + operator hosts =="
reset
export ANTHROPIC_BASE_URL="https://cctui.dorsk.dev/gateway/anthropic"
REPLY_URL="https://cctui.dorsk.dev/cb/abc"
CONTEXT_PACK_URL="https://github.com/org/pack.git"
WORKER_NET_ALLOW=" youtrack.example:443 , api.github.com:443 "
seed_policy
check "allowed_hosts" "api.github.com:443 cctui.cctui.svc:8700 cctui.dorsk.dev:443 github.com:443 youtrack.example:443" "$(allowed)"

exit $fail
