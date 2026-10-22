#!/usr/bin/env bash
# Exercises default_node_ca() from deploy/worker-entrypoint.sh: every worker's
# node reads the system trust store unless NODE_OPTIONS already names a CA source.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }

eval "$(awk '/^default_node_ca\(\)/,/^}/' "$ENTRY")"
type default_node_ca >/dev/null 2>&1 || { bad "default_node_ca not found in $ENTRY"; exit 1; }

echo "== 1. unset gets the flag, exported =="
unset NODE_OPTIONS
default_node_ca
check "value"    "--use-openssl-ca" "${NODE_OPTIONS:-}"
check "exported" "--use-openssl-ca" "$(bash -c 'printf %s "${NODE_OPTIONS:-}"')"

echo "== 2. appended to existing options =="
export NODE_OPTIONS="--max-old-space-size=4096"
default_node_ca
check "appended" "--max-old-space-size=4096 --use-openssl-ca" "${NODE_OPTIONS:-}"

echo "== 3. never duplicated =="
default_node_ca
check "idempotent" "--max-old-space-size=4096 --use-openssl-ca" "${NODE_OPTIONS:-}"
export NODE_OPTIONS="--use-openssl-ca --max-old-space-size=4096"
default_node_ca
check "already first" "--use-openssl-ca --max-old-space-size=4096" "${NODE_OPTIONS:-}"

echo "== 4. a profile's own CA source wins =="
for flag in --use-bundled-ca --use-system-ca; do
  export NODE_OPTIONS="--max-old-space-size=4096 $flag"
  default_node_ca
  check "$flag kept alone" "--max-old-space-size=4096 $flag" "${NODE_OPTIONS:-}"
done

echo "== 5. empty is treated as unset =="
export NODE_OPTIONS=
default_node_ca
check "value" "--use-openssl-ca" "${NODE_OPTIONS:-}"

echo "== 6. a lookalike flag is not a CA source =="
export NODE_OPTIONS="--use-openssl-ca-foo"
default_node_ca
check "appended" "--use-openssl-ca-foo --use-openssl-ca" "${NODE_OPTIONS:-}"

echo "== 7. it runs after the extensions and before the daemon is launched =="
_ext=$(grep -n '^phase_extensions$' "$ENTRY" | head -1 | cut -d: -f1)
_def=$(grep -n '^default_node_ca$' "$ENTRY" | head -1 | cut -d: -f1)
_run=$(grep -n '^    run_supervised_daemon "\$@" &$' "$ENTRY" | head -1 | cut -d: -f1)
if [ -n "$_ext" ] && [ -n "$_def" ] && [ -n "$_run" ] && [ "$_ext" -lt "$_def" ] && [ "$_def" -lt "$_run" ]; then
  ok "extensions $_ext < default_node_ca $_def < daemon $_run"
else
  bad "order: extensions=$_ext default_node_ca=$_def daemon=$_run"
fi

exit $fail
