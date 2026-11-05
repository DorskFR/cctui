#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/bench-search.sh"
stub="$(mktemp -d)"
trap 'rm -rf "$stub"' EXIT

cat >"$stub/curl" <<'EOF'
#!/usr/bin/env bash
out="" url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -H | -w | --max-time) shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\n' "$url" >>"$BENCH_STUB_LOG"
case "$url" in
  *slow*) exit 28 ;;
  *part*) printf '{"hits":[1,2],"partial":true}' >"$out"; printf '200 0.900' ;;
  *gone*) printf '{}' >"$out"; printf '404 0.010' ;;
  *) n="${url##*%22q}"; n="${n%%%22*}"
     printf '{"sessions":[1],"partial":false}' >"$out"; printf '200 0.%03d' "$n" ;;
esac
EOF
chmod +x "$stub/curl"

fail() {
  printf 'FAIL: %s\n%s\n' "$1" "$2" >&2
  exit 1
}

run() {
  : >"$stub/log"
  PATH="$stub:$PATH" BENCH_STUB_LOG="$stub/log" BENCH_URL=http://bench.invalid \
    BENCH_TOKEN=secret bash "$script" "$@" 2>/dev/null
}

phrases=()
for i in $(seq 1 17); do phrases+=("q$((i * 10))"); done
out="$(run --concurrency 3 "${phrases[@]}" part1 part2 slow gone)"
grep -q '^queries   21 ' <<<"$out" || fail "query count" "$out"
grep -q 'status 200 *19' <<<"$out" || fail "200 count" "$out"
grep -q 'status timeout *1' <<<"$out" || fail "timeout count" "$out"
grep -q 'status 404 *1' <<<"$out" || fail "404 count" "$out"
grep -q '^partial   2/19 answers (10.5%)' <<<"$out" || fail "partial share" "$out"
grep -q '^p50       0.100s' <<<"$out" || fail "p50" "$out"
grep -q '^p95       0.900s' <<<"$out" || fail "p95" "$out"
grep -q '^p99       30.000s' <<<"$out" || fail "p99 counts the timeout" "$out"
[ "$(wc -l <"$stub/log")" -eq 21 ] || fail "one request per query" "$(cat "$stub/log")"
grep -q 'include_archived=true' "$stub/log" || fail "archived scope by default" "$(cat "$stub/log")"
if grep -q secret "$stub/log"; then fail "token leaked into the URL" "$(cat "$stub/log")"; fi

out="$(run --session s1 --runs 3 q1)"
[ "$(grep -c '/api/v1/sessions/s1/search?q=%22q1%22$' "$stub/log")" -eq 3 ] ||
  fail "conversation endpoint, cycled phrase" "$(cat "$stub/log")"

out="$(run --dry-run --runs 2 a b c)"
[ ! -s "$stub/log" ] || fail "dry run sent requests" "$(cat "$stub/log")"
grep -q 'queries      2 (3 distinct phrases)' <<<"$out" || fail "dry-run plan" "$out"

if run --concurrency 0 a >/dev/null; then fail "concurrency 0 accepted" ""; fi

echo "bench-search tests passed"
