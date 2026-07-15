#!/usr/bin/env bash
# Fails if <range> ADDS a `:global(` line in a webui Svelte file (webui/DESIGN.md
# rule 4). Runs in CI because the lefthook mirror is bypassable via a local
# core.hooksPath override (CCT-670). Forward-only: only added lines are flagged.
set -euo pipefail

range="${1:?usage: no-css-global.sh <git-diff-range>}"

added=$(git diff "$range" -U0 -- ':(glob)webui/**/*.svelte' \
  | awk '/^\+/ && !/^\+\+\+/ && /:global\(/')

if [ -n "$added" ]; then
  echo "✖ New :global(...) CSS override in a Svelte file — forbidden (webui/DESIGN.md rule 4)."
  echo "  Style tsumikit atoms via props/variants, or wrap the styled bit in a LOCAL"
  echo "  element so its scoped CSS reaches it. Offending added lines:"
  echo "$added"
  exit 1
fi

echo "✓ no new :global(...) overrides in webui Svelte files"
