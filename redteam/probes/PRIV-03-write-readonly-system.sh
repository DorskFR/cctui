#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# Landlock makes the system tree read-only: a write under /usr must fail.
_t=/usr/.cctui-redteam-priv03
if as_worker sh -c "echo x > $_t" 2>/dev/null; then
    as_worker rm -f "$_t" 2>/dev/null || true
    emit_result PRIV-03 escaped "wrote $_t under the read-only system tree"
else
    emit_result PRIV-03 blocked "write to $_t refused"
fi
