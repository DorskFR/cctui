#!/usr/bin/env bash
# Exercises phase_dispatch_env() from deploy/worker-entrypoint.sh: a dispatched
# pod exports the payload env the guard-proxy hands it, and fails at startup
# with the reason (termination log + exit 1) when there is none.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }

FNS=$(awk '/^dispatch_identity_failed\(\)/,/^}/; /^phase_dispatch_env\(\)/,/^}/' "$ENTRY")
[ -n "$FNS" ] || { echo "functions not found in $ENTRY"; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
cat > "$TMP/bin/curl" <<'EOF'
#!/bin/sh
case "$FAKE_CURL" in
    ok)      printf '%s\n200' '{"env":{"FEATURE_FLAG":"on","GREETING":"hello world"}}' ;;
    refused) printf '%s\n503' '{"error":"redeem refused by the cctui server (401 Unauthorized): dispatch identity rejected: token issuer `https://k3s` is not a trusted cluster issuer"}' ;;
    down)    echo 'curl: (7) Failed to connect to 127.0.0.1 port 15002' >&2; printf '\n000' ;;
    badname) printf '%s\n200' '{"env":{"BAD-NAME":"x"}}' ;;
    notenv)  printf '%s\n200' '{"oops":1}' ;;
    pem)     printf '%s\n200' '{"env":{"KEY_PEM":"-----BEGIN-----\nabc\n-----END-----\n"}}' ;;
    number)  printf '%s\n200' '{"env":{"PORT":8080}}' ;;
esac
EOF
chmod +x "$TMP/bin/curl"

printf '%s\n' "$FNS" > "$TMP/fns.sh"

run() {
    PATH="$TMP/bin:$PATH" FAKE_CURL="$1" CCTUI_DISPATCH_ID="$2" \
        TERMINATION_LOG="$TMP/termination-log" bash -c '
        set -eu
        log() { echo "cctui-worker: $*"; }
        DISPATCH_ENV_URL=http://127.0.0.1:15002/dispatch/env
        . "$0"
        phase_dispatch_env
        echo "FEATURE_FLAG=${FEATURE_FLAG:-}"
        echo "GREETING=${GREETING:-}"
        printf "KEY_PEM=[%s]\n" "${KEY_PEM:-}"
    ' "$TMP/fns.sh" 2>&1
}

echo "== 1. not a dispatched pod: no-op =="
rm -f "$TMP/termination-log"
out=$(run ok ""); rc=$?
check "rc 0" 0 "$rc"
check "nothing exported" "FEATURE_FLAG=" "$(printf '%s\n' "$out" | grep '^FEATURE_FLAG=')"
check "no termination log" "" "$(cat "$TMP/termination-log" 2>/dev/null)"

echo "== 2. redeemed: the env is exported =="
out=$(run ok d-1); rc=$?
check "rc 0" 0 "$rc"
has "FEATURE_FLAG exported" "FEATURE_FLAG=on" "$out"
has "spaces survive" "GREETING=hello world" "$out"

echo "== 3. refused token: the pod fails with the server's reason =="
rm -f "$TMP/termination-log"
out=$(run refused d-1); rc=$?
check "rc 1" 1 "$rc"
has "reason on stderr" "not a trusted cluster issuer" "$out"
has "reason in termination log" "dispatch identity: redeem refused" "$(cat "$TMP/termination-log")"

echo "== 4. guard-proxy unreachable: the pod fails naming it =="
rm -f "$TMP/termination-log"
out=$(run down d-1); rc=$?
check "rc 1" 1 "$rc"
has "names the status" "HTTP 000" "$(cat "$TMP/termination-log")"

echo "== 5. a malformed answer fails closed =="
out=$(run badname d-1); rc=$?
check "invalid env name rc 1" 1 "$rc"
out=$(run notenv d-1); rc=$?
check "missing env object rc 1" 1 "$rc"

echo "== 6. values are exported byte for byte =="
out=$(run pem d-1); rc=$?
check "rc 0" 0 "$rc"
nl='
'
has "trailing newline survives" "-----END-----${nl}]" "$out"
out=$(run number d-1); rc=$?
check "non-string value rc 1" 1 "$rc"
has "names the variable" "non-string value for PORT" "$(cat "$TMP/termination-log")"

exit $fail
