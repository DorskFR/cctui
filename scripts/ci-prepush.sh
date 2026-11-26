#!/usr/bin/env bash
# Usage: ci-prepush.sh schema
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
area="$1"

base=""
if pushed="$(git rev-parse -q --verify '@{push}' 2>/dev/null)"; then
  base="$(git merge-base HEAD "$pushed" 2>/dev/null || true)"
fi
if [ -z "$base" ]; then
  for ref in origin/HEAD origin/main; do
    if git rev-parse -q --verify "$ref" >/dev/null; then
      base="$(git merge-base HEAD "$ref" 2>/dev/null || true)"
      break
    fi
  done
fi

if [ -n "$base" ]; then
  changes="$(bash scripts/ci-changes.sh "$base" HEAD)"
else
  changes="$(bash scripts/ci-changes.sh --all)"
fi
if ! grep -qx "$area=true" <<<"$changes"; then
  echo "pre-push $area: nothing to check since ${base:0:12}"
  exit 0
fi

case "$area" in
  schema)
    url=https://json.schemastore.org/claude-code-settings.json
    vendored=crates/cctui-server-accounts/src/settings_catalog/claude-code-settings.schema.json
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/upstream.json"
    jq -S . "$vendored" > "$tmp/vendored.norm.json"
    jq -S . "$tmp/upstream.json" > "$tmp/upstream.norm.json"
    if ! diff -q "$tmp/vendored.norm.json" "$tmp/upstream.norm.json" >/dev/null; then
      echo "✖ Claude Code settings schema drift: upstream differs from $vendored"
      echo "  Re-vendor:  curl -fsSL $url -o $vendored"
      echo "  Then review catalog.toml for added/removed keys and run:"
      echo "    cargo test -p cctui-server-accounts settings_catalog"
      exit 1
    fi
    ;;
  *)
    echo "usage: ci-prepush.sh schema" >&2
    exit 2
    ;;
esac
