#!/usr/bin/env bash
# Exercises deploy/worker-net-init.sh in dry-run mode: the worker uid's TCP is
# REDIRECTed to the proxy, its UDP is REJECTed except UDP/53 to the resolver,
# every other protocol is REJECTed, and root / proxy uid rules are untouched.
set -u

SCRIPT="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-net-init.sh}"
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
run() { OUT=$(WORKER_NET_INIT_DRY_RUN=1 RESOLV_CONF="$1" sh "$SCRIPT" 2>&1) || bad "script exited $?"; }

echo "== 1. cluster resolver: UDP/53 allowed, other UDP and non-TCP rejected =="
printf 'search workers.svc.cluster.local svc.cluster.local\nnameserver 10.43.0.10\noptions ndots:5\n' >"$TMP/resolv.conf"
run "$TMP/resolv.conf"
has "tcp still REDIRECTed to the proxy" \
    "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner 1000 -j REDIRECT --to-port 15001"
has "resolver exempt from REDIRECT" "iptables -t nat -A OUTPUT -d 10.43.0.10 -j RETURN"
has "loopback accepted" "iptables -A OUTPUT -m owner --uid-owner 1000 -o lo -j ACCEPT"
has "UDP/53 to the resolver accepted" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -p udp -d 10.43.0.10 --dport 53 -j ACCEPT"
has "everything non-TCP REJECTed" "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"
before "DNS accept precedes the REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -p udp -d 10.43.0.10 --dport 53 -j ACCEPT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"
before "metadata REJECT precedes the loopback accept" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -d 169.254.0.0/16 -j REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1000 -o lo -j ACCEPT"
has "IPv6 egress REJECTed" "ip6tables -A OUTPUT -j REJECT"
if grep -E '^iptables ' <<<"$OUT" | grep -v -- '-t nat' | grep -qvE -- '--uid-owner 1000 '; then
  bad "filter rules touch a uid other than the worker"
else
  ok "filter rules only match the worker uid"
fi
if grep -E -- '--uid-owner (0|1337) ' <<<"$OUT" | grep -qv -- '-t nat .* -j RETURN'; then
  bad "root / proxy uid rules changed"
else
  ok "root and proxy uid keep their nat RETURN only"
fi

echo "== 2. no IPv4 resolver: no UDP/53 allow, REJECT stays, warning logged =="
printf 'nameserver fd00::53\n' >"$TMP/resolv6.conf"
run "$TMP/resolv6.conf"
not "no IPv6 resolver rule in iptables" "iptables -t nat -A OUTPUT -d fd00::53 -j RETURN"
if grep -qE -- '-p udp .* --dport 53 -j ACCEPT' <<<"$OUT"; then bad "UDP/53 allowed with no resolver"; else ok "no UDP/53 allow"; fi
has "non-TCP still REJECTed" "iptables -A OUTPUT -m owner --uid-owner 1000 ! -p tcp -j REJECT"
if grep -q 'WARNING: no IPv4 nameserver' <<<"$OUT"; then ok "warning logged"; else bad "no warning"; fi

echo "== 3. uids and port come from the environment =="
OUT=$(WORKER_NET_INIT_DRY_RUN=1 RESOLV_CONF="$TMP/resolv.conf" WORKER_UID=2000 PROXY_UID=4242 PROXY_PORT=9999 sh "$SCRIPT" 2>&1) || bad "script exited $?"
has "custom worker uid REDIRECT" \
    "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner 2000 -j REDIRECT --to-port 9999"
has "custom worker uid REJECT" "iptables -A OUTPUT -m owner --uid-owner 2000 ! -p tcp -j REJECT"
has "custom proxy uid RETURN" "iptables -t nat -A OUTPUT -m owner --uid-owner 4242 -j RETURN"

echo "== 4. dry run never invokes iptables =="
OUT=$(PATH="$TMP/nobin" WORKER_NET_INIT_DRY_RUN=1 RESOLV_CONF="$TMP/resolv.conf" /bin/sh "$SCRIPT" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && grep -q '^cctui-worker-net-init: done$' <<<"$OUT"; then ok "completes without iptables on PATH"; else bad "rc=$rc"; fi

exit $fail
