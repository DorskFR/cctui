#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# Through the proxy, a Host header for an unlisted host must be denied whatever
# the client sends. Target the sink by name via the proxy env.
if [ -z "$SINK_HTTP_HOST" ]; then
    emit_result EGR-03 skipped "no sink configured"
else
    _code=$(as_worker curl -q -sS -o /dev/null -w '%{http_code}' --max-time 5 \
        -H "Host: ${SINK_HTTP_HOST}" "http://${SINK_HTTP_HOST}:${SINK_HTTP_PORT}/canary/egr03" 2>/dev/null || true)
    case "$_code" in
        "" | 000 | 403) emit_result EGR-03 blocked "proxy refused unlisted host (code ${_code:-none})" ;;
        *) emit_result EGR-03 escaped "proxy relayed to unlisted sink (HTTP $_code)" ;;
    esac
fi
