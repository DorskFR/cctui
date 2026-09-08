#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-claude-version-drift.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fixture() { printf 'ARG CLAUDE_CODE_VERSION=%s\n' "$1" > "$tmp/Dockerfile"; echo "$tmp/Dockerfile"; }

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

expect 0 "installed above the floor passes" \
  env CLAUDE_INSTALLED_VERSION=2.1.263 bash "$script" "$(fixture 2.1.258)"
expect 0 "installed equal to the floor passes" \
  env CLAUDE_INSTALLED_VERSION=2.1.258 bash "$script" "$(fixture 2.1.258)"
expect 1 "installed below the floor fails" \
  env CLAUDE_INSTALLED_VERSION=2.1.9 bash "$script" "$(fixture 2.1.258)"
expect 1 "a floating floor fails" \
  env CLAUDE_INSTALLED_VERSION=2.1.263 bash "$script" "$(fixture latest)"
expect 1 "a non-x.y.z floor fails" \
  env CLAUDE_INSTALLED_VERSION=2.1.263 bash "$script" "$(fixture 2.1)"
expect 0 "no installed binary is not a failure" \
  env CLAUDE_INSTALLED_VERSION= PATH=/usr/bin:/bin bash "$script" "$(fixture 2.1.258)"

[ "$failures" -eq 0 ] || exit 1
echo "all drift-check cases passed"
