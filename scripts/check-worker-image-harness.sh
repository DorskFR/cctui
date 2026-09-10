#!/usr/bin/env bash
# The drift checks degrade to a format check on a runner, which has no claude or
# codex on PATH. This reads the versions out of a built worker image and hands
# them over, so it must run BEFORE the image is pushed: that is the only point
# where a harness below the floor can still fail the release.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${1:-}"

fail() { echo "::error::$*" >&2; exit 1; }

[ -n "$image" ] || fail "usage: $(basename "$0") <worker-image>"

docker_bin="${DOCKER:-docker}"

harness_version() {
  local bin="$1" out version
  out="$("$docker_bin" run --rm --entrypoint "$bin" "$image" --version 2>&1)" \
    || fail "could not run '$bin --version' in $image: $out"
  version="$(printf '%s\n' "$out" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1 || true)"
  [ -n "$version" ] || fail "'$bin --version' in $image printed no x.y.z version: $out"
  printf '%s\n' "$version"
}

claude_version="$(harness_version claude)"
codex_version="$(harness_version codex)"

echo "worker image                = $image"
echo "image claude --version      = $claude_version"
echo "image codex --version       = $codex_version"

CLAUDE_INSTALLED_VERSION="$claude_version" \
  bash "$repo_root/scripts/check-claude-version-drift.sh"
CODEX_INSTALLED_VERSION="$codex_version" \
  bash "$repo_root/scripts/check-codex-version-drift.sh"

echo "OK: the harnesses baked into $image satisfy the declared floors."
