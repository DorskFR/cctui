#!/usr/bin/env bash
# Needs docker. Usage: test-webui-dir-routes.sh [webui/build]
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
image="$(sed -n 's/^FROM \(nginx:[^ ]*\).*/\1/p' "$root/webui/Dockerfile" | tail -n1)"
tmp="$(mktemp -d)"
name="cctui-ui-dirs-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT

if [ $# -gt 0 ]; then
  html="$(cd "$1" && pwd)"
else
  html="$tmp/html"
  mkdir -p "$html/settings" "$html/cctuiverse"
  for f in index sessions bookmarks settings settings/interface settings/accounts cctuiverse/join; do
    echo "<!doctype html><script>1</script><!-- $f -->" >"$html/$f.html"
  done
  chmod -R a+rX "$html"
fi

docker run -d --name "$name" -p 127.0.0.1::8080 \
  -e CCTUI_FRAME_ANCESTORS= -e 'NGINX_ENVSUBST_FILTER=^CCTUI_FRAME_ANCESTORS$' \
  -v "$root/webui/nginx.conf:/etc/nginx/templates/default.conf.template:ro" \
  -v "$html:/usr/share/nginx/html:ro" "$image" >/dev/null
origin="http://$(docker port "$name" 8080/tcp | head -n1)"
for _ in $(seq 60); do curl -sf -o /dev/null "$origin/" && break; sleep 0.5; done

fail=0
check() {
  local got location
  got="$(curl -sS -o "$tmp/body" -w '%{http_code}|%{redirect_url}' "$origin$1")"
  location="${got#*|}"
  if [ "${got%%|*}" = 200 ] && [ -z "$location" ] && grep -q '<script nonce=' "$tmp/body"; then
    printf '  ok   %s\n' "$1"
  else
    printf '  FAIL %s: got [%s]\n' "$1" "$got"; fail=1
  fi
}

mapfile -t routes < <(cd "$html" && find . -name '*.html' ! -path './_app/*' ! -path './plugin-runtime/*' \
  | sed -e 's|^\.||' -e 's|\.html$||' -e 's|^/index$|/|' | sort)
for r in "${routes[@]}"; do
  check "$r"
  [ "$r" = / ] || check "$r/"
done
check /does-not-exist
check /sessions/some-session-id

if docker logs "$name" 2>&1 | grep -E '\[(emerg|error)\]'; then fail=1; fi
exit "$fail"
