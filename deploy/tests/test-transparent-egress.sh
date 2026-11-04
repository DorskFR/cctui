#!/usr/bin/env bash
# Exercises install_transparent_rules() from deploy/worker-entrypoint.sh: the
# in-container transparent mode installs the same rules as worker-net-init.sh.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0
ok()  { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fail=1; }
has() { if grep -qxF -- "$2" <<<"$OUT"; then ok "$1"; else bad "$1: missing [$2]"; fi; }
not() { if grep -qxF -- "$2" <<<"$OUT"; then bad "$1: unexpected [$2]"; else ok "$1"; fi; }
before() {
  local a b
  a=$(grep -nxF -- "$2" <<<"$OUT" | cut -d: -f1 | head -1)
  b=$(grep -nxF -- "$3" <<<"$OUT" | cut -d: -f1 | head -1)
  if [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]; then ok "$1"; else bad "$1: [$2] not before [$3]"; fi
}

eval "$(awk '/^install_transparent_rules\(\)/,/^}/' "$ENTRY")"
type install_transparent_rules >/dev/null 2>&1 || { bad "install_transparent_rules not found"; exit 1; }

log() { :; }
iptables()  { [ "${1:-}" = -L ] && return 0; echo "iptables $*"; }
ip6tables() { [ "${1:-}" = -L ] && return "${IP6_RC:-0}"; echo "ip6tables $*"; }
getent() { echo "203.0.113.7 STREAM $2"; }
PROXY_UID=1337
PROXY_PORT=15001

echo "== 1. every uid but the proxy is redirected; metadata and non-TCP rejected =="
printf 'nameserver 192.168.65.7\noptions ndots:0\n' > "$TMP/resolv.conf"
unset WORKER_NET_EXEMPT WORKER_DNS_RESOLVER
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "proxy uid exempt from nat" "iptables -t nat -A OUTPUT -m owner --uid-owner 1337 -j RETURN"
has "loopback exempt from nat" "iptables -t nat -A OUTPUT -d 127.0.0.0/8 -j RETURN"
has "all TCP REDIRECTed" "iptables -t nat -A OUTPUT -p tcp -j REDIRECT --to-port 15001"
has "metadata RETURNed from nat" "iptables -t nat -A OUTPUT -d 169.254.0.0/16 -j RETURN"
before "metadata RETURN precedes the REDIRECT" \
    "iptables -t nat -A OUTPUT -d 169.254.0.0/16 -j RETURN" \
    "iptables -t nat -A OUTPUT -p tcp -j REDIRECT --to-port 15001"
has "proxy uid accepted in filter" "iptables -A OUTPUT -m owner --uid-owner 1337 -j ACCEPT"
has "metadata REJECTed for all" "iptables -A OUTPUT -d 169.254.0.0/16 -j REJECT"
before "proxy accept precedes the metadata REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1337 -j ACCEPT" \
    "iptables -A OUTPUT -d 169.254.0.0/16 -j REJECT"
has "loopback accepted" "iptables -A OUTPUT -o lo -j ACCEPT"
has "resolver kept as is" "iptables -t nat -A OUTPUT -p udp -d 192.168.65.7 --dport 53 -j RETURN"
has "UDP/53 to the resolver accepted" "iptables -A OUTPUT -p udp -d 192.168.65.7 --dport 53 -j ACCEPT"
has "TCP/53 to the resolver accepted" "iptables -A OUTPUT -p tcp -d 192.168.65.7 --dport 53 -j ACCEPT"
has "everything non-TCP REJECTed" "iptables -A OUTPUT ! -p tcp -j REJECT"
before "DNS accept precedes the non-TCP REJECT" \
    "iptables -A OUTPUT -p udp -d 192.168.65.7 --dport 53 -j ACCEPT" \
    "iptables -A OUTPUT ! -p tcp -j REJECT"
has "IPv6 egress REJECTed" "ip6tables -A OUTPUT -j REJECT"
if grep -E -- '--uid-owner' <<<"$OUT" | grep -qv -- '--uid-owner 1337 '; then
  bad "an owner match names a uid other than the proxy"
else
  ok "only the proxy uid is ever matched by owner"
fi
if grep -q -- '! --uid-owner\|! -m owner' <<<"$OUT"; then
  bad "negated owner match"
else
  ok "no negated owner match"
fi
not "root is not exempt" "iptables -t nat -A OUTPUT -m owner --uid-owner 0 -j RETURN"

echo "== 2. restricted resolver: DNS DNATed to it, only it accepted =="
WORKER_DNS_RESOLVER=10.9.9.53
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "UDP DNATed" "iptables -t nat -A OUTPUT -p udp -d 192.168.65.7 --dport 53 -j DNAT --to-destination 10.9.9.53:53"
has "TCP DNATed" "iptables -t nat -A OUTPUT -p tcp -d 192.168.65.7 --dport 53 -j DNAT --to-destination 10.9.9.53:53"
has "restricted resolver accepted" "iptables -A OUTPUT -p udp -d 10.9.9.53 --dport 53 -j ACCEPT"
not "pod resolver not accepted" "iptables -A OUTPUT -p udp -d 192.168.65.7 --dport 53 -j ACCEPT"
WORKER_DNS_RESOLVER=dns.example
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "a resolver name is resolved" "iptables -A OUTPUT -p udp -d 203.0.113.7 --dport 53 -j ACCEPT"
rc=$( (getent() { :; }; install_transparent_rules "$TMP/resolv.conf" >/dev/null); echo $?)
if [ "$rc" != 0 ]; then ok "an unresolvable restricted resolver is fatal"; else bad "unresolvable resolver accepted"; fi
unset WORKER_DNS_RESOLVER

echo "== 3. IPv6 resolver first: only the IPv4 one is used =="
printf 'nameserver fd00::53\nnameserver 10.0.0.53\n' > "$TMP/resolv.conf"
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "IPv4 resolver RETURNed" "iptables -t nat -A OUTPUT -p udp -d 10.0.0.53 --dport 53 -j RETURN"
if grep -q 'fd00::53' <<<"$OUT"; then bad "IPv6 resolver reached iptables"; else ok "IPv6 resolver ignored"; fi

echo "== 4. no IPv4 resolver: no DNS accept at all =="
printf 'nameserver fd00::53\n' > "$TMP/resolv.conf"
OUT=$(install_transparent_rules "$TMP/resolv.conf")
if grep -q -- '--dport 53' <<<"$OUT"; then bad "DNS accepted without a resolver"; else ok "no DNS accept"; fi
has "non-TCP still REJECTed" "iptables -A OUTPUT ! -p tcp -j REJECT"

echo "== 4b. operator exemptions are RETURNed =="
printf 'nameserver 127.0.0.11\n' > "$TMP/resolv.conf"
WORKER_NET_EXEMPT=exempt.example:443
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "exempt host RETURNed" "iptables -t nat -A OUTPUT -d 203.0.113.7 -j RETURN"
unset WORKER_NET_EXEMPT

echo "== 5. the transparent branch never falls back to forward =="
net=$(awk '/^phase_network\(\)/,/^}/' "$ENTRY")
if grep -q 'falling back to forward' <<<"$net"; then
  bad "transparent still falls back to forward"
else
  ok "no silent forward fallback"
fi
if grep -q 'command -v setpriv' <<<"$net"; then
  ok "transparent refuses a proxy it cannot run as the proxy uid"
else
  bad "no setpriv check in phase_network"
fi
if grep -q 'install_transparent_rules /etc/resolv.conf' <<<"$net"; then
  ok "phase_network installs the rules"
else
  bad "phase_network does not call install_transparent_rules"
fi

exit $fail
