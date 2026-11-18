#!/usr/bin/env bash
# Exercises the external-pod guard path and the periodic egress re-check of
# deploy/worker-entrypoint.sh.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }
hasnt(){ case "$3" in *"$2"*) bad "$1: [$2] found" ;; *) ok "$1" ;; esac; }

FNS=$(awk '
    /^external_guard_failed\(\)/,/^}/; /^guard_curl\(\)/,/^}/; /^external_guard_wait\(\)/,/^}/;
    /^dispatch_identity_failed\(\)/,/^}/; /^phase_dispatch_env\(\)/,/^}/;
    /^guard_put\(\)/,/^}/; /^guard_policy_sync\(\)/,/^}/; /^phase_external_guard\(\)/,/^}/;
    /^redact_url\(\)/,/^}/; /^egress_breaches\(\)/,/^}/; /^egress_recheck\(\)/,/^}/
' "$ENTRY")
for fn in phase_external_guard guard_policy_sync egress_recheck redact_url; do
    case "$FNS" in *"$fn()"*) ;; *) echo "$fn not found in $ENTRY"; exit 1 ;; esac
done

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/auth" "$TMP/policy" "$TMP/ca"
PROXY_TOKEN=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
CONTROL_TOKEN=fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210
printf '%s' "$PROXY_TOKEN" > "$TMP/auth/proxy-token"
printf '%s' "$CONTROL_TOKEN" > "$TMP/auth/control-token"

cat > "$TMP/bin/curl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_DIR/curl.argv"
for a in "$@"; do
    if [ "$a" = "-K" ]; then cat >> "$FAKE_DIR/curl.stdin"; break; fi
done
url=""; out=""; prev=""
for a in "$@"; do
    [ "$prev" = "-o" ] && out=$a
    case "$a" in http://*) url=$a ;; esac
    prev=$a
done
case "$url" in
    */health) [ "${FAKE_GUARD:-up}" = up ] || exit 7 ;;
    */dispatch/env) printf '%s\n200' '{"env":{"FEATURE_FLAG":"on"}}' ;;
    */policy | */private-allow)
        [ "${FAKE_PUT:-ok}" = ok ] || exit 22
        echo "$url" >> "$FAKE_DIR/puts" ;;
    */ca.pem)
        [ "${FAKE_CA:-yes}" = yes ] || exit 22
        printf 'GUARD-CA\n' > "$out" ;;
esac
exit 0
EOF
chmod +x "$TMP/bin/curl"
printf '%s\n' "$FNS" > "$TMP/fns.sh"

reset() {
    rm -f "$TMP/curl.argv" "$TMP/curl.stdin" "$TMP/puts" "$TMP/termination-log" "$TMP/result.json"
    rm -f "$TMP/ca/ca.pem"
    printf '{"allowed_hosts":["a.test:443"],"default":"deny"}\n' > "$TMP/policy/policy.json"
    printf 'cctui.cctui.svc:8700 CCTUI_URL\n' > "$TMP/policy/private-allow"
}

run() {
    PATH="$TMP/bin:$PATH" FAKE_DIR="$TMP" TERMINATION_LOG="$TMP/termination-log" \
        GUARD_AUTH_DIR="$TMP/auth" POLICY_FILE="$TMP/policy/policy.json" \
        PRIVATE_ALLOW_FILE="$TMP/policy/private-allow" GUARD_CA_FILE="$TMP/ca/ca.pem" \
        RESULT_FILE="$TMP/result.json" bash -c '
        set -eu
        log() { echo "cctui-worker: $*"; }
        install_guard_ca() { echo "CA installed from $GUARD_CA_FILE"; }
        PROXY_PORT=15001
        PROXY_HEALTH_PORT=15002
        GUARD_GATEWAY_PORT=15003
        . "$0"
        guard_policy_sync() { :; }
        eval "$1"
    ' "$TMP/fns.sh" "$1" 2>&1
}

echo "== 1. no guard host or no tokens: the pod fails with the reason =="
reset
out=$(run 'external_guard_wait'); rc=$?
check "rc 1" 1 "$rc"
has "names the missing host" "GUARD_PROXY_HOST is unset" "$(cat "$TMP/termination-log")"
reset
out=$(GUARD_PROXY_HOST=g run 'GUARD_AUTH_DIR=/nonexistent; external_guard_wait'); rc=$?
check "rc 1 without tokens" 1 "$rc"
has "names the missing token" "no proxy-token" "$(cat "$TMP/termination-log")"

