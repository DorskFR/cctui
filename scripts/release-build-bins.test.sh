#!/usr/bin/env bash
# Exercise packaging with a fake compiler; no cross toolchain is required.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/tools"
cat > "$tmp/tools/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" > cargo-args
shift
packages=() target=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) target="$2"; shift 2 ;;
    -p) packages+=("$2"); shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "target/$target/release"
for package in "${packages[@]}"; do
  bin="$package"
  [ "$package" != cctui-tui ] || bin=cctui
  [ "${OMIT_CORE:-false}" != true ] || [ "$bin" != cctui-core-server ] || continue
  printf '#!/usr/bin/env bash\nprintf "%%s\\n" "%s $*" >> invocations\n' "$bin" > "target/$target/release/$bin"
  chmod +x "target/$target/release/$bin"
done
CARGO
chmod +x "$tmp/tools/cargo"
export PATH="$tmp/tools:$PATH"

for platform in linux-amd64 linux-arm64 darwin-arm64; do
  mkdir "$tmp/$platform"
  (
    cd "$tmp/$platform"
    os="${platform%-*}" arch="${platform#*-}"
    bash "$here/release-build-bins.sh" "$os" "$arch" fixture-target
    for binary in cctui cctui-daemon cctui-core-server; do
      test -x "dist/$binary-$platform"
    done
    if [ "$platform" = linux-amd64 ]; then
      cmp dist/cctui-core-server-linux-amd64 image-bins/cctui-core-server
      test -x image-bins/cctui-guard
      grep -qx 'cctui-core-server --version' invocations
    else
      test ! -d image-bins
    fi
    if [ "$os" = darwin ]; then
      grep -q '^build ' cargo-args
      grep -qx 'cctui-core-server --version' invocations
    else
      grep -q '^zigbuild ' cargo-args
    fi
    if [ "$platform" = linux-arm64 ]; then
      test ! -e invocations
    fi
  )
  echo "ok — $platform stages core, daemon and TUI assets"
done
mkdir "$tmp/missing"
if (cd "$tmp/missing" && OMIT_CORE=true bash "$here/release-build-bins.sh" linux amd64 fixture-target) > "$tmp/missing.log" 2>&1; then
  echo 'FAIL — packaging accepted a missing core binary' >&2
  exit 1
fi
echo 'ok — a missing core binary refuses the release'
