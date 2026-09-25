#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-ci-green.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/gh" <<'STUB'
#!/usr/bin/env bash
n="$(cat "$GH_STUB/calls")"
n=$((n + 1)); echo "$n" > "$GH_STUB/calls"
sed -n "${n}p" "$GH_STUB/responses"
STUB
chmod +x "$tmp/gh"

responses() { echo 0 > "$tmp/calls"; printf '%s\n' "$@" > "$tmp/responses"; }

failures=0
expect() {
  local want="$1" name="$2" out status
  set +e
  out="$(PATH="$tmp:$PATH" GH_STUB="$tmp" CI_POLL_SECONDS=1 CI_WAIT_SECONDS=2 \
    bash "$script" o/r abc 2>&1)"; status=$?
  set -e
  if [ "$status" -eq "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want)"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

responses "completed success https://x/1"
expect 0 "a successful run passes"
responses "completed failure https://x/1"
expect 1 "a failed run refuses"
responses "completed cancelled https://x/1"
expect 1 "a cancelled run refuses"
responses ""
expect 1 "no run refuses"
responses "in_progress null https://x/1" "queued null https://x/1" "completed success https://x/1"
expect 0 "an in-progress run is waited on"
responses "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1" "in_progress null https://x/1"
expect 1 "a run still going after the timeout refuses"

[ "$failures" -eq 0 ] || exit 1
echo "all ci-green cases passed"
