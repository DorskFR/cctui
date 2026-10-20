#!/usr/bin/env bash
# The drift checks degrade to a format check on a runner, which has no harness
# on PATH. This reads every harness version out of a built worker image and
# checks it against deploy/worker.Dockerfile, so it must run BEFORE the image
# is pushed: that is the only point where a wrong harness can still fail the
# release.
#
# One row per harness: name | binary | Dockerfile ARG | policy. `floor` hands
# the version to that harness's drift script (a minimum); `exact` requires the
# baked version to equal the ARG.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dockerfile="$repo_root/deploy/worker.Dockerfile"
image="${1:-}"

fail() { echo "::error::$*" >&2; exit 1; }

[ -n "$image" ] || fail "usage: $(basename "$0") <worker-image>"

docker_bin="${DOCKER:-docker}"

HARNESSES=(
  "claude|claude|CLAUDE_CODE_VERSION|floor"
  "codex|codex|CODEX_VERSION|floor"
  "opencode|opencode|OPENCODE_VERSION|exact"
  "gemini|gemini|GEMINI_CLI_VERSION|exact"
  "qwen|qwen|QWEN_CODE_VERSION|exact"
  "goose|goose|GOOSE_VERSION|exact"
)

harness_version() {
  local bin="$1" out version
  out="$("$docker_bin" run --rm --entrypoint "$bin" "$image" --version 2>&1)" \
    || fail "could not run '$bin --version' in $image: $out"
  version="$(printf '%s\n' "$out" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1 || true)"
  [ -n "$version" ] || fail "'$bin --version' in $image printed no x.y.z version: $out"
  printf '%s\n' "$version"
}

dockerfile_arg() {
  local arg="$1" value
  value="$(sed -n "s/^ARG ${arg}=\(.*\)$/\1/p" "$dockerfile" | head -n1)"
  [ -n "$value" ] || fail "deploy/worker.Dockerfile declares no ARG ${arg}"
  printf '%s\n' "$value"
}

check_floor() {
  local name="$1" version="$2"
  case "$name" in
    claude) CLAUDE_INSTALLED_VERSION="$version" bash "$repo_root/scripts/check-claude-version-drift.sh" ;;
    codex)  CODEX_INSTALLED_VERSION="$version"  bash "$repo_root/scripts/check-codex-version-drift.sh" ;;
    *) fail "no drift script for floor harness '$name'" ;;
  esac
}

check_exact() {
  local name="$1" version="$2" pinned="$3"
  [ "$version" = "$pinned" ] \
    || fail "$name in $image is $version, deploy/worker.Dockerfile pins $pinned"
}

echo "worker image                = $image"
for row in "${HARNESSES[@]}"; do
  IFS='|' read -r name bin arg policy <<< "$row"
  version="$(harness_version "$bin")"
  pinned="$(dockerfile_arg "$arg")"
  printf 'image %-10s --version   = %s (%s %s)\n' "$bin" "$version" "$policy" "$pinned"
  case "$policy" in
    floor) check_floor "$name" "$version" ;;
    exact) check_exact "$name" "$version" "$pinned" ;;
    *) fail "unknown policy '$policy' for $name" ;;
  esac
done

echo "OK: the harnesses baked into $image match deploy/worker.Dockerfile."
