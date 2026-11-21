#!/usr/bin/env bash
# Usage: ci-changes.sh <base> <head> | --all | --rust-files <base> <head>
# Prints key=true|false per area the diff touches. A Cargo.toml, Cargo.lock or
# webui/package-lock.json diff that only moves the workspace version is ignored.
# --rust-files prints the changed paths that make it a Rust change.
# e2e: a spec, its fixtures or the build changed, or a webui/src path names the
# area of a spec (its filename up to the first dash, e.g. tiles-docks → tile).
set -euo pipefail

keys=(ci rust webui deps schema deploy worker e2e)

if [ "${1:-}" = "--all" ]; then
  for k in "${keys[@]}"; do echo "$k=true"; done
  exit 0
fi

mode=keys
if [ "${1:-}" = "--rust-files" ]; then
  mode=rust_files
  shift
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

e2e_areas=()
while IFS= read -r spec; do
  case "$spec" in *probe*) continue ;; esac
  area="${spec##*/}"
  area="${area%%-*}"
  area="${area%.spec.ts}"
  case "$area" in
    *ed) area="${area%ed}" ;;
    *s) area="${area%s}" ;;
  esac
  [ -n "$area" ] && e2e_areas+=("$area")
done < <(git ls-tree --name-only "$head" webui/e2e/ | grep '\.spec\.ts$' || true)

names_e2e_area() {
  local path a
  path="$(tr '[:upper:]' '[:lower:]' <<<"${1#webui/src/}")"
  for a in ${e2e_areas[@]+"${e2e_areas[@]}"}; do
    case "$path" in *"$a"*) return 0 ;; esac
  done
  return 1
}

ci=false rust=false webui=false deps=false schema=false deploy=false worker=false e2e=false
rust_files=()
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    .github/workflows/ci.yml | .github/actions/* | scripts/ci-changes.sh | scripts/ci-tested-tree.sh \
      | scripts/affected-crates.sh) ci=true ;;
  esac
  case "$f" in
    Cargo.toml | Cargo.lock)
      if ! version_only "$f"; then
        rust=true rust_files+=("$f")
        [ "$f" = Cargo.lock ] && deps=true
      fi
      ;;
    crates/* | migrations/* | rust-toolchain.toml | rustfmt.toml | .cargo/* \
      | keys/* | packaging/* | plugins/* | fixtures/* | deploy/examples/* | deploy/workerprofile-crd.yaml \
      | webui/src/lib/bindings/* | webui/src/lib/scrubDetectors.generated.ts \
      | webui/scripts/gen-bindings.sh | webui/scripts/gen-scrub-detectors.mjs)
      rust=true rust_files+=("$f")
      ;;
  esac
  case "$f" in
    .cargo/*) deps=true ;;
    webui/package-lock.json) version_only "$f" || deps=true ;;
  esac
  case "$f" in
    docs/journeys/*) rust=true webui=true rust_files+=("$f") ;;
    webui/*) webui=true ;;
  esac
  case "$f" in
    crates/cctui-server/src/settings_catalog/* | crates/cctui-server-core/src/settings_catalog/*) schema=true ;;
  esac
  case "$f" in
    deploy/* | webui/Dockerfile | webui/nginx.conf | scripts/check-vad-assets.sh) deploy=true ;;
  esac
  case "$f" in
    deploy/worker.Dockerfile | deploy/examples/worker-agents/* | scripts/check-worker-image-harness.sh \
      | scripts/check-claude-version-drift.sh | scripts/check-codex-version-drift.sh) worker=true ;;
  esac
  case "$f" in
    webui/e2e/* | webui/playwright.config.ts | webui/vite.config.ts | webui/svelte.config.js \
      | webui/src/app.html | webui/src/app.css | webui/src/lib/features.fixture* \
      | webui/src/lib/bindings/routes* | webui/scripts/local-token.mjs | webui/plugin-sdk/*)
      e2e=true
      ;;
    webui/package-lock.json) version_only "$f" || e2e=true ;;
    *.test.ts | webui/src/lib/bindings/*) ;;
    webui/src/*) ! names_e2e_area "$f" || e2e=true ;;
  esac
done <<<"$files"

if [ "$mode" = rust_files ]; then
  printf '%s\n' ${rust_files[@]+"${rust_files[@]}"}
  exit 0
fi

echo "ci=$ci"
echo "rust=$rust"
echo "webui=$webui"
echo "deps=$deps"
echo "schema=$schema"
echo "deploy=$deploy"
echo "worker=$worker"
echo "e2e=$e2e"
