#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-host-deps.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

write_pkg() {
  local path="$tmp/$1" svelte="$2" tsumikit="$3"
  cat > "$path" <<JSON
{
  "name": "${1%.json}",
  "devDependencies": { "svelte": "$svelte" },
  "dependencies": { "@dorsk/tsumikit": "$tsumikit" }
}
JSON
  echo "$path"
}

failures=0
expect() {
  local want="$1" name="$2"; shift 2
  local out status
  set +e
  out="$("$@" 2>&1)"; status=$?
  set -e
  if [ "$status" -eq "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (exit $status, expected $want)"
    echo "$out" | sed 's/^/       /'
    failures=$((failures + 1))
  fi
}

host="$(write_pkg host.json '^5.25.0' '^0.64.0')"

expect 0 "identical ranges pass" \
  bash "$script" "$(write_pkg same.json '^5.25.0' '^0.64.0')" "$host"
expect 1 "a tsumikit drift fails" \
  bash "$script" "$(write_pkg tsu.json '^5.25.0' '^0.38.1')" "$host"
expect 1 "a svelte drift fails" \
  bash "$script" "$(write_pkg svelte.json '^5.56.8' '^0.64.0')" "$host"
expect 1 "a missing declaration fails" \
  bash "$script" "$tmp/nope.json" "$host"

echo '{ "name": "bare" }' > "$tmp/bare.json"
expect 1 "an undeclared dependency fails" bash "$script" "$tmp/bare.json" "$host"

[ "$failures" -eq 0 ] || exit 1
echo "all host-deps cases passed"
