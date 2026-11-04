#!/usr/bin/env sh
# Exercises worker_on_exit() from deploy/worker-entrypoint.sh: the payload the
# exit trap POSTs for a valid RESULT_FILE, an idle-ended turn, and a bare exit.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

FNS=$(awk '/^dispatch_done_marker\(\) \{/,/^\}/; /^worker_on_exit\(\) \{/,/^\}/' "$ENTRY")
[ -n "$FNS" ] || { echo "functions not found in $ENTRY"; exit 1; }
eval "$FNS"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
send_callback() { printf '%s' "$1" > "$TMP/posted"; }
TASK_ID=t1
_SHORT=t1
RESULT_FILE="$TMP/result.json"
DISPATCH_DONE_MARKER="$TMP/dispatch_done"

# $1 = exit code the trap observes.
on_exit() { rm -f "$TMP/posted"; (exit "$1"); worker_on_exit; jq -c . "$TMP/posted"; }

echo '{"task_id":"t1","status":"success"}' > "$RESULT_FILE"
touch "$DISPATCH_DONE_MARKER"
check "valid result is posted as written" '{"task_id":"t1","status":"success"}' "$(on_exit 0)"

: > "$RESULT_FILE"
check "idle turn without a result carries the cause" \
    '{"task_id":"t1","status":"failed","cause":"idle-without-result","error":"session ended its turn without writing a result"}' \
    "$(on_exit 0)"

echo 'not json' > "$RESULT_FILE"
check "invalid result after an idle turn carries the cause" idle-without-result "$(on_exit 0 | jq -r .cause)"

rm -f "$RESULT_FILE" "$DISPATCH_DONE_MARKER"
check "exit without the marker stays a plain failure" \
    '{"task_id":"t1","status":"failed","error":"worker exited (code 3) without a valid result"}' \
    "$(on_exit 3)"

exit "$fail"
