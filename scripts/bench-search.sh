#!/usr/bin/env bash
# Benchmark phrase search through the server's own search endpoints.
#
# Usage: bench-search.sh [options] PHRASE...
#        bench-search.sh [options] --phrases FILE
#
# Every query is a GET, so the run is read-only. Each phrase is sent quoted
# ("…"), i.e. as an exact-phrase query.
#
# Environment:
#   BENCH_URL     server base URL (required), e.g. http://localhost:3000
#   BENCH_TOKEN   bearer token (required unless --dry-run)
#
# Options:
#   --session ID        search inside one conversation (GET /sessions/ID/search)
#                       instead of across sessions (GET /sessions/search)
#   --phrases FILE      phrases, one per line; blank lines and # lines skipped
#   --runs N            queries to send, cycling the phrases (default: one each)
#   --concurrency C     queries in flight at once (default 1)
#   --timeout SECONDS   per-query timeout (default 30)
#   --live-only         cross-session search over live sessions only
#   --dry-run           print the plan and exit without sending anything
set -euo pipefail

usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; }

die() {
  printf 'bench-search: %s\n' "$1" >&2
  exit 2
}

session="" phrases_file="" runs="" concurrency=1 timeout=30 archived=true dry_run=false
phrases=()
while [ $# -gt 0 ]; do
  case "$1" in
    --session) session="${2:?--session needs an id}"; shift 2 ;;
    --phrases) phrases_file="${2:?--phrases needs a file}"; shift 2 ;;
    --runs) runs="${2:?--runs needs a number}"; shift 2 ;;
    --concurrency) concurrency="${2:?--concurrency needs a number}"; shift 2 ;;
    --timeout) timeout="${2:?--timeout needs seconds}"; shift 2 ;;
    --live-only) archived=false; shift ;;
    --dry-run) dry_run=true; shift ;;
    -h | --help) usage; exit 0 ;;
    --) shift; phrases+=("$@"); break ;;
    -*) die "unknown option $1" ;;
    *) phrases+=("$1"); shift ;;
  esac
done

if [ -n "$phrases_file" ]; then
  [ -r "$phrases_file" ] || die "cannot read $phrases_file"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in '' | '#'*) continue ;; esac
    phrases+=("$line")
  done <"$phrases_file"
fi
[ "${#phrases[@]}" -gt 0 ] || die "no phrases given (see --help)"
runs="${runs:-${#phrases[@]}}"
for n in "$runs" "$concurrency" "$timeout"; do
  [[ "$n" =~ ^[1-9][0-9]*$ ]] || die "expected a positive integer, got '$n'"
done
base="${BENCH_URL:-}"
[ -n "$base" ] || die "BENCH_URL is not set"
base="${base%/}"

command -v jq >/dev/null || die "jq is required"
command -v curl >/dev/null || die "curl is required"

uri() { jq -rn --arg v "$1" '$v | @uri'; }

if [ -n "$session" ]; then
  endpoint="/api/v1/sessions/$(uri "$session")/search"
  extra=""
else
  endpoint="/api/v1/sessions/search"
  extra="&include_archived=$archived"
fi

query_url() {
  local phrase="$1"
  printf '%s%s?q=%s%s' "$base" "$endpoint" "$(uri "\"$phrase\"")" "$extra"
}

if $dry_run; then
  printf 'endpoint     GET %s%s\n' "$base" "$endpoint"
  printf 'queries      %s (%s distinct phrases)\n' "$runs" "${#phrases[@]}"
  printf 'concurrency  %s\n' "$concurrency"
  printf 'timeout      %ss per query\n' "$timeout"
  printf 'token        %s\n' "$([ -n "${BENCH_TOKEN:-}" ] && echo set || echo 'NOT SET')"
  for ((i = 0; i < runs; i++)); do
    printf '  %3d  %s\n' "$((i + 1))" "$(query_url "${phrases[i % ${#phrases[@]}]}")"
  done
  exit 0
fi

[ -n "${BENCH_TOKEN:-}" ] || die "BENCH_TOKEN is not set"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The token goes through a header file so it never shows in the process list.
(umask 077 && printf 'Authorization: Bearer %s\n' "$BENCH_TOKEN" >"$work/auth")
results="$work/results"
: >"$results"

# One result line per query: status seconds partial hits.
# status is the HTTP code, "timeout", or "error".
one_query() {
  local i="$1" phrase="$2" body="$work/body.$1" out code secs rc=0
  out="$(curl -sS -o "$body" -H "@$work/auth" -H 'Accept: application/json' \
    --max-time "$timeout" -w '%{http_code} %{time_total}' "$(query_url "$phrase")" 2>/dev/null)" || rc=$?
  code="${out%% *}" secs="${out##* }"
  if [ "$rc" -eq 28 ]; then
    printf 'timeout %s - 0\n' "$timeout" >>"$results"
  elif [ "$rc" -ne 0 ]; then
    printf 'error %s - 0\n' "${secs:-0}" >>"$results"
  elif [ "$code" = 200 ]; then
    jq -r --arg secs "$secs" \
      '"200 \($secs) \(.partial // false) \((.hits // .sessions // []) | length)"' \
      "$body" >>"$results" 2>/dev/null || printf 'error %s - 0\n' "$secs" >>"$results"
  else
    printf '%s %s - 0\n' "$code" "$secs" >>"$results"
  fi
  rm -f "$body"
  printf 'query %d/%d done\n' "$i" "$runs" >&2
}

for ((i = 0; i < runs; i++)); do
  while [ "$(jobs -rp | wc -l)" -ge "$concurrency" ]; do
    wait -n || true
  done
  one_query "$((i + 1))" "${phrases[i % ${#phrases[@]}]}" &
done
wait

sort -k2,2g "$results" | awk -v want="$runs" '
  { n++; secs[n] = $2; status[$1]++
    if ($1 == "200") { ok++; if ($3 == "true") partial++ } }
  END {
    if (n != want) printf "warning: %d of %d results recorded\n", n, want
    printf "queries   %d (latencies below include timeouts and errors)\n", n
    for (s in status) printf "  status %-8s %d\n", s, status[s]
    split("50 95 99", ps, " ")
    for (k = 1; k <= 3; k++) {
      r = int((ps[k] / 100) * n); if (r < (ps[k] / 100) * n) r++
      if (r < 1) r = 1
      printf "p%-2s       %.3fs\n", ps[k], secs[r]
    }
    printf "max       %.3fs\n", secs[n]
    if (ok > 0) printf "partial   %d/%d answers (%.1f%%)\n", partial, ok, 100 * partial / ok
    else print "partial   n/a (no 200 answers)"
  }
'
