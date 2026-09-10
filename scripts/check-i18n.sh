#!/usr/bin/env bash
# Guards webui/messages/*.json: no sentence-fragment keys, and every locale
# carries the same key set as the base locale.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dir="${1:-$root/webui/messages}"
base="$dir/en.json"

# Do not add to this list. The replacement message providers_advanced_help
# already exists in both locales; swapping AdvancedPage.svelte over to it drops
# these two keys and this entry with them.
ALLOWLIST="providers_advanced_help_before providers_advanced_help_after"

keys() { grep -oE '^[[:space:]]*"[A-Za-z0-9_$]+"[[:space:]]*:' "$1" | tr -d ' \t":' | sort; }

allowed() {
  for a in $ALLOWLIST; do [ "$a" = "$1" ] && return 0; done
  return 1
}

status=0

for f in "$dir"/*.json; do
  while read -r k; do
    [ -n "$k" ] || continue
    allowed "$k" && continue
    echo "✖ ${f#"$root"/}: fragment key \"$k\""
    status=1
  done < <(keys "$f" | grep -E '_(pre|post|before|after|prefix|suffix)$' || true)
done

if [ "$status" -ne 0 ]; then
  echo "  A message key ending in _pre|_post|_before|_after|_prefix|_suffix is a"
  echo "  sentence fragment. Merge the pair into one message with a named param"
  echo "  (e.g. \"Install {daemon} on the target machine.\") and render the inline"
  echo "  markup with src/lib/components/atoms/InlineCode.svelte."
fi

for f in "$dir"/*.json; do
  [ "$f" = "$base" ] && continue
  if ! diff_out=$(diff <(keys "$base") <(keys "$f")); then
    echo "✖ ${f#"$root"/} key set differs from en.json:"
    echo "$diff_out" | sed -n 's/^< /  only in en.json: /p;s/^> /  only in this locale: /p'
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  n=$(keys "$base" | wc -l | tr -d ' ')
  echo "✓ no fragment keys; all locales in sync ($n keys)"
fi

exit "$status"
