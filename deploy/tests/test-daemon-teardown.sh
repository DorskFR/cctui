#!/usr/bin/env bash
# Exercises the dispatched-worker teardown: SIGTERM must reach the daemon itself.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat > "$work/bin/cctui-supervisor" <<'EOF'
#!/bin/sh
trap 'sleep 1; echo flushed > "$TEARDOWN_MARK"; exit 0' TERM
while :; do sleep 0.1; done
EOF
chmod +x "$work/bin/cctui-supervisor"

script="$work/run.sh"
{
    echo 'log() { echo "$*" >> "$TEARDOWN_LOG"; }'
    echo 'extra_ro_flags() { :; }'
    echo 'extra_rw_flags() { :; }'
    echo 'WORKER_TEARDOWN_GRACE_SECS=5'
    awk '/^run_supervised_daemon\(\)/,/^}/' "$ENTRY"
    awk '/^stop_daemon_gracefully\(\)/,/^}/' "$ENTRY"
    echo 'run_supervised_daemon &'
    echo 'DAEMON_PID=$!'
    echo 'sleep 0.5'
    echo 'stop_daemon_gracefully'
} > "$script"

for sh in sh bash; do
    echo "== $sh: SIGTERM reaches the daemon and teardown waits for its flush =="
    export TEARDOWN_MARK="$work/mark.$sh" TEARDOWN_LOG="$work/log.$sh"
    PATH="$work/bin:$PATH" timeout 20 "$sh" "$script"
    if [ "$(cat "$TEARDOWN_MARK" 2>/dev/null)" = flushed ]; then
        ok "daemon got SIGTERM and flushed"
    else
        bad "daemon never saw SIGTERM"
    fi
    if grep -q 'teardown: daemon flushed and exited' "$TEARDOWN_LOG" 2>/dev/null; then
        ok "teardown logged a clean exit"
    else
        bad "teardown log: $(cat "$TEARDOWN_LOG" 2>/dev/null)"
    fi
    pkill -f "$work/bin/cctui-supervisor" 2>/dev/null || true
done

exit $fail
