#!/usr/bin/env sh
# Pod-netns iptables for sidecar egress mode: must run with
# CAP_NET_ADMIN in the shared pod network namespace before the worker starts.
# PROXY_UID must match the uid the guard-proxy sidecar container runs as, or
# the proxy's own upstream connects would loop back into itself.
# WORKER_NET_INIT_DRY_RUN=1 prints the rules instead of applying them.
set -eu

log() { echo "cctui-worker-net-init: $*"; }

WORKER_UID="${WORKER_UID:-1000}"
PROXY_UID="${PROXY_UID:-1337}"
PROXY_PORT="${PROXY_PORT:-15001}"
RESOLV_CONF="${RESOLV_CONF:-/etc/resolv.conf}"
DRY_RUN="${WORKER_NET_INIT_DRY_RUN:-}"

ipt()  { if [ -n "$DRY_RUN" ]; then echo "iptables $*"; else iptables "$@"; fi; }
ip6t() { if [ -n "$DRY_RUN" ]; then echo "ip6tables $*"; else ip6tables "$@"; fi; }

# Unlike the entrypoint (which falls back to forward mode), this container's
# only job is the redirect: unusable iptables is a hard failure.
if [ -z "$DRY_RUN" ] && ! iptables -t nat -L >/dev/null 2>&1; then
    echo "cctui-worker-net-init: iptables unusable (need CAP_NET_ADMIN)" >&2
    exit 1
fi

ipt -t nat -A OUTPUT -m owner --uid-owner 0 -j RETURN
ipt -t nat -A OUTPUT -m owner --uid-owner "$PROXY_UID" -j RETURN
ipt -t nat -A OUTPUT -d 127.0.0.0/8 -j RETURN

# First IPv4 resolver (IPv6 egress is denied wholesale below).
_dns=$(awk '/^nameserver/ && $2 ~ /^[0-9.]+$/ {print $2; exit}' "$RESOLV_CONF" 2>/dev/null || true)
if [ -n "$_dns" ]; then
    ipt -t nat -A OUTPUT -d "$_dns" -j RETURN
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
            if ipt -t nat -A OUTPUT -d "$_ip" -j RETURN 2>/dev/null; then
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

# Hard-REJECT the worker uid's egress to the link-local metadata /
# credential range (169.254.0.0/16 — cloud IMDS 169.254.169.254, EKS Pod
# Identity Agent 169.254.170.23), independent of the guard-proxy. RETURN it from
# nat first (else the REDIRECT below rewrites the dst and filter can't match),
# then REJECT it in the filter table.
ipt -t nat -A OUTPUT -m owner --uid-owner "$WORKER_UID" -d 169.254.0.0/16 -j RETURN
ipt -A OUTPUT -m owner --uid-owner "$WORKER_UID" -d 169.254.0.0/16 -j REJECT
log "iptables: worker egress to 169.254.0.0/16 REJECTed (metadata/credential deny)"

ipt -t nat -A OUTPUT -p tcp -m owner --uid-owner "$WORKER_UID" \
    -j REDIRECT --to-port "$PROXY_PORT"
log "iptables: worker egress (uid $WORKER_UID) -> :$PROXY_PORT"

# Only TCP reaches the proxy; REJECT (not DROP) so UDP/QUIC clients fail fast
# and fall back to TCP.
ipt -A OUTPUT -m owner --uid-owner "$WORKER_UID" -o lo -j ACCEPT
if [ -n "$_dns" ]; then
    ipt -A OUTPUT -m owner --uid-owner "$WORKER_UID" -p udp -d "$_dns" --dport 53 -j ACCEPT
else
    log "WARNING: no IPv4 nameserver in $RESOLV_CONF — worker UDP DNS is REJECTed"
fi
ipt -A OUTPUT -m owner --uid-owner "$WORKER_UID" ! -p tcp -j REJECT
log "iptables: worker non-TCP egress REJECTed (UDP/53 to ${_dns:-<none>} allowed)"

# The proxy + REDIRECT are IPv4-only (SO_ORIGINAL_DST): IPv6 egress would
# bypass the filter, so deny it.
if [ -n "$DRY_RUN" ] || ip6tables -L >/dev/null 2>&1; then
    ip6t -A OUTPUT -o lo -j ACCEPT
    ip6t -A OUTPUT -j REJECT
    log "ip6tables: IPv6 egress denied (forces IPv4 fallback)"
else
    log "WARNING: ip6tables unavailable — IPv6 egress is UNFILTERED"
fi

log "done"
