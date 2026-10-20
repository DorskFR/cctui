#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-ci-green.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin" "$tmp/stub"

# `runs` holds one answer per poll; other endpoints answer from a fixed file.
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
url="$2"
case "$url" in
  *"/runs?head_sha="*)
    n="$(cat "$GH_STUB/runs.n")"
    n=$((n + 1)); echo "$n" > "$GH_STUB/runs.n"
    sed -n "${n}p" "$GH_STUB/runs"
    exit 0
    ;;
  */git/commits/*) key=tree ;;
  *"/actions/artifacts?name=ci-tree-"*) key="artifacts-${url#*name=ci-tree-}"; key="${key%%&*}" ;;
  */actions/runs/*) key="run-${url##*/}" ;;
  *) echo "unexpected gh call: $*" >&2; exit 1 ;;
esac
cat "$GH_STUB/$key" 2>/dev/null || true
STUB
chmod +x "$tmp/bin/gh"

reset() { rm -f "$tmp/stub"/*; echo 0 > "$tmp/stub/runs.n"; : > "$tmp/stub/runs"; }
runs() { printf '%s\n' "$@" > "$tmp/stub/runs"; }
stub() { printf '%s\n' "$2" > "$tmp/stub/$1"; }

failures=0
expect() {
  local want="$1" name="$2" pattern="${3:-}" out status
  set +e
  out="$(PATH="$tmp/bin:$PATH" GH_STUB="$tmp/stub" CI_POLL_SECONDS=1 CI_WAIT_SECONDS=2 \
    bash "$script" o/r abc 2>&1)"; status=$?
  set -e
  if [ "$status" -eq "$want" ] && { [ -z "$pattern" ] || grep -q -- "$pattern" <<<"$out"; }; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want${pattern:+, output matching '$pattern'})"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

reset; runs "completed success https://x/1"
expect 0 "a successful run passes"
reset; runs "completed failure https://x/1"
expect 1 "a failed run refuses"
reset; runs "completed cancelled https://x/1"
expect 1 "a cancelled run refuses"
reset
expect 1 "no run refuses"
reset; runs "in_progress null https://x/1" "queued null https://x/1" "completed success https://x/1"
expect 0 "an in-progress run is waited on"
reset; runs "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1"
expect 1 "a run still going after the timeout refuses"

reset; runs "in_progress null https://x/1"
stub tree T1; stub artifacts-T1 7; stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 0 "a green run on the same tree passes while the commit's run is going" "https://x/7"

reset
stub tree T1; stub artifacts-T1 7; stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 0 "a green run on the same tree passes when the commit has no run" "https://x/7"

reset; runs "in_progress null https://x/1" "in_progress null https://x/1" "completed success https://x/1"
stub tree T1; stub artifacts-T2 7; stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 0 "a green run on another tree is ignored and the commit's run is waited on" "waiting"

reset; runs "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1"
stub tree T1; stub artifacts-T2 7; stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 1 "a green run on another tree does not stand in for a run that never finishes"

reset; runs "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1"
stub tree T1; stub artifacts-T1 8; stub run-8 "completed failure .github/workflows/ci.yml https://x/8"
expect 1 "a red run on the same tree refuses"

reset
stub tree T1; stub artifacts-T1 8; stub run-8 "completed failure .github/workflows/ci.yml https://x/8"
expect 1 "a red run on the same tree with no run on the commit refuses"

reset; runs "completed failure https://x/1"
stub tree T1; stub artifacts-T1 7; stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 1 "the commit's own red run wins over a green run on the same tree"

reset
stub tree T1; stub artifacts-T1 9; stub run-9 "completed success .github/workflows/preview.yml https://x/9"
expect 1 "a green run of another workflow on the same tree does not count"

reset
stub tree T1; stub artifacts-T1 "8
7"
stub run-8 "completed failure .github/workflows/ci.yml https://x/8"
stub run-7 "completed success .github/workflows/ci.yml https://x/7"
expect 0 "any green run among several on the same tree passes" "https://x/7"

[ "$failures" -eq 0 ] || exit 1
echo "all ci-green cases passed"
