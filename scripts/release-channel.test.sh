#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/release-channel.sh"

failures=0
expect() {
  local tag="$1" want="$2" out
  if out="$(bash "$script" "$tag" 2>/dev/null)"; then :; else out="refused"; fi
  if [ "$out" = "$want" ]; then
    echo "ok   — $tag"
  else
    echo "FAIL — $tag"
    diff <(echo "$want") <(echo "$out") | sed 's/^/       /' || true
    failures=$((failures + 1))
  fi
}

expect v0.20.0 "version=0.20.0
channel=stable
prerelease=false
make_latest=true
floating_tag=latest"
expect v1.2.10-beta.3 "version=1.2.10-beta.3
channel=beta
prerelease=true
make_latest=false
floating_tag=beta"
expect v0.21.0-beta.12 "version=0.21.0-beta.12
channel=beta
prerelease=true
make_latest=false
floating_tag=beta"
expect v0.20.0-rc.1 refused
expect v0.20.0-beta refused
expect v0.20.0-beta.01 refused
expect v0.20 refused
expect 0.20.0 refused
expect v0.20.0-beta.1-extra refused
expect "" refused

[ "$failures" -eq 0 ] || exit 1
echo "all release-channel cases passed"
