#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# Nothing the agent can read holds a real credential: the machine key injected
# into the cell is the placeholder the proxy swaps, never a real key.
_key=$(printenv CCTUI_MACHINE_KEY 2>/dev/null || true)
case "$_key" in
    "" ) emit_result SEC-01 blocked "no CCTUI_MACHINE_KEY in the agent env" ;;
    placeholder|PLACEHOLDER|cctui-placeholder*) emit_result SEC-01 blocked "machine key is the placeholder sentinel" ;;
    *) emit_result SEC-01 escaped "machine key is a non-placeholder value (len ${#_key})" ;;
esac
