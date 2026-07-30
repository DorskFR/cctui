#!/usr/bin/env bash
# The worker image must pin an EXACT Claude Code version: `latest`/`stable`
# resolve at build time, so two builds of the same commit can ship different
# harnesses and the daemon's version gate then cycles against a binary nobody
# tested.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dockerfile="$repo_root/deploy/worker.Dockerfile"

fail() { echo "::error::$*" >&2; exit 1; }

[ -f "$dockerfile" ] || fail "Dockerfile not found: $dockerfile"

pinned="$(sed -n 's/^ARG CLAUDE_CODE_VERSION=\(.*\)$/\1/p' "$dockerfile" | head -n1)"
[ -n "$pinned" ] || fail "could not read ARG CLAUDE_CODE_VERSION from $dockerfile"

echo "Dockerfile ARG CLAUDE_CODE_VERSION = $pinned"

case "$pinned" in
  latest | stable)
    fail "CLAUDE_CODE_VERSION is floating ('$pinned'). Pin an exact x.y.z."
    ;;
esac

echo "$pinned" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "CLAUDE_CODE_VERSION '$pinned' is not an exact x.y.z version."

upstream="$(curl -fsSL https://downloads.claude.ai/claude-code-releases/latest || true)"
if [ -z "$upstream" ]; then
  echo "warning: could not fetch the upstream latest version; pin format check only."
  exit 0
fi

echo "upstream latest                    = $upstream"
if [ "$pinned" = "$upstream" ]; then
  echo "OK: the worker image pins the current upstream latest ($pinned)."
  exit 0
fi

echo "::notice::Claude Code pin $pinned is behind upstream latest $upstream."
echo "Bump ARG CLAUDE_CODE_VERSION in deploy/worker.Dockerfile when you want the newer harness."
