#!/usr/bin/env bash
# Usage: release-build-bins.sh <os> <arch> <target>
# Builds the release binaries for one target in a single cargo invocation and
# stages dist/ (cctui + cctui-daemon) and, on linux-amd64, image-bins/ (the
# static binaries the deploy/*.Dockerfile prebuilt stages COPY in).
set -euo pipefail

os="$1" arch="$2" target="$3"
release="target/$target/release"

pkgs=(cctui-tui cctui-daemon)
image_bins=()
if [ "$os" = linux ] && [ "$arch" = amd64 ]; then
  image_bins=(cctui-daemon cctui-guard-proxy cctui-supervisor cctui-guard cctui-dispatcher-kube cctui-orchestrator)
  pkgs+=(cctui-guard-proxy cctui-supervisor cctui-guard cctui-dispatcher-kube cctui-orchestrator)
fi

args=(--release --locked --target "$target")
for p in "${pkgs[@]}"; do args+=(-p "$p"); done

if [ "$os" = linux ]; then
  cargo zigbuild "${args[@]}"
else
  cargo build "${args[@]}"
fi

mkdir -p dist
cp "$release/cctui" "dist/cctui-$os-$arch"
cp "$release/cctui-daemon" "dist/cctui-daemon-$os-$arch"

if [ "${#image_bins[@]}" -gt 0 ]; then
  mkdir -p image-bins
  for b in "${image_bins[@]}"; do cp "$release/$b" image-bins/; done
fi
