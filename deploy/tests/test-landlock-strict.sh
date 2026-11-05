#!/usr/bin/env bash
# Exercises the Landlock strict wiring in deploy/worker-entrypoint.sh.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }

eval "$(grep '^landlock_strict_on()' "$ENTRY")"
for f in landlock_strict_flags phase_landlock; do
  eval "$(awk "/^$f\\(\\)/,/^}/" "$ENTRY")"
done
for f in landlock_strict_on landlock_strict_flags phase_landlock; do
  type "$f" >/dev/null 2>&1 || { bad "$f not found in $ENTRY"; exit 1; }
done

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
LOG="$TMP/log"
log() { echo "cctui-worker: $*" >> "$LOG"; }

echo "== 1. strict by default =="
unset CCTUI_LANDLOCK_STRICT
check "unset -> --strict" "--strict" "$(landlock_strict_flags)"
: > "$LOG"; phase_landlock
check "nothing logged" "" "$(cat "$LOG")"

echo "== 2. only an exact 0 opts out =="
for v in 1 "" false off no 00 " 0"; do
  CCTUI_LANDLOCK_STRICT=$v
  check "CCTUI_LANDLOCK_STRICT=[$v] stays strict" "--strict" "$(landlock_strict_flags)"
done
CCTUI_LANDLOCK_STRICT=0
check "0 -> no --strict" "" "$(landlock_strict_flags)"
: > "$LOG"; phase_landlock
has "opt-out logged loudly" "WARNING: CCTUI_LANDLOCK_STRICT=0: Landlock is best-effort" "$(cat "$LOG")"
unset CCTUI_LANDLOCK_STRICT

echo "== 3. the supervisor invocation and phase order carry it =="
sup=$(awk '/^run_supervised_daemon\(\)/,/^}/' "$ENTRY")
has "supervisor gets the strict flags" '$(landlock_strict_flags)' "$sup"
grep -qx 'phase_landlock' "$ENTRY" && ok "phase_landlock runs" || bad "phase_landlock is never called"

echo "== 4. a dispatch env cannot set the opt-out =="
FNS=$(awk '/^dispatch_identity_failed\(\)/,/^}/; /^phase_dispatch_env\(\)/,/^}/' "$ENTRY")
printf '%s\n' "$FNS" > "$TMP/fns.sh"
mkdir -p "$TMP/bin"
cat > "$TMP/bin/curl" <<'EOF'
#!/bin/sh
printf '%s\n200' '{"env":{"CCTUI_LANDLOCK_STRICT":"0"}}'
EOF
chmod +x "$TMP/bin/curl"
out=$(PATH="$TMP/bin:$PATH" CCTUI_DISPATCH_ID=d-1 TERMINATION_LOG="$TMP/termination-log" bash -c '
    set -eu
    log() { echo "cctui-worker: $*"; }
    DISPATCH_ENV_URL=http://127.0.0.1:15002/dispatch/env
    trap "echo CCTUI_LANDLOCK_STRICT=\${CCTUI_LANDLOCK_STRICT:-unset}" EXIT
    . "$0"
    phase_dispatch_env
' "$TMP/fns.sh" 2>&1); rc=$?
check "rc 1" 1 "$rc"
has "opt-out not exported" "CCTUI_LANDLOCK_STRICT=unset" "$out"
has "names the reserved key" "reserved env name: CCTUI_LANDLOCK_STRICT" "$(cat "$TMP/termination-log" 2>/dev/null)"

exit $fail
