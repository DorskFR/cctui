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
getent() { echo "203.0.113.7 STREAM exempt.example"; }
WORKER_UID=1000
PROXY_UID=1337
PROXY_PORT=15001

echo "== 1. docker embedded resolver: TCP redirected, metadata and non-TCP rejected =="
printf 'nameserver 127.0.0.11\noptions ndots:0\n' > "$TMP/resolv.conf"
unset WORKER_NET_EXEMPT
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "root exempt"  "iptables -t nat -A OUTPUT -m owner --uid-owner 0 -j RETURN"
has "proxy exempt" "iptables -t nat -A OUTPUT -m owner --uid-owner 1337 -j RETURN"
has "tcp REDIRECTed to the proxy" \
    "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner 1000 -j REDIRECT --to-port 15001"
has "metadata RETURNed from nat" \
    "iptables -t nat -A OUTPUT -m owner --uid-owner 1000 -d 169.254.0.0/16 -j RETURN"
has "metadata REJECTed" "iptables -A OUTPUT -m owner --uid-owner 1000 -d 169.254.0.0/16 -j REJECT"
before "metadata RETURN precedes the REDIRECT" \
    "iptables -t nat -A OUTPUT -m owner --uid-owner 1000 -d 169.254.0.0/16 -j RETURN" \
    "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner 1000 -j REDIRECT --to-port 15001"
has "loopback accepted" "iptables -A OUTPUT -m owner --uid-owner 1000 -o lo -j ACCEPT"
has "UDP/53 to the resolver accepted" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -p udp -d 127.0.0.11 --dport 53 -j ACCEPT"
has "everything non-TCP REJECTed" "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"
before "metadata REJECT precedes the loopback accept" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -d 169.254.0.0/16 -j REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -o lo -j ACCEPT"
before "DNS accept precedes the non-TCP REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -p udp -d 127.0.0.11 --dport 53 -j ACCEPT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"
has "IPv6 egress REJECTed" "ip6tables -A OUTPUT -j REJECT"
if grep -E '^iptables ' <<<"$OUT" | grep -v -- '-t nat' | grep -qvE -- '--uid-owner 1000 '; then
  bad "filter rules touch a uid other than the worker"
else
  ok "filter rules only match the worker uid"
fi

echo "== 2. IPv6 resolver first: only the IPv4 one is exempted =="
printf 'nameserver fd00::53\nnameserver 10.0.0.53\n' > "$TMP/resolv.conf"
OUT=$(install_transparent_rules "$TMP/resolv.conf")
has "IPv4 resolver RETURNed" "iptables -t nat -A OUTPUT -d 10.0.0.53 -j RETURN"
not "IPv6 resolver never reaches iptables" "iptables -t nat -A OUTPUT -d fd00::53 -j RETURN"

echo "== 3. no IPv4 resolver: no UDP accept at all =="
printf 'nameserver fd00::53\n' > "$TMP/resolv.conf"
OUT=$(install_transparent_rules "$TMP/resolv.conf")
if grep -q -- '-p udp' <<<"$OUT"; then bad "UDP accepted without a resolver"; else ok "no UDP accept"; fi
has "non-TCP still REJECTed" "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"

echo "== 4. operator exemptions are RETURNed =="
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
if grep -q 'install_transparent_rules /etc/resolv.conf' <<<"$net"; then
  ok "phase_network installs the rules"
else
  bad "phase_network does not call install_transparent_rules"
fi

exit $fail
