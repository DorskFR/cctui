#!/usr/bin/env bash
# Runs the real check-plugin-catalog.sh over scratch catalogs. Offline: the
# release job is what proves the pinned digests against the live assets.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/check-plugin-catalog.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

export CCTUI_CATALOG_CHECK_OFFLINE=1

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}

HEX="$(printf 'a%.0s' $(seq 64))"

entry() {
  cat <<JSON
{"version":1,"plugins":[{"id":"${1:-demo}","name":"Demo","description":"d",
 "version":"1.0.0","url":"${2:-https://example.com/demo.tgz}",
 "sha256":"${3:-$HEX}","homepage":"https://example.com"}]}
JSON
}

run() { printf '%s' "$1" > "$tmp/c.json"; "$guard" "$tmp/c.json" >/dev/null 2>&1; }
not_found() { ! "$guard" "$tmp/nope.json" >/dev/null 2>&1; }
passes() { run "$1"; }
fails() { ! run "$1"; }

check "a well-formed entry passes" passes "$(entry)"
check "an uppercase id fails" fails "$(entry Demo)"
check "an id with a dot fails" fails "$(entry 'de.mo')"
check "a plain http url fails" fails "$(entry demo http://example.com/demo.tgz)"
check "a short sha256 fails" fails "$(entry demo https://example.com/demo.tgz "${HEX:1}")"
check "an uppercase sha256 fails" fails "$(entry demo https://example.com/demo.tgz "$(printf 'A%.0s' $(seq 64))")"
check "an empty plugin list fails" fails '{"version":1,"plugins":[]}'
check "a malformed document fails" fails 'not json'
check "one bad entry among good ones fails the run" fails \
  "$(printf '{"version":1,"plugins":[{"id":"BAD","name":"B","description":"d","version":"1","url":"https://e.example/b.tgz","sha256":"%s"},{"id":"demo","name":"D","description":"d","version":"1","url":"https://e.example/d.tgz","sha256":"%s"}]}' "$HEX" "$HEX")"
check "a missing catalog file fails" not_found
check "the committed catalog satisfies the schema" "$guard"

bundle() {
  rm -rf "$tmp/b" && mkdir -p "$tmp/b/demo/web"
  printf '{"id":"demo"}' > "$tmp/b/demo/plugin.json"
  printf '%s\n' "$1" > "$tmp/b/demo/web/index.js"
  tar czf "$tmp/b.tgz" -C "$tmp/b" demo
  "$guard" --bundle "$tmp/b.tgz" >/dev/null 2>&1
}
bundle_passes() { bundle "$1"; }
bundle_fails() { ! bundle "$1"; }

check "a bundle with process.env fails" bundle_fails 'if (process.env.NODE_ENV !== "production") warn();'
check "a bundle with a bare process. at line start fails" bundle_fails 'process.nextTick(f);'
check "a clean bundle passes" bundle_passes 'const mode = "production";'
check "a member named process passes" bundle_passes 'job.process.start(); const subprocess = 1;'
check "a missing bundle fails" bash -c '! "$0" --bundle /nonexistent.tgz >/dev/null 2>&1' "$guard"

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
