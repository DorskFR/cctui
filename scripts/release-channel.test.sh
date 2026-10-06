#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/release-channel.sh"

failures=0
expect() {
  local tag="$1" want="$2" mode="${3:-}" out
  if out="$(bash "$script" "$tag" $mode 2>/dev/null)"; then :; else out="refused"; fi
  local name="$tag${mode:+ $mode}${CCTUI_STABLE_TAGS:+ [published: $CCTUI_STABLE_TAGS]}"
  if [ "$out" = "$want" ]; then
    echo "ok   — $name"
  else
    echo "FAIL — $name"
    diff <(echo "$want") <(echo "$out") | sed 's/^/       /' || true
    failures=$((failures + 1))
  fi
}

expect v0.20.0 "version=0.20.0
channel=stable
prerelease=false
make_latest=true
floating_tag=latest
minor_tag=0.20"
expect v1.2.10-beta.3 "version=1.2.10-beta.3
channel=beta
prerelease=true
make_latest=false
floating_tag=beta
minor_tag="
expect v0.21.0-beta.12 "version=0.21.0-beta.12
channel=beta
prerelease=true
make_latest=false
floating_tag=beta
minor_tag="
expect v0.21.0 "version=0.21.0
channel=stable
prerelease=true
make_latest=false
floating_tag=candidate
minor_tag=" candidate
expect v0.21.0-beta.1 refused candidate
expect v0.21.0 refused bogus
expect v0.20.0-rc.1 refused
expect v0.20.0-beta refused
expect v0.20.0-beta.01 refused
expect v0.20 refused
expect 0.20.0 refused
expect v0.20.0-beta.1-extra refused
expect "" refused

CCTUI_STABLE_TAGS="v0.23.0 v0.24.0 v0.25.1"
export CCTUI_STABLE_TAGS

expect v0.24.1 "version=0.24.1
channel=stable
prerelease=false
make_latest=false
floating_tag=
minor_tag=0.24"
expect v0.25.2 "version=0.25.2
channel=stable
prerelease=false
make_latest=true
floating_tag=latest
minor_tag=0.25"
expect v0.25.1 "version=0.25.1
channel=stable
prerelease=false
make_latest=true
floating_tag=latest
minor_tag=0.25"
expect v1.0.0 "version=1.0.0
channel=stable
prerelease=false
make_latest=true
floating_tag=latest
minor_tag=1.0"
expect v0.9.9 "version=0.9.9
channel=stable
prerelease=false
make_latest=false
floating_tag=
minor_tag=0.9"
expect v0.24.1 "version=0.24.1
channel=stable
prerelease=true
make_latest=false
floating_tag=candidate
minor_tag=" candidate

unset CCTUI_STABLE_TAGS

[ "$failures" -eq 0 ] || exit 1
echo "all release-channel cases passed"
