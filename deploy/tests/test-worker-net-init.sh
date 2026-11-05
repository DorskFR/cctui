#!/usr/bin/env bash
# Dry-run rules of deploy/worker-net-init.sh: every uid but the proxy's is
# REDIRECTed / REJECTed, DNS reaches only the restricted worker resolver.
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

mkdir -p "$TMP/bin"
cat >"$TMP/bin/getent" <<'EOF'
#!/bin/sh
[ "$1" = ahostsv4 ] || exit 2
for _kv in ${FAKE_HOSTS:-}; do
    if [ "${_kv%%=*}" = "$2" ]; then echo "${_kv#*=}      STREAM $2"; exit 0; fi
done
exit 2
EOF
chmod +x "$TMP/bin/getent"

printf 'search workers.svc.cluster.local svc.cluster.local\nnameserver 10.43.0.10\noptions ndots:5\n' >"$TMP/resolv.conf"
printf 'nameserver fd00::53\n' >"$TMP/resolv6.conf"

run() {
  local conf=$1; shift
  OUT=$(env -u WORKER_DNS_RESOLVER -u WORKER_NET_EXEMPT PATH="$TMP/bin:$PATH" \
    WORKER_NET_INIT_DRY_RUN=1 RESOLV_CONF="$conf" "$@" sh "$SCRIPT" 2>&1)
  RC=$?
}
run_ok() { run "$@"; [ "$RC" -eq 0 ] || bad "script exited $RC: $OUT"; }

R=10.43.77.53
REDIRECT="iptables -t nat -A OUTPUT -p tcp -j REDIRECT --to-port 15001"
REJECT="iptables -A OUTPUT ! -p tcp -j REJECT"

echo "== 1. owner match inverted: only the proxy uid is exempt =="
run_ok "$TMP/resolv.conf" WORKER_DNS_RESOLVER="$R"
has "proxy uid RETURNed from nat" "iptables -t nat -A OUTPUT -m owner --uid-owner 1337 -j RETURN"
has "proxy uid ACCEPTed in filter" "iptables -A OUTPUT -m owner --uid-owner 1337 -j ACCEPT"
has "TCP of every other uid REDIRECTed" "$REDIRECT"
has "non-TCP of every other uid REJECTed" "$REJECT"
before "proxy RETURN precedes the REDIRECT" \
    "iptables -t nat -A OUTPUT -m owner --uid-owner 1337 -j RETURN" "$REDIRECT"
before "proxy ACCEPT precedes the REJECT" \
    "iptables -A OUTPUT -m owner --uid-owner 1337 -j ACCEPT" "$REJECT"
if grep -- '--uid-owner' <<<"$OUT" | grep -qvE -- '-m owner --uid-owner 1337 -j (RETURN|ACCEPT)$'; then
  bad "an owner match other than the proxy's RETURN/ACCEPT: $(grep -- '--uid-owner' <<<"$OUT" | grep -vE -- '--uid-owner 1337 ')"
else
  ok "every owner match is the proxy's RETURN/ACCEPT"
fi
if grep -qE -- '! --uid-owner|! -m owner' <<<"$OUT"; then
  bad "negated owner match (also matches socketless packets)"
else
  ok "no negated owner match"
fi
for uid in 0 999 1000 1001 65534; do
  if grep -qE -- "--uid-owner $uid( |$)" <<<"$OUT"; then
    bad "uid $uid has its own rule"
  else
    ok "uid $uid gets the default REDIRECT/REJECT"
  fi
done
has "metadata RETURNed from nat" "iptables -t nat -A OUTPUT -d 169.254.0.0/16 -j RETURN"
has "metadata REJECTed for every uid" "iptables -A OUTPUT -d 169.254.0.0/16 -j REJECT"
before "metadata RETURN precedes the REDIRECT" "iptables -t nat -A OUTPUT -d 169.254.0.0/16 -j RETURN" "$REDIRECT"
before "metadata REJECT precedes the loopback accept" \
    "iptables -A OUTPUT -d 169.254.0.0/16 -j REJECT" "iptables -A OUTPUT -o lo -j ACCEPT"
has "loopback accepted" "iptables -A OUTPUT -o lo -j ACCEPT"
has "IPv6 egress REJECTed" "ip6tables -A OUTPUT -j REJECT"

echo "== 2. DNS: only to the restricted resolver =="
has "UDP DNS to the pod resolver DNATed" \
    "iptables -t nat -A OUTPUT -p udp -d 10.43.0.10 --dport 53 -j DNAT --to-destination $R:53"
has "TCP DNS to the pod resolver DNATed" \
    "iptables -t nat -A OUTPUT -p tcp -d 10.43.0.10 --dport 53 -j DNAT --to-destination $R:53"
before "DNS DNAT precedes the REDIRECT" \
    "iptables -t nat -A OUTPUT -p tcp -d 10.43.0.10 --dport 53 -j DNAT --to-destination $R:53" "$REDIRECT"
