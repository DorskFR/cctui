#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# unshare() is on the seccomp denylist: a new namespace must not be creatable.
if ! command -v unshare >/dev/null 2>&1; then
    emit_result PRIV-02 skipped "no unshare"
elif as_worker unshare --user --net true >/dev/null 2>&1; then
    emit_result PRIV-02 escaped "unshare(CLONE_NEWUSER|CLONE_NEWNET) succeeded"
else
    emit_result PRIV-02 blocked "unshare refused"
fi
