#!/usr/bin/env bash
# Usage: ci-changes.sh <base> <head> | --all
# Prints key=true|false per area the diff touches. A Cargo.toml, Cargo.lock or
# webui/package-lock.json diff that only moves the workspace version is ignored.
set -euo pipefail

keys=(ci rust webui deps schema)

if [ "${1:-}" = "--all" ]; then
  for k in "${keys[@]}"; do echo "$k=true"; done
  exit 0
fi

base="$1" head="$2"

normalize_manifest() {
  git show "$1" | awk '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version *=/ { print "version = \"\""; next }
    { print }'
}

normalize_npm_lock() {
  git show "$1" | jq -S 'del(.version, .packages[""].version)'
}

normalize_lock() {
  git show "$1" | awk '
    /^name = "cctui-/ { print; own = 1; next }
    own && /^version = / { print "version = \"\""; own = 0; next }
    { own = 0; print }'
}

version_only() {
  local f="$1" norm=normalize_manifest
  case "$f" in
    Cargo.lock) norm=normalize_lock ;;
    webui/package-lock.json) norm=normalize_npm_lock ;;
  esac
  local before after
  before="$("$norm" "$base:$f" 2>/dev/null)" || return 1
  after="$("$norm" "$head:$f" 2>/dev/null)" || return 1
  [ "$before" = "$after" ]
}

files="$(git diff --name-only --no-renames "$base" "$head")"

ci=false rust=false webui=false deps=false schema=false
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    .github/workflows/ci.yml | .github/actions/* | scripts/ci-changes.sh | scripts/ci-tested-tree.sh) ci=true ;;
  esac
  case "$f" in
    Cargo.toml | Cargo.lock)
      if ! version_only "$f"; then
        rust=true
        [ "$f" = Cargo.lock ] && deps=true
      fi
      ;;
    crates/* | migrations/* | rust-toolchain.toml | rustfmt.toml | .cargo/* \
      | keys/* | packaging/* | plugins/* | fixtures/* | deploy/examples/* \
      | webui/src/lib/bindings/* | webui/src/lib/scrubDetectors.generated.ts \
      | webui/scripts/gen-bindings.sh | webui/scripts/gen-scrub-detectors.mjs)
      rust=true
      ;;
  esac
  case "$f" in
    .cargo/*) deps=true ;;
    webui/package-lock.json) version_only "$f" || deps=true ;;
  esac
  case "$f" in
    docs/journeys/*) rust=true webui=true ;;
    webui/*) webui=true ;;
  esac
  case "$f" in
    crates/cctui-server/src/settings_catalog/*) schema=true ;;
  esac
done <<<"$files"

echo "ci=$ci"
echo "rust=$rust"
echo "webui=$webui"
echo "deps=$deps"
echo "schema=$schema"
