#!/usr/bin/env bash
# No `:global(` overrides in webui Svelte files (webui/DESIGN.md rule 4).
# Run from the repository root.
set -euo pipefail

BUDGET=0

usage='usage: no-css-global.sh <git-diff-range> | --staged | --all'

mode=diff
case "${1:-}" in
  --all) mode=all ;;
  --staged) diff_args=(--cached) ;;
  "" | -h | --help) echo "$usage" >&2; exit 2 ;;
  -*) echo "$usage" >&2; exit 2 ;;
  *) diff_args=("$1") ;;
esac

pathspec=':(glob)webui/**/*.svelte'

if [ "$mode" = all ]; then
  count=$(git grep -o ':global(' -- "$pathspec" | wc -l || true)
  subject="count"
else
  count=$(
    git diff "${diff_args[@]}" -- "$pathspec" \
      | awk '/^\+\+\+ /{next} /^\+/{print}' \
      | grep -o ':global(' \
      | wc -l || true
  )
  subject="added count"
fi

if [ "$count" -gt "$BUDGET" ]; then
  echo "✖ :global(...) $subject is $count, over the budget of $BUDGET."
  echo "  Style tsumikit atoms via props/variants, or wrap the styled bit in a LOCAL"
  echo "  element so its scoped CSS reaches it."
  exit 1
fi

if [ "$mode" = all ] && [ "$count" -lt "$BUDGET" ]; then
  echo "✓ :global(...) count is $count, under the budget of $BUDGET."
  echo "  Lower BUDGET in scripts/no-css-global.sh to $count to lock the win in."
  exit 0
fi

echo "✓ no new :global(...) overrides in webui Svelte files ($count at budget)"
