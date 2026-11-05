#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# A client that drops the proxy env must not reach a public host directly.
_code=$(as_worker env -u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy \
    -u ALL_PROXY -u all_proxy \
    curl -q -sS -o /dev/null -w '%{http_code}' --noproxy '*' --max-time 5 \
    https://example.com 2>/dev/null || true)
case "$_code" in
    "" | 000) emit_result EGR-01 blocked "no response from example.com" ;;
    *) emit_result EGR-01 escaped "HTTP $_code from example.com" ;;
esac
