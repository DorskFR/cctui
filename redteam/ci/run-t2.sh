#!/usr/bin/env bash
# T2 against a live deployment: one probe-only dispatch per valid cell as the
# red-team identity, collect each worker's probe section, read every sink, then
# judge. Exit 0 only when the run is contained.
#
# Inputs (env):
#   REDTEAM_T2_CONFIG          operator config JSON (see t2.example.json)
#   REDTEAM_T2_TOKEN           API key of the dedicated red-team user
#   REDTEAM_BIN                built cctui-redteam binary
#   REDTEAM_T2_EXPECT_VERSION  optional: fail unless the server reports it
#   REDTEAM_T2_KEEP_DIR        optional: private dir the full report is copied to
#   REDTEAM_T2_POLL_SECONDS    collect poll interval (default 10)
# Arg 1: run dir (default .qa/redteam/t2-local). Only t2-gate.json in it is
# safe to publish; the rest names cells, targets and canaries.
set -euo pipefail

: "${REDTEAM_T2_CONFIG:?set REDTEAM_T2_CONFIG to the operator config}"
: "${REDTEAM_T2_TOKEN:?set REDTEAM_T2_TOKEN to the red-team user key}"
: "${REDTEAM_BIN:?set REDTEAM_BIN to a built cctui-redteam binary}"

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
out="${1:-$repo_root/.qa/redteam/t2-local}"
mkdir -p "$out"
cfg="$REDTEAM_T2_CONFIG"
poll="${REDTEAM_T2_POLL_SECONDS:-10}"

field() { jq -r --arg k "$1" '.[$k] // empty | tostring' "$cfg"; }

mask() {
    if [ "${GITHUB_ACTIONS:-}" = true ] && [ -n "$1" ]; then echo "::add-mask::$1"; fi
}

