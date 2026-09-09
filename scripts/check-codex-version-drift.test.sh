#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-codex-version-drift.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fixture() {
  printf 'pub const CODEX_MIN_VERSION: &str = "%s";\n' "$1" > "$tmp/contract.rs"
  printf 'ARG CODEX_VERSION=%s\n' "${2:-$1}" > "$tmp/Dockerfile"
  echo "$tmp/contract.rs" "$tmp/Dockerfile"
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

# shellcheck disable=SC2046
{
expect 0 "installed above the floor passes" \
  env CODEX_INSTALLED_VERSION=0.153.9 bash "$script" $(fixture 0.153.4)
expect 0 "installed equal to the floor passes" \
  env CODEX_INSTALLED_VERSION=0.153.4 bash "$script" $(fixture 0.153.4)
expect 1 "installed below the floor fails" \
  env CODEX_INSTALLED_VERSION=0.144.1 bash "$script" $(fixture 0.153.4)
expect 1 "contract and Dockerfile disagreeing fails" \
  env CODEX_INSTALLED_VERSION=0.153.4 bash "$script" $(fixture 0.153.4 0.144.1)
expect 1 "a floating floor fails" \
  env CODEX_INSTALLED_VERSION=0.153.4 bash "$script" $(fixture latest)
expect 1 "a non-x.y.z floor fails" \
  env CODEX_INSTALLED_VERSION=0.153.4 bash "$script" $(fixture 0.153)
expect 0 "no installed binary is not a failure" \
  env CODEX_INSTALLED_VERSION= PATH=/usr/bin:/bin bash "$script" $(fixture 0.153.4)
}

[ "$failures" -eq 0 ] || exit 1
echo "all drift-check cases passed"
