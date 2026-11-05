#!/usr/bin/env bash
# Exercises mkdir_worker() from deploy/worker-entrypoint.sh: a root boot must
# not leave intermediate dirs under the worker home root-owned.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

eval "$(awk '/^mkdir_worker\(\)/,/^}/' "$ENTRY")"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
chowned="$tmp/chowned"
chown() { printf '%s %s\n' "$1" "$2" >> "$chowned"; }
WORKER_UID=1000
WORKER_USER=worker
mkdir -p "$tmp/home/worker"
# The real home is /home/worker; point the helper at the sandbox one.
eval "$(declare -f mkdir_worker | sed "s#/home/#$tmp/home/#")"

echo "== 1. root: every level under the home is chowned, the home is not =="
is_root() { return 0; }
mkdir_worker "$tmp/home/worker/.config/cctui/opencode/plugins"
check "dir created" "yes" "$([ -d "$tmp/home/worker/.config/cctui/opencode/plugins" ] && echo yes)"
check "chowned levels" "1000:1000 $tmp/home/worker/.config/cctui/opencode/plugins
1000:1000 $tmp/home/worker/.config/cctui/opencode
1000:1000 $tmp/home/worker/.config/cctui
1000:1000 $tmp/home/worker/.config" "$(cat "$chowned")"

echo "== 2. outside the home nothing is chowned =="
: > "$chowned"
mkdir_worker "$tmp/elsewhere/x"
check "no chown" "" "$(cat "$chowned")"

echo "== 3. non-root: plain mkdir -p =="
is_root() { return 1; }
mkdir_worker "$tmp/home/worker/.claude/skills"
check "dir created" "yes" "$([ -d "$tmp/home/worker/.claude/skills" ] && echo yes)"
check "no chown" "" "$(cat "$chowned")"

echo "== 4. root-run callers under the home use it =="
check "guard shims" "1" "$(grep -c 'mkdir_worker "\$_ocdir/plugins"' "$ENTRY")"
check "context pack dirs" "1" "$(grep -c 'mkdir_worker "\${_home}/\${_tgt}"' "$ENTRY")"

[ "$fail" -eq 0 ] || exit 1
echo "all mkdir-worker cases passed"
