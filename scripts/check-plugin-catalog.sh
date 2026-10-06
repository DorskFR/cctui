#!/usr/bin/env bash
# Re-verifies each entry's pinned sha256 against the live release asset, and
# runs it through the installer's own validation (cctui-server), so a
# bad bump is caught before a release rather than at an admin's install. Runs in
# the release job only: on every PR a network blip would block unrelated work.
# CCTUI_CATALOG_CHECK_OFFLINE=1 checks the schema and skips every download.
# `--bundle <archive.tgz>` runs only the bundle checks on a local archive.
set -euo pipefail

fail() { echo "::error::$*" >&2; exit 1; }

# The host imports plugin JS straight into the browser, where `process` does
# not exist; a library-mode build leaves `process.env.NODE_ENV` unreplaced.
bare_process() {
  grep -rlE --include='*.js' --include='*.mjs' '(^|[^.$[:alnum:]_])process\.' "$1" || true
}

if [ "${1:-}" = "--bundle" ]; then
  archive="${2:-}"
  [ -f "$archive" ] || fail "bundle not found: $archive"
  dir="$(mktemp -d)"
  trap 'rm -rf "$dir"' EXIT
  tar xzf "$archive" -C "$dir"
  hits="$(bare_process "$dir")"
  [ -z "$hits" ] || fail "bundle references bare process. (define process.env.NODE_ENV): ${hits//$dir\//}"
  echo "bundle ok"
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
catalog="${1:-$repo_root/plugins/catalog.json}"

command -v jq >/dev/null 2>&1 || fail "jq is required"
[ -f "$catalog" ] || fail "catalog not found: $catalog"
jq -e . "$catalog" >/dev/null 2>&1 || fail "$catalog is not valid JSON"

count="$(jq '.plugins | length' "$catalog")"
[ "$count" -gt 0 ] || fail "$catalog lists no plugins"
echo "checking $count catalog entr$([ "$count" = 1 ] && echo y || echo ies) in $catalog"

# The installer's own validation; CCTUI_ARCHIVE_CHECK overrides the command.
archive_check="${CCTUI_ARCHIVE_CHECK:-cargo run -q --manifest-path $repo_root/Cargo.toml -p cctui-server -- check-plugin-archive}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

problems=0
while IFS=$'\t' read -r id version url want; do
  if ! printf '%s' "$id" | grep -qE '^[a-z0-9-]{1,40}$'; then
    echo "::error::id '$id' does not match [a-z0-9-]{1,40}"
    problems=$((problems + 1))
    continue
  fi
  if ! printf '%s' "$want" | grep -qE '^[0-9a-f]{64}$'; then
    echo "::error::$id sha256 '$want' is not 64 lowercase hex characters"
    problems=$((problems + 1))
    continue
  fi
  case "$url" in
    https://*) ;;
    *)
      echo "::error::$id url is not https: $url"
      problems=$((problems + 1))
      continue
      ;;
  esac
  if [ -n "${CCTUI_CATALOG_CHECK_OFFLINE:-}" ]; then
    echo "ok   — $id $version (schema only)"
    continue
  fi
  if ! curl -fsSL --retry 3 --retry-delay 2 --max-time 120 -o "$tmp/$id.tgz" "$url"; then
    echo "::error::$id url does not resolve: $url"
    problems=$((problems + 1))
    continue
  fi
  got="$(sha256sum "$tmp/$id.tgz" | cut -d' ' -f1)"
  if [ "$got" != "$want" ]; then
    echo "::error::$id sha256 mismatch: catalog $want, asset $got"
    problems=$((problems + 1))
    continue
  fi
  if ! verdict="$($archive_check "$tmp/$id.tgz" "$id" "$version" 2>&1)"; then
    echo "::error::$id archive would be refused at install: $verdict"
    problems=$((problems + 1))
    continue
  fi
  mkdir -p "$tmp/$id"
  tar xzf "$tmp/$id.tgz" -C "$tmp/$id"
  hits="$(bare_process "$tmp/$id")"
  if [ -n "$hits" ]; then
    echo "::error::$id bundle references bare process. (define process.env.NODE_ENV): ${hits//$tmp\/$id\//}"
    problems=$((problems + 1))
    continue
  fi
  echo "ok   — $id $version $got"
done < <(jq -r '.plugins[] | [.id, .version, .url, .sha256] | @tsv' "$catalog")

[ "$problems" -eq 0 ] || fail "$problems catalog entr$([ "$problems" = 1 ] && echo y || echo ies) failed"
if [ -n "${CCTUI_CATALOG_CHECK_OFFLINE:-}" ]; then
  echo "every catalog entry satisfies the schema"
else
  echo "every catalog entry resolves and matches its sha256"
fi
