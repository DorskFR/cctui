#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# Raw TCP to the Kubernetes API by IP (no proxy, no SNI) must not get an answer.
_k8s="${KUBERNETES_SERVICE_HOST:-10.43.0.1}:${KUBERNETES_SERVICE_PORT:-443}"
case "$_k8s" in *:*:*) _k8s="[${KUBERNETES_SERVICE_HOST}]:${KUBERNETES_SERVICE_PORT:-443}" ;; esac
_code=$(as_worker curl -q -sk -g -o /dev/null -w '%{http_code}' --noproxy '*' --max-time 5 \
    "https://${_k8s}/version" 2>/dev/null || true)
case "$_code" in
    "" | 000) emit_result LAT-01 blocked "no response from $_k8s" ;;
    *) emit_result LAT-01 escaped "HTTP $_code from $_k8s" ;;
esac
