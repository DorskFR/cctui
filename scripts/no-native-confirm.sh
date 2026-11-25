#!/usr/bin/env bash
# No native window.confirm() in webui Svelte files: destructive actions use Tsumikit ConfirmModal.
# The .ts side is covered by biome's noRestrictedGlobals. Run from the repository root.
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

pathspec=':(glob)webui/**/*.svelte'
pattern='(^|[^.[:alnum:]_$#])confirm[[:space:]]*\(|window\.confirm[[:space:]]*\('
method='(function|async)[[:space:]]+confirm[[:space:]]*\('

if [ "$mode" = all ]; then
  hits=$(git grep -nE "$pattern" -- "$pathspec" | grep -vE "$method" || true)
else
  hits=$(
    git diff "${diff_args[@]}" -- "$pathspec" \
      | awk '/^\+\+\+ /{next} /^\+/{print}' \
      | grep -E "$pattern" \
      | grep -vE "$method" || true
  )
fi

if [ -n "$hits" ]; then
  echo "✖ native confirm() in webui Svelte files:"
  printf '%s\n' "$hits" | sed 's/^/  /'
  echo "  Use Tsumikit ConfirmModal (tone=\"danger\" for destructive actions)."
  exit 1
fi

echo "✓ no native confirm() in webui Svelte files"
