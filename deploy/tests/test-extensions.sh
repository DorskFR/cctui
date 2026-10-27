#!/usr/bin/env bash
# Exercises phase_extensions() from deploy/worker-entrypoint.sh under dash with
# the entrypoint's `set -eu`: a failing hook is logged by name and the
# entrypoint continues, and hook exports stay visible to later phases.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }

command -v dash >/dev/null 2>&1 || { echo "dash not installed"; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
awk '/^phase_extensions\(\)/,/^}/' "$ENTRY" > "$TMP/phase.sh"
grep -q '^phase_extensions()' "$TMP/phase.sh" || { bad "phase_extensions not found in $ENTRY"; exit 1; }

EXT="$TMP/ext"
mkdir -p "$EXT"

run() {
  dash -c '
    set -eu
    log() { echo "cctui-worker: $*"; }
    EXT_DIR=$1
    . "$2"
    phase_extensions
    echo "after: HOOK_A=${HOOK_A:-} HOOK_C=${HOOK_C:-} PATH_HAS_HOOK=$(case ":$PATH:" in *:/opt/hook/bin:*) echo yes ;; *) echo no ;; esac)"
    echo "errexit: $(case $- in *e*) echo on ;; *) echo off ;; esac) nounset: $(case $- in *u*) echo on ;; *) echo off ;; esac)"
    echo "exported: $(sh -c "printf %s \"\${HOOK_A:-}\"")"
  ' run "$EXT" "$TMP/phase.sh" 2>&1
  echo "rc=$?"
}

echo "== 1. a command failing mid-hook does not abort =="
cat > "$EXT/10-first.sh" <<'EOF'
export HOOK_A=one
export PATH="/opt/hook/bin:$PATH"
EOF
cat > "$EXT/20-broken.sh" <<'EOF'
false
export HOOK_B=two
EOF
cat > "$EXT/30-fails.sh" <<'EOF'
export HOOK_C=three
return 3
EOF
out=$(run)
has "continued to the end" "rc=0" "$out"
has "every hook sourced" "ext: sourcing 30-fails.sh" "$out"
has "failing hook named" "WARNING: extension 30-fails.sh failed (rc 3, continuing)" "$out"
has "exports visible" "after: HOOK_A=one HOOK_C=three PATH_HAS_HOOK=yes" "$out"
has "exports reach children" "exported: one" "$out"
has "options restored" "errexit: on nounset: on" "$out"

echo "== 2. an unset variable in a hook does not abort =="
rm -f "$EXT"/*.sh
cat > "$EXT/10-unset.sh" <<'EOF'
echo "value=$NOT_SET_ANYWHERE"
export HOOK_A=after-unset
EOF
out=$(run)
has "continued" "rc=0" "$out"
has "export after the unset reference" "HOOK_A=after-unset" "$out"

echo "== 3. no hooks =="
rm -f "$EXT"/*.sh
out=$(run)
has "no-op" "rc=0" "$out"
check "nothing sourced" "" "$(printf '%s\n' "$out" | grep 'ext: sourcing')"

exit $fail
