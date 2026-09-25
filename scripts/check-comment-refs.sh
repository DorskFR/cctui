#!/usr/bin/env bash
# Rejects ticket ids and wave/lane notes in comments and test names on ADDED
# lines. Checks the staged diff, or `<base-ref>...HEAD` when a ref is given.
set -euo pipefail

if [ $# -gt 0 ]; then
  diff_args=("$1...HEAD")
else
  diff_args=(--cached)
fi

hits=$(git diff -U0 --no-color --diff-filter=AMR "${diff_args[@]}" -- \
  '*.rs' '*.ts' '*.svelte' '*.sh' '*.yml' '*.yaml' 'Makefile' '**/Makefile' |
  awk '
    /^\+\+\+ / { file = substr($0, 7); next }
    /^@@/ {
      split($3, a, ","); line = substr(a[1], 2) + 0; next
    }
    /^\+/ {
      text = substr($0, 2)
      trimmed = text; sub(/^[ \t]+/, "", trimmed)
      if (file ~ /\.(sh|ya?ml)$/ || file ~ /(^|\/)Makefile$/) {
        comment = (trimmed ~ /^#/)
      } else {
        comment = (trimmed ~ /^(\/\/|\/\*|\*|<!--)/)
      }
      bad = 0
      if (comment && (trimmed ~ /CCT-[0-9]+/ || tolower(trimmed) ~ /wave [0-9]+/ || tolower(trimmed) ~ /lane w[0-9]+/)) bad = 1
      if (text ~ /(^|[^A-Za-z0-9_.])(describe|it|test)(\.[a-z]+)?(\([^)]*\))?\([ \t]*["\047`][^"\047`]*CCT-[0-9]+/) bad = 1
      if (bad) printf "%s:%d: %s\n", file, line, trimmed
      line++
    }
  ')

if [ -n "$hits" ]; then
  echo "✖ ticket ids or wave/lane notes in comments or test names:"
  echo "$hits" | sed 's/^/  /'
  echo "  Say what the code does; the ticket belongs in the commit or PR."
  exit 1
fi
