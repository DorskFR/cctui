#!/usr/bin/env bash
# Svelte and @dorsk/tsumikit come from the host at runtime (/plugin-runtime/*),
# so equality — not semver compatibility — is the requirement: a drift
# type-checks fine here and explodes in the browser.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ui="${1:-$here/../package.json}"
host="${2:-$here/../../webui/package.json}"

fail() { echo "::error::$*" >&2; exit 1; }

[ -f "$ui" ] || fail "ghreview-ui package.json not found: $ui"
[ -f "$host" ] || fail "host package.json not found: $host"

read_dep() {
  node -e '
    const p = require(process.argv[1]);
    const v = (p.dependencies ?? {})[process.argv[2]] ?? (p.devDependencies ?? {})[process.argv[2]];
    process.stdout.write(v ?? "");
  ' "$1" "$2"
}

status=0
for dep in svelte @dorsk/tsumikit; do
  want="$(read_dep "$host" "$dep")"
  got="$(read_dep "$ui" "$dep")"
  [ -n "$want" ] || fail "$dep is not declared in $host"
  [ -n "$got" ] || fail "$dep is not declared in $ui"
  if [ "$want" = "$got" ]; then
    printf 'OK   %-18s %s\n' "$dep" "$got"
  else
    printf 'DRIFT %-17s ghreview-ui %s != webui %s\n' "$dep" "$got" "$want"
    status=1
  fi
done

[ "$status" -eq 0 ] || fail "ghreview-ui must declare the host's Svelte and Tsumikit versions exactly."
echo "OK: ghreview-ui builds against the host's shared runtime versions."
