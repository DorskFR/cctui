#!/usr/bin/env bash
# T1 orchestration on the CI host: a sink sibling container, a worker container
# from the built image running the real sandbox + probes, then the oracle.
#
# Inputs (env):
#   WORKER_IMAGE  the worker image to test (required)
#   REDTEAM_BIN   host-built cctui-redteam binary, bind-mounted to run the sink
#                 and the oracle (required; forward-compatible glibc)
# Arg 1: output dir for report.{json,md} (default .qa/redteam/t1-local).
set -euo pipefail

: "${WORKER_IMAGE:?set WORKER_IMAGE to the worker image under test}"
: "${REDTEAM_BIN:?set REDTEAM_BIN to a built cctui-redteam binary}"

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
out_dir="${1:-$repo_root/.qa/redteam/t1-local}"
mkdir -p "$out_dir"

net="redteam-net-$$"
sink="redteam-sink-$$"
worker="redteam-worker-$$"

cleanup() {
    docker rm -f "$sink" "$worker" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker network create "$net" >/dev/null

docker run -d --name "$sink" --network "$net" --network-alias sink \
    -v "$REDTEAM_BIN:/usr/local/bin/cctui-redteam:ro" \
    --entrypoint /usr/local/bin/cctui-redteam \
    "$WORKER_IMAGE" sink --http 0.0.0.0:8080 --tcp 0.0.0.0:9090 >/dev/null

docker run --name "$worker" --network "$net" --cap-add NET_ADMIN \
    -e SINK_HTTP_HOST=sink -e SINK_HTTP_PORT=8080 \
    -e SINK_TCP_HOST=sink -e SINK_TCP_PORT=9090 \
    -v "$repo_root/redteam/probes:/opt/redteam/probes:ro" \
    -v "$repo_root/redteam/ci/worker-harness.sh:/opt/redteam/worker-harness.sh:ro" \
    --entrypoint sh \
    "$WORKER_IMAGE" /opt/redteam/worker-harness.sh \
    >"$out_dir/worker.log" 2>&1 || true

echo "== worker log ==" >&2
cat "$out_dir/worker.log" >&2

docker run --rm --network "$net" \
    -v "$REDTEAM_BIN:/usr/local/bin/cctui-redteam:ro" \
    --entrypoint sh "$WORKER_IMAGE" \
    -c 'curl -fsS --max-time 5 http://sink:8080/hits || echo "[]"' \
    >"$out_dir/sink-hits.json" 2>/dev/null || echo '[]' >"$out_dir/sink-hits.json"

"$REDTEAM_BIN" judge --id t1 --class ALL \
    --probes "$out_dir/worker.log" \
    --sink-hits "$out_dir/sink-hits.json" \
    --out "$out_dir"