echo "== 2. a guard that never comes up fails the pod =="
reset
out=$(GUARD_PROXY_HOST=g FAKE_GUARD=down WORKER_GUARD_WAIT_SECS=1 run 'external_guard_wait'); rc=$?
check "rc 1" 1 "$rc"
has "says it is not up" "g is not up after 1s" "$(cat "$TMP/termination-log")"

echo "== 3. dispatch env comes from the guard pod with the proxy token on stdin =="
reset
out=$(GUARD_PROXY_HOST=cctui-worker-abc-guard WORKER_NET_MODE=external-pod CCTUI_DISPATCH_ID=d-1 \
    run 'phase_dispatch_env; echo "FEATURE_FLAG=${FEATURE_FLAG:-}"'); rc=$?
check "rc 0" 0 "$rc"
has "env exported" "FEATURE_FLAG=on" "$out"
has "asked the guard pod" "http://cctui-worker-abc-guard:15002/dispatch/env" "$(cat "$TMP/curl.argv")"
has "bearer proxy token sent" "Authorization: Bearer $PROXY_TOKEN" "$(cat "$TMP/curl.stdin")"
hasnt "token not in argv" "$PROXY_TOKEN" "$(cat "$TMP/curl.argv")"

echo "== 4. the guard pod is steered, trusted and routed through =="
reset
out=$(GUARD_PROXY_HOST=cctui-worker-abc-guard run '
    phase_external_guard
    echo "HTTPS_PROXY=$HTTPS_PROXY"
    echo "NO_PROXY=$NO_PROXY"
    echo "CCTUI_URL=$CCTUI_URL"
    echo "BASE=$CCTUI_BASE_URL"
'); rc=$?
check "rc 0" 0 "$rc"
check "policy then private-allow pushed" \
    "http://cctui-worker-abc-guard:15002/policy http://cctui-worker-abc-guard:15002/private-allow" \
    "$(tr '\n' ' ' < "$TMP/puts" | sed 's/ $//')"
has "control token on stdin" "Authorization: Bearer $CONTROL_TOKEN" "$(cat "$TMP/curl.stdin")"
hasnt "control token not in argv" "$CONTROL_TOKEN" "$(cat "$TMP/curl.argv")"
check "CA fetched from /ca.pem" "GUARD-CA" "$(cat "$TMP/ca/ca.pem")"
has "CA installed" "CA installed from $TMP/ca/ca.pem" "$out"
has "proxy env carries the proxy token" \
    "HTTPS_PROXY=http://cctui:${PROXY_TOKEN}@cctui-worker-abc-guard:15001" "$out"
has "guard host bypasses the proxy" "NO_PROXY=127.0.0.1,localhost,cctui-worker-abc-guard" "$out"
has "daemon dials the gateway" "CCTUI_URL=http://cctui-worker-abc-guard:15003/${PROXY_TOKEN}" "$out"
has "base URL follows" "BASE=http://cctui-worker-abc-guard:15003/${PROXY_TOKEN}" "$out"
logs=$(printf '%s\n' "$out" | grep '^cctui-worker:')
hasnt "no proxy token in the log" "$PROXY_TOKEN" "$logs"
hasnt "no control token in the log" "$CONTROL_TOKEN" "$logs"
has "unsigned commits are said" "commits are UNSIGNED" "$logs"

echo "== 5. no CA served: passthrough, nothing installed =="
reset
out=$(GUARD_PROXY_HOST=g FAKE_CA=no run 'phase_external_guard'); rc=$?
check "rc 0" 0 "$rc"
has "says injection is off" "injection off" "$out"
hasnt "nothing installed" "CA installed" "$out"

echo "== 6. a guard that refuses the policy fails the pod =="
reset
out=$(GUARD_PROXY_HOST=g FAKE_PUT=no run 'phase_external_guard'); rc=$?
check "rc 1" 1 "$rc"
has "names the refusal" "refused the policy" "$(cat "$TMP/termination-log")"

echo "== 7. a per-step policy rewrite reaches the guard pod =="
reset
PATH="$TMP/bin:$PATH" FAKE_DIR="$TMP" GUARD_AUTH_DIR="$TMP/auth" GUARD_PROXY_HOST=g \
    POLICY_FILE="$TMP/policy/policy.json" PRIVATE_ALLOW_FILE="$TMP/policy/private-allow" \
    WORKER_GUARD_SYNC_SECS=0.1 bash -c '
    log() { :; }
    PROXY_HEALTH_PORT=15002
    . "$0"
    guard_policy_sync
' "$TMP/fns.sh" &
sync_pid=$!
sleep 0.4
check "nothing pushed while unchanged" "" "$(cat "$TMP/puts" 2>/dev/null)"
printf '{"allowed_hosts":[],"default":"deny"}\n' > "$TMP/policy/policy.json"
for _ in $(seq 1 30); do [ -s "$TMP/puts" ] && break; sleep 0.1; done
kill "$sync_pid" 2>/dev/null; wait "$sync_pid" 2>/dev/null
has "rewrite pushed" "http://g:15002/policy" "$(cat "$TMP/puts" 2>/dev/null)"

echo "== 8. the gateway token never reaches a log line =="
out=$(run "redact_url 'http://g:15003/${PROXY_TOKEN}/api/v1/sessions'")
check "masked mid-path" "http://g:15003/<token>/api/v1/sessions" "$out"
out=$(run "redact_url 'http://g:15003/${PROXY_TOKEN}'")
check "masked at the end" "http://g:15003/<token>" "$out"
out=$(run "redact_url 'http://cctui.cctui.svc:8700/health'")
check "a plain URL is unchanged" "http://cctui.cctui.svc:8700/health" "$out"

echo "== 9. the egress re-check ends an enforcing dispatch on a breach =="
recheck() {
    run "
        preflight_targets() {
            echo 'http 1.1.1.1:80 direct public egress'
            echo 'https kubernetes.default.svc:443 k8s API reachable'
        }
        preflight_probe() { [ \"\$2\" = \"\${OPEN_TARGET:-none}\" ]; }
        stop_daemon_gracefully() { echo 'daemon stopped'; }
        PREFLIGHT_RECHECK_MODE=$1
        EGRESS_RECHECK_LOGGED=''
        DAEMON_PID=1
        TASK_ID=t-1
        egress_recheck
        echo 'still running'
    "
}
reset
out=$(OPEN_TARGET=1.1.1.1:80 recheck enforce); rc=$?
check "rc 1" 1 "$rc"
check "termination log names the breach" "egress re-check: direct public egress (1.1.1.1:80)" \
    "$(cat "$TMP/termination-log")"
check "result is a failure" "failed" "$(jq -r .status "$TMP/result.json")"
has "daemon stopped first" "daemon stopped" "$out"
hasnt "the run does not continue" "still running" "$out"
reset
out=$(OPEN_TARGET=1.1.1.1:80 recheck warn); rc=$?
check "warn: rc 0" 0 "$rc"
has "warn: logged" "WARNING: egress re-check: direct public egress" "$out"
has "warn: continues" "still running" "$out"
reset
out=$(OPEN_TARGET=1.1.1.1:80 recheck off); rc=$?
check "off: rc 0" 0 "$rc"
hasnt "off: no probe verdict" "egress re-check" "$out"
reset
out=$(recheck enforce); rc=$?
check "isolated: rc 0" 0 "$rc"
has "isolated: continues" "still running" "$out"
check "isolated: no termination log" "" "$(cat "$TMP/termination-log" 2>/dev/null)"

echo "== 10. the wait loop re-checks on its own cadence =="
has "await_dispatch_done calls egress_recheck" "egress_recheck" \
    "$(awk '/^await_dispatch_done\(\)/,/^}/' "$ENTRY")"
has "preflight hands its mode to the re-check" 'PREFLIGHT_RECHECK_MODE=$_mode' \
    "$(awk '/^phase_preflight\(\)/,/^}/' "$ENTRY")"

[ "$fail" = 0 ] && echo "PASS" || { echo "FAILED"; exit 1; }
