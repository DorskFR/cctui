#!/usr/bin/env bash
# Usage: ci-prepush.sh <rust|clippy|webui|schema>
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
key="$area"
[ "$area" = clippy ] && key=rust
if ! grep -qx "$key=true" <<<"$changes"; then
  echo "pre-push $area: nothing to check since ${base:0:12}"
  exit 0
fi

case "$area" in
  rust | clippy | webui)
    [ -n "${CCTUI_HEAVY:-}" ] || exec bash scripts/heavy.sh bash scripts/ci-prepush.sh "$area"
    ;;
esac

case "$area" in
  clippy)
    exec cargo clippy --workspace --all-targets -- -D warnings
    ;;
  rust)
    # DB-gated tests skip themselves without DATABASE_URL; `make test` runs
    # them against one.
    exec env -u DATABASE_URL -u TEST_DATABASE_URL cargo test --workspace
    ;;
  webui)
    files=()
    if [ -n "$base" ]; then
      while IFS= read -r f; do
        case "$f" in
          *.ts | *.js | *.mjs | *.svelte) files+=("${f#webui/}") ;;
        esac
      done < <(git diff --name-only --diff-filter=d "$base" HEAD -- webui/src webui/scripts)
    fi
    if [ -n "$base" ] && [ "${#files[@]}" -eq 0 ]; then
      echo "pre-push webui: no webui sources changed since ${base:0:12}"
      exit 0
    fi
    if [ ! -d webui/node_modules ]; then
      echo "pre-push webui: webui/node_modules is missing; run 'make webui/install'" >&2
      exit 1
    fi
    cd webui
    npm run --silent codegen:ensure
    if [ "${#files[@]}" -eq 0 ]; then
      exec bash ../scripts/heavy.sh npx --no-install vitest run
    fi
    exec bash ../scripts/heavy.sh npx --no-install vitest related --run --passWithNoTests "${files[@]}"
    ;;
  schema)
    url=https://json.schemastore.org/claude-code-settings.json
    vendored=crates/cctui-server-core/src/settings_catalog/claude-code-settings.schema.json
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/upstream.json"
    jq -S . "$vendored" > "$tmp/vendored.norm.json"
    jq -S . "$tmp/upstream.json" > "$tmp/upstream.norm.json"
    if ! diff -q "$tmp/vendored.norm.json" "$tmp/upstream.norm.json" >/dev/null; then
      echo "✖ Claude Code settings schema drift: upstream differs from $vendored"
      echo "  Re-vendor:  curl -fsSL $url -o $vendored"
      echo "  Then review catalog.toml for added/removed keys and run:"
      echo "    cargo test -p cctui-server-core settings_catalog"
      exit 1
    fi
    ;;
  *)
    echo "usage: ci-prepush.sh <rust|clippy|webui|schema>" >&2
    exit 2
    ;;
esac
