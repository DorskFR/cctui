#!/usr/bin/env sh
# Egress bypass probes for the restricted-worker canary, run as a fixed boot
# step rather than through the agent. Mount as
# /opt/worker-entrypoint.d/99-egress-probe.sh (sourced: probes run as the agent
# uid via as_worker) or run it directly. PASS means the bypass was blocked.
# Never exits, so it is safe to source.

if ! command -v as_worker >/dev/null 2>&1; then
    as_worker() { "$@"; }
fi

probe_result() {
    if [ "$2" = blocked ]; then
        echo "probe: PASS $1 ($3)"
    else
        echo "probe: FAIL $1 ($3)"
    fi
}

echo "probe: uid=$(as_worker id -u) net_mode=${WORKER_NET_MODE:-unset}"

# A client that ignores the proxy env must not reach a public host directly.
_code=$(as_worker env -u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy \
    curl -q -sS -o /dev/null -w '%{http_code}' --noproxy '*' --max-time 5 https://example.com 2>/dev/null || true)
case "$_code" in
    "" | 000) probe_result direct-public-https blocked "no response" ;;
    *) probe_result direct-public-https reached "HTTP $_code from example.com" ;;
esac

# Raw TCP to the Kubernetes API by IP (no proxy, no SNI) must not get an answer.
_k8s="${KUBERNETES_SERVICE_HOST:-10.43.0.1}:${KUBERNETES_SERVICE_PORT:-443}"
case "$_k8s" in *:*:*) _k8s="[${KUBERNETES_SERVICE_HOST}]:${KUBERNETES_SERVICE_PORT:-443}" ;; esac
_code=$(as_worker curl -q -sk -g -o /dev/null -w '%{http_code}' --noproxy '*' --max-time 5 \
    "https://${_k8s}/version" 2>/dev/null || true)
case "$_code" in
    "" | 000) probe_result raw-k8s-api blocked "no response from $_k8s" ;;
    *) probe_result raw-k8s-api reached "HTTP $_code from $_k8s" ;;
esac

# UDP DNS straight to a public resolver must get no answer.
if command -v dig >/dev/null 2>&1; then
    if as_worker dig @1.1.1.1 +time=3 +tries=1 +short example.com A >/dev/null 2>&1; then
        probe_result udp-dns-public reached "1.1.1.1 answered (dig)"
    else
        probe_result udp-dns-public blocked "no answer from 1.1.1.1 (dig)"
    fi
elif command -v node >/dev/null 2>&1; then
    _dns=$(as_worker node -e '
        const r = new (require("dns").promises.Resolver)({ timeout: 3000, tries: 1 });
        r.setServers(["1.1.1.1"]);
        r.resolve4("example.com").then((a) => console.log("answer " + a[0]), (e) => console.log("none " + e.code));
    ' 2>&1 || true)
    case "$_dns" in
        answer*) probe_result udp-dns-public reached "1.1.1.1 answered: ${_dns#answer }" ;;
        *) probe_result udp-dns-public blocked "no answer from 1.1.1.1: ${_dns#none }" ;;
    esac
else
    echo "probe: SKIP udp-dns-public (no dig or node)"
fi
