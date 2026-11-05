#!/usr/bin/env bash
# A dispatched worker exits non-zero, naming the launch error, only when its session never started.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

script="$work/exit.sh"
{
    echo 'log() { echo "$*" >> "$WORK/log"; }'
    echo 'RESULT_FILE="$WORK/result.json"'
    echo 'TERMINATION_LOG="$WORK/termination-log"'
    echo 'GUARD_STATE_FILE="$WORK/guard-state.json"'
    echo 'DISPATCH_DONE_MARKER="$WORK/dispatch_done"'
    echo '_SHORT=abcdef12'
    echo 'SESSION_ID=abcdef12-0000-4000-8000-000000000000'
    for fn in dispatch_done_marker guard_exited result_valid session_never_started dispatch_exit; do
        awk "/^${fn}\\(\\)/,/^}/" "$ENTRY"
    done
    echo 'dispatch_exit'
    echo 'echo "dispatch_exit returned" >&2; exit 99'
} > "$script"

run() {
    _case="$1"
    rm -rf "${work:?}/$_case"
    mkdir -p "$work/$_case"
    [ -n "${4:-}" ] && printf '%s' "$4" > "$work/$_case/result.json"
    [ -n "${5:-}" ] && : > "$work/$_case/dispatch_done"
    WORK="$work/$_case" _SEEN_ALIVE="$2" GUARD_ON="$3" timeout 10 sh "$script"
}

echo "== a refused launch fails the Job on its error =="
run refused 0 off '{"task_id":"t","status":"failed","error":"hook policy unavailable","cause":"launch-refused"}'
code=$?
[ "$code" = 1 ] && ok "exit 1" || bad "exit $code, want 1"
msg=$(cat "$work/refused/termination-log" 2>/dev/null)
[ "$msg" = "launch refused: hook policy unavailable" ] && ok "termination message names the error" \
    || bad "termination message: '$msg'"

echo "== a one-shot launch refusal fails too, though seen alive was pre-seeded =="
run oneshot 1 off '{"task_id":"t","status":"failed","error":"codex binary missing","cause":"launch-refused"}'
code=$?
[ "$code" = 1 ] && ok "exit 1" || bad "exit $code, want 1"

echo "== a daemon that died before registering the session fails the Job =="
run unregistered 0 off
code=$?
[ "$code" = 1 ] && ok "exit 1" || bad "exit $code, want 1"
grep -q 'launch refused: the daemon exited before registering the session' \
    "$work/unregistered/termination-log" 2>/dev/null && ok "termination message says so" \
    || bad "termination message: '$(cat "$work/unregistered/termination-log" 2>/dev/null)'"

echo "== a session that ran and failed its task still completes the Job =="
run ranfailed 1 off '{"task_id":"t","status":"failed","error":"tests failed"}'
code=$?
[ "$code" = 0 ] && ok "exit 0" || bad "exit $code, want 0"
[ ! -e "$work/ranfailed/termination-log" ] && ok "no termination message" \
    || bad "unexpected termination message"

echo "== a session that settled idle exits 0 =="
run settled 1 off '' marker
code=$?
[ "$code" = 0 ] && ok "exit 0" || bad "exit $code, want 0"

echo "== a session that wrote a success result exits 0 =="
run success 0 off '{"task_id":"t","status":"success"}'
code=$?
[ "$code" = 0 ] && ok "exit 0" || bad "exit $code, want 0"

exit $fail
