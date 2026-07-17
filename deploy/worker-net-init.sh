#!/usr/bin/env sh
# Pod-netns iptables for sidecar egress mode (CCT-716): must run with
# CAP_NET_ADMIN in the shared pod network namespace before the worker starts.
# PROXY_UID must match the uid the guard-proxy sidecar container runs as, or
# the proxy's own upstream connects would loop back into itself.
set -eu

log() { echo "cctui-worker-net-init: $*"; }

WORKER_UID="${WORKER_UID:-1000}"
PROXY_UID="${PROXY_UID:-1337}"
PROXY_PORT="${PROXY_PORT:-15001}"

# Unlike the entrypoint (which falls back to forward mode), this container's
# only job is the redirect: unusable iptables is a hard failure.
if ! iptables -t nat -L >/dev/null 2>&1; then
    echo "cctui-worker-net-init: iptables unusable (need CAP_NET_ADMIN)" >&2
    exit 1
fi

iptables -t nat -A OUTPUT -m owner --uid-owner 0 -j RETURN
iptables -t nat -A OUTPUT -m owner --uid-owner "$PROXY_UID" -j RETURN
iptables -t nat -A OUTPUT -d 127.0.0.0/8 -j RETURN

# DNS TCP fallback only; UDP/53 already bypasses the TCP-only REDIRECT.
_dns=$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf 2>/dev/null || true)
if [ -n "$_dns" ]; then
    iptables -t nat -A OUTPUT -d "$_dns" -j RETURN
    log "iptables: RETURN DNS $_dns"
fi

if [ -n "${WORKER_NET_EXEMPT:-}" ]; then
    _OLDIFS=$IFS; IFS=,
    for _e in $WORKER_NET_EXEMPT; do
        IFS=$_OLDIFS
        _h=$(printf '%s' "$_e" | sed 's,:.*$,,')
        [ -n "$_h" ] || continue
        # getent ahostsv4, not hosts: dual-stack hosts return AAAA first,
        # which this IPv4 chain rejects.
        _ip=$(getent ahostsv4 "$_h" 2>/dev/null | awk '{print $1; exit}' || true)
        if [ -n "$_ip" ]; then
            # One unreachable exempt host must not abort the setup (set -e).
            if iptables -t nat -A OUTPUT -d "$_ip" -j RETURN 2>/dev/null; then
                log "iptables: RETURN exempt $_h ($_ip)"
            else
                log "WARNING: iptables exempt rule failed for $_h ($_ip)"
            fi
        else
            log "WARNING: could not resolve WORKER_NET_EXEMPT host $_h"
        fi
        IFS=,
    done
    IFS=$_OLDIFS
fi

iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner "$WORKER_UID" \
    -j REDIRECT --to-port "$PROXY_PORT"
log "iptables: worker egress (uid $WORKER_UID) -> :$PROXY_PORT"

# The proxy + REDIRECT are IPv4-only (SO_ORIGINAL_DST): IPv6 egress would
# bypass the filter, so deny it.
if ip6tables -L >/dev/null 2>&1; then
    ip6tables -A OUTPUT -o lo -j ACCEPT
    ip6tables -A OUTPUT -j REJECT
    log "ip6tables: IPv6 egress denied (forces IPv4 fallback)"
else
    log "WARNING: ip6tables unavailable — IPv6 egress is UNFILTERED"
fi

log "done"
