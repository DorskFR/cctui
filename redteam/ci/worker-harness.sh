#!/usr/bin/env sh
# Runs inside the worker image for T1. Stands up the real sandbox — the
# transparent iptables redirect (worker-net-init), a deny-all guard-proxy as
# uid 1337, then the probes under cctui-supervisor as the agent uid 1000 — with
# no cctui-server in the loop. Egress is deny-all, so every reachability probe
# must come back `blocked`. Needs CAP_NET_ADMIN.
set -eu

PROXY_UID="${PROXY_UID:-1337}"
PROXY_PORT="${PROXY_PORT:-15001}"
PROXY_HEALTH_PORT="${PROXY_HEALTH_PORT:-15002}"
PROBES_DIR="${PROBES_DIR:-/opt/redteam/probes}"

mkdir -p /var/run/guard-proxy /var/run/workflow-guard
printf '{"allowed_hosts":[],"default":"deny"}\n' > /var/run/guard-proxy/policy.json
: > /var/run/workflow-guard/state
chmod 0644 /var/run/guard-proxy/policy.json /var/run/workflow-guard/state
chown -R root:root /var/run/guard-proxy /var/run/workflow-guard

PROXY_UID="$PROXY_UID" PROXY_PORT="$PROXY_PORT" sh /usr/local/bin/cctui-worker-net-init 2>&1 \
    | sed 's/^/[net] /' || echo "[net] WARNING: net-init failed (egress may be unfiltered)"

setpriv --reuid="$PROXY_UID" --regid="$PROXY_UID" --clear-groups \
    cctui-guard-proxy --mode transparent \
        --listen "0.0.0.0:${PROXY_PORT}" \
        --health-listen "0.0.0.0:${PROXY_HEALTH_PORT}" \
        --policy /var/run/guard-proxy/policy.json 2>&1 | sed 's/^/[proxy] /' &

_i=0
while [ "$_i" -lt 40 ]; do
    curl -fsS "http://127.0.0.1:${PROXY_HEALTH_PORT}/ready" >/dev/null 2>&1 && break
    _i=$((_i + 1))
    sleep 0.25
done

exec cctui-supervisor \
    --ro /usr --ro /lib --ro /lib64 --ro /bin --ro /sbin --ro /etc --ro /proc \
    --ro /opt/redteam --ro /var/run/guard-proxy --ro /var/run/workflow-guard \
    --rw /dev --rw /tmp \
    --user 1000 \
    -- "${PROBES_DIR}/run-all.sh"
