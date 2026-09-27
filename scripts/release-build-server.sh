#!/usr/bin/env bash
# Cross-links cctui-server against glibc 2.36 (the bookworm-slim runtime of
# deploy/Dockerfile) and stages it in image-bins/. The distro OpenSSL is linked
# dynamically by soname (libssl.so.3 / libcrypto.so.3), which the runtime
# provides through the libssl3 package.
set -euo pipefail

target=x86_64-unknown-linux-gnu
glibc="${SERVER_GLIBC:-2.36}"

cargo zigbuild --release --locked -p cctui-server --target "$target.$glibc"

bin="target/$target/release/cctui-server"
mkdir -p image-bins
cp "$bin" image-bins/

# Fail here rather than at container start if the binary needs a newer glibc.
if command -v objdump >/dev/null; then
  needed="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
  echo "highest glibc symbol version: $needed"
  case "$(printf '%s\n%s\n' "${needed#GLIBC_}" "$glibc" | sort -V | tail -1)" in
    "$glibc") ;;
    *) echo "cctui-server requires $needed > $glibc" >&2; exit 1 ;;
  esac
fi
