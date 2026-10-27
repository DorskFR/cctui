#!/usr/bin/env bash
# Exercises repo_hostport() + seed_policy() from deploy/worker-entrypoint.sh.
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
for fn in url_hostport repo_hostport url_host gateway_url gateway_hostports private_allow_lines seed_policy; do
  eval "$(awk "/^${fn}\\(\\)/,/^}/" "$ENTRY")"
done

echo "== 1. repo_hostport =="
check "https default port" "github.com:443" "$(repo_hostport "https://github.com/acme/widgets")"
check "https .git suffix" "github.com:443" "$(repo_hostport "https://github.com/acme/widgets.git")"
check "explicit port" "git.example:8443" "$(repo_hostport "https://git.example:8443/acme/widgets.git")"
check "token userinfo dropped" "github.com:443" "$(repo_hostport "https://x-access-token:tok@github.com/acme/widgets")"
check "http default port" "git.internal:80" "$(repo_hostport "http://git.internal/acme/widgets")"
check "ssh:// skipped" "" "$(repo_hostport "ssh://git@github.com/acme/widgets.git")"
check "scp-style skipped" "" "$(repo_hostport "git@github.com:acme/widgets.git")"
check "file:// skipped" "" "$(repo_hostport "file:///srv/widgets.git")"
check "empty" "" "$(repo_hostport "")"

reset() {
  unset REPLY_URL CONTEXT_PACK_URL TASK_REPO_URL WORKER_NET_ALLOW TASK_PAYLOAD_JSON CCTUI_URL \
        CCTUI_SERVER_URL ANTHROPIC_BASE_URL OPENAI_BASE_URL FIREWORKS_BASE_URL GOOGLE_GEMINI_BASE_URL
  CCTUI_BASE_URL="http://cctui.cctui.svc:8700"
  : > "$POLICY_FILE"
}
allowed() { jq -r '.allowed_hosts | sort | join(" ")' "$POLICY_FILE"; }

echo "== 2. the dispatch's repo host joins the boot policy =="
reset
TASK_REPO_URL="https://github.com/acme/widgets"
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 github.com:443" "$(allowed)"
check "default deny" "deny" "$(jq -r .default "$POLICY_FILE")"

echo "== 3. deduped against WORKER_NET_ALLOW =="
reset
TASK_REPO_URL="https://github.com/acme/widgets"
WORKER_NET_ALLOW="github.com:443"
seed_policy
check "allowed_hosts" "cctui.cctui.svc:8700 github.com:443" "$(allowed)"

echo "== 4. an ssh repo or none adds nothing =="
reset
TASK_REPO_URL="git@github.com:acme/widgets.git"
seed_policy
check "scp-style" "cctui.cctui.svc:8700" "$(allowed)"
reset
seed_policy
check "no TASK_REPO_URL" "cctui.cctui.svc:8700" "$(allowed)"

exit $fail
