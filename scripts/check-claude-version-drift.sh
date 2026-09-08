#!/usr/bin/env bash
# CLAUDE_CODE_VERSION is a FLOOR, not an exact pin: derived images (the harbor
# worker bake) refetch the harness, so the repo cannot promise which build ships
# — only that it is never older than the declared version. `latest`/`stable`
# would make even that promise meaningless, so a floor must stay a concrete
# x.y.z.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dockerfile="${1:-$repo_root/deploy/worker.Dockerfile}"

fail() { echo "::error::$*" >&2; exit 1; }

# True when $1 is at least $2.
at_least() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]; }

[ -f "$dockerfile" ] || fail "Dockerfile not found: $dockerfile"

floor="$(sed -n 's/^ARG CLAUDE_CODE_VERSION=\(.*\)$/\1/p' "$dockerfile" | head -n1)"
[ -n "$floor" ] || fail "could not read ARG CLAUDE_CODE_VERSION from $dockerfile"

echo "Dockerfile ARG CLAUDE_CODE_VERSION (floor) = $floor"

case "$floor" in
  latest | stable)
    fail "CLAUDE_CODE_VERSION is floating ('$floor'). Declare a concrete x.y.z floor."
    ;;
esac

echo "$floor" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "CLAUDE_CODE_VERSION '$floor' is not a concrete x.y.z version."

installed="${CLAUDE_INSTALLED_VERSION:-}"
if [ -z "$installed" ] && command -v claude >/dev/null 2>&1; then
  installed="$(claude --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1 || true)"
fi

if [ -n "$installed" ]; then
  echo "installed claude                           = $installed"
  at_least "$installed" "$floor" \
    || fail "installed Claude Code $installed is below the declared floor $floor."
  echo "OK: the installed harness satisfies the floor."
else
  echo "no claude binary to check; floor format check only."
fi

upstream="$(curl -fsSL https://downloads.claude.ai/claude-code-releases/latest || true)"
if [ -z "$upstream" ]; then
  echo "warning: could not fetch the upstream latest version."
  exit 0
fi

echo "upstream latest                            = $upstream"
if at_least "$floor" "$upstream"; then
  echo "OK: the floor is at the current upstream latest ($floor)."
  exit 0
fi

echo "::notice::Claude Code floor $floor is behind upstream latest $upstream."
echo "Raise ARG CLAUDE_CODE_VERSION in deploy/worker.Dockerfile when workers must not run anything older."