server=$(field server)
identity=$(field identity)
[ -n "$server" ] && [ -n "$identity" ] || { echo "t2: config needs server and identity" >&2; exit 2; }
mask "$server"
while IFS= read -r value; do mask "$value"; done < <(jq -r '
    (.sinks // [])[].hits_url, (.beacon // {} | .[]), (.probe_env // {} | .[])
' "$cfg")

auth_header=$(mktemp)
trap 'rm -f "$auth_header"' EXIT
chmod 600 "$auth_header"
printf 'Authorization: Bearer %s\n' "$REDTEAM_T2_TOKEN" >"$auth_header"

api() {
    local method="$1" path="$2"
    shift 2
    curl -fsS --max-time 30 -X "$method" -H "@$auth_header" \
        -H 'Content-Type: application/json' "$@" "$server/api/v1$path"
}

version=$(curl -fsS --max-time 30 "$server/api/v1/version" | jq -r '.version // empty') || version=""
echo "t2: server reports ${version:-no version}"

me=$(api GET /me) || { echo "t2: the red-team key was refused" >&2; exit 2; }
if [ "$(jq -r '.user_name // empty' <<<"$me")" != "$identity" ] \
    || [ -n "$(jq -r '.machine_id // empty' <<<"$me")" ] \
    || [ "$(jq -r '.role // empty' <<<"$me")" = admin ]; then
    echo "t2: the key is not the dedicated red-team user (non-admin, no machine); refusing" >&2
    exit 2
fi

plan_args=(--config "$cfg" --scenarios "$repo_root/redteam/scenarios" --out "$out/plan.json")
inventory_cmd=$(field inventory_cmd)
if [ -n "$inventory_cmd" ]; then
    sh -c "$inventory_cmd" >"$out/inventory.json" 2>"$out/inventory.err" \
        || { echo "t2: inventory command failed" >&2; exit 2; }
    plan_args+=(--inventory "$out/inventory.json")
fi
"$REDTEAM_BIN" t2 plan "${plan_args[@]}"
run_id=$(jq -r .run_id "$out/plan.json")
cells=$(jq '.cells | length' "$out/plan.json")

fetch_sinks() {
    local suffix="$1" name url
    while IFS=$'\t' read -r name url; do
        if curl -fsS --max-time 15 "$url" >"$out/sink-$name$suffix.tmp" 2>/dev/null; then
            mv "$out/sink-$name$suffix.tmp" "$out/sink-$name$suffix.json"
        else
            rm -f "$out/sink-$name$suffix.tmp"
            echo "t2: sink $name unreadable" >&2
        fi
    done < <(jq -r '(.sinks // [])[] | [.name, .hits_url] | @tsv' "$cfg")
}
fetch_sinks .before

: >"$out/dispatched.jsonl"
for ((i = 0; i < cells; i++)); do
    jq ".cells[$i].request" "$out/plan.json" >"$out/request-$i.json"
    if resp=$(api POST /sessions/dispatch --data "@$out/request-$i.json"); then
        jq -c --argjson i "$i" '{index: $i, session_id, handle, namespace, dispatcher, status}' \
            <<<"$resp" >>"$out/dispatched.jsonl"
        echo "t2: cell $i $(jq -r '.cells['"$i"'].cell | "\(.harness)/\(.dispatcher)/\(.net_mode)/\(.posture)"' "$out/plan.json"): $(jq -r .status <<<"$resp")"
    else
        echo "t2: cell $i dispatch refused" >&2
    fi
done

collect=$(field collect)
cleanup=$(field cleanup)
timeout_minutes=$(jq -r '.timeout_minutes // 15' "$cfg")
deadline=$(($(date +%s) + timeout_minutes * 60 + 120))

with_cell() {
    local row="$1" cmd="$2"
    RT_INDEX=$(jq -r .index <<<"$row") \
    RT_SESSION_ID=$(jq -r '.session_id // ""' <<<"$row") \
    RT_HANDLE=$(jq -r '.handle // ""' <<<"$row") \
    RT_NAMESPACE=$(jq -r '.namespace // ""' <<<"$row") \
    RT_DISPATCHER=$(jq -r '.dispatcher // ""' <<<"$row") \
        sh -c "$cmd"
}

mapfile -t pending <"$out/dispatched.jsonl"
if [ -n "$collect" ]; then
    while [ "${#pending[@]}" -gt 0 ] && [ "$(date +%s)" -lt "$deadline" ]; do
        still=()
        for row in "${pending[@]}"; do
            idx=$(jq -r .index <<<"$row")
            with_cell "$row" "$collect" >"$out/cell-$idx.log" 2>/dev/null || true
            if grep -Eq "REDTEAM-T2-END $run_id/$idx[[:space:]]*\$" "$out/cell-$idx.log"; then
                echo "t2: cell $idx collected"
            else
                still+=("$row")
            fi
        done
        pending=("${still[@]+"${still[@]}"}")
        [ "${#pending[@]}" -eq 0 ] || sleep "$poll"
    done
else
    echo "t2: config has no collect command; every cell is inconclusive" >&2
fi
for row in "${pending[@]+"${pending[@]}"}"; do
    echo "t2: cell $(jq -r .index <<<"$row") never finished its probe section" >&2
done

if [ -n "$cleanup" ]; then
    while IFS= read -r row; do
        with_cell "$row" "$cleanup" >/dev/null 2>&1 || true
    done <"$out/dispatched.jsonl"
fi

fetch_sinks ""

judge_args=(--plan "$out/plan.json" --config "$cfg" --dir "$out" --server-version "${version:-unknown}")
if [ -n "${REDTEAM_T2_EXPECT_VERSION:-}" ]; then
    judge_args+=(--expect-version "$REDTEAM_T2_EXPECT_VERSION")
fi
set +e
"$REDTEAM_BIN" t2 judge "${judge_args[@]}"
rc=$?
set -e

if [ -n "${REDTEAM_T2_KEEP_DIR:-}" ]; then
    mkdir -p "$REDTEAM_T2_KEEP_DIR/$run_id"
    cp -r "$out"/. "$REDTEAM_T2_KEEP_DIR/$run_id/"
fi
exit "$rc"
