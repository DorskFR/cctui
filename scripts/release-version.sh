#!/usr/bin/env bash
# Usage: release-version.sh <tag> [--manifest Cargo.toml] [--binary <path>]
# Refuses a release whose tag (minus the `v`) is not exactly the version the
# tree or a built binary reports: a stable tag on a beta tree would otherwise
# publish beta binaries as latest.
#   --manifest  [workspace.package].version must equal the tag version (default: Cargo.toml)
#   --binary    `<path> --version` must report the tag version as a whole word
set -euo pipefail

tag="${1:?usage: release-version.sh <tag> [--manifest Cargo.toml] [--binary <path>]}"
shift
want="${tag#v}"
manifest="" binary=""
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) manifest="${2:?--manifest needs a path}"; shift 2 ;;
    --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
    *) echo "release-version: unknown argument '$1'" >&2; exit 2 ;;
  esac
done
[ -n "$binary" ] || manifest="${manifest:-Cargo.toml}"

if [ -n "$manifest" ]; then
  got="$(awk '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version *= *"/ { sub(/^version *= *"/, ""); sub(/".*$/, ""); print; exit }' "$manifest")"
  if [ "$got" != "$want" ]; then
    echo "release-version: $tag does not match $manifest [workspace.package] version '${got:-<none>}'" >&2
    exit 1
  fi
fi

if [ -n "$binary" ]; then
  out="$("$binary" --version)"
  if ! grep -qE "(^|[[:space:]])${want//./\\.}([[:space:]]|\$)" <<<"$out"; then
    echo "release-version: $binary --version reports '$out', not $want" >&2
    exit 1
  fi
fi

echo "version=$want"
