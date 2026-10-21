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
PRIVATE_ALLOW_FILE="$TMP/private-allow"
eval "$(grep '^GATEWAY_URL_KEYS=' "$ENTRY")"
for fn in url_hostport url_host gateway_url gateway_hostports private_allow_lines seed_policy; do
  eval "$(awk "/^${fn}\\(\\)/,/^}/" "$ENTRY")"
done

reset() {
  unset REPLY_URL CONTEXT_PACK_URL WORKER_NET_ALLOW TASK_PAYLOAD_JSON CCTUI_URL CCTUI_SERVER_URL \
        ANTHROPIC_BASE_URL OPENAI_BASE_URL FIREWORKS_BASE_URL GOOGLE_GEMINI_BASE_URL
  CCTUI_BASE_URL="http://cctui.cctui.svc:8700"
  : > "$POLICY_FILE"
  rm -f "$PRIVATE_ALLOW_FILE"
}
allowed() { jq -r '.allowed_hosts | sort | join(" ")' "$POLICY_FILE"; }
private_allow() { sort "$PRIVATE_ALLOW_FILE" | tr '\n' ',' | sed 's/,$//'; }

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

echo "== 6b. private allow-list: control-plane hosts only, labelled by source =="
reset
CCTUI_SERVER_URL="http://cctui.cctui.svc:8700"
export ANTHROPIC_BASE_URL="https://cctui.dorsk.dev/gateway/anthropic"
REPLY_URL="https://n8n.internal:5678/webhook/abc"
CONTEXT_PACK_URL="https://github.com/org/pack.git"
WORKER_NET_ALLOW="youtrack.example:443"
seed_policy
check "private-allow" "cctui.cctui.svc:8700 CCTUI_SERVER_URL,cctui.dorsk.dev:443 ANTHROPIC_BASE_URL,n8n.internal:5678 REPLY_URL" "$(private_allow)"
check "private-allow mode" "644" "$(stat -c %a "$PRIVATE_ALLOW_FILE")"

reset
TASK_PAYLOAD_JSON='{"env":{"OPENAI_BASE_URL":"http://gw.internal:8080/v1"}}'
seed_policy
check "private-allow payload gateway" "gw.internal:8080 OPENAI_BASE_URL" "$(private_allow)"

reset
seed_policy
check "private-allow empty without control-plane env" "" "$(private_allow)"

echo "== 7. url_hostport parses the authority only =="
check "empty" "" "$(url_hostport "")"
check "https default" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev")"
check "http default" "gw.internal:80" "$(url_hostport "http://gw.internal")"
check "explicit port" "gw.internal:8080" "$(url_hostport "http://gw.internal:8080")"
check "path" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev/gateway/anthropic")"
check "port + path" "pod.example:9443" "$(url_hostport "https://pod.example:9443/gateway/anthropic")"
check "query" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev/cb?x=1")"
check "userinfo" "github.com:443" "$(url_hostport "https://x-access-token:tok@github.com/org/pack.git")"
check "userinfo + port" "git.example:8443" "$(url_hostport "https://u:p@git.example:8443/r.git")"
check "unknown scheme keeps bare host" "host.example" "$(url_hostport "ssh://git@host.example/r")"
check "ipv6 literal with port" "[::1]:8443" "$(url_hostport "https://[::1]:8443/x")"
check "query @ is not userinfo" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev?x=@evil.com")"
check "fragment @ is not userinfo" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev#@evil.com")"
check "path @ is not userinfo" "cctui.dorsk.dev:443" "$(url_hostport "https://cctui.dorsk.dev/path@evil.com")"
check "last @ in authority wins" "host.example:443" "$(url_hostport "https://a@b@host.example/x")"
check "url_host strips port" "cctui.dorsk.dev" "$(url_host "https://u@cctui.dorsk.dev:8443/p?q=@evil")"

exit $fail
