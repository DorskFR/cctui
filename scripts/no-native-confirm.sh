#!/usr/bin/env bash
# No native confirm() in webui: destructive actions use Tsumikit ConfirmModal.
# Svelte files: any call. TS files: member access (window./globalThis./self.confirm),
# since biome's noRestrictedGlobals only sees the bare global. Run from the repository root.
set -euo pipefail

usage='usage: no-native-confirm.sh <git-diff-range> | --staged | --all'

mode=diff
case "${1:-}" in
  --all) mode=all ;;
  --staged) diff_args=(--cached) ;;
  "" | -h | --help) echo "$usage" >&2; exit 2 ;;
  -*) echo "$usage" >&2; exit 2 ;;
  *) diff_args=("$1") ;;
esac

svelte_spec=':(glob)webui/**/*.svelte'
ts_spec=(':(glob)webui/**/*.ts' ':(exclude,glob)webui/src/lib/paraglide/**' ':(exclude,glob)webui/src/lib/bindings/**' ':(exclude,glob)**/node_modules/**')
member='(window|globalThis|self)[[:space:]]*\.[[:space:]]*confirm[[:space:]]*\('
svelte_pattern="(^|[^.[:alnum:]_\$#])confirm[[:space:]]*\(|$member"
method='(function|async)[[:space:]]+confirm[[:space:]]*\('

scan() {
  local pattern="$1"; shift
  if [ "$mode" = all ]; then
    git grep -nE "$pattern" -- "$@" | grep -vE "$method" || true
  else
    git diff "${diff_args[@]}" -- "$@" \
      | awk '/^\+\+\+ /{next} /^\+/{print}' \
      | grep -E "$pattern" \
      | grep -vE "$method" || true
  fi
}

hits=$(
  scan "$svelte_pattern" "$svelte_spec"
  scan "$member" "${ts_spec[@]}"
)

if [ -n "$hits" ]; then
  echo "✖ native confirm() in webui:"
  printf '%s\n' "$hits" | sed 's/^/  /'
  echo "  Use Tsumikit ConfirmModal (tone=\"danger\" for destructive actions)."
  exit 1
fi

echo "✓ no native confirm() in webui"
