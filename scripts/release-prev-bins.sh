#!/usr/bin/env bash
# Usage: release-prev-bins.sh <image-ref> <out-dir> <bin>...
# Copies /usr/local/bin/<bin> out of a published image, so an image whose
# Dockerfile changed without any Rust change is rebuilt around the binaries it
# already shipped.
set -euo pipefail

ref="${1:?usage: release-prev-bins.sh <image-ref> <out-dir> <bin>...}"
out="${2:?usage: release-prev-bins.sh <image-ref> <out-dir> <bin>...}"
shift 2
[ "$#" -gt 0 ] || { echo "release-prev-bins: no binaries named" >&2; exit 1; }

docker_bin="${DOCKER:-docker}"
mkdir -p "$out"
cid="$("$docker_bin" create --platform linux/amd64 "$ref")"
trap '"$docker_bin" rm -f "$cid" >/dev/null' EXIT
for b in "$@"; do
  "$docker_bin" cp "$cid:/usr/local/bin/$b" "$out/$b"
  chmod 0755 "$out/$b"
done
