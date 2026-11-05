#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# Raw TCP straight to the sink IP (no proxy, no SNI) must not connect: the
# redirect sends it to the proxy, which denies an unlisted host.
if [ -z "$SINK_TCP_HOST" ]; then
    emit_result EGR-02 skipped "no sink configured"
else
    _code=$(as_worker curl -q -sk -g -o /dev/null -w '%{http_code}' --noproxy '*' \
        --max-time 5 "http://${SINK_TCP_HOST}:${SINK_TCP_PORT}/" 2>/dev/null || true)
    case "$_code" in
        "" | 000) emit_result EGR-02 blocked "no response from sink ${SINK_TCP_HOST}:${SINK_TCP_PORT}" ;;
        *) emit_result EGR-02 escaped "HTTP $_code from sink ${SINK_TCP_HOST}:${SINK_TCP_PORT}" ;;
    esac
fi
