#!/usr/bin/env sh
# Entrypoint for the cctui worker container.
#
# The dispatcher injects the session identity as env (CCTUI_URL,
# CCTUI_MACHINE_KEY, SESSION_ID, TASK_PAYLOAD_JSON, optional REPLY_URL /
# TASK_NAME). The cctui-daemon reads CCTUI_MACHINE_KEY + CCTUI_URL from the
# environment (Config::from_env), so no enroll step and no config file are
# needed here.
#
# Ephemeral worker: --no-auto-update, since the container is short-lived and
# re-fetched on every spawn (k8s/docker workers never self-update — CCT memory).
#
# Kept deliberately thin so the guard concept (egress proxy, policy.json) can be
# layered in here later — set up the guard, then exec the daemon — without
# restructuring. Guard wiring is explicitly out of scope for now (CCT-245).
set -eu

if [ -z "${CCTUI_MACHINE_KEY:-}" ]; then
    echo "cctui-worker: CCTUI_MACHINE_KEY is required (injected by the dispatcher)" >&2
    exit 1
fi
if [ -z "${CCTUI_URL:-}" ] && [ -z "${CCTUI_SERVER_URL:-}" ]; then
    echo "cctui-worker: CCTUI_URL (or CCTUI_SERVER_URL) is required (injected by the dispatcher)" >&2
    exit 1
fi

exec cctui-daemon run --no-auto-update "$@"
