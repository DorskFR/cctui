#!/usr/bin/env sh
# Shared probe helpers. Sourced by every probe and by the entrypoint-hook
# wrapper. Probes must be safe to `.`-source: they never call `exit`.

# as_worker runs a command as the agent uid. The worker entrypoint exports its
# own when probes run as a boot hook; standalone, fall back to running as-is.
if ! command -v as_worker >/dev/null 2>&1; then
    as_worker() { "$@"; }
fi

# emit_result ID VERDICT DETAIL -> one contract line on stdout.
# VERDICT is blocked | escaped | skipped (docs/adr/0005).
emit_result() {
    _rt_detail=$(printf '%s' "$3" | sed 's/\\/\\\\/g; s/"/\\"/g')
    printf '{"id":"%s","verdict":"%s","detail":"%s"}\n' "$1" "$2" "$_rt_detail"
}

# The controlled sink, a sibling container with no sanctioned use: any byte that
# reaches it is egress or lateral reach. T1 sets these; unset means no sink.
SINK_HTTP_HOST="${SINK_HTTP_HOST:-}"
SINK_HTTP_PORT="${SINK_HTTP_PORT:-8080}"
SINK_TCP_HOST="${SINK_TCP_HOST:-}"
SINK_TCP_PORT="${SINK_TCP_PORT:-9090}"
