#!/usr/bin/env sh
# Pod-netns iptables for sidecar egress mode: must run with CAP_NET_ADMIN in
# the shared pod network namespace before the worker starts. The rules cover
# every container of the pod; only PROXY_UID egresses directly.
# WORKER_DNS_RESOLVER: IPv4 or pod-search-relative name of the restricted
# resolver; unresolvable is fatal when set, a warning for the default.
# WORKER_NET_INIT_DRY_RUN=1 prints the rules instead of applying them.
set -eu

log() { echo "cctui-worker-net-init: $*"; }

PROXY_UID="${PROXY_UID:-1337}"
PROXY_PORT="${PROXY_PORT:-15001}"
RESOLV_CONF="${RESOLV_CONF:-/etc/resolv.conf}"
DRY_RUN="${WORKER_NET_INIT_DRY_RUN:-}"
DEFAULT_DNS_RESOLVER=cctui-worker-dns

ipt()  { if [ -n "$DRY_RUN" ]; then echo "iptables $*"; else iptables "$@"; fi; }
ip6t() { if [ -n "$DRY_RUN" ]; then echo "ip6tables $*"; else ip6tables "$@"; fi; }

is_ipv4() {
    case $1 in '' | *[!0-9.]* | .* | *. | *..*) return 1 ;; esac
    _oifs=$IFS; IFS=.
    # shellcheck disable=SC2086
    set -- $1
    IFS=$_oifs
    [ $# -eq 4 ]
}

# getent ahostsv4, not hosts: dual-stack hosts return AAAA first, which this
# IPv4 chain rejects.
resolve_v4() {
    if is_ipv4 "$1"; then
        echo "$1"
    else
        getent ahostsv4 "$1" 2>/dev/null | awk '{print $1; exit}' || true
    fi
}

# Unlike the entrypoint (which falls back to forward mode), this container's
# only job is the redirect: unusable iptables is a hard failure.
if [ -z "$DRY_RUN" ] && ! iptables -t nat -L >/dev/null 2>&1; then
    echo "cctui-worker-net-init: iptables unusable (need CAP_NET_ADMIN)" >&2
    exit 1
fi

# First IPv4 resolver (IPv6 egress is denied wholesale below).
_dns=$(awk '/^nameserver/ && $2 ~ /^[0-9.]+$/ {print $2; exit}' "$RESOLV_CONF" 2>/dev/null || true)

# Resolved before any rule exists, so this lookup still uses the pod resolver.
_rdns=""
if [ -n "$_dns" ]; then
    _rdns=$(resolve_v4 "${WORKER_DNS_RESOLVER:-$DEFAULT_DNS_RESOLVER}")
    if [ -z "$_rdns" ]; then
        if [ -n "${WORKER_DNS_RESOLVER:-}" ]; then
            echo "cctui-worker-net-init: cannot resolve WORKER_DNS_RESOLVER $WORKER_DNS_RESOLVER" >&2
            exit 1
        fi
        log "WARNING: no restricted resolver ($DEFAULT_DNS_RESOLVER does not resolve) — DNS goes to $_dns, which may recurse public names"
        _rdns=$_dns
    fi
fi

ipt -t nat -A OUTPUT -m owner --uid-owner "$PROXY_UID" -j RETURN
ipt -t nat -A OUTPUT -d 127.0.0.0/8 -j RETURN

if [ -n "$_dns" ]; then
    for _p in udp tcp; do
        if [ "$_rdns" = "$_dns" ]; then
            ipt -t nat -A OUTPUT -p "$_p" -d "$_dns" --dport 53 -j RETURN
        else
            ipt -t nat -A OUTPUT -p "$_p" -d "$_dns" --dport 53 -j DNAT --to-destination "$_rdns:53"
        fi
    done
    log "iptables: DNS to $_dns:53 -> $_rdns:53"
fi

if [ -n "${WORKER_NET_EXEMPT:-}" ]; then
    _OLDIFS=$IFS; IFS=,
    for _e in $WORKER_NET_EXEMPT; do
        IFS=$_OLDIFS
        _h=$(printf '%s' "$_e" | sed 's,:.*$,,')
        [ -n "$_h" ] || continue
        _ip=$(resolve_v4 "$_h")
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

# The link-local metadata / credential range (cloud IMDS 169.254.169.254, EKS
# Pod Identity Agent 169.254.170.23) is RETURNed from nat (else the REDIRECT
# rewrites the dst and filter can't match) and REJECTed in filter.
ipt -t nat -A OUTPUT -d 169.254.0.0/16 -j RETURN

ipt -t nat -A OUTPUT -p tcp -j REDIRECT --to-port "$PROXY_PORT"
log "iptables: egress of every uid but $PROXY_UID -> :$PROXY_PORT"

ipt -A OUTPUT -m owner --uid-owner "$PROXY_UID" -j ACCEPT
ipt -A OUTPUT -d 169.254.0.0/16 -j REJECT
log "iptables: egress to 169.254.0.0/16 REJECTed (metadata/credential deny)"

# Only TCP reaches the proxy; REJECT (not DROP) so UDP/QUIC clients fail fast
# and fall back to TCP. TCP is not filtered here: replies of the sidecars'
# inbound connections (kubelet probes) carry their uid.
ipt -A OUTPUT -o lo -j ACCEPT
if [ -n "$_rdns" ]; then
    ipt -A OUTPUT -p udp -d "$_rdns" --dport 53 -j ACCEPT
    ipt -A OUTPUT -p tcp -d "$_rdns" --dport 53 -j ACCEPT
else
    log "WARNING: no IPv4 nameserver in $RESOLV_CONF — DNS is REJECTed"
fi
ipt -A OUTPUT ! -p tcp -j REJECT
log "iptables: non-TCP egress REJECTed (DNS to ${_rdns:-<none>}:53 allowed)"

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