has "UDP/53 to the restricted resolver accepted" "iptables -A OUTPUT -p udp -d $R --dport 53 -j ACCEPT"
has "TCP/53 to the restricted resolver accepted" "iptables -A OUTPUT -p tcp -d $R --dport 53 -j ACCEPT"
before "DNS accept precedes the REJECT" "iptables -A OUTPUT -p udp -d $R --dport 53 -j ACCEPT" "$REJECT"
if grep -E '^iptables .*10\.43\.0\.10' <<<"$OUT" | grep -qv -- '--dport 53 -j DNAT'; then
  bad "the recursing pod resolver is reachable: $(grep -E '^iptables .*10\.43\.0\.10' <<<"$OUT" | grep -v DNAT)"
else
  ok "the pod resolver is only a DNAT source"
fi
accepts=$(grep -E '^iptables -A OUTPUT .*-j ACCEPT$' <<<"$OUT" \
  | grep -vxF -e "iptables -A OUTPUT -m owner --uid-owner 1337 -j ACCEPT" \
      -e "iptables -A OUTPUT -o lo -j ACCEPT" \
      -e "iptables -A OUTPUT -p udp -d $R --dport 53 -j ACCEPT" \
      -e "iptables -A OUTPUT -p tcp -d $R --dport 53 -j ACCEPT")
if [ -n "$accepts" ]; then bad "unexpected filter ACCEPT: $accepts"; else ok "no other filter ACCEPT"; fi

echo "== 3. default resolver name resolves through the pod search path =="
run_ok "$TMP/resolv.conf" FAKE_HOSTS="cctui-worker-dns=10.43.88.1"
has "DNATed to the resolved default resolver" \
    "iptables -t nat -A OUTPUT -p udp -d 10.43.0.10 --dport 53 -j DNAT --to-destination 10.43.88.1:53"
has "accepted to the resolved default resolver" "iptables -A OUTPUT -p udp -d 10.43.88.1 --dport 53 -j ACCEPT"

echo "== 4. default resolver absent: pod resolver kept, warning logged =="
run_ok "$TMP/resolv.conf"
has "UDP DNS RETURNed to the pod resolver" "iptables -t nat -A OUTPUT -p udp -d 10.43.0.10 --dport 53 -j RETURN"
has "UDP/53 to the pod resolver accepted" "iptables -A OUTPUT -p udp -d 10.43.0.10 --dport 53 -j ACCEPT"
not "no any-port RETURN to the pod resolver" "iptables -t nat -A OUTPUT -d 10.43.0.10 -j RETURN"
if grep -q 'WARNING: no restricted resolver' <<<"$OUT"; then ok "warning logged"; else bad "no warning"; fi

echo "== 5. explicit resolver unresolvable: hard failure =="
run "$TMP/resolv.conf" WORKER_DNS_RESOLVER=nope.invalid
if [ "$RC" -ne 0 ] && grep -q 'cannot resolve WORKER_DNS_RESOLVER nope.invalid' <<<"$OUT"; then
  ok "exits non-zero with the reason"
else
  bad "rc=$RC out=$OUT"
fi
if grep -q '^iptables' <<<"$OUT"; then bad "rules emitted before the failure"; else ok "no rule emitted"; fi

echo "== 6. no IPv4 resolver: no DNS allow, REJECT stays, warning logged =="
run_ok "$TMP/resolv6.conf" WORKER_DNS_RESOLVER="$R"
if grep -qE -- '--dport 53' <<<"$OUT"; then bad "DNS rule with no resolver"; else ok "no DNS rule"; fi
has "non-TCP still REJECTed" "$REJECT"
if grep -q 'WARNING: no IPv4 nameserver' <<<"$OUT"; then ok "warning logged"; else bad "no warning"; fi

echo "== 7. proxy uid and port come from the environment =="
run_ok "$TMP/resolv.conf" WORKER_DNS_RESOLVER="$R" PROXY_UID=4242 PROXY_PORT=9999
has "custom proxy uid RETURN" "iptables -t nat -A OUTPUT -m owner --uid-owner 4242 -j RETURN"
has "custom proxy uid ACCEPT" "iptables -A OUTPUT -m owner --uid-owner 4242 -j ACCEPT"
has "custom port REDIRECT" "iptables -t nat -A OUTPUT -p tcp -j REDIRECT --to-port 9999"
not "default proxy uid gone" "iptables -t nat -A OUTPUT -m owner --uid-owner 1337 -j RETURN"

echo "== 8. exempt hosts bypass the REDIRECT =="
run_ok "$TMP/resolv.conf" WORKER_DNS_RESOLVER="$R" WORKER_NET_EXEMPT="10.9.9.9:443,gone.invalid:443"
before "exempt RETURN precedes the REDIRECT" "iptables -t nat -A OUTPUT -d 10.9.9.9 -j RETURN" "$REDIRECT"
if grep -q 'could not resolve WORKER_NET_EXEMPT host gone.invalid' <<<"$OUT"; then ok "unresolvable exempt warned"; else bad "no exempt warning"; fi

echo "== 9. dry run never invokes iptables =="
OUT=$(PATH="$TMP/nobin" WORKER_NET_INIT_DRY_RUN=1 RESOLV_CONF="$TMP/resolv.conf" WORKER_DNS_RESOLVER="$R" /bin/sh "$SCRIPT" 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && grep -q '^cctui-worker-net-init: done$' <<<"$OUT"; then ok "completes without iptables on PATH"; else bad "rc=$rc"; fi

exit $fail
