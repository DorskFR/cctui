#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/release-version.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

manifest() {
  printf '[workspace]\nmembers = ["crates/a"]\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace.dependencies]\nversion = "9.9.9"\n' "$1" > "$tmp/Cargo.toml"
}
binary() {
  printf '#!/usr/bin/env bash\necho "cctui-daemon %s"\n' "$1" > "$tmp/daemon"
  chmod +x "$tmp/daemon"
}

failures=0
expect() {
  local name="$1" want="$2"; shift 2
  local out
  if out="$(bash "$script" "$@" 2>/dev/null)"; then :; else out="refused"; fi
  if [ "$out" = "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name (wanted '$want', got '$out')"
    failures=$((failures + 1))
  fi
}

manifest 0.25.0
expect "stable tag on a stable tree" version=0.25.0 v0.25.0 --manifest "$tmp/Cargo.toml"
expect "beta tag on a stable tree" refused v0.25.0-beta.30 --manifest "$tmp/Cargo.toml"
expect "patch tag on the minor tree" refused v0.25.1 --manifest "$tmp/Cargo.toml"
manifest 0.25.0-beta.30
expect "stable tag on a beta tree" refused v0.25.0 --manifest "$tmp/Cargo.toml"
expect "beta tag on its beta tree" version=0.25.0-beta.30 v0.25.0-beta.30 --manifest "$tmp/Cargo.toml"
expect "an older beta tag" refused v0.25.0-beta.3 --manifest "$tmp/Cargo.toml"
printf '[workspace]\nmembers = []\n' > "$tmp/Cargo.toml"
expect "a manifest without a workspace version" refused v0.25.0 --manifest "$tmp/Cargo.toml"
expect "a missing manifest" refused v0.25.0 --manifest "$tmp/missing.toml"

manifest 0.25.0
binary "0.25.0 (stable)"
expect "a binary reporting the tag version" version=0.25.0 v0.25.0 --binary "$tmp/daemon"
binary "0.25.0-beta.30 (beta)"
expect "a beta binary for a stable tag" refused v0.25.0 --binary "$tmp/daemon"
expect "a beta binary for its beta tag" version=0.25.0-beta.30 v0.25.0-beta.30 --binary "$tmp/daemon"
binary "0.25.10 (stable)"
expect "a version that only contains the tag version" refused v0.25.1 --binary "$tmp/daemon"
expect "manifest and binary checked together" refused v0.25.0 --manifest "$tmp/Cargo.toml" --binary "$tmp/daemon"
binary "0.25.0 (stable)"
expect "manifest and binary both match" version=0.25.0 v0.25.0 --manifest "$tmp/Cargo.toml" --binary "$tmp/daemon"
printf '#!/usr/bin/env bash\nexit 3\n' > "$tmp/daemon"
expect "a binary that cannot print its version" refused v0.25.0 --binary "$tmp/daemon"
expect "an unknown flag" refused v0.25.0 --bogus

[ "$failures" -eq 0 ] || exit 1
echo "all release-version cases passed"
