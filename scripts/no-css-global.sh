#!/usr/bin/env bash
# Shrinking budget on `:global(` overrides in webui Svelte files (webui/DESIGN.md
# rule 4). The total may never exceed BUDGET; lower BUDGET whenever it drops.
set -euo pipefail

BUDGET=76

case "${1:-}" in
  --staged) grep_args=(--cached) ;;
  "") echo "usage: no-css-global.sh <git-diff-range> | --staged" >&2; exit 2 ;;
  *) grep_args=() ;;
esac

count=$(git grep -o "${grep_args[@]}" ':global(' -- ':(glob)webui/**/*.svelte' | wc -l || true)

if [ "$count" -gt "$BUDGET" ]; then
  echo "✖ :global(...) count is $count, over the budget of $BUDGET."
  echo "  Style tsumikit atoms via props/variants, or wrap the styled bit in a LOCAL"
  echo "  element so its scoped CSS reaches it."
  exit 1
fi

if [ "$count" -lt "$BUDGET" ]; then
  echo "✓ :global(...) count is $count, under the budget of $BUDGET."
  echo "  Lower BUDGET in scripts/no-css-global.sh to $count to lock the win in."
  exit 0
fi

echo "✓ no new :global(...) overrides in webui Svelte files ($count at budget)"
