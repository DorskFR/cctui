#!/usr/bin/env bash
# Usage: check-migrations.sh [repo-root]
set -euo pipefail
shopt -s nullglob

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${1:-$here/..}"

errors=0
fail() {
  echo "check-migrations: $*" >&2
  errors=$((errors + 1))
}

core_rs="$root/crates/cctui-server-core/src/schema.rs"
baseline="$(grep -oE 'LEGACY_BASELINE: i64 = [0-9]+' "$core_rs" 2>/dev/null | grep -oE '[0-9]+$' || true)"
if [ -z "$baseline" ]; then
  echo "check-migrations: no LEGACY_BASELINE in crates/cctui-server-core/src/schema.rs" >&2
  exit 1
fi

for path in "$root"/migrations/*; do
  name="${path##*/}"
  if [[ ! "$name" =~ ^([0-9]{3})_[a-z0-9_]+(\.up|\.down)?\.sql$ ]] || ((10#${BASH_REMATCH[1]} > baseline)); then
    fail "migrations/$name: the legacy stream is frozen at $baseline; add a package migration under crates/<package>/schema/ instead"
  fi
done

for dir in "$root"/crates/*/schema; do
  files=("$dir"/*.sql)
  [ "${#files[@]}" -gt 0 ] || continue
  crate="${dir%/schema}"
  rel="crates/${crate##*/}"
  src="$crate/src/schema.rs"
  if [ ! -f "$src" ]; then
    fail "$rel/schema: no $rel/src/schema.rs registers these migrations"
    continue
  fi

  unset seen
  declare -A seen=()
  for path in "${files[@]}"; do
    name="${path##*/}"
    if [[ ! "$name" =~ ^([0-9]{4})_[a-z0-9_]+\.sql$ ]]; then
      fail "$rel/schema/$name: expected NNNN_name.sql"
      continue
    fi
    version=$((10#${BASH_REMATCH[1]}))
    if [ -n "${seen[$version]:-}" ]; then
      fail "$rel/schema: version $version is used by both ${seen[$version]} and $name"
    else
      seen[$version]="$name"
    fi
    count="$(grep -cF "include_str!(\"../schema/$name\")" "$src" || true)"
    if [ "$count" -eq 0 ]; then
      fail "$rel/schema/$name: not registered in $rel/src/schema.rs"
    elif [ "$count" -gt 1 ]; then
      fail "$rel/schema/$name: registered $count times in $rel/src/schema.rs"
    fi
  done

  max=0
  for version in "${!seen[@]}"; do
    if ((version > max)); then max=$version; fi
  done
  for ((version = 1; version <= max; version++)); do
    [ -n "${seen[$version]:-}" ] || fail "$rel/schema: version $version is missing"
  done

  on_disk="$(printf '%s\n' "${!seen[@]}" | sort -n)"
  registered="$(grep -oE 'version: *[0-9]+' "$src" | grep -oE '[0-9]+$' | sort -n || true)"
  if [ "$registered" != "$on_disk" ]; then
    fail "$rel/src/schema.rs: registered versions [$(tr '\n' ' ' <<<"$registered")] do not match the files' [$(tr '\n' ' ' <<<"$on_disk")]"
  fi
done

[ "$errors" -eq 0 ] || exit 1
