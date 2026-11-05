#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/redteam-t2-gate.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin" "$tmp/stub"
now="$(date +%s)"

# The stub ignores --jq: each file holds the already-filtered answer.
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
case "$1" in
  api)
    case "$2" in
      *"/actions/artifacts?name="*) cat "$GH_STUB/artifacts" ;;
      */actions/runs/*) cat "$GH_STUB/run-${2##*/}" 2>/dev/null || exit 1 ;;
      *) echo "unexpected gh api: $*" >&2; exit 1 ;;
    esac
    ;;
  run)
    id="$3" dir=""
    while [ $# -gt 0 ]; do [ "$1" = -D ] && dir="$2"; shift; done
    cp "$GH_STUB/record-$id" "$dir/t2-gate.json"
    ;;
  *) echo "unexpected gh: $*" >&2; exit 1 ;;
esac
STUB
chmod +x "$tmp/bin/gh"

record() {
  local verdict="$1" cells="$2" contained="$3" at="$4" version="${5:-0.29.0-beta.11}"
  printf '{"schema":1,"run_id":"0123456789ab","version":"%s","verdict":"%s","cells":%s,"contained_cells":%s,"generated_at":%s}\n' \
    "$version" "$verdict" "$cells" "$contained" "$at"
}

failures=0
check() {
  local want="$1" name="$2" status="$3" out="$4"
  if [ "$status" -eq "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want)"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

expect() {
  local want="$1" name="$2" file="$3" version="${4:-0.29.0-beta.11}" out status
  set +e
  out="$(PATH="$tmp/bin:$PATH" GH_STUB="$tmp/stub" REDTEAM_T2_RECORD="$file" \
    bash "$script" "$version" 2>&1)"; status=$?
  set -e
  check "$want" "$name" "$status" "$out"
}

expect_gh() {
  local want="$1" name="$2" out status
  set +e
  out="$(PATH="$tmp/bin:$PATH" GH_STUB="$tmp/stub" GH_REPO=o/r REDTEAM_T2_RECORD='' \
    bash "$script" 0.29.0-beta.11 2>&1)"; status=$?
  set -e
  check "$want" "$name" "$status" "$out"
}

reset() {
  find "$tmp/stub" -mindepth 1 -delete
  : > "$tmp/stub/artifacts"
}

record contained 6 6 "$now" > "$tmp/good.json"
expect 0 "a fresh contained run on the version passes" "$tmp/good.json"
record escaped 6 5 "$now" > "$tmp/escaped.json"
expect 1 "an escaped run refuses" "$tmp/escaped.json"
record inconclusive 6 6 "$now" > "$tmp/inconclusive.json"
expect 1 "an inconclusive run refuses" "$tmp/inconclusive.json"
expect 1 "a run on another version refuses" "$tmp/good.json" "0.29.0-beta.12"
record contained 0 0 "$now" > "$tmp/empty.json"
expect 1 "a run with no cell refuses" "$tmp/empty.json"
record contained 6 5 "$now" > "$tmp/partial.json"
expect 1 "a run with an uncontained cell refuses" "$tmp/partial.json"
record contained 6 6 "$((now - 8 * 86400))" > "$tmp/stale.json"
expect 1 "a stale run refuses" "$tmp/stale.json"
record contained 6 6 "$((now + 86400))" > "$tmp/future.json"
expect 1 "a run dated in the future refuses" "$tmp/future.json"
echo 'not json' > "$tmp/garbage.json"
expect 1 "an unreadable record refuses" "$tmp/garbage.json"
expect 1 "a missing record refuses" "$tmp/missing.json"
expect 1 "an invalid version refuses" "$tmp/good.json" "../x"
record contained 6 6 "$now" "unknown" > "$tmp/unknown.json"
expect 1 "a record without a known version refuses" "$tmp/unknown.json"

reset
printf '%s\n' "11 101" "10 100" > "$tmp/stub/artifacts"
echo "failure .github/workflows/redteam-t2.yml" > "$tmp/stub/run-101"
echo "success .github/workflows/redteam-t2.yml" > "$tmp/stub/run-100"
cp "$tmp/escaped.json" "$tmp/stub/record-101"
cp "$tmp/good.json" "$tmp/stub/record-100"
expect_gh 0 "a failed newer run is skipped for the newest successful one"

reset
echo "10 100" > "$tmp/stub/artifacts"
echo "success .github/workflows/other.yml" > "$tmp/stub/run-100"
cp "$tmp/good.json" "$tmp/stub/record-100"
expect_gh 1 "an artifact from another workflow refuses"

reset
expect_gh 1 "no artifact for the version refuses"

reset
echo "10 100" > "$tmp/stub/artifacts"
echo "success .github/workflows/redteam-t2.yml" > "$tmp/stub/run-100"
cp "$tmp/inconclusive.json" "$tmp/stub/record-100"
expect_gh 1 "a successful run whose record is not contained still refuses"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)"; exit 1; }
echo "all redteam-t2-gate tests passed"
