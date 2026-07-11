#!/usr/bin/env bash
# CCT-630: fail if the pinned Codex version drifts across its sources of truth.
#
# The single source of truth is contract::CODEX_PINNED_VERSION in the daemon.
# The worker image ARG CODEX_VERSION must match it exactly, otherwise the image
# ships a Codex whose app-server protocol may differ from the one the adapter
# handshake / retained JSON Schema were written against.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="$repo_root/crates/cctui-daemon/src/adapters/codex/contract.rs"
dockerfile="$repo_root/deploy/worker.Dockerfile"

fail() { echo "::error::$*" >&2; exit 1; }

[ -f "$contract" ]   || fail "contract file not found: $contract"
[ -f "$dockerfile" ] || fail "Dockerfile not found: $dockerfile"

rust_version="$(sed -n 's/.*CODEX_PINNED_VERSION: &str = "\([^"]*\)".*/\1/p' "$contract" | head -n1)"
docker_version="$(sed -n 's/^ARG CODEX_VERSION=\(.*\)$/\1/p' "$dockerfile" | head -n1)"

[ -n "$rust_version" ]   || fail "could not read CODEX_PINNED_VERSION from $contract"
[ -n "$docker_version" ] || fail "could not read ARG CODEX_VERSION from $dockerfile"

echo "contract CODEX_PINNED_VERSION = $rust_version"
echo "Dockerfile ARG CODEX_VERSION  = $docker_version"

if [ "$rust_version" != "$docker_version" ]; then
  fail "Codex version drift: contract=$rust_version vs Dockerfile=$docker_version. Update both and regenerate the retained schema."
fi

echo "OK: Codex version pin is consistent ($rust_version)."
