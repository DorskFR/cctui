#!/usr/bin/env bash
# Exercises phase_codex_package() from deploy/worker-entrypoint.sh: the baked
# codex package is linked where `codex app-server daemon` looks for it.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
log()  { :; }

eval "$(awk '/^phase_codex_package\(\)/,/^}/' "$ENTRY")"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/pkg/bin"
printf '#!/bin/sh\n' > "$tmp/pkg/bin/codex"
chmod +x "$tmp/pkg/bin/codex"
ln -s bin/codex "$tmp/pkg/codex"
WORKER_UID="$(id -u)"

echo "== 1. links current to the package =="
CODEX_PACKAGE_DIR="$tmp/pkg" CODEX_HOME="$tmp/home1" phase_codex_package
check "current target" "$tmp/pkg" "$(readlink "$tmp/home1/packages/standalone/current")"
check "current/codex runnable" "yes" "$([ -x "$tmp/home1/packages/standalone/current/codex" ] && echo yes)"

echo "== 2. an existing install is left alone =="
mkdir -p "$tmp/home2/packages/standalone/current"
CODEX_PACKAGE_DIR="$tmp/pkg" CODEX_HOME="$tmp/home2" phase_codex_package
check "not a symlink" "" "$(readlink "$tmp/home2/packages/standalone/current")"

echo "== 3. no baked package is a no-op =="
CODEX_PACKAGE_DIR="$tmp/missing" CODEX_HOME="$tmp/home3" phase_codex_package
check "nothing created" "no" "$([ -e "$tmp/home3" ] && echo yes || echo no)"
unset CODEX_PACKAGE_DIR
CODEX_HOME="$tmp/home4" phase_codex_package
check "unset dir is a no-op" "no" "$([ -e "$tmp/home4" ] && echo yes || echo no)"

[ "$fail" -eq 0 ] || exit 1
echo "all codex-package cases passed"
