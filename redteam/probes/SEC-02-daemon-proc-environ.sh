#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# The daemon's /proc/<pid>/environ must not leak the real machine key to the
# agent uid (PR_SET_DUMPABLE=0 + child-env scrub).
_leak=blocked
_detail="no readable daemon environ exposes a key"
for _p in /proc/[0-9]*; do
    _comm=$(cat "$_p/comm" 2>/dev/null || true)
    case "$_comm" in
        cctui-daemon|cctui-superviso*)
            if as_worker grep -qa 'CCTUI_MACHINE_KEY=' "$_p/environ" 2>/dev/null; then
                _leak=escaped; _detail="CCTUI_MACHINE_KEY readable in $_p/environ"
            fi
            ;;
    esac
done
emit_result SEC-02 "$_leak" "$_detail"
