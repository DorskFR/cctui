#!/usr/bin/env bash
# Map a release tag to its channel, as GITHUB_OUTPUT lines.
#   vX.Y.Z         -> stable: full release, marked latest, images move :latest
#   vX.Y.Z-beta.N  -> beta:   pre-release, never latest, images move :beta
#   vX.Y.Z + "candidate" -> stable version as a pre-release, never latest,
#                   images move :candidate (stable daemons of a server pinned to it update)
# Anything else is refused.
set -euo pipefail

tag="${1:?usage: release-channel.sh <tag> [candidate]}"
mode="${2:-}"
num='(0|[1-9][0-9]*)'

if [[ "$tag" =~ ^v${num}\.${num}\.${num}$ && "$mode" == candidate ]]; then
  channel=stable prerelease=true make_latest=false floating_tag=candidate
elif [[ -n "$mode" ]]; then
  echo "release-channel: mode '$mode' only applies to vX.Y.Z tags" >&2
  exit 1
elif [[ "$tag" =~ ^v${num}\.${num}\.${num}$ ]]; then
  channel=stable prerelease=false make_latest=true floating_tag=latest
elif [[ "$tag" =~ ^v${num}\.${num}\.${num}-beta\.${num}$ ]]; then
  channel=beta prerelease=true make_latest=false floating_tag=beta
else
  echo "release-channel: $tag is neither vX.Y.Z nor vX.Y.Z-beta.N" >&2
  exit 1
fi

echo "version=${tag#v}"
echo "channel=$channel"
echo "prerelease=$prerelease"
echo "make_latest=$make_latest"
echo "floating_tag=$floating_tag"
