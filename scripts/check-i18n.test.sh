#!/usr/bin/env bash
# Runs the real check-i18n.sh against synthetic message dirs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/check-i18n.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

mk() {
  local dir="$tmp/$1"; shift
  mkdir -p "$dir"
  printf '{\n\t"$schema": "x",\n' > "$dir/en.json"
  printf '{\n\t"$schema": "x",\n' > "$dir/fr.json"
  for spec in "$@"; do
    local loc="${spec%%:*}" key="${spec#*:}"
    printf '\t"%s": "v",\n' "$key" >> "$dir/$loc.json"
  done
  for loc in en fr; do
    printf '\t"zz_last": "v"\n}\n' >> "$dir/$loc.json"
  done
  echo "$dir"
}

good="$(mk good en:common_close fr:common_close)"
check "clean dir passes" "$guard" "$good"

frag="$(mk frag en:home_enroll_install_before fr:home_enroll_install_before)"
check "fragment key fails" bash -c '! "$1" "$2"' _ "$guard" "$frag"
check "fragment key is named in the output" \
  bash -c '"$1" "$2" 2>&1 | grep -q home_enroll_install_before' _ "$guard" "$frag"

for suffix in pre post before after prefix suffix; do
  d="$(mk "s_$suffix" "en:some_key_$suffix" "fr:some_key_$suffix")"
  check "_$suffix is rejected" bash -c '! "$1" "$2"' _ "$guard" "$d"
done

drift="$(mk drift en:only_in_en fr:only_in_fr)"
check "locale key drift fails" bash -c '! "$1" "$2"' _ "$guard" "$drift"
check "drift output names both sides" \
  bash -c '"$1" "$2" 2>&1 | grep -q only_in_en' _ "$guard" "$drift"

check "the repo's own messages pass" "$guard"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
