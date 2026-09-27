#!/usr/bin/env bash
# Exercises derive_task_env() from deploy/worker-entrypoint.sh: payload fields
# map to TASK_* env, and an explicit env value wins over the payload.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

eval "$(awk '/^derive_task_env\(\)/,/^}/' "$ENTRY")"

reset() {
  unset TASK_IDENTITY TASK_REPO TASK_EFFORT TASK_MODEL TASK_ADAPTER TASK_CODEX_MODEL \
        TASK_CONTEXT_JSON TASK_REPO_URL TASK_REPO_REF TASK_PAYLOAD_JSON
}

echo "== 1. payload fields map to TASK_* =="
reset
TASK_PAYLOAD_JSON='{"repo":"cryptact","model":"opus","effort":"low","codex_model":"gpt-6-astra","context":{"owner":"Cryptact","head_sha":"abc123"}}'
derive_task_env
check "TASK_CODEX_MODEL" "gpt-6-astra" "${TASK_CODEX_MODEL:-}"
check "TASK_MODEL"       "opus"        "${TASK_MODEL:-}"
check "TASK_REPO_URL"    "https://github.com/Cryptact/cryptact" "${TASK_REPO_URL:-}"
check "TASK_REPO_REF"    "abc123"      "${TASK_REPO_REF:-}"

echo "== 2. explicit env wins =="
reset
export TASK_CODEX_MODEL=gpt-5.6-terra
TASK_PAYLOAD_JSON='{"codex_model":"gpt-6-astra"}'
derive_task_env
check "TASK_CODEX_MODEL kept" "gpt-5.6-terra" "${TASK_CODEX_MODEL:-}"

echo "== 3. no codex_model leaves it unset =="
reset
TASK_PAYLOAD_JSON='{"model":"opus"}'
derive_task_env
check "TASK_CODEX_MODEL unset" "" "${TASK_CODEX_MODEL:-}"

echo "== 4. no payload is a no-op =="
reset
derive_task_env
check "rc 0, nothing set" "" "${TASK_MODEL:-}${TASK_CODEX_MODEL:-}"

exit $fail
