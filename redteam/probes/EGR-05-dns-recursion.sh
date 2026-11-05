#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# The pod resolver must not recurse public names; a restricted resolver answers
# with the 192.0.2.1 sentinel (or nothing).
if command -v dig >/dev/null 2>&1; then
    _a=$(as_worker dig +time=3 +tries=1 +short example.com A 2>/dev/null | grep -E '^[0-9.]+$' || true)
    case "$_a" in
        "" | 192.0.2.1) emit_result EGR-05 blocked "example.com A: ${_a:-no answer}" ;;
        *) emit_result EGR-05 escaped "example.com A: $(printf '%s' "$_a" | tr '\n' ' ')" ;;
    esac
else
    emit_result EGR-05 skipped "no dig"
fi
