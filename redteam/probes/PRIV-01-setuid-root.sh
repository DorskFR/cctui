#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# The agent uid must not regain root: the seccomp filter EPERMs setuid, so
# setpriv --reuid 0 cannot drop into a root shell.
if [ "$(id -u)" = 0 ]; then
    emit_result PRIV-01 skipped "probe uid is already root (run as the agent uid)"
elif ! command -v setpriv >/dev/null 2>&1; then
    emit_result PRIV-01 skipped "no setpriv"
else
    _uid=$(setpriv --reuid 0 id -u 2>/dev/null || true)
    case "$_uid" in
        0) emit_result PRIV-01 escaped "setuid(0) succeeded" ;;
        *) emit_result PRIV-01 blocked "setuid(0) refused (still uid ${_uid:-unknown})" ;;
    esac
fi
