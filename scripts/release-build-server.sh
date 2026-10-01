#!/usr/bin/env bash
# Builds cctui-server natively and stages it in image-bins/. Run it on a host
# whose glibc is no newer than the trixie-slim runtime of deploy/Dockerfile
# (2.41); the distro OpenSSL is linked by soname (libssl.so.3), which the
# runtime provides through libssl3t64.
set -euo pipefail

glibc="${SERVER_GLIBC:-2.41}"

# The Anthropic usage endpoints only serve limit resets to a current Claude Code
# `User-Agent`, so every release bakes in upstream latest.
if [ -z "${CCTUI_CLAUDE_CLI_VERSION:-}" ]; then
  CCTUI_CLAUDE_CLI_VERSION="$(curl -fsSL https://downloads.claude.ai/claude-code-releases/latest)"
fi
echo "$CCTUI_CLAUDE_CLI_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || { echo "bad Claude Code version: '$CCTUI_CLAUDE_CLI_VERSION'" >&2; exit 1; }
export CCTUI_CLAUDE_CLI_VERSION
echo "Claude Code User-Agent version: $CCTUI_CLAUDE_CLI_VERSION"

cargo build --release --locked -p cctui-server

bin="target/release/cctui-server"
mkdir -p image-bins
cp "$bin" image-bins/

# Fail here rather than at container start if the binary needs a newer glibc.
if command -v objdump >/dev/null; then
  needed="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
  echo "highest glibc symbol version: $needed"
  case "$(printf '%s\n%s\n' "${needed#GLIBC_}" "$glibc" | sort -V | tail -1)" in
    "$glibc") ;;
    *) echo "cctui-server requires $needed > $glibc" >&2; exit 1 ;;
  esac
fi
