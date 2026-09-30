#!/usr/bin/env bash
# Re-verifies each entry's pinned sha256 against the live release asset, so a
# bad bump is caught before a release rather than at an admin's install. Runs in
# the release job only: on every PR a network blip would block unrelated work.
# CCTUI_CATALOG_CHECK_OFFLINE=1 checks the schema and skips every download.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
catalog="${1:-$repo_root/plugins/catalog.json}"

fail() { echo "::error::$*" >&2; exit 1; }

command -v jq >/dev/null 2>&1 || fail "jq is required"
[ -f "$catalog" ] || fail "catalog not found: $catalog"
jq -e . "$catalog" >/dev/null 2>&1 || fail "$catalog is not valid JSON"

count="$(jq '.plugins | length' "$catalog")"
[ "$count" -gt 0 ] || fail "$catalog lists no plugins"
echo "checking $count catalog entr$([ "$count" = 1 ] && echo y || echo ies) in $catalog"

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
  # Read the whole listing first: `grep -q` exits on the first match, and under
  # pipefail the SIGPIPE it deals `tar` would fail a correct archive at random.
  listing=$(tar tzf "$tmp/$id.tgz")
  if ! grep -qx "$id/plugin.json" <<<"$listing"; then
    echo "::error::$id archive has no $id/plugin.json (an npm tarball puts files under package/)"
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
