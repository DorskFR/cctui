#!/usr/bin/env bash
# Exercises default_bg_isolation() from deploy/worker-entrypoint.sh: workers
# default to no worktree isolation, and a profile/payload value wins.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

eval "$(awk '/^default_bg_isolation\(\)/,/^}/' "$ENTRY")"
type default_bg_isolation >/dev/null 2>&1 || { bad "default_bg_isolation not found in $ENTRY"; exit 1; }

echo "== 1. unset defaults to none, exported =="
unset CLAUDE_BG_ISOLATION
default_bg_isolation
check "value"    "none" "${CLAUDE_BG_ISOLATION:-}"
check "exported" "none" "$(bash -c 'printf %s "${CLAUDE_BG_ISOLATION:-}"')"

echo "== 2. profile value wins =="
export CLAUDE_BG_ISOLATION=worktree
default_bg_isolation
check "value"    "worktree" "${CLAUDE_BG_ISOLATION:-}"
check "exported" "worktree" "$(bash -c 'printf %s "${CLAUDE_BG_ISOLATION:-}"')"

echo "== 3. empty is treated as unset =="
export CLAUDE_BG_ISOLATION=
default_bg_isolation
check "value" "none" "${CLAUDE_BG_ISOLATION:-}"

echo "== 4. it runs before the daemon is launched =="
_def=$(grep -n '^default_bg_isolation$' "$ENTRY" | head -1 | cut -d: -f1)
_run=$(grep -n '^run_supervised_daemon()' "$ENTRY" | head -1 | cut -d: -f1)
if [ -n "$_def" ] && [ -n "$_run" ] && [ "$_def" -lt "$_run" ]; then
  ok "called at line $_def, before the daemon launch at $_run"
else
  bad "default_bg_isolation call ($_def) must precede run_supervised_daemon ($_run)"
fi

exit $fail
