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

# One stream per directory: crates/<c>/schema/ is registered in src/schema.rs,
# crates/<c>/schema/<stream>/ in src/schema/<stream>.rs. Each registration file
# holds exactly its own stream's `version:` entries.
check_stream() {
  local dir="$1" crate="$2" src_rel="$3" include="$4"
  local rel="crates/${crate##*/}"
  local schema_rel="$rel/${dir#"$crate"/}"
  local files=("$dir"/*.sql)
  [ "${#files[@]}" -gt 0 ] || return 0
  local src="$crate/$src_rel"
  if [ ! -f "$src" ]; then
    fail "$schema_rel: no $rel/$src_rel registers these migrations"
    return 0
  fi

  unset seen
  declare -A seen=()
  local path name version count
  for path in "${files[@]}"; do
    name="${path##*/}"
    if [[ ! "$name" =~ ^([0-9]{4})_[a-z0-9_]+\.sql$ ]]; then
      fail "$schema_rel/$name: expected NNNN_name.sql"
      continue
    fi
    version=$((10#${BASH_REMATCH[1]}))
    if [ -n "${seen[$version]:-}" ]; then
      fail "$schema_rel: version $version is used by both ${seen[$version]} and $name"
    else
      seen[$version]="$name"
    fi
    count="$(grep -cF "include_str!(\"$include$name\")" "$src" || true)"
    if [ "$count" -eq 0 ]; then
      fail "$schema_rel/$name: not registered in $rel/$src_rel"
    elif [ "$count" -gt 1 ]; then
      fail "$schema_rel/$name: registered $count times in $rel/$src_rel"
    fi
  done

  local max=0
  for version in "${!seen[@]}"; do
    if ((version > max)); then max=$version; fi
  done
  for ((version = 1; version <= max; version++)); do
    [ -n "${seen[$version]:-}" ] || fail "$schema_rel: version $version is missing"
  done

  local on_disk registered
  on_disk="$(printf '%s\n' "${!seen[@]}" | sort -n)"
  registered="$(grep -oE 'version: *[0-9]+' "$src" | grep -oE '[0-9]+$' | sort -n || true)"
  if [ "$registered" != "$on_disk" ]; then
    fail "$rel/$src_rel: registered versions [$(tr '\n' ' ' <<<"$registered")] do not match the files' [$(tr '\n' ' ' <<<"$on_disk")]"
  fi
}

for dir in "$root"/crates/*/schema; do
  crate="${dir%/schema}"
  check_stream "$dir" "$crate" "src/schema.rs" "../schema/"
  for sub in "$dir"/*/; do
    sub="${sub%/}"
    stream="${sub##*/}"
    if [[ ! "$stream" =~ ^[a-z0-9_]+$ ]]; then
      fail "crates/${crate##*/}/schema/$stream: expected a lower_snake_case stream name"
      continue
    fi
    for nested in "$sub"/*/; do
      fail "crates/${crate##*/}/schema/$stream/${nested#"$sub"/}: streams do not nest"
    done
    check_stream "$sub" "$crate" "src/schema/$stream.rs" "../../schema/$stream/"
  done
done

[ "$errors" -eq 0 ] || exit 1
