#!/usr/bin/env bash
# Usage: affected-crates.sh <base> <head> | --all
#          Changed workspace packages plus their dependents, one per line.
#        affected-crates.sh test <package>... [-- <cargo test args>... [-- <test binary args>...]]
#          Runs only these packages' test binaries out of the `cargo test
#          --workspace` build: a -p selection unifies dependency features
#          differently and rebuilds the dependencies.
#   CCTUI_CARGO_METADATA  `cargo metadata --no-deps` JSON to use instead of cargo
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(git rev-parse --show-toplevel)"

metadata() {
  if [ -n "${CCTUI_CARGO_METADATA:-}" ]; then
    cat "$CCTUI_CARGO_METADATA"
  else
    cargo metadata --no-deps --format-version 1
  fi
}

meta="$(metadata)"
mapfile -t members < <(jq -r '.packages[].name' <<<"$meta" | sort)

if [ "${1:-}" = test ]; then
  shift
  wanted=()
  while [ "$#" -gt 0 ] && [ "$1" != -- ]; do
    wanted+=("$1")
    shift
  done
  [ "${1:-}" != -- ] || shift
  cargo_args=()
  while [ "$#" -gt 0 ] && [ "$1" != -- ]; do
    cargo_args+=("$1")
    shift
  done
  [ "${1:-}" != -- ] || shift
  if [ "${#wanted[@]}" -eq 0 ]; then
    echo "affected-crates: no package to test" >&2
    exit 0
  fi
  if [ "$(printf '%s\n' "${wanted[@]}" | sort -u)" = "$(printf '%s\n' "${members[@]}")" ]; then
    exec cargo test --workspace ${cargo_args[@]+"${cargo_args[@]}"} -- "$@"
  fi

  manifests="$(jq -r --arg names "${wanted[*]}" \
    '($names | split(" ")) as $w | .packages[] | select(.name | IN($w[])) | .manifest_path' <<<"$meta")"
  exes="$(cargo test --workspace ${cargo_args[@]+"${cargo_args[@]}"} --no-run --message-format=json-render-diagnostics |
    jq -r --arg manifests "$manifests" '
      ($manifests | split("\n")) as $m
      | select(.reason == "compiler-artifact" and .profile.test and .executable != null)
      | select(.manifest_path | IN($m[]))
      | "\(.manifest_path)\t\(.executable)"')"

  failed=()
  while IFS=$'\t' read -r manifest exe; do
    [ -n "$exe" ] || continue
    dir="$(dirname "$manifest")"
    echo "     Running ${exe#"$PWD/"} (${dir#"$PWD/"})"
    if ! (cd "$dir" && CARGO_MANIFEST_DIR="$dir" "$exe" "$@"); then
      failed+=("$exe")
    fi
  done <<<"$exes"
  if [ "${#failed[@]}" -gt 0 ]; then
    printf 'affected-crates: failed: %s\n' "${failed[@]}" >&2
    exit 101
  fi
  exit 0
fi

if [ "${1:-}" = --all ]; then
  printf '%s\n' "${members[@]}"
  exit 0
fi

base="$1" head="$2"
mapfile -t dirs < <(jq -r '.workspace_root as $r | .packages[]
  | "\(.name)\t\(.manifest_path | ltrimstr($r + "/") | rtrimstr("Cargo.toml"))"' <<<"$meta")

declare -A affected=()
is_member() { printf '%s\n' "${members[@]}" | grep -qx -- "$1"; }
add_or_all() {
  if is_member "$1"; then affected[$1]=1; else all=1; fi
}

all=""
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    webui/src/lib/bindings/* | webui/scripts/gen-bindings.sh)
      add_or_all cctui-proto
      continue
      ;;
    webui/src/lib/scrubDetectors.generated.ts | webui/scripts/gen-scrub-detectors.mjs)
      add_or_all cctui-crypto
      continue
      ;;
  esac
  owner=""
  for entry in "${dirs[@]}"; do
    dir="${entry#*$'\t'}"
    case "$f" in "$dir"*) owner="${entry%%$'\t'*}" ;; esac
  done
  if [ -n "$owner" ]; then affected[$owner]=1; else all=1; fi
done < <(bash "$here/ci-changes.sh" --rust-files "$base" "$head")

if [ -n "$all" ]; then
  printf '%s\n' "${members[@]}"
  exit 0
fi

declare -A dependents=()
while IFS=$'\t' read -r dep pkg; do
  [ -n "$dep" ] || continue
  dependents[$dep]+=" $pkg"
done < <(jq -r '[.packages[].name] as $m | .packages[] | .name as $p
  | .dependencies[] | select(.path != null and (.name | IN($m[]))) | "\(.name)\t\($p)"' <<<"$meta")

queue=("${!affected[@]}")
while [ "${#queue[@]}" -gt 0 ]; do
  pkg="${queue[0]}"
  queue=("${queue[@]:1}")
  for d in ${dependents[$pkg]:-}; do
    [ -z "${affected[$d]:-}" ] || continue
    affected[$d]=1
    queue+=("$d")
  done
done

[ "${#affected[@]}" -eq 0 ] || printf '%s\n' "${!affected[@]}" | sort
