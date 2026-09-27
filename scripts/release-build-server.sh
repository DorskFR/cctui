#!/usr/bin/env bash
# Builds cctui-server natively and stages it in image-bins/. Run it on a host
# whose glibc is no newer than the trixie-slim runtime of deploy/Dockerfile
# (2.41); the distro OpenSSL is linked by soname (libssl.so.3), which the
# runtime provides through libssl3t64.
set -euo pipefail

glibc="${SERVER_GLIBC:-2.41}"

cargo build --release --locked -p cctui-server

bin="target/release/cctui-server"
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
