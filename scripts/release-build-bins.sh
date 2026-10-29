#!/usr/bin/env bash
# Usage: release-build-bins.sh <os> <arch> <target>
# Builds the release binaries for one target in a single cargo invocation and
# stages dist/ (cctui, daemon and core server) and, on linux-amd64,
# image-bins/ (static binaries for deploy/*.Dockerfile prebuilt stages).
set -euo pipefail

os="$1" arch="$2" target="$3"
release="target/$target/release"

pkgs=(cctui-tui cctui-daemon cctui-core-server)
image_bins=()
if [ "$os" = linux ] && [ "$arch" = amd64 ]; then
  image_bins=(cctui-core-server cctui-daemon cctui-guard-proxy cctui-supervisor cctui-guard cctui-dispatcher-kube cctui-orchestrator)
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
cp "$release/cctui-core-server" "dist/cctui-core-server-$os-$arch"

# A daemon whose TLS set-up panics passes `--version`; refuse to publish it.
# The x86 runner cannot execute the arm64 musl build.
if [ "$os" = darwin ] || [ "$arch" = amd64 ]; then
  "dist/cctui-daemon-$os-$arch" selfcheck
  "dist/cctui-core-server-$os-$arch" --version
fi

if [ "${#image_bins[@]}" -gt 0 ]; then
  mkdir -p image-bins
  for b in "${image_bins[@]}"; do cp "$release/$b" image-bins/; done
fi
