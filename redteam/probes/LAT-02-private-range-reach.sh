#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# A connect to a private-range address (no proxy) must be refused: the proxy's
# built-in denylist blocks private destinations even if a policy listed them.
_hit=""
for _p in 10.0.0.1:80 172.16.0.1:80 192.168.0.1:80; do
    [ -n "$_hit" ] && continue
    _code=$(as_worker curl -q -sk -g -o /dev/null -w '%{http_code}' --noproxy '*' \
        --max-time 3 "http://${_p}/" 2>/dev/null || true)
    case "$_code" in
        "" | 000) : ;;
        *) _hit="HTTP $_code from $_p" ;;
    esac
done
if [ -n "$_hit" ]; then
    emit_result LAT-02 escaped "$_hit"
else
    emit_result LAT-02 blocked "no private-range host answered"
fi
