#!/usr/bin/env bash
# Map a release tag to its channel, as GITHUB_OUTPUT lines.
#   vX.Y.Z         -> stable: full release. It takes `latest` and the :latest
#                   image tag only when no higher stable tag already exists
#                   (CCTUI_STABLE_TAGS); a patch on an older line only moves :X.Y.
#   vX.Y.Z-beta.N  -> beta:   pre-release, never latest, images move :beta
#   vX.Y.Z + "candidate" -> stable version as a pre-release, never latest,
#                   images move :candidate (stable daemons of a server pinned to it update)
# Anything else is refused.
#
# CCTUI_STABLE_TAGS: whitespace-separated existing stable tags (vX.Y.Z). Unset
# means "nothing published yet", so the tag is the highest by definition.
set -euo pipefail

tag="${1:?usage: release-channel.sh <tag> [candidate]}"
mode="${2:-}"
num='(0|[1-9][0-9]*)'

is_highest_stable() {
  local candidate="${1#v}" other highest
  highest="$candidate"
  for other in ${CCTUI_STABLE_TAGS:-}; do
    other="${other#v}"
    [[ "$other" =~ ^${num}\.${num}\.${num}$ ]] || continue
    highest="$(printf '%s\n%s\n' "$highest" "$other" | sort -V | tail -n1)"
  done
  [ "$highest" = "$candidate" ]
}

minor_tag=""
if [[ "$tag" =~ ^v${num}\.${num}\.${num}$ && "$mode" == candidate ]]; then
  channel=stable prerelease=true make_latest=false floating_tag=candidate
elif [[ -n "$mode" ]]; then
  echo "release-channel: mode '$mode' only applies to vX.Y.Z tags" >&2
  exit 1
elif [[ "$tag" =~ ^v${num}\.${num}\.${num}$ ]]; then
  version="${tag#v}"
  minor_tag="${version%.*}"
  channel=stable prerelease=false
  if is_highest_stable "$tag"; then
    make_latest=true floating_tag=latest
  else
    make_latest=false floating_tag=""
  fi
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
echo "minor_tag=$minor_tag"
