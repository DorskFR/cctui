#!/usr/bin/env bash
# Exercises install_guard_shims() from deploy/worker-entrypoint.sh: with a live
# guard the OpenCode plugin lands in OPENCODE_CONFIG_DIR and the Codex shim is
# registered once in $CODEX_HOME/hooks.json; without one nothing is installed.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
ENTRY="${1:-$here/../worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
log()  { :; }

eval "$(awk '/^install_guard_shims\(\)/,/^}/' "$ENTRY")"
type install_guard_shims >/dev/null 2>&1 || { echo "FAIL install_guard_shims not found"; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
WORKER_USER=worker
WORKER_UID="$(id -u)"
GUARD_SHIMS_DIR="$(cd "$here/../guard-shims" && pwd)"
shim_cmd="$GUARD_SHIMS_DIR/codex/pretool.sh"
count_cmd() { jq --arg c "$shim_cmd" '[.hooks.PreToolUse[].hooks[] | select(.command == $c)] | length' "$1"; }

echo "== 1. guard off installs nothing =="
(GUARD_ON=off; CODEX_HOME="$tmp/c1"; OPENCODE_CONFIG_DIR="$tmp/o1"; install_guard_shims)
check "no codex dir" no "$([ -e "$tmp/c1" ] && echo yes || echo no)"
check "no opencode dir" no "$([ -e "$tmp/o1" ] && echo yes || echo no)"

echo "== 2. guard on installs both shims =="
got_dir=$(GUARD_ON=on; CODEX_HOME="$tmp/c2"; OPENCODE_CONFIG_DIR="$tmp/o2"
    install_guard_shims; printf '%s' "$OPENCODE_CONFIG_DIR")
check "OPENCODE_CONFIG_DIR exported" "$tmp/o2" "$got_dir"
check "plugin copied" same \
    "$(cmp -s "$GUARD_SHIMS_DIR/opencode/plugins/cctui-guard.js" "$tmp/o2/plugins/cctui-guard.js" && echo same)"
check "codex shim registered once" 1 "$(count_cmd "$tmp/c2/hooks.json")"
check "shim is executable" yes "$([ -x "$shim_cmd" ] && echo yes)"

echo "== 3. re-run is idempotent =="
(GUARD_ON=on; CODEX_HOME="$tmp/c2"; OPENCODE_CONFIG_DIR="$tmp/o2"; install_guard_shims)
check "still one entry" 1 "$(count_cmd "$tmp/c2/hooks.json")"

echo "== 4. existing codex hooks are kept =="
mkdir -p "$tmp/c4"
printf '%s\n' '{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x/own.sh"}]}],"Stop":[{"hooks":[{"type":"command","command":"/x/stop.sh"}]}]}}' \
    > "$tmp/c4/hooks.json"
(GUARD_ON=on; CODEX_HOME="$tmp/c4"; OPENCODE_CONFIG_DIR="$tmp/o4"; install_guard_shims)
check "own PreToolUse kept" "/x/own.sh" "$(jq -r '.hooks.PreToolUse[0].hooks[0].command' "$tmp/c4/hooks.json")"
check "Stop kept" "/x/stop.sh" "$(jq -r '.hooks.Stop[0].hooks[0].command' "$tmp/c4/hooks.json")"
check "shim appended" 1 "$(count_cmd "$tmp/c4/hooks.json")"

echo "== 5. invalid hooks.json is left untouched =="
mkdir -p "$tmp/c5"
printf 'not json' > "$tmp/c5/hooks.json"
(GUARD_ON=on; CODEX_HOME="$tmp/c5"; OPENCODE_CONFIG_DIR="$tmp/o5"; install_guard_shims)
check "file unchanged" "not json" "$(cat "$tmp/c5/hooks.json")"

echo "== 6. missing shims dir is a no-op =="
(GUARD_ON=on; GUARD_SHIMS_DIR="$tmp/none"; CODEX_HOME="$tmp/c6"; OPENCODE_CONFIG_DIR="$tmp/o6"; install_guard_shims)
check "no codex dir" no "$([ -e "$tmp/c6" ] && echo yes || echo no)"
check "no opencode dir" no "$([ -e "$tmp/o6" ] && echo yes || echo no)"

[ "$fail" -eq 0 ] || exit 1
echo "all guard-shims install cases passed"
